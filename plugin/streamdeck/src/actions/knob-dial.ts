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
        // Stream Deck reports this dial's physical direction opposite to the
        // Codex Micro rotor convention (positive = ENC_CW), so invert it.
        const delta = ev.payload.ticks >= 0 ? -1 : 1;
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
        const card = this.ctx.getSelectedTaskCard();
        if (card) {
            stripCtx.task_id = nonEmptyText(card.task_id) ?? stripCtx.task_id;
            const sourceSlot = Number(card.source_slot ?? card.id ?? card.slot ?? card.i);
            if (Number.isFinite(sourceSlot)) stripCtx.task_number = sourceSlot + 1;
            stripCtx.task = descriptiveCardTitle(card) ?? stripCtx.task;
            stripCtx.project = nonEmptyText(card.project) ?? stripCtx.project;
            stripCtx.model = nonEmptyText(card.model) ?? stripCtx.model;
            stripCtx.effort = nonEmptyText(card.effort) ?? stripCtx.effort;
            stripCtx.status = nonEmptyText(card.status ?? card.state) ?? stripCtx.status;
            if (card.progress != null) {
                const progress = Number(card.progress);
                if (Number.isFinite(progress)) stripCtx.progress = progress;
            }
        }
        action.setFeedback({ canvas: renderKnobStrip(stripCtx) });
    }
}

/**
 * Returns only a real task title. The bridge's `t` field is also used for the
 * legacy HID key fallback (AG00-AG05); that protocol label must never replace
 * the descriptive title already supplied by the selected display context.
 */
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
