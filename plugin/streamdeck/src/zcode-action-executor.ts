import { spawn } from "child_process";

export interface ZCodeActionContext {
    selectedTask: Record<string, unknown> | null;
    taskId?: string | null;
}

/**
 * Capability-gated ZCode action executor.
 *
 * ZCode 3.7.5 does not expose a public task/session control API. Keep every
 * mutating action disabled until that changes; the only local action that can
 * be completed safely is copying an already-rendered workspace path.
 */
export class ZCodeActionExecutor {
    async execute(actionId: string, context: ZCodeActionContext): Promise<void> {
        if (actionId === "task.copy-path") {
            const path = context.selectedTask?.workspace_path;
            if (typeof path !== "string" || !path.trim()) {
                throw new Error("The selected ZCode task has no workspace path");
            }
            await writeClipboard(path);
            return;
        }

        throw new Error(
            "ZCode does not expose a supported API for " + actionId
                + ". The action was not sent to Codex.",
        );
    }
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
