import { action, SingletonAction, type KeyAction, type DialAction, type WillAppearEvent, type KeyDownEvent, type KeyUpEvent } from "@elgato/streamdeck";
import { PluginContext } from "../context";
import { renderKeyImage, renderDisconnectedImage } from "../images";

/**
 * Mic action — maps a Stream Deck key to Codex Micro ACT10 (microphone toggle).
 * Emits EncoderButton index 2 (which the bridge maps to ACT10).
 */
@action({ UUID: "com.micro-emu.codex.mic" })
export class MicAction extends SingletonAction {
    private readonly ctx: PluginContext;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent): void {
        this.refresh(ev.action as KeyAction | DialAction);
    }

    onKeyDown(ev: KeyDownEvent): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        this.ctx.daemon.sendEncoderButton(2, true);
    }

    onKeyUp(): void {
        this.ctx.daemon.sendEncoderButton(2, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            this.refresh(action);
        }
    }

    private refresh(action: KeyAction | DialAction): void {
        if (!this.ctx.isConnected()) {
            action.setImage(renderDisconnectedImage("MIC"));
            return;
        }
        action.setImage(renderKeyImage("MIC", undefined));
    }
}
