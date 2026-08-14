import streamDeck, { action, SingletonAction, type KeyAction, type DialAction, type WillAppearEvent, type DidReceiveSettingsEvent, type KeyDownEvent, type KeyUpEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderTaskCardImage, renderDisconnectedImage } from "../images";
import { TASK_SLOT_COUNT } from "../task-slots";

const TIMER_REFRESH_MS = 1_000;
// The ninth card is desktop-only when the attached hardware has fewer keys.
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
    private readonly renderedTargets = new WeakMap<TaskCardInstance, string>();
    private readonly pressedTargets = new WeakMap<TaskCardInstance, { slot: number; taskId: string }>();
    private readonly longPressedActions = new WeakSet<TaskCardInstance>();
    private readonly longPressTimers = new WeakMap<TaskCardInstance, ReturnType<typeof setTimeout>>();
    // Stream Deck key-image uploads are not safe to flood concurrently. Keep
    // one device-wide queue and supersede stale renders for the same action.
    private renderQueue: Promise<void> = Promise.resolve();
    private readonly renderVersions = new WeakMap<TaskCardInstance, number>();
    private readonly actionSettings = new WeakMap<TaskCardInstance, TaskCardSettings>();
    private repairSlot = 0;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
        this.timer = setInterval(() => this.refreshDynamicCards(), TIMER_REFRESH_MS);
    }

    onWillAppear(ev: WillAppearEvent<TaskCardSettings>): void {
        const action = ev.action as TaskCardInstance;
        this.actionSettings.set(action, ev.payload.settings);
        this.enqueueRefresh(action, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<TaskCardSettings>): void {
        const action = ev.action as TaskCardInstance;
        this.actionSettings.set(action, ev.payload.settings);
        this.enqueueRefresh(action, ev.payload.settings);
    }
    onKeyDown(ev: KeyDownEvent<TaskCardSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const action = ev.action as TaskCardInstance;
        const slot = Number(ev.payload.settings.slot ?? 0);
        // Bind the gesture to the identity whose image actually reached this
        // key. Live task state may reflow while the render queue is draining;
        // resolving the slot again here could activate a different task.
        const taskId = this.renderedTargets.get(action);
        if (!taskId) return;
        const target = { slot, taskId };
        this.pressedTargets.set(action, target);
        const previousTimer = this.longPressTimers.get(action);
        if (previousTimer) clearTimeout(previousTimer);
        this.longPressTimers.set(action, setTimeout(() => {
            this.longPressTimers.delete(action);
            if (this.pressedTargets.get(action) !== target) return;
            this.longPressedActions.add(action);
            const current = this.ctx.getTaskCardById(taskId);
            if (isWaitingInteraction(current) && isSelectedTask(this.ctx, taskId) && hasAction(current, "long")) {
                this.ctx.daemon.sendTaskAction(slot, "long", taskId);
            } else {
                this.ctx.daemon.sendTaskToggle(slot, taskId);
            }
        }, LONG_PRESS_MS));
    }

    onKeyUp(ev: KeyUpEvent<TaskCardSettings>): void {
        const action = ev.action as TaskCardInstance;
        const target = this.pressedTargets.get(action);
        // Never emit a release without its matching press (e.g. when the
        // press was swallowed while disconnected); unpaired releases confuse
        // the daemon's key routing.
        if (!target) return;
        this.pressedTargets.delete(action);
        const { slot, taskId } = target;
        const timer = this.longPressTimers.get(action);
        if (timer) clearTimeout(timer);
        this.longPressTimers.delete(action);
        // A completed long press is a native window toggle, not a second task
        // click. Short presses retain the existing task-selection behavior.
        if (this.longPressedActions.delete(action)) return;
        const current = this.ctx.getTaskCardById(taskId);
        if (isWaitingInteraction(current) && isSelectedTask(this.ctx, taskId) && hasAction(current, "short")) {
            this.ctx.daemon.sendTaskAction(slot, "short", taskId);
            return;
        }
        const currentSlot = cardSlot(current);
        if (currentSlot != null) this.ctx.selectTaskSlot(currentSlot);
        this.ctx.daemon.sendTaskButton(slot, true, taskId);
        this.ctx.daemon.sendTaskButton(slot, false, taskId);
        // Codex's Micro protocol has no AG06/AG07 keys. Extra desktop task
        // cards still select through the daemon, then open their actual thread
        // through the local app-server bridge so slots 7-8 remain useful.
        if (isExtendedCodexTask(current)) {
            this.ctx.codex
                .execute("task.open", { selectedTask: current, taskId })
                .catch((error) => this.logRenderFailure(error));
        }
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            const settings = this.actionSettings.get(action);
            if (settings) this.enqueueRefresh(action, settings);
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
            const settings = this.actionSettings.get(action);
            if (!settings) continue;
            const slot = Number(settings.slot ?? 0);
            const card = this.ctx.getTaskCard(slot) ?? this.ctx.getSlot(slot);
            const status = String(card?.status ?? card?.state ?? "").toLowerCase();
            if (isRunningStatus(status) || slot === repairSlot) {
                this.enqueueRefresh(action, settings);
            }
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
            this.renderedTargets.delete(action);
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
            this.renderedTargets.delete(action);
            await action.setImage(renderTaskCardImage(slot, null, "", 0x263238));
            return;
        }
        const taskId = taskIdentity(taskCard);
        if (!taskId) {
            this.renderedTargets.delete(action);
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
        this.renderedTargets.set(action, taskId);
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

function taskIdentity(card: Record<string, unknown>): string | undefined {
    const value = card.task_id;
    return typeof value === "string" && value.length > 0 ? value : undefined;
}

function cardSlot(card: Record<string, unknown> | null): number | undefined {
    const value = Number(card?.id ?? card?.slot ?? card?.i);
    return Number.isFinite(value) ? value : undefined;
}

function isSelectedTask(ctx: PluginContext, taskId: string): boolean {
    return ctx.getSelectedTaskCard()?.task_id === taskId;
}

function isExtendedCodexTask(card: Record<string, unknown> | null): boolean {
    if (!card || card.agent !== "codex") return false;
    const sourceSlot = Number(card.source_slot);
    const taskId = card.task_id;
    return Number.isInteger(sourceSlot)
        && sourceSlot >= 6
        && typeof taskId === "string"
        && taskId.length > 0
        && !taskId.startsWith("legacy:")
        && !taskId.startsWith("codex-hid:");
}
