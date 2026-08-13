import { DaemonClient, DisplayContext } from "./daemon-client";
import { CodexActionExecutor } from "./codex-action-executor";
import { ZCodeActionExecutor } from "./zcode-action-executor";
import { HermesActionExecutor } from "./hermes-action-executor";

/** Display context enriched with the selected task physical number. */
export interface SelectedDisplayContext extends DisplayContext {
    agent?: string | null;
    task_number?: number | null;
}

/**
 * Shared plugin context: holds the daemon client and the latest render state
 * so action classes can update their key images when state arrives.
 */
export class PluginContext {
    readonly daemon: DaemonClient;
    readonly codex: CodexActionExecutor;
    readonly zcode: ZCodeActionExecutor;
    readonly hermes: HermesActionExecutor;
    private threadStatus: unknown[] = [];
    private taskCards: unknown[] = [];
    private displayContext: DisplayContext | null = null;
    private selectedTaskSlot: number | null = null;
    private connected = false;

    /** Callbacks invoked when render state changes. */
    private listeners: Set<() => void> = new Set();

    constructor(
        daemon: DaemonClient,
        codex = new CodexActionExecutor(),
        zcode = new ZCodeActionExecutor(),
        hermes = new HermesActionExecutor(),
    ) {
        this.daemon = daemon;
        this.codex = codex;
        this.zcode = zcode;
        this.hermes = hermes;

        daemon.on("connect", () => {
            this.connected = true;
            this.notifyListeners();
        });
        daemon.on("disconnect", () => {
            this.connected = false;
            this.selectedTaskSlot = null;
            this.notifyListeners();
        });
        daemon.on("render:threadStatus", (status: unknown[]) => {
            this.threadStatus = status;
            this.notifyListeners();
        });
        daemon.on("render:taskCards", (cards: unknown[]) => {
            const records = cards.filter((card): card is Record<string, unknown> =>
                card != null && typeof card === "object");
            const selected = records.find((card) => card.selected === true);
            const selectedSlot = Number(selected?.id ?? selected?.slot ?? selected?.i);
            this.selectedTaskSlot = Number.isFinite(selectedSlot) ? selectedSlot : null;
            // Normalize defensively: even a malformed/replayed payload can
            // expose at most one selected card to every action.
            this.taskCards = records.map((card) => {
                const slot = Number(card.id ?? card.slot ?? card.i);
                return {
                    ...card,
                    selected: this.selectedTaskSlot != null && slot === this.selectedTaskSlot,
                };
            });
            this.notifyListeners();
        });
        daemon.on("render:displayContext", (ctx: DisplayContext) => {
            this.displayContext = ctx;
            this.notifyListeners();
        });
    }

    isConnected(): boolean {
        return this.connected;
    }

    getThreadStatus(): unknown[] {
        return this.threadStatus;
    }

    getDisplayContext(): DisplayContext | null {
        return this.displayContext;
    }
    /** Returns live display context with selected task-card metadata overlaid. */
    getSelectedDisplayContext(): SelectedDisplayContext {
        const merged: SelectedDisplayContext = { ...(this.displayContext ?? {}) };
        const card = this.getSelectedTaskCard();
        if (!card) return merged;
        merged.agent = nonEmptyText(card.agent) ?? merged.agent;
        merged.task_id = nonEmptyText(card.task_id) ?? merged.task_id;
        const sourceSlot = Number(card.source_slot ?? card.id ?? card.slot ?? card.i);
        if (Number.isFinite(sourceSlot)) merged.task_number = sourceSlot + 1;
        merged.task = descriptiveCardTitle(card) ?? merged.task;
        merged.project = nonEmptyText(card.project) ?? merged.project;
        merged.model = nonEmptyText(card.model) ?? merged.model;
        merged.effort = nonEmptyText(card.effort) ?? merged.effort;
        merged.status = nonEmptyText(card.status ?? card.state) ?? merged.status;
        if (card.progress != null) {
            const progress = Number(card.progress);
            if (Number.isFinite(progress)) merged.progress = progress;
        }
        return merged;
    }

    selectTaskSlot(slot: number): void {
        this.selectedTaskSlot = slot;
        this.notifyListeners();
    }

    getSelectedTaskSlot(): number | null {
        return this.selectedTaskSlot;
    }

    async executeSelectedAgentAction(actionId: string): Promise<void> {
        const selectedTask = this.getSelectedTaskCard();
        const taskId = this.displayContext?.task_id;
        const owner = selectedTask?.agent
            ?? (taskId?.startsWith("zcode:") ? "zcode"
                : taskId?.startsWith("hermes:") ? "hermes" : undefined);
        if (owner === "zcode") {
            await this.zcode.execute(actionId, { selectedTask, taskId });
            return;
        }
        if (owner === "hermes") {
            await this.hermes.execute(actionId, { selectedTask, taskId });
            return;
        }
        if (owner && owner !== "codex") {
            throw new Error("No action executor is available for " + String(owner));
        }
        await this.codex.execute(actionId, { selectedTask, taskId });
    }

    getSelectedTaskCard(): Record<string, unknown> | null {
        return this.selectedTaskSlot == null
            ? null
            : this.getTaskCard(this.selectedTaskSlot);
    }

    /** Gets the thread status slot for the given index, if present. */
    getSlot(index: number): Record<string, unknown> | null {
        const slot = this.threadStatus.find((s: any) => Number(s.id) === index);
        return (slot as Record<string, unknown>) ?? null;
    }

    /** Gets the explicit task-card slot for the given index, if present. */
    getTaskCard(index: number): Record<string, unknown> | null {
        const card = this.taskCards.find((s: any) => Number(s.id ?? s.slot ?? s.i) === index);
        return (card as Record<string, unknown>) ?? null;
    }

    /** Gets a task card by its stable daemon identity, independent of slot reflow. */
    getTaskCardById(taskId: string): Record<string, unknown> | null {
        const card = this.taskCards.find((candidate) =>
            candidate != null
            && typeof candidate === "object"
            && (candidate as Record<string, unknown>).task_id === taskId);
        return (card as Record<string, unknown>) ?? null;
    }

    /** Registers a listener that is called when render state changes. */
    addListener(fn: () => void): () => void {
        this.listeners.add(fn);
        return () => this.listeners.delete(fn);
    }

    private notifyListeners(): void {
        for (const listener of this.listeners) {
            listener();
        }
    }
}

/** Returns only a real task title, excluding legacy AG labels. */
function descriptiveCardTitle(card: Record<string, unknown>): string | undefined {
    const title = nonEmptyText(card.title);
    if (title) return title;

    const fallback = nonEmptyText(card.t);
    if (!fallback || /^AG0[0-5]$/i.test(fallback)) return undefined;
    return fallback;
}

function nonEmptyText(value: unknown): string | undefined {
    if (typeof value !== "string") return undefined;
    const text = value.trim();
    return text.length > 0 ? text : undefined;
}
