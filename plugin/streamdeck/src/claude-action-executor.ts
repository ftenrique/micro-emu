import { spawn } from "child_process";
import { promises as fs } from "fs";
import { homedir } from "os";
import { join } from "path";

export interface ClaudeActionContext {
    selectedTask: Record<string, unknown> | null;
    taskId?: string | null;
}

/**
 * Claude Code action executor.
 *
 * Everything runs against Claude Code's local state: the workspace path comes
 * from the task card, and the last prompt/response are read from the session
 * transcript (`~/.claude/projects/<project>/<session-id>.jsonl`). Actions
 * Claude Code offers no local integration for fail closed and are never
 * forwarded to Codex.
 */
export class ClaudeActionExecutor {
    private readonly home: string;

    constructor(home: string = claudeHome()) {
        this.home = home;
    }

    async execute(actionId: string, context: ClaudeActionContext): Promise<void> {
        if (actionId === "task.copy-path") {
            const path = context.selectedTask?.workspace_path;
            if (typeof path !== "string" || !path.trim()) {
                throw new Error("The selected Claude task has no workspace path");
            }
            await writeClipboard(path);
            return;
        }

        if (actionId === "task.copy-prompt" || actionId === "task.copy-response") {
            const id = claudeTaskId(context);
            if (!id) throw new Error("Select a Claude task before running " + actionId);
            const entries = await this.readTranscript(id);
            const role = actionId === "task.copy-prompt" ? "user" : "assistant";
            const content = lastMessageText(entries, role);
            if (!content) throw new Error(`The selected Claude task has no ${role} message`);
            await writeClipboard(content);
            return;
        }

        throw new Error(
            "Claude Code does not expose a supported integration for " + actionId
                + ". The action was not sent to Codex.",
        );
    }

    /** Resolves a card id (desktop host id or session id) to its transcript. */
    async readTranscript(id: string): Promise<Array<Record<string, unknown>>> {
        const sessionId = await this.resolveSessionId(id);
        if (!/^[\w-]+$/.test(sessionId)) throw new Error("Invalid Claude session id");
        const projectsDir = join(this.home, "projects");
        for (const project of await fs.readdir(projectsDir).catch(() => [] as string[])) {
            const text = await fs.readFile(join(projectsDir, project, sessionId + ".jsonl"), "utf8")
                .catch(() => null);
            if (text == null) continue;
            return text.split("\n").flatMap((line) => {
                try {
                    const entry = JSON.parse(line);
                    return entry != null && typeof entry === "object" ? [entry] : [];
                } catch {
                    return [];
                }
            });
        }
        throw new Error("The Claude transcript for this task was not found");
    }

    /**
     * Desktop-hosted cards are keyed by the stable host id (`local_...`); the
     * live registry in `~/.claude/sessions` maps it to the current session id.
     */
    private async resolveSessionId(id: string): Promise<string> {
        if (!id.startsWith("local_")) return id;
        const sessionsDir = join(this.home, "sessions");
        for (const file of await fs.readdir(sessionsDir).catch(() => [] as string[])) {
            if (!file.endsWith(".json")) continue;
            try {
                const record = JSON.parse(await fs.readFile(join(sessionsDir, file), "utf8"));
                if (record?.hostSessionId === id && typeof record.sessionId === "string") {
                    return record.sessionId;
                }
            } catch {
                continue;
            }
        }
        throw new Error("The selected Claude session is no longer running");
    }
}

function claudeHome(): string {
    return process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude");
}

function claudeTaskId(context: ClaudeActionContext): string | null {
    const raw = context.selectedTask?.task_id ?? context.taskId;
    if (typeof raw !== "string" || !raw.startsWith("claude:")) return null;
    return raw.slice("claude:".length) || null;
}

/**
 * Returns the text of the latest real message for `role`. Tool results are
 * stored as user entries and tool calls as assistant entries; neither has a
 * text part, so they are skipped.
 */
export function lastMessageText(entries: Array<Record<string, unknown>>, role: "user" | "assistant"): string {
    for (let index = entries.length - 1; index >= 0; index--) {
        const entry = entries[index];
        if (entry.type !== role || entry.isMeta === true || entry.isSidechain === true) continue;
        const message = entry.message;
        if (message == null || typeof message !== "object") continue;
        const text = messageText((message as Record<string, unknown>).content);
        if (text) return text;
    }
    return "";
}

function messageText(content: unknown): string {
    if (typeof content === "string") return content.trim();
    if (!Array.isArray(content)) return "";
    return content
        .map((part) => part != null && typeof part === "object"
            && (part as Record<string, unknown>).type === "text"
            ? (part as Record<string, unknown>).text
            : "")
        .filter((part): part is string => typeof part === "string" && part.length > 0)
        .join("\n")
        .trim();
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
