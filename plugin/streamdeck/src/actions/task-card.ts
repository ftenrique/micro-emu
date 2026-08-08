import { action, SingletonAction, type KeyAction, type DialAction, type WillAppearEvent, type KeyDownEvent, type KeyUpEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderTaskCardImage, renderDisconnectedImage } from "../images";

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

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<TaskCardSettings>): void {
        this.refresh(ev.action as TaskCardInstance, ev.payload.settings);
    }

    onKeyDown(ev: KeyDownEvent<TaskCardSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const slot = Number(ev.payload.settings.slot ?? 0);
        this.ctx.daemon.sendButton(slot, true);
    }

    onKeyUp(ev: KeyUpEvent<TaskCardSettings>): void {
        const slot = Number(ev.payload.settings.slot ?? 0);
        this.ctx.daemon.sendButton(slot, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) => this.refresh(action, settings));
        }
    }

    private refresh(action: TaskCardInstance, settings: TaskCardSettings): void {
        const slot = Number(settings.slot ?? 0);
        if (!this.ctx.isConnected()) {
            action.setImage(renderDisconnectedImage(`#${slot}`));
            return;
        }
        const taskCard = this.ctx.getTaskCard(slot);
        const status = taskCard
            ? ((taskCard.status as string) ?? (taskCard.state as string) ?? "")
            : "";
        const agent = taskCard
            ? ((taskCard.agent as string) ?? null)
            : null;
        const color = taskCard?.color ?? taskCard?.c;
        action.setImage(renderTaskCardImage(slot, agent, status, color));
    }
}
