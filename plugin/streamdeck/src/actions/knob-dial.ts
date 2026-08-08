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
import { renderKnobStrip, renderStripOffline } from "../images";

/**
 * Knob dial — emulates the original Codex Micro rotor knob.
 * Rotation sends `EncoderTurn` index 1 (ENC_CW/ENC_CC); press sends
 * `EncoderButton` index 1 (ENC_CLK). The touch strip shows the selected
 * task info (number, project, shortened name).
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
        this.refresh(ev.action as DialAction);
    }

    onDialRotate(ev: DialRotateEvent): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const ticks = ev.payload.ticks;
        const delta = ticks >= 0 ? 1 : -1;
        for (let i = 0; i < Math.abs(ticks); i++) {
            this.ctx.daemon.sendEncoderTurn(1, delta);
        }
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
        const ctx = this.ctx.getDisplayContext();
        action.setFeedback({ canvas: renderKnobStrip(ctx ?? {}) });
    }
}
