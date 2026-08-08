import {
    action,
    SingletonAction,
    type WillAppearEvent,
    type DialRotateEvent,
    type DialDownEvent,
    type DialUpEvent,
    type DialAction,
} from "@elgato/streamdeck";
import { PluginContext } from "../context";
import { renderKnobStrip, renderStripOffline, type StripContext } from "../images";

/**
 * Knob dial — emulates the original Codex Micro rotor knob.
 * Rotation sends `EncoderTurn` index 1 (ENC_CW/ENC_CC); press sends
 * `EncoderButton` index 1 (ENC_CLK). The touch strip shows the selected
 * task info (number, project, shortened name), merging display context
 * with task-card data when available.
 */
@action({ UUID: "com.micro-emu.codex.knob" })
export class KnobDialAction extends SingletonAction {
    private readonly ctx: PluginContext;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent): void {
        const action = ev.action as DialAction;
        if (action.isDial()) {
            action.setFeedbackLayout("layouts/canvas.json");
        }
        this.refresh(action);
    }

    onDialRotate(ev: DialRotateEvent): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const delta = ev.payload.ticks >= 0 ? 1 : -1;
        this.ctx.daemon.sendEncoderTurn(1, delta);
    }

    onDialDown(ev: DialDownEvent): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        this.ctx.daemon.sendEncoderButton(1, true);
    }

    onDialUp(): void {
        this.ctx.daemon.sendEncoderButton(1, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            this.refresh(action as DialAction);
        }
    }

    private refresh(action: DialAction): void {
        if (!action.isDial()) return;
        if (!this.ctx.isConnected()) {
            action.setFeedback({ canvas: renderStripOffline("KNOB") });
            return;
        }
        const displayCtx = this.ctx.getDisplayContext();
        const stripCtx: StripContext = { ...(displayCtx ?? {}) };
        // Enrich with task-card data: find the first assigned task card
        // to show its task_id and title when display context lacks them.
        for (let i = 0; i < 8; i++) {
            const card = this.ctx.getTaskCard(i);
            if (card && (card.e as number) !== 0) {
                if (!stripCtx.task_id) {
                    stripCtx.task_id = (card.task_id as string) ?? String(i);
                }
                if (!stripCtx.task || stripCtx.task === "BRIDGE") {
                    stripCtx.task = (card.t as string) ?? stripCtx.task;
                }
                break;
            }
        }
        action.setFeedback({ canvas: renderKnobStrip(stripCtx) });
    }
}
