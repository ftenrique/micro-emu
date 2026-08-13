import { spawn, type ChildProcessWithoutNullStreams } from "child_process";
import { existsSync } from "fs";
import * as path from "path";

type JsonRecord = Record<string, unknown>;

interface PendingRequest {
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
    timer: NodeJS.Timeout;
}

interface LaunchSpec {
    command: string;
    args: string[];
}

export interface CodexActionContext {
    selectedTask: Record<string, unknown> | null;
    taskId?: string | null;
}

/** A small JSON-RPC client for the locally installed Codex app-server. */
class CodexAppServerClient {
    private child: ChildProcessWithoutNullStreams | null = null;
    private startPromise: Promise<void> | null = null;
    private buffer = "";
    private nextId = 1;
    private readonly pending = new Map<number, PendingRequest>();

    constructor(private readonly log: (message: string) => void) {}

    async request<T = unknown>(method: string, params: JsonRecord): Promise<T> {
        await this.ensureStarted();
        return this.sendRequest<T>(method, params);
    }

    private async ensureStarted(): Promise<void> {
        if (this.startPromise) {
            await this.startPromise;
            return;
        }
        if (this.child && !this.child.killed) return;
        this.startPromise = this.start().catch((error) => {
            this.startPromise = null;
            throw error;
        });
        await this.startPromise;
    }

    private async start(): Promise<void> {
        const launch = resolveCodexLaunch();
        this.log(`Starting Codex action service: ${launch.command}`);
        const child = spawn(launch.command, launch.args, {
            stdio: ["pipe", "pipe", "pipe"],
            windowsHide: true,
        });
        this.child = child;
        this.buffer = "";

        child.stdout.setEncoding("utf8");
        child.stdout.on("data", (chunk: string) => this.handleData(chunk));
        child.stderr.setEncoding("utf8");
        child.stderr.on("data", (chunk: string) => {
            const message = chunk.trim();
            if (message) this.log(`Codex action service: ${message}`);
        });
        child.on("error", (error) => this.failChild(child, error));
        child.on("close", (code) => {
            this.failChild(child, new Error(`Codex action service exited (${code ?? "unknown"})`));
        });

        await new Promise<void>((resolve, reject) => {
            child.once("spawn", resolve);
            child.once("error", reject);
        });
        await this.sendRequest("initialize", {
            clientInfo: {
                name: "codex-micro-streamdeck",
                title: "Codex Micro Stream Deck",
                version: "1.1.0",
            },
            capabilities: { experimentalApi: true },
        });
    }

    private sendRequest<T>(method: string, params: JsonRecord): Promise<T> {
        const child = this.child;
        if (!child || child.killed || !child.stdin.writable) {
            return Promise.reject(new Error("Codex action service is not available"));
        }
        const id = this.nextId++;
        return new Promise<T>((resolve, reject) => {
            const timer = setTimeout(() => {
                this.pending.delete(id);
                reject(new Error(`Codex ${method} timed out`));
            }, 30_000);
            this.pending.set(id, {
                resolve: (value) => resolve(value as T),
                reject,
                timer,
            });
            child.stdin.write(`${JSON.stringify({ id, method, params })}\n`, (error) => {
                if (!error) return;
                const pending = this.pending.get(id);
                if (!pending) return;
                clearTimeout(pending.timer);
                this.pending.delete(id);
                pending.reject(error);
            });
        });
    }

    private handleData(chunk: string): void {
        this.buffer += chunk;
        let newline: number;
        while ((newline = this.buffer.indexOf("\n")) >= 0) {
            const line = this.buffer.slice(0, newline).trim();
            this.buffer = this.buffer.slice(newline + 1);
            if (!line) continue;
            let message: JsonRecord;
            try {
                message = JSON.parse(line) as JsonRecord;
            } catch {
                this.log("Codex action service returned malformed JSON");
                continue;
            }
            if (typeof message.method === "string") {
                this.handleServerRequest(message);
                continue;
            }
            const id = Number(message.id);
            if (!Number.isInteger(id)) continue;
            const pending = this.pending.get(id);
            if (!pending) continue;
            clearTimeout(pending.timer);
            this.pending.delete(id);
            const rpcError = asRecord(message.error);
            if (rpcError) {
                const detail = typeof rpcError.message === "string"
                    ? rpcError.message
                    : JSON.stringify(rpcError);
                pending.reject(new Error(detail));
            } else {
                pending.resolve(message.result);
            }
        }
    }

    private handleServerRequest(message: JsonRecord): void {
        if (message.id == null) return;
        const method = String(message.method);
        this.log("Codex requested interactive input that must be answered in the app: " + method);
        this.child?.stdin.write(JSON.stringify({
            id: message.id,
            error: {
                code: -32601,
                message: "Interactive requests must be answered in the Codex app.",
            },
        }) + "\n");
    }

    private failChild(child: ChildProcessWithoutNullStreams, error: Error): void {
        if (this.child !== child) return;
        this.child = null;
        this.startPromise = null;
        for (const request of this.pending.values()) {
            clearTimeout(request.timer);
            request.reject(error);
        }
        this.pending.clear();
    }
}

/** Executes every non-Micro catalog action that has a supported Codex API. */
export class CodexActionExecutor {
    private readonly client: CodexAppServerClient;

    constructor(private readonly log: (message: string) => void = () => undefined) {
        this.client = new CodexAppServerClient(log);
    }

    async execute(actionId: string, context: CodexActionContext): Promise<void> {
        if (actionId === "agent.new-task") {
            await openCodexUri("codex://threads/new");
            return;
        }
        if (actionId === "agent.settings") {
            await openCodexUri("codex://settings");
            return;
        }

        const threadId = requireThreadId(context);
        switch (actionId) {
            case "task.open":
                await openThread(threadId);
                return;
            case "task.retry": {
                const thread = await this.readThread(threadId, true);
                const input = latestUserInput(thread);
                await this.resumeThread(threadId);
                await this.client.request("turn/start", { threadId, input });
                await openThread(threadId);
                return;
            }
            case "task.fork": {
                const result = asRecord(await this.client.request("thread/fork", {
                    threadId,
                    excludeTurns: true,
                }));
                const forked = asRecord(result?.thread);
                const forkedId = typeof forked?.id === "string" ? forked.id : null;
                if (!forkedId) throw new Error("Codex did not return the forked task id");
                await openThread(forkedId);
                return;
            }
            case "task.archive":
                await this.client.request("thread/archive", { threadId });
                return;
            case "task.copy-prompt": {
                const thread = await this.readThread(threadId, true);
                await writeClipboard(latestUserText(thread));
                return;
            }
            case "task.copy-response": {
                const thread = await this.readThread(threadId, true);
                await writeClipboard(latestAgentText(thread));
                return;
            }
            case "task.copy-path": {
                const thread = await this.readThread(threadId, false);
                if (typeof thread.cwd !== "string" || !thread.cwd) {
                    throw new Error("The selected task has no workspace path");
                }
                await writeClipboard(thread.cwd);
                return;
            }
            case "agent.review-changes": {
                await this.resumeThread(threadId);
                const result = asRecord(await this.client.request("review/start", {
                    threadId,
                    target: { type: "uncommittedChanges" },
                    delivery: "inline",
                }));
                const reviewThreadId = typeof result?.reviewThreadId === "string"
                    ? result.reviewThreadId
                    : threadId;
                await openThread(reviewThreadId);
                return;
            }
            case "agent.run-tests":
                await this.resumeThread(threadId);
                await this.client.request("turn/start", {
                    threadId,
                    input: [{
                        type: "text",
                        text: "Run the relevant tests for the current workspace. Diagnose any failures and report the results. Do not make unrelated changes.",
                    }],
                });
                await openThread(threadId);
                return;
            case "agent.compact-context":
                await this.resumeThread(threadId);
                await this.client.request("thread/compact/start", { threadId });
                await openThread(threadId);
                return;
            default:
                throw new Error(`Action ${actionId} has no Codex executor`);
        }
    }

    private async readThread(threadId: string, includeTurns: boolean): Promise<JsonRecord> {
        const result = asRecord(await this.client.request("thread/read", {
            threadId,
            includeTurns,
        }));
        const thread = asRecord(result?.thread);
        if (!thread) throw new Error("Codex did not return the selected task");
        return thread;
    }

    private async resumeThread(threadId: string): Promise<void> {
        await this.client.request("thread/resume", { threadId, excludeTurns: true });
    }
}

function resolveCodexLaunch(): LaunchSpec {
    const configured = process.env.MICRO_EMU_CODEX_CLI;
    if (configured) return launchForConfiguredPath(configured);

    if (process.platform === "win32") {
        const appData = process.env.APPDATA;
        if (appData) {
            const cliScript = path.join(
                appData,
                "npm",
                "node_modules",
                "@openai",
                "codex",
                "bin",
                "codex.js",
            );
            if (existsSync(cliScript)) {
                return {
                    command: process.env.MICRO_EMU_NODE_EXE ?? nodeExecutable(),
                    args: [cliScript, "app-server", "--listen", "stdio://"],
                };
            }
        }
        throw new Error(
            "Codex CLI was not found. Install @openai/codex or set MICRO_EMU_CODEX_CLI to codex.js/codex.exe.",
        );
    }

    return { command: "codex", args: ["app-server", "--listen", "stdio://"] };
}

function nodeExecutable(): string {
    const executable = path.basename(process.execPath).toLowerCase();
    if (executable === "node" || executable === "node.exe") return process.execPath;
    return process.platform === "win32" ? "node.exe" : "node";
}
function launchForConfiguredPath(configured: string): LaunchSpec {
    if (configured.toLowerCase().endsWith(".js")) {
        return {
            command: process.env.MICRO_EMU_NODE_EXE ?? nodeExecutable(),
            args: [configured, "app-server", "--listen", "stdio://"],
        };
    }
    if (configured.toLowerCase().endsWith(".cmd")) {
        const command = process.env.ComSpec ?? "cmd.exe";
        const escaped = configured.replaceAll('"', '""');
        return {
            command,
            args: ["/d", "/s", "/c", `""${escaped}" app-server --listen stdio://"`],
        };
    }
    return { command: configured, args: ["app-server", "--listen", "stdio://"] };
}

function requireThreadId(context: CodexActionContext): string {
    const cardId = context.selectedTask?.task_id;
    const candidate = typeof cardId === "string" && cardId ? cardId : context.taskId;
    if (typeof candidate !== "string" || !candidate) {
        throw new Error("Select a Codex Task Card first");
    }
    if (candidate.startsWith("legacy:") || candidate.startsWith("codex-hid:")) {
        throw new Error("The selected card is not backed by a Codex task id");
    }
    return candidate;
}

async function openThread(threadId: string): Promise<void> {
    await openCodexUri(`codex://threads/${encodeURIComponent(threadId)}`);
}

function openCodexUri(uri: string): Promise<void> {
    const command = process.platform === "win32"
        ? "explorer.exe"
        : process.platform === "darwin" ? "open" : "xdg-open";
    return new Promise((resolve, reject) => {
        const child = spawn(command, [uri], {
            detached: true,
            stdio: "ignore",
            windowsHide: true,
        });
        child.once("error", reject);
        child.once("spawn", () => {
            child.unref();
            resolve();
        });
    });
}

function writeClipboard(text: string): Promise<void> {
    const command = process.platform === "win32" ? "clip.exe" : "pbcopy";
    return new Promise((resolve, reject) => {
        const child = spawn(command, [], {
            stdio: ["pipe", "ignore", "pipe"],
            windowsHide: true,
        });
        let stderr = "";
        child.stderr?.setEncoding("utf8");
        child.stderr?.on("data", (chunk: string) => { stderr += chunk; });
        child.once("error", reject);
        child.once("close", (code) => {
            if (code === 0) resolve();
            else reject(new Error(stderr.trim() || `Clipboard command exited (${code})`));
        });
        child.stdin?.end(text, "utf8");
    });
}

function latestUserInput(thread: JsonRecord): JsonRecord[] {
    const item = latestThreadItem(thread, "userMessage");
    const content = Array.isArray(item?.content)
        ? item.content.filter((entry): entry is JsonRecord => asRecord(entry) != null)
        : [];
    if (content.length === 0) throw new Error("The selected task has no prompt to retry");
    return content;
}

function latestUserText(thread: JsonRecord): string {
    const content = latestUserInput(thread);
    const text = content
        .filter((entry) => entry.type === "text" && typeof entry.text === "string")
        .map((entry) => String(entry.text))
        .join("\n\n")
        .trim();
    if (!text) throw new Error("The selected task's latest prompt has no text");
    return text;
}

function latestAgentText(thread: JsonRecord): string {
    const messages = threadItems(thread)
        .filter((item) => item.type === "agentMessage" && typeof item.text === "string" && item.text.trim());
    const message = [...messages].reverse().find((item) => item.phase === "final_answer")
        ?? messages.at(-1);
    if (!message || typeof message.text !== "string") {
        throw new Error("The selected task has no agent response");
    }
    return message.text;
}

function latestThreadItem(thread: JsonRecord, type: string): JsonRecord | null {
    const items = threadItems(thread);
    return [...items].reverse().find((item) => item.type === type) ?? null;
}

function threadItems(thread: JsonRecord): JsonRecord[] {
    if (!Array.isArray(thread.turns)) return [];
    const result: JsonRecord[] = [];
    for (const turnValue of thread.turns) {
        const turn = asRecord(turnValue);
        if (!turn || !Array.isArray(turn.items)) continue;
        for (const item of turn.items) {
            const record = asRecord(item);
            if (record) result.push(record);
        }
    }
    return result;
}

function asRecord(value: unknown): JsonRecord | null {
    return value != null && typeof value === "object" && !Array.isArray(value)
        ? value as JsonRecord
        : null;
}
