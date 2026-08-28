import { EventEmitter } from "events";
import * as net from "net";
import { spawn, ChildProcess } from "child_process";
import * as path from "path";

/** Default daemon TCP endpoint. */
export const DEFAULT_DAEMON_PORT = 48360;
export const DEFAULT_DAEMON_HOST = "127.0.0.1";
const CONNECT_TIMEOUT_MS = 10_000;
const HEARTBEAT_INTERVAL_MS = 15_000;
const HEARTBEAT_TIMEOUT_MS = 45_000;
const KEEP_ALIVE_INITIAL_DELAY_MS = 10_000;

/** Render state pushed from the daemon to the plugin. */
export interface RenderState {
    threadStatus?: unknown[];
    taskCards?: unknown[];
    displayContext?: DisplayContext;
    rgbConfig?: unknown;
}

/** Usage fields for a single agent, mirroring the Rust `UsageSnapshot`. */
export interface AgentUsageFields {
    five_hour_remaining?: number | null;
    weekly_remaining?: number | null;
    five_hour_reset_at?: number | null;
    weekly_reset_at?: number | null;
}

/** Display context fields, mirroring the Rust `DisplayContext`. */
export interface DisplayContext {
    project?: string | null;
    task?: string | null;
    model?: string | null;
    effort?: string | null;
    status?: string | null;
    progress?: number | null;
    task_id?: string | null;
    weekly_remaining?: number | null;
    five_hour_remaining?: number | null;
    weekly_reset_at?: number | null;
    five_hour_reset_at?: number | null;
    /** Agent the usage fields belong to ("codex" | "zcode"); bridge-derived. */
    usage_agent?: string | null;
    /** Per-agent usage snapshots pushed by the bridge so displays can render
     * either agent regardless of the globally selected source. */
    agents_usage?: Record<string, AgentUsageFields | null> | null;
    wait_reason?: string | null;
    prompt?: string | null;
    interaction_id?: string | null;
    short_action?: string | null;
    long_action?: string | null;
    pending_wait_count?: number | null;
}

/** Configuration for the daemon client. */
export interface DaemonClientOptions {
    host?: string;
    port?: number;
    /** Path to the bridge executable for autostart. */
    bridgeExe?: string;
    /** Extra args to pass to the daemon on autostart. */
    daemonArgs?: string[];
    /** Working directory for the daemon process. */
    cwd?: string;
}

/**
 * TCP line-delimited JSON client for the rp2040-bridge daemon.
 *
 * Sends a controller hello on connect, forwards inbound render lines as
 * events, and provides methods to send controller events (button presses,
 * encoder turns) and capacity updates.
 */
export class DaemonClient extends EventEmitter {
    private socket: net.Socket | null = null;
    private buffer = "";
    private reconnectDelay = 250;
    private readonly maxReconnectDelay = 2000;
    private reconnectTimer: NodeJS.Timeout | null = null;
    private connectTimer: NodeJS.Timeout | null = null;
    private heartbeatTimer: NodeJS.Timeout | null = null;
    private daemonProcess: ChildProcess | null = null;
    private connected = false;
    private connecting = false;
    private stopped = true;
    private autostartAttempted = false;
    private lastInboundAt = 0;
    private taskSlots = 0;
    private slotAgents: Record<string, string> = {};
    private instanceId: string;
    /** Pending new-task round trips, resolved in request order. */
    private newTaskWaiters: Array<(handled: boolean) => void> = [];
    private options: Required<Pick<DaemonClientOptions, "host" | "port">> & DaemonClientOptions;

    constructor(options: DaemonClientOptions = {}) {
        super();
        this.instanceId = `plugin-${process.pid}-${Date.now()}`;
        this.options = {
            host: options.host ?? DEFAULT_DAEMON_HOST,
            port: options.port ?? DEFAULT_DAEMON_PORT,
            bridgeExe: options.bridgeExe,
            daemonArgs: options.daemonArgs,
            cwd: options.cwd,
        };
    }

    /** Whether the client is currently connected to the daemon. */
    isConnected(): boolean {
        return this.connected;
    }

    /** Updates the task-slot capacity and notifies the daemon. */
    setTaskSlots(slots: number): void {
        this.taskSlots = slots;
        if (this.connected) {
            this.send({ type: "capacity", taskSlots: slots, slotAgents: this.slotAgents });
        }
    }

    setTaskSlotAgent(slot: number, agent: string): void {
        if (agent === "auto") delete this.slotAgents[String(slot)];
        else this.slotAgents[String(slot)] = agent;
        if (this.connected) {
            this.send({ type: "capacity", taskSlots: this.taskSlots, slotAgents: this.slotAgents });
        }
    }

    /** Starts the client: connects (and autostarts if needed). */
    start(): void {
        if (!this.stopped) return;
        this.stopped = false;
        this.connect();
    }

    /** Forces a fresh daemon session, including while a socket is half-open. */
    reconnect(reason = "requested"): void {
        if (this.stopped) return;
        this.emit(
            "log",
            `refreshing daemon connection (${reason})`,
        );
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = null;
        }
        this.clearConnectTimer();
        this.clearHeartbeat();
        if (this.socket) {
            this.socket.destroy();
            return;
        }
        this.buffer = "";
        this.connected = false;
        this.connecting = false;
        this.connect();
    }

    /** Stops the client and cleans up. */
    stop(): void {
        this.stopped = true;
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = null;
        }
        this.clearConnectTimer();
        this.clearHeartbeat();
        if (this.socket) {
            this.socket.destroy();
            this.socket = null;
        }
        this.connected = false;
        this.connecting = false;
        this.failNewTaskWaiters();
    }

    /** Sends a legacy physical button event to the daemon. */
    sendButton(index: number, pressed: boolean): void {
        this.send({ type: "event", kind: "button", index, pressed });
    }

    /** Selects the task that was rendered on a task-board slot. */
    sendTaskButton(index: number, pressed: boolean, taskId: string): void {
        this.send({ type: "event", kind: "task-button", index, pressed, task_id: taskId });
    }

    /** Requests the native Codex window toggle for an occupied task slot. */
    sendTaskAction(index: number, gesture: "short" | "long", taskId: string): void {
        this.send({ type: "event", kind: "task-action", index, gesture, task_id: taskId });
    }

    sendTaskToggle(index: number, taskId: string): void {
        this.send({ type: "event", kind: "task-toggle", index, task_id: taskId });
    }

    /** Sends an explicit Codex Micro key, bypassing task-card routing. */
    sendMicroKey(key: string, pressed: boolean): void {
        this.send({ type: "event", kind: "micro-key", key, pressed });
    }

    /** Sends a stable logical action from the extended action catalog. */
    sendCatalogAction(action: string): void {
        this.send({ type: "event", kind: "catalog-action", action });
    }

    /** Sends an encoder turn event to the daemon. */
    sendEncoderTurn(index: number, delta: number): void {
        this.send({ type: "event", kind: "encoder-turn", index, delta });
    }

    /** Sends an encoder button event to the daemon. */
    sendEncoderButton(index: number, pressed: boolean): void {
        this.send({ type: "event", kind: "encoder-button", index, pressed });
    }

    /** Asks the daemon to advance the selected Codex task to the next featured model. */
    sendModelCycle(): void {
        this.send({ type: "event", kind: "model-cycle" });
    }

    /** Selects which agent's usage limits feed the usage displays. */
    sendUsageAgent(agent: "codex" | "zcode"): void {
        this.send({ type: "event", kind: "usage-agent", agent });
    }

    /** Asks the daemon to start a new task in the ZCode desktop app.
     * Resolves true when ZCode is the foreground app and the daemon queued
     * the creation; resolves false when ZCode is not focused, the daemon is
     * unreachable, or the reply times out (older bridges never answer). */
    requestZcodeNewTask(timeoutMs = 3_000): Promise<boolean> {
        if (!this.connected) {
            return Promise.resolve(false);
        }
        return new Promise<boolean>((resolve) => {
            let settled = false;
            const finish = (handled: boolean): void => {
                if (settled) return;
                settled = true;
                clearTimeout(timer);
                resolve(handled);
            };
            const timer = setTimeout(() => finish(false), timeoutMs);
            this.newTaskWaiters.push(finish);
            this.send({ type: "new-task" });
        });
    }

    /** Sends a raw JSON line to the daemon. */
    private send(message: unknown): void {
        if (!this.socket || !this.connected) return;
        this.socket.write(JSON.stringify(message) + "\n");
    }
    private connect(): void {
        if (this.stopped || this.connecting || this.connected) return;
        this.connecting = true;
        const socket = new net.Socket();
        this.socket = socket;
        socket.setEncoding("utf-8");
        socket.setNoDelay(true);
        socket.setKeepAlive(true, KEEP_ALIVE_INITIAL_DELAY_MS);
        this.connectTimer = setTimeout(() => {
            if (this.socket !== socket || !this.connecting) return;
            this.emit("log", `daemon connect timed out after ${CONNECT_TIMEOUT_MS}ms; retrying`);
            socket.destroy();
        }, CONNECT_TIMEOUT_MS);

        socket.on("connect", () => {
            if (this.stopped || this.socket !== socket) {
                socket.destroy();
                return;
            }
            this.clearConnectTimer();
            this.connecting = false;
            this.connected = true;
            this.autostartAttempted = false;
            this.reconnectDelay = 250;
            this.socket = socket;
            this.lastInboundAt = Date.now();
            this.startHeartbeat(socket);
            // Send the controller hello.
            const hello = {
                bridge: "hello",
                version: 1,
                role: "controller",
                controller: "streamdeck-plugin",
                instance_id: this.instanceId,
                taskSlots: this.taskSlots,
                slotAgents: this.slotAgents,
            };
            socket.write(JSON.stringify(hello) + "\n");
            this.emit("log", "daemon connected");
            this.emit("connect");
        });

        socket.on("data", (data: string) => {
            if (this.socket !== socket) return;
            this.lastInboundAt = Date.now();
            this.buffer += data;
            let newlineIndex: number;
            while ((newlineIndex = this.buffer.indexOf("\n")) >= 0) {
                const line = this.buffer.slice(0, newlineIndex).trim();
                this.buffer = this.buffer.slice(newlineIndex + 1);
                if (line) {
                    this.handleLine(line);
                }
            }
        });

        socket.on("error", (error: Error) => {
            if (this.socket !== socket) return;
            this.emit("error", error);
            if (!this.connected && !this.autostartAttempted && !this.daemonProcess) {
                this.autostartAttempted = true;
                this.tryAutostart();
            }
        });

        socket.on("close", () => {
            if (this.socket !== socket) return;
            const wasConnected = this.connected;
            this.clearConnectTimer();
            this.clearHeartbeat();
            this.connecting = false;
            this.buffer = "";
            this.connected = false;
            this.socket = null;
            this.failNewTaskWaiters();
            if (wasConnected) {
                this.emit("log", "daemon connection closed; reconnecting");
                this.emit("disconnect");
                if (!this.daemonProcess) {
                    this.autostartAttempted = false;
                }
            }
            if (!this.stopped) {
                this.scheduleReconnect();
            }
        });

        socket.connect(this.options.port, this.options.host);
    }

    /**
     * Render updates are event-driven and may legitimately be quiet for hours,
     * so probe the daemon explicitly instead of treating ordinary inactivity as
     * a disconnect. A suspended process resumes with an old `lastInboundAt`;
     * the first watchdog tick then tears down a half-open socket immediately.
     */
    private startHeartbeat(socket: net.Socket): void {
        this.clearHeartbeat();
        this.heartbeatTimer = setInterval(() => {
            if (this.stopped || !this.connected || this.socket !== socket) return;
            const idleMs = Date.now() - this.lastInboundAt;
            if (idleMs >= HEARTBEAT_TIMEOUT_MS) {
                this.emit(
                    "log",
                    `daemon heartbeat timed out after ${idleMs}ms; reconnecting`,
                );
                socket.destroy();
                return;
            }
            socket.write(JSON.stringify({ type: "ping", timestamp: Date.now() }) + "\n");
        }, HEARTBEAT_INTERVAL_MS);
    }

    private clearConnectTimer(): void {
        if (!this.connectTimer) return;
        clearTimeout(this.connectTimer);
        this.connectTimer = null;
    }

    private clearHeartbeat(): void {
        if (!this.heartbeatTimer) return;
        clearInterval(this.heartbeatTimer);
        this.heartbeatTimer = null;
    }

    private scheduleReconnect(): void {
        if (this.stopped || this.reconnectTimer) return;
        const delay = this.reconnectDelay;
        this.reconnectTimer = setTimeout(() => {
            this.reconnectTimer = null;
            this.reconnectDelay = Math.min(this.reconnectDelay * 2, this.maxReconnectDelay);
            this.connect();
        }, delay);
    }

    private tryAutostart(): void {
        const exe = this.options.bridgeExe;
        if (!exe) {
            this.emit(
                "log",
                "daemon unreachable; bridge executable was not found (set MICRO_EMU_BRIDGE_EXE or reinstall the bridge)",
            );
            return;
        }
        try {
            const args = ["--daemon", ...(this.options.daemonArgs ?? [])];
            this.daemonProcess = spawn(exe, args, {
                cwd: this.options.cwd ?? path.dirname(exe),
                stdio: "ignore",
                detached: false,
                windowsHide: true,
            });
            this.daemonProcess.on("error", (error: Error) => {
                this.emit("log", `daemon autostart failed: ${error.message}`);
                this.daemonProcess = null;
                this.autostartAttempted = false;
            });
            this.daemonProcess.on("exit", () => {
                this.daemonProcess = null;
                this.autostartAttempted = false;
            });
            this.emit("log", "daemon autostarted");
        } catch (error) {
            this.daemonProcess = null;
            this.autostartAttempted = false;
            this.emit("log", `daemon autostart error: ${error}`);
        }
    }

    private handleLine(line: string): void {
        let message: any;
        try {
            message = JSON.parse(line);
        } catch {
            return;
        }
        const type = message.type;
        if (type === "render") {
            const render = message.render;
            if (render === "threadStatus") {
                this.emit("render:threadStatus", message.threadStatus);
            } else if (render === "taskCards") {
                this.emit("render:taskCards", message.taskCards);
            } else if (render === "displayContext") {
                this.emit("render:displayContext", message.displayContext as DisplayContext);
            } else if (render === "rgbConfig") {
                this.emit("render:rgbConfig", message.rgbConfig);
            }
            this.emit("render", message as RenderState);
        } else if (type === "goodbye") {
            this.emit("goodbye");
            // A goodbye means this controller was detached. Do not wait for
            // TCP teardown to become observable before entering reconnect.
            this.socket?.destroy();
        } else if (type === "new-task-result") {
            // Replies pair with requests in order; a waiter that already
            // timed out stays settled and swallows its late reply.
            this.newTaskWaiters.shift()?.(message.handled === true);
        }
    }

    /** Resolves every pending new-task round trip so callers fall back to
     * the Codex screen instead of waiting out their timeouts. */
    private failNewTaskWaiters(): void {
        for (const waiter of this.newTaskWaiters.splice(0)) {
            waiter(false);
        }
    }
}
