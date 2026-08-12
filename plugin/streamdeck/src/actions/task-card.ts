import streamDeck, { action, SingletonAction, type KeyAction, type DialAction, type WillAppearEvent, type KeyDownEvent, type KeyUpEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderTaskCardImage, renderDisconnectedImage } from "../images";

const TIMER_REFRESH_MS = 1_000;
const TASK_SLOT_COUNT = 6;
const LONG_PRESS_MS = 650;

/** Settings for the Task Card action. */
export interface TaskCardSettings {
    [key: string]: JsonValue;
    /** Task-board slot index (0-based). */
    slot?: number;
}

type TaskCardInstance = KeyAction<TaskCardSettings> | DialAction<TaskCardSettings>;

/** Renders a task-board slot and selects it when pressed. */
@action({ UUID: "com.micro-emu.codex.task" })
export class TaskCardAction extends SingletonAction<TaskCardSettings> {
    private readonly ctx: PluginContext;
    private readonly timer: ReturnType<typeof setInterval>;
    private readonly pressedSlots = new Set<number>();
    private readonly longPressedSlots = new Set<number>();
    private readonly longPressTimers = new Map<number, ReturnType<typeof setTimeout>>();
    // Stream Deck key-image uploads are not safe to flood concurrently. Keep
    // one device-wide queue and supersede stale renders for the same action.
    private renderQueue: Promise<void> = Promise.resolve();
    private readonly renderVersions = new WeakMap<TaskCardInstance, number>();
    private repairSlot = 0;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
        this.timer = setInterval(() => this.refreshDynamicCards(), TIMER_REFRESH_MS);
    }

    onWillAppear(ev: WillAppearEvent<TaskCardSettings>): void {
        this.enqueueRefresh(ev.action as TaskCardInstance, ev.payload.settings);
    }

    onKeyDown(ev: KeyDownEvent<TaskCardSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const slot = Number(ev.payload.settings.slot ?? 0);
        // Only occupied cards are selectable; selecting an empty slot would
        // desync the local highlight from the daemon's selection state.
        const card = this.ctx.getTaskCard(slot) ?? this.ctx.getSlot(slot);
        if (!card || Number(card.e ?? 1) === 0) return;
        this.pressedSlots.add(slot);
        const previousTimer = this.longPressTimers.get(slot);
        if (previousTimer) clearTimeout(previousTimer);
        this.longPressTimers.set(slot, setTimeout(() => {
            this.longPressTimers.delete(slot);
            if (!this.pressedSlots.has(slot)) return;
            this.longPressedSlots.add(slot);
            const current = this.ctx.getTaskCard(slot) ?? this.ctx.getSlot(slot);
            if (isWaitingInteraction(current) && this.ctx.getSelectedTaskSlot() === slot && hasAction(current, "long")) {
                this.ctx.daemon.sendTaskAction(slot, "long");
            } else {
                this.ctx.daemon.sendTaskToggle(slot);
            }
        }, LONG_PRESS_MS));
    }

    onKeyUp(ev: KeyUpEvent<TaskCardSettings>): void {
        const slot = Number(ev.payload.settings.slot ?? 0);
        // Never emit a release without its matching press (e.g. when the
        // press was swallowed while disconnected); unpaired releases confuse
        // the daemon's key routing.
        if (!this.pressedSlots.delete(slot)) return;
        const timer = this.longPressTimers.get(slot);
        if (timer) clearTimeout(timer);
        this.longPressTimers.delete(slot);
        // A completed long press is a native window toggle, not a second task
        // click. Short presses retain the existing task-selection behavior.
        if (this.longPressedSlots.delete(slot)) return;
        const current = this.ctx.getTaskCard(slot) ?? this.ctx.getSlot(slot);
        if (isWaitingInteraction(current) && this.ctx.getSelectedTaskSlot() === slot && hasAction(current, "short")) {
            this.ctx.daemon.sendTaskAction(slot, "short");
            return;
        }
        this.ctx.selectTaskSlot(slot);
        this.ctx.daemon.sendTaskButton(slot, true);
        this.ctx.daemon.sendTaskButton(slot, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings()
                .then((settings) => this.enqueueRefresh(action, settings))
                .catch((error) => this.logRenderFailure(error));
        }
    }

    private refreshDynamicCards(): void {
        if (!this.ctx.isConnected()) return;
        // Running timers repaint every second. One additional slot is swept on
        // each tick so transient hardware upload failures cannot leave stale
        // selection strips or lifecycle colors stuck indefinitely.
        const repairSlot = this.repairSlot;
        this.repairSlot = (this.repairSlot + 1) % TASK_SLOT_COUNT;
        for (const action of this.actions) {
            action.getSettings()
                .then((settings) => {
                    const slot = Number(settings.slot ?? 0);
                    const card = this.ctx.getTaskCard(slot) ?? this.ctx.getSlot(slot);
                    const status = String(card?.status ?? card?.state ?? "").toLowerCase();
                    if (isRunningStatus(status) || slot === repairSlot) {
                        this.enqueueRefresh(action, settings);
                    }
                })
                .catch((error) => this.logRenderFailure(error));
        }
    }

    private enqueueRefresh(action: TaskCardInstance, settings: TaskCardSettings): void {
        const version = (this.renderVersions.get(action) ?? 0) + 1;
        this.renderVersions.set(action, version);
        this.renderQueue = this.renderQueue
            .then(async () => {
                if (this.renderVersions.get(action) !== version) return;
                await this.refresh(action, settings);
            })
            .catch((error) => this.logRenderFailure(error));
    }

    private logRenderFailure(error: unknown): void {
        const message = error instanceof Error ? error.message : String(error);
        streamDeck.logger.warn(`Task-card image update failed: ${message}`);
    }

    private async refresh(action: TaskCardInstance, settings: TaskCardSettings): Promise<void> {
        const slot = Number(settings.slot ?? 0);
        if (!this.ctx.isConnected()) {
            await action.setImage(renderDisconnectedImage(`#${slot}`));
            return;
        }
        const taskCard = this.ctx.getTaskCard(slot) ?? this.ctx.getSlot(slot);
        const enabled = taskCard ? Number(taskCard.e ?? 1) !== 0 : false;
        const status = taskCard ? statusForCard(taskCard) : "";
        const agent = taskCard
            ? ((taskCard.agent as string) ?? null)
            : null;
        const color = taskCard?.color ?? taskCard?.c;
        const selected = this.ctx.getSelectedTaskSlot() === slot;
        // Assigned task records remain visible in every lifecycle state.
        // e:0 is reserved exclusively for an actually empty slot.
        if (!taskCard || (!enabled && !isFinishedStatus(status))) {
            await action.setImage(renderTaskCardImage(slot, null, "", 0x263238));
            return;
        }
        const startedAt = timestamp(taskCard.started_at_ms);
        const finishedAt = timestamp(taskCard.finished_at_ms);
        await action.setImage(renderTaskCardImage(
            slot, agent, status, color,
            startedAt,
            finishedAt,
            selected,
        ));
    }
}

function timestamp(value: unknown): number | undefined {
    const parsed = typeof value === "number" ? value : Number(value);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

function statusForCard(card: Record<string, unknown>): string {
    const explicit = card.status ?? card.state;
    if (typeof explicit === "string" && explicit.length > 0) return explicit;
    // e:0 means the LCD slot is clear. It never means task completion.
    return Number(card.e ?? 0) === 0 ? "" : "idle";
}

function isRunningStatus(status: string): boolean {
    return ["running", "working", "active", "thinking"].includes(status.toLowerCase());
}

function isFinishedStatus(status: string): boolean {
    return ["completed", "complete", "done"].includes(status.toLowerCase());
}


function isWaitingInteraction(card: Record<string, unknown> | null): boolean {
    const state = String(card?.status ?? card?.state ?? "").toLowerCase();
    return state === "waiting" && card?.interaction != null && typeof card.interaction === "object";
}

function hasAction(card: Record<string, unknown> | null, gesture: "short" | "long"): boolean {
    if (!card?.interaction || typeof card.interaction !== "object") return false;
    const action = (card.interaction as Record<string, unknown>)[gesture];
    return action != null && typeof action === "object";
}
