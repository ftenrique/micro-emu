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
import { renderCruxVStrip, renderStripOffline } from "../images";
import { clickLabel, sendClick, type CruxDialSettings } from "./dial-common";

/**
 * Crux Vertical dial — emulates the up/down axis of the original Codex
 * Micro crux. Rotation sends `EncoderTurn` index 2 (radial Y); press
 * sends an assignable click action (default: the native encoder-2 press,
 * ACT10 / Mic). The touch strip shows the usage limits (5-hourly and
 * weekly) as bars with exact percentages.
 */
@action({ UUID: "com.micro-emu.codex.crux-v" })
export class CruxVerticalAction extends SingletonAction<CruxDialSettings> {
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
        this.ctx.daemon.sendEncoderTurn(2, delta);
    }

    onDialDown(ev: DialDownEvent<CruxDialSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        sendClick(this.ctx, ev.payload.settings.click, 2, true);
    }

    onDialUp(ev: DialUpEvent<CruxDialSettings>): void {
        sendClick(this.ctx, ev.payload.settings.click, 2, false);
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
            action.setFeedback({ canvas: renderStripOffline("CRUX V") });
            return;
        }
        const ctx = this.ctx.getDisplayContext();
        const label = clickLabel(settings.click, "MIC");
        action.setFeedback({ canvas: renderCruxVStrip(ctx ?? {}, label) });
    }
}
