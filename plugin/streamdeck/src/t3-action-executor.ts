import { spawn } from "child_process";
import { existsSync, promises as fs } from "fs";
import { homedir } from "os";
import { join } from "path";

export interface T3ActionContext {
    selectedTask: Record<string, unknown> | null;
    taskId?: string | null;
}

export type T3MessageReader = (threadId: string, role: "user" | "assistant") => Promise<string>;

/**
 * T3 Code local action executor.
 *
 * Workspace paths come from the selected task card, thread opening uses the
 * `t3code://threads/<environmentId>/<threadId>` deep link with strict
 * identifier validation, and prompt/response copying delegates to the bridge's
 * read-only SQLite helper (`rp2040-bridge --read-t3-message`). Actions T3 Code
 * does not support fail closed and are never forwarded to Codex.
 */
export class T3ActionExecutor {
    private readonly home: string;
    private readonly openUrl: (url: string) => Promise<void>;
    private readonly readMessage: T3MessageReader;
    private readonly writeClip: (text: string) => Promise<void>;

    constructor(
        home: string = t3Home(),
        openUrl: (url: string) => Promise<void> = openDeepLink,
        readMessage?: T3MessageReader,
        writeClip: (text: string) => Promise<void> = writeClipboard,
        bridgeExe?: string,
    ) {
        this.home = home;
        this.openUrl = openUrl;
        this.readMessage = readMessage ?? ((threadId, role) =>
            readT3MessageViaBridge(threadId, role, bridgeExe));
        this.writeClip = writeClip;
    }

    async execute(actionId: string, context: T3ActionContext): Promise<void> {
        if (actionId === "t3.new-thread") {
            await this.openUrl("t3code://new");
            return;
        }

        if (actionId === "task.open") {
            const threadId = t3ThreadId(context);
            const envId = threadId ? await this.readEnvironmentId() : null;
            await this.openUrl(
                threadId && envId
                    ? `t3code://threads/${envId}/${threadId}`
                    : "t3code://",
            );
            return;
        }

        if (actionId === "task.copy-path") {
            const path = context.selectedTask?.workspace_path;
            if (typeof path !== "string" || !path.trim()) {
                throw new Error("The selected T3 Code task has no workspace path");
            }
            await this.writeClip(path);
            return;
        }

        if (actionId === "task.copy-prompt" || actionId === "task.copy-response") {
            const threadId = t3ThreadId(context);
            if (!threadId) throw new Error("Select a T3 Code task before running " + actionId);
            const role = actionId === "task.copy-prompt" ? "user" : "assistant";
            const content = (await this.readMessage(threadId, role)).trim();
            if (!content) throw new Error(`The selected T3 Code task has no ${role} message`);
            await this.writeClip(content);
            return;
        }

        throw new Error(
            "T3 Code does not expose a supported integration for " + actionId
                + ". The action was not sent to Codex.",
        );
    }

    private async readEnvironmentId(): Promise<string | null> {
        const raw = await fs
            .readFile(join(this.home, "userdata", "environment-id"), "utf8")
            .catch(() => null);
        return validT3Identifier(raw?.trim());
    }
}

function t3Home(): string {
    return process.env.T3CODE_HOME || join(homedir(), ".t3");
}

/**
 * Strict identifier validation before interpolating environment or thread IDs
 * into `t3code://` deep links or CLI arguments.
 */
export function validT3Identifier(id: string | null | undefined): string | null {
    return typeof id === "string" && /^[A-Za-z0-9-]{1,128}$/.test(id) ? id : null;
}

export function t3ThreadId(context: T3ActionContext): string | null {
    const raw = context.selectedTask?.task_id ?? context.taskId;
    if (typeof raw !== "string" || !raw.startsWith("t3:")) return null;
    return validT3Identifier(raw.slice("t3:".length));
}

function readT3MessageViaBridge(
    threadId: string,
    role: "user" | "assistant",
    explicitBridgeExe?: string,
): Promise<string> {
    const safeThreadId = validT3Identifier(threadId);
    if (!safeThreadId) return Promise.reject(new Error("Invalid T3 Code thread id"));
    const bridgeExe = explicitBridgeExe ?? fallbackBridgeExecutable();
    if (!bridgeExe) {
        return Promise.reject(new Error("rp2040-bridge executable was not found for T3 message lookup"));
    }
    return new Promise((resolve, reject) => {
        const child = spawn(bridgeExe, ["--read-t3-message", safeThreadId, role], {
            stdio: ["ignore", "pipe", "pipe"],
            windowsHide: true,
        });
        let stdout = "";
        let stderr = "";
        child.stdout?.setEncoding("utf8");
        child.stderr?.setEncoding("utf8");
        child.stdout?.on("data", (chunk: string) => { stdout += chunk; });
        child.stderr?.on("data", (chunk: string) => { stderr += chunk; });
        child.once("error", reject);
        child.once("close", (code) => {
            if (code !== 0) {
                reject(new Error(stderr.trim() || `T3 message lookup failed (exit ${code ?? "unknown"})`));
                return;
            }
            try {
                const parsed = JSON.parse(stdout) as { text?: unknown };
                resolve(typeof parsed?.text === "string" ? parsed.text : "");
            } catch {
                reject(new Error("T3 message lookup returned invalid JSON"));
            }
        });
    });
}

function openDeepLink(url: string): Promise<void> {
    const [command, args] = process.platform === "win32"
        ? ["cmd.exe", ["/C", "start", "", url]]
        : ["open", [url]];
    return new Promise((resolve, reject) => {
        const child = spawn(command, args, { stdio: "ignore", windowsHide: true });
        child.once("error", reject);
        child.once("close", (code) => {
            if (code === 0) resolve();
            else reject(new Error("Could not open " + url + " (exit " + (code ?? "unknown") + ")"));
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
            else reject(new Error(stderr.trim() || "Clipboard command exited (" + (code ?? "unknown") + ")"));
        });
        child.stdin?.end(text, "utf8");
    });
}

function fallbackBridgeExecutable(): string | undefined {
    const candidates = [
        process.env.MICRO_EMU_BRIDGE_EXE,
        process.env.LOCALAPPDATA
            ? join(process.env.LOCALAPPDATA, "micro-emu", "rp2040-bridge.exe")
            : undefined,
    ];
    for (const candidate of candidates) {
        if (candidate && existsSync(candidate)) return candidate;
    }
    return undefined;
}
