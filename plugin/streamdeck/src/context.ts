import { DaemonClient, DisplayContext } from "./daemon-client";

/**
 * Shared plugin context: holds the daemon client and the latest render state
 * so action classes can update their key images when state arrives.
 */
export class PluginContext {
    readonly daemon: DaemonClient;
    private threadStatus: unknown[] = [];
    private taskCards: unknown[] = [];
    private displayContext: DisplayContext | null = null;
    private connected = false;

    /** Callbacks invoked when render state changes. */
    private listeners: Set<() => void> = new Set();

    constructor(daemon: DaemonClient) {
        this.daemon = daemon;

        daemon.on("connect", () => {
            this.connected = true;
            this.notifyListeners();
        });
        daemon.on("disconnect", () => {
            this.connected = false;
            this.notifyListeners();
        });
        daemon.on("render:threadStatus", (status: unknown[]) => {
            this.threadStatus = status;
            this.notifyListeners();
        });
        daemon.on("render:taskCards", (cards: unknown[]) => {
            this.taskCards = cards;
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

    /** Gets the thread status slot for the given index, if present. */
    getSlot(index: number): Record<string, unknown> | null {
        const slot = this.threadStatus.find((s: any) => Number(s.id) === index);
        return (slot as Record<string, unknown>) ?? null;
    }

    /** Gets the explicit task-card slot for the given index, if present. */
    getTaskCard(index: number): Record<string, unknown> | null {
        const card = this.taskCards.find((s: any) => Number(s.id ?? s.slot) === index);
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
