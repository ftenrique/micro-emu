import {
    action,
    SingletonAction,
    type WillAppearEvent,
    type DidReceiveSettingsEvent,
    type DialRotateEvent,
    type DialDownEvent,
    type DialUpEvent,
    type DialAction,
} from "@elgato/streamdeck";
import { PluginContext } from "../context";
import { renderCruxHStrip, renderStripOffline } from "../images";
import { clickLabel, sendClick, type CruxDialSettings } from "./dial-common";

/**
 * Crux Horizontal dial — emulates the left/right axis of the original
 * Codex Micro crux. Rotation sends `EncoderTurn` index 0 (radial X);
 * press sends an assignable click action (default: the native encoder-0
 * press, ACT12 / Send). The touch strip shows the model and effort of
 * the currently selected task.
 */
@action({ UUID: "com.micro-emu.codex.crux-h" })
export class CruxHorizontalAction extends SingletonAction<CruxDialSettings> {
    private readonly ctx: PluginContext;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<CruxDialSettings>): void {
        const action = ev.action as DialAction<CruxDialSettings>;
        if (action.isDial()) {
            action.setFeedbackLayout("layouts/canvas.json");
        }
        this.refresh(action, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<CruxDialSettings>): void {
        this.refresh(ev.action as DialAction<CruxDialSettings>, ev.payload.settings);
    }

    onDialRotate(ev: DialRotateEvent<CruxDialSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const delta = ev.payload.ticks >= 0 ? 1 : -1;
        this.ctx.daemon.sendEncoderTurn(0, delta);
    }

    onDialDown(ev: DialDownEvent<CruxDialSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        sendClick(this.ctx, ev.payload.settings.click, 0, true);
    }

    onDialUp(ev: DialUpEvent<CruxDialSettings>): void {
        sendClick(this.ctx, ev.payload.settings.click, 0, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) =>
                this.refresh(action as DialAction<CruxDialSettings>, settings));
        }
    }

    private refresh(action: DialAction<CruxDialSettings>, settings: CruxDialSettings): void {
        if (!action.isDial()) return;
        if (!this.ctx.isConnected()) {
            action.setFeedback({ canvas: renderStripOffline("CRUX H") });
            return;
        }
        const ctx = this.ctx.getDisplayContext();
        const label = clickLabel(settings.click, "SEND");
        action.setFeedback({ canvas: renderCruxHStrip(ctx ?? {}, label) });
    }
}