import { action, SingletonAction, type KeyAction, type DialAction, type WillAppearEvent, type KeyDownEvent, type KeyUpEvent } from "@elgato/streamdeck";
import { PluginContext } from "../context";
import { renderActionKeyImage, renderDisconnectedImage } from "../images";

/**
 * Send action — maps a Stream Deck key to Codex Micro ACT12 (send to Codex).
 * Emits EncoderButton index 0 (which the bridge maps to ACT12).
 */
@action({ UUID: "com.micro-emu.codex.send" })
export class SendAction extends SingletonAction {
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
        this.ctx.daemon.sendEncoderButton(0, true);
    }

    onKeyUp(): void {
        this.ctx.daemon.sendEncoderButton(0, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            this.refresh(action);
        }
    }

    private refresh(action: KeyAction | DialAction): void {
        if (!this.ctx.isConnected()) {
            action.setImage(renderDisconnectedImage("SEND"));
            return;
        }
        action.setImage(renderActionKeyImage("SEND", "send", undefined, 1.8, "#000"));
    }
}
