import { DaemonClient, DisplayContext } from "./daemon-client";
import { CodexActionExecutor } from "./codex-action-executor";

/**
 * Shared plugin context: holds the daemon client and the latest render state
 * so action classes can update their key images when state arrives.
 */
export class PluginContext {
    readonly daemon: DaemonClient;
    readonly codex: CodexActionExecutor;
    private threadStatus: unknown[] = [];
    private taskCards: unknown[] = [];
    private displayContext: DisplayContext | null = null;
    private selectedTaskSlot: number | null = null;
    private connected = false;

    /** Callbacks invoked when render state changes. */
    private listeners: Set<() => void> = new Set();

    constructor(daemon: DaemonClient, codex = new CodexActionExecutor()) {
        this.daemon = daemon;
        this.codex = codex;

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

    selectTaskSlot(slot: number): void {
        this.selectedTaskSlot = slot;
        this.notifyListeners();
    }

    getSelectedTaskSlot(): number | null {
        return this.selectedTaskSlot;
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
