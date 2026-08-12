import { EventEmitter } from "events";
import * as net from "net";
import { spawn, ChildProcess } from "child_process";
import * as path from "path";

/** Default daemon TCP endpoint. */
export const DEFAULT_DAEMON_PORT = 48360;
export const DEFAULT_DAEMON_HOST = "127.0.0.1";

/** Render state pushed from the daemon to the plugin. */
export interface RenderState {
    threadStatus?: unknown[];
    taskCards?: unknown[];
    displayContext?: DisplayContext;
    rgbConfig?: unknown;
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
    private daemonProcess: ChildProcess | null = null;
    private connected = false;
    private connecting = false;
    private stopped = true;
    private autostartAttempted = false;
    private taskSlots = 0;
    private instanceId: string;
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
            this.send({ type: "capacity", taskSlots: slots });
        }
    }

    /** Starts the client: connects (and autostarts if needed). */
    start(): void {
        if (!this.stopped) return;
        this.stopped = false;
        this.connect();
    }

    /** Stops the client and cleans up. */
    stop(): void {
        this.stopped = true;
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = null;
        }
        if (this.socket) {
            this.socket.destroy();
            this.socket = null;
        }
        this.connected = false;
        this.connecting = false;
    }

    /** Sends a legacy physical button event to the daemon. */
    sendButton(index: number, pressed: boolean): void {
        this.send({ type: "event", kind: "button", index, pressed });
    }

    /** Selects a task-board slot without reinterpreting it as a Micro key. */
    sendTaskButton(index: number, pressed: boolean): void {
        this.send({ type: "event", kind: "task-button", index, pressed });
    }

    /** Requests the native Codex window toggle for an occupied task slot. */
    sendTaskAction(index: number, gesture: "short" | "long"): void {
        this.send({ type: "event", kind: "task-action", index, gesture });
    }

    sendTaskToggle(index: number): void {
        this.send({ type: "event", kind: "task-toggle", index });
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

        socket.on("connect", () => {
            this.connecting = false;
            this.connected = true;
            this.reconnectDelay = 250;
            this.socket = socket;
            // Send the controller hello.
            const hello = {
                bridge: "hello",
                version: 1,
                role: "controller",
                controller: "streamdeck-plugin",
                instance_id: this.instanceId,
                taskSlots: this.taskSlots,
            };
            socket.write(JSON.stringify(hello) + "\n");
            this.emit("connect");
        });

        socket.on("data", (data: string) => {
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
            this.emit("error", error);
            if (!this.connected && !this.autostartAttempted) {
                this.autostartAttempted = true;
                this.tryAutostart();
            }
        });

        socket.on("close", () => {
            const wasConnected = this.connected;
            this.connecting = false;
            this.buffer = "";
            this.connected = false;
            if (this.socket === socket) {
                this.socket = null;
            }
            if (wasConnected) {
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
            this.emit("log", "daemon unreachable and no bridgeExe configured for autostart");
            return;
        }
        try {
            const args = ["--daemon", ...(this.options.daemonArgs ?? [])];
            this.daemonProcess = spawn(exe, args, {
                cwd: this.options.cwd,
                stdio: "ignore",
                detached: false,
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
        }
    }
}
