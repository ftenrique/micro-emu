import {
    action,
    SingletonAction,
    type WillAppearEvent,
    type DidReceiveSettingsEvent,
    type DialRotateEvent,
    type DialDownEvent,
    type DialUpEvent,
    type DialAction,
    type TouchTapEvent,
} from "@elgato/streamdeck";
import { PluginContext } from "../context";
import { renderCruxVStrip, renderStripOffline, type StripContext } from "../images";
import {
    clickLabel,
    handleDialDown,
    resolveClick,
    sendClick,
    type CruxDialSettings,
} from "./dial-common";
import { normalizeUsageAgent, type UsageAgent } from "./context";

/** Crux V settings: the dial click assignment plus the usage source shown
 * on the touch strip (independent per dial, so two dials can show codex and
 * zcode side by side). */
export interface CruxVSettings extends CruxDialSettings {
    usageAgent?: UsageAgent;
}

/**
 * Crux Vertical dial — emulates the up/down axis of the original Codex
 * Micro crux. Rotation sends `EncoderTurn` index 2 (radial Y); press
 * sends an assignable action from the shared action catalog (default: the
 * native encoder-2 press, ACT10 / Mic). The touch strip shows the usage
 * limits (5-hourly and weekly) of the configured source agent as bars with
 * exact percentages.
 */
@action({ UUID: "com.micro-emu.codex.crux-v" })
export class CruxVerticalAction extends SingletonAction<CruxVSettings> {
    private readonly ctx: PluginContext;
    private showResetTimes = false;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<CruxVSettings>): void {
        const action = ev.action as DialAction<CruxVSettings>;
        if (action.isDial()) {
            action.setFeedbackLayout("layouts/canvas.json");
        }
        this.refresh(action, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<CruxVSettings>): void {
        this.refresh(ev.action as DialAction<CruxVSettings>, ev.payload.settings);
    }

    onDialRotate(ev: DialRotateEvent<CruxVSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const delta = ev.payload.ticks >= 0 ? 1 : -1;
        this.ctx.daemon.sendEncoderTurn(2, delta);
    }

    onDialDown(ev: DialDownEvent<CruxVSettings>): Promise<void> {
        return handleDialDown(this.ctx, ev.action as DialAction<CruxVSettings>,
            ev.payload.settings.click, 2);
    }

    onDialUp(ev: DialUpEvent<CruxVSettings>): void {
        void sendClick(this.ctx, resolveClick(ev.payload.settings.click, 2), false);
    }
    onTouchTap(ev: TouchTapEvent<CruxVSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        this.showResetTimes = !this.showResetTimes;
        void ev.action.getSettings().then((settings) =>
            this.refresh(ev.action as DialAction<CruxVSettings>, settings));
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) =>
                this.refresh(action as DialAction<CruxVSettings>, settings));
        }
    }

    private refresh(action: DialAction<CruxVSettings>, settings: CruxVSettings): void {
        if (!action.isDial()) return;
        if (!this.ctx.isConnected()) {
            action.setFeedback({ canvas: renderStripOffline("CRUX V") });
            return;
        }
        // Each dial renders its own configured source, so codex and zcode
        // usage can be displayed at the same time on separate dials.
        const agent = normalizeUsageAgent(settings.usageAgent);
        const ctx: StripContext = {
            ...this.ctx.getDisplayContext(),
            ...this.ctx.getUsageFields(agent),
            usage_agent: agent,
        };
        const label = clickLabel(settings.click, "MIC");
        action.setFeedback({ canvas: renderCruxVStrip(ctx, label, this.showResetTimes) });
    }
}
