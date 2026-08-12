import { spawn } from "child_process";

export interface HermesActionContext {
    selectedTask: Record<string, unknown> | null;
    taskId?: string | null;
}

/**
 * Capability-gated Hermes action executor.
 *
 * Local metadata actions work without a backend. Session reads and forks use
 * Hermes' documented REST API and are enabled only when the user explicitly
 * configures MICRO_EMU_HERMES_API_URL and MICRO_EMU_HERMES_API_KEY. Unknown
 * actions fail closed and are never forwarded to Codex.
 */
export class HermesActionExecutor {
    async execute(actionId: string, context: HermesActionContext): Promise<void> {
        if (actionId === "task.copy-path") {
            const path = context.selectedTask?.workspace_path;
            if (typeof path !== "string" || !path.trim()) {
                throw new Error("The selected Hermes task has no workspace path");
            }
            await writeClipboard(path);
            return;
        }

        const sessionId = hermesSessionId(context);
        if (!sessionId) {
            throw new Error("Select a Hermes task before running " + actionId);
        }

        if (actionId === "task.copy-prompt" || actionId === "task.copy-response") {
            const messages = await hermesRequest<unknown>(
                `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
            );
            const rows = messageRows(messages);
            const role = actionId === "task.copy-prompt" ? "user" : "assistant";
            const message = [...rows].reverse().find((row) => row.role === role);
            const content = message == null ? "" : messageText(message.content);
            if (!content) throw new Error(`The selected Hermes task has no ${role} message`);
            await writeClipboard(content);
            return;
        }

        if (actionId === "task.fork") {
            await hermesRequest(`/api/sessions/${encodeURIComponent(sessionId)}/fork`, {
                method: "POST",
                body: JSON.stringify({}),
            });
            return;
        }

        throw new Error(
            "Hermes does not expose a supported integration for " + actionId
                + ". The action was not sent to Codex.",
        );
    }
}

function hermesSessionId(context: HermesActionContext): string | null {
    const raw = context.selectedTask?.task_id ?? context.taskId;
    if (typeof raw !== "string" || !raw.startsWith("hermes:")) return null;
    return raw.slice("hermes:".length) || null;
}

async function hermesRequest<T = unknown>(path: string, init: RequestInit = {}): Promise<T> {
    const base = process.env.MICRO_EMU_HERMES_API_URL?.replace(/\/$/, "");
    const key = process.env.MICRO_EMU_HERMES_API_KEY;
    if (!base || !key) {
        throw new Error(
            "Hermes API actions require MICRO_EMU_HERMES_API_URL and MICRO_EMU_HERMES_API_KEY",
        );
    }
    const response = await fetch(base + path, {
        ...init,
        headers: {
            "Authorization": `Bearer ${key}`,
            "Content-Type": "application/json",
            ...init.headers,
        },
    });
    if (!response.ok) {
        const body = await response.text();
        throw new Error(`Hermes API ${response.status}: ${body.slice(0, 240) || response.statusText}`);
    }
    if (response.status === 204) return undefined as T;
    return await response.json() as T;
}

function messageRows(value: unknown): Array<Record<string, unknown>> {
    const rows = Array.isArray(value)
        ? value
        : value != null && typeof value === "object"
            ? (value as Record<string, unknown>).messages
            : [];
    return Array.isArray(rows)
        ? rows.filter((row): row is Record<string, unknown> => row != null && typeof row === "object")
        : [];
}

function messageText(content: unknown): string {
    if (typeof content === "string") return content;
    if (!Array.isArray(content)) return "";
    return content
        .map((part) => part != null && typeof part === "object"
            ? (part as Record<string, unknown>).text
            : "")
        .filter((part): part is string => typeof part === "string")
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
