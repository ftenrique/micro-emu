/** How an Action Button command is delivered to the bridge. */
export type ActionDispatch =
    | { kind: "micro-key"; key: string }
    | { kind: "encoder-button"; index: number }
    | { kind: "encoder-turn"; index: number; delta: number }
    | { kind: "catalog-action" }
    | { kind: "codex-action" }
    | { kind: "unsupported"; reason: string };

export interface ActionCatalogItem {
    /** Stable persisted identifier. Never reuse an id for different behavior. */
    id: string;
    category: string;
    label: string;
    title: string;
    description: string;
    icon: string;
    executor: "Micro" | "Bridge" | "Codex" | "Unavailable";
    dispatch: ActionDispatch;
}

/**
 * Stream Deck action catalog. Micro entries execute immediately; task
 * navigation and desktop shortcuts are handled by the daemon; Codex workflow
 * entries execute through deep links and the local Codex app-server.
 */
export const ACTION_CATALOG: readonly ActionCatalogItem[] = [
    // Physical Codex Micro controls. AG00-AG05 intentionally stay out of this
    // visible catalog because those positions are represented by Task Cards.
    { id: "micro.act06", category: "Codex Micro", label: "ACT06", title: "ACT06", description: "Press the physical ACT06 command. Its meaning remains configurable in Codex Micro.", icon: "action", executor: "Micro", dispatch: { kind: "micro-key", key: "ACT06" } },
    { id: "micro.act07", category: "Codex Micro", label: "ACT07", title: "ACT07", description: "Press the physical ACT07 command. Its meaning remains configurable in Codex Micro.", icon: "action", executor: "Micro", dispatch: { kind: "micro-key", key: "ACT07" } },
    { id: "micro.act08", category: "Codex Micro", label: "ACT08", title: "ACT08", description: "Press the physical ACT08 command. Its meaning remains configurable in Codex Micro.", icon: "action", executor: "Micro", dispatch: { kind: "micro-key", key: "ACT08" } },
    { id: "micro.mic", category: "Codex Micro", label: "Microphone (ACT10)", title: "Mic", description: "Hold to talk: Codex push-to-talk, or Windows dictation into the focused ZCode or Hermes app.", icon: "mic", executor: "Micro", dispatch: { kind: "encoder-button", index: 2 } },
    { id: "micro.send", category: "Codex Micro", label: "Send (ACT12)", title: "Send", description: "Press the Codex Micro send control (ACT12).", icon: "send", executor: "Micro", dispatch: { kind: "encoder-button", index: 0 } },
    { id: "micro.rotor-click", category: "Codex Micro", label: "Rotor click (ENC_CLK)", title: "Click", description: "Press the Codex Micro rotor.", icon: "rotor", executor: "Micro", dispatch: { kind: "encoder-button", index: 1 } },
    { id: "micro.rotor-cw", category: "Codex Micro", label: "Rotor clockwise (ENC_CW)", title: "CW", description: "Turn the Codex Micro rotor one step clockwise.", icon: "clockwise", executor: "Micro", dispatch: { kind: "encoder-turn", index: 1, delta: 1 } },
    { id: "micro.rotor-ccw", category: "Codex Micro", label: "Rotor counter-clockwise (ENC_CC)", title: "CCW", description: "Turn the Codex Micro rotor one step counter-clockwise.", icon: "counter-clockwise", executor: "Micro", dispatch: { kind: "encoder-turn", index: 1, delta: -1 } },
    { id: "micro.up", category: "Codex Micro", label: "Navigate up", title: "Up", description: "Move the Codex Micro radial control upward one step.", icon: "up", executor: "Micro", dispatch: { kind: "encoder-turn", index: 2, delta: 1 } },
    { id: "micro.down", category: "Codex Micro", label: "Navigate down", title: "Down", description: "Move the Codex Micro radial control downward one step.", icon: "down", executor: "Micro", dispatch: { kind: "encoder-turn", index: 2, delta: -1 } },
    { id: "micro.left", category: "Codex Micro", label: "Navigate left", title: "Left", description: "Move the Codex Micro radial control left one step.", icon: "left", executor: "Micro", dispatch: { kind: "encoder-turn", index: 0, delta: -1 } },
    { id: "micro.right", category: "Codex Micro", label: "Navigate right", title: "Right", description: "Move the Codex Micro radial control right one step.", icon: "right", executor: "Micro", dispatch: { kind: "encoder-turn", index: 0, delta: 1 } },

    // These are implemented inside the bridge and do not require agent support.
    { id: "task.previous", category: "Task navigation", label: "Previous task", title: "Prev Task", description: "Select the previous occupied Task Card, wrapping at the beginning.", icon: "previous", executor: "Bridge", dispatch: { kind: "catalog-action" } },
    { id: "task.next", category: "Task navigation", label: "Next task", title: "Next Task", description: "Select the next occupied Task Card, wrapping at the end.", icon: "next", executor: "Bridge", dispatch: { kind: "catalog-action" } },
    { id: "task.first", category: "Task navigation", label: "First task", title: "First", description: "Select the first occupied Task Card.", icon: "first", executor: "Bridge", dispatch: { kind: "catalog-action" } },
    { id: "task.last", category: "Task navigation", label: "Last task", title: "Last", description: "Select the last occupied Task Card.", icon: "last", executor: "Bridge", dispatch: { kind: "catalog-action" } },

    // Codex actions below have concrete desktop or app-server executors.
    { id: "task.open", category: "Task lifecycle", label: "Open selected task", title: "Open", description: "Open and focus the selected Codex task.", icon: "open", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "task.retry", category: "Task lifecycle", label: "Retry task", title: "Retry", description: "Replay the selected task's latest prompt as a new Codex turn.", icon: "retry", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "task.fork", category: "Task lifecycle", label: "Fork task", title: "Fork", description: "Fork the selected Codex task and open the new task.", icon: "fork", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "task.archive", category: "Task lifecycle", label: "Archive task", title: "Archive", description: "Archive the selected Codex task.", icon: "archive", executor: "Codex", dispatch: { kind: "codex-action" } },

    { id: "task.copy-prompt", category: "Task interaction", label: "Copy prompt", title: "Copy Prompt", description: "Copy the selected Codex task's latest user prompt.", icon: "copy-prompt", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "task.copy-response", category: "Task interaction", label: "Copy last response", title: "Copy Reply", description: "Copy the selected Codex task's latest agent response.", icon: "copy", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "task.copy-path", category: "Task interaction", label: "Copy workspace path", title: "Copy Path", description: "Copy the selected Codex task's workspace path.", icon: "clipboard", executor: "Codex", dispatch: { kind: "codex-action" } },

    { id: "agent.new-task", category: "Codex workflow", label: "New task", title: "New Task", description: "Start a new task in the focused ZCode or Hermes app; otherwise open the Codex new-task screen.", icon: "new-chat", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "agent.search", category: "Codex workflow", label: "Search tasks", title: "Search", description: "Focus Codex and open task search.", icon: "search", executor: "Bridge", dispatch: { kind: "catalog-action" } },
    { id: "agent.review-changes", category: "Codex workflow", label: "Review changes", title: "Review", description: "Start a Codex review of uncommitted workspace changes.", icon: "review", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "agent.run-tests", category: "Codex workflow", label: "Run tests", title: "Tests", description: "Start a Codex turn that runs and diagnoses the relevant tests.", icon: "tests", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "agent.open-terminal", category: "Codex workflow", label: "Open terminal", title: "Terminal", description: "Focus Codex and toggle its bottom terminal.", icon: "terminal", executor: "Bridge", dispatch: { kind: "catalog-action" } },
    { id: "agent.compact-context", category: "Codex workflow", label: "Compact context", title: "Compact", description: "Start Codex context compaction for the selected task.", icon: "compact", executor: "Codex", dispatch: { kind: "codex-action" } },
    { id: "agent.settings", category: "Codex workflow", label: "Codex settings", title: "Settings", description: "Open Codex settings.", icon: "settings", executor: "Codex", dispatch: { kind: "codex-action" } },
];

const RETIRED_ACTIONS: readonly ActionCatalogItem[] = [
    { id: "task.interrupt", category: "Unavailable", label: "Interrupt task", title: "Stop", description: "Unavailable: Codex cannot interrupt a turn owned by another client.", icon: "stop", executor: "Unavailable", dispatch: { kind: "unsupported", reason: "Cross-client task interruption is not supported by this Codex version." } },
    { id: "task.pin", category: "Unavailable", label: "Pin task", title: "Pin", description: "Unavailable: the installed Codex protocol does not expose task pinning.", icon: "pin", executor: "Unavailable", dispatch: { kind: "unsupported", reason: "Task pinning is not supported by this Codex version." } },
    { id: "task.unpin", category: "Unavailable", label: "Unpin task", title: "Unpin", description: "Unavailable: the installed Codex protocol does not expose task unpinning.", icon: "unpin", executor: "Unavailable", dispatch: { kind: "unsupported", reason: "Task unpinning is not supported by this Codex version." } },
    { id: "task.approve", category: "Unavailable", label: "Approve", title: "Approve", description: "Unavailable: approval decisions must be answered by the client that owns the request.", icon: "approve", executor: "Unavailable", dispatch: { kind: "unsupported", reason: "Cross-client approval is not supported." } },
    { id: "task.reject", category: "Unavailable", label: "Reject", title: "Reject", description: "Unavailable: approval decisions must be answered by the client that owns the request.", icon: "reject", executor: "Unavailable", dispatch: { kind: "unsupported", reason: "Cross-client rejection is not supported." } },
    { id: "agent.open-browser", category: "Unavailable", label: "Open browser", title: "Browser", description: "Unavailable: Codex has no public command for opening its browser surface.", icon: "browser", executor: "Unavailable", dispatch: { kind: "unsupported", reason: "Open browser is not exposed by Codex." } },
    { id: "agent.open-editor", category: "Unavailable", label: "Open editor", title: "Editor", description: "Unavailable: Codex has no public command for opening its editor surface.", icon: "code", executor: "Unavailable", dispatch: { kind: "unsupported", reason: "Open editor is not exposed by Codex." } },
];
const RETIRED_BY_ID = new Map(RETIRED_ACTIONS.map((item) => [item.id, item]));
const ACTIONS_BY_ID = new Map(ACTION_CATALOG.map((item) => [item.id, item]));

const LEGACY_AGENT_ACTIONS: readonly ActionCatalogItem[] = Array.from({ length: 6 }, (_, index) => ({
    id: `legacy.ag${String(index).padStart(2, "0")}`,
    category: "Legacy",
    label: `Legacy AG${String(index).padStart(2, "0")}`,
    title: `AG${String(index).padStart(2, "0")}`,
    description: "Compatibility mapping for an existing profile. New Action Buttons use Task Cards instead of AG codes.",
    icon: "agent",
    executor: "Micro" as const,
    dispatch: { kind: "micro-key" as const, key: `AG${String(index).padStart(2, "0")}` },
}));
const LEGACY_BY_ID = new Map(LEGACY_AGENT_ACTIONS.map((item) => [item.id, item]));

export const DEFAULT_ACTION_ID = "micro.act06";

/** Looks up a catalog id (including retired and legacy entries) without
 * falling back to the default action. */
export function findCatalogAction(actionId: unknown): ActionCatalogItem | undefined {
    if (typeof actionId !== "string") return undefined;
    return ACTIONS_BY_ID.get(actionId) ?? RETIRED_BY_ID.get(actionId) ?? LEGACY_BY_ID.get(actionId);
}

/** Resolves persisted settings, including the pre-catalog numeric format. */
export function resolveCatalogAction(actionId: unknown, legacyIndex?: unknown): ActionCatalogItem {
    const item = findCatalogAction(actionId);
    if (item) return item;
    const index = Number(legacyIndex);
    if (Number.isInteger(index) && index >= 0 && index <= 5) {
        return LEGACY_AGENT_ACTIONS[index];
    }
    if (Number.isInteger(index) && index >= 6 && index <= 8) {
        return ACTIONS_BY_ID.get(`micro.act0${index}`)!;
    }
    return ACTIONS_BY_ID.get(DEFAULT_ACTION_ID)!;
}
