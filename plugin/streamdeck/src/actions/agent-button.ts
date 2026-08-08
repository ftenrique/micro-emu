import { action, SingletonAction, type WillAppearEvent, type KeyDownEvent, type KeyUpEvent, type KeyAction, type DialAction } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderKeyImage, renderDisconnectedImage } from "../images";

/** Settings for the Agent Button action. */
export interface AgentButtonSettings {
    [key: string]: JsonValue;
    /** Agent index 0-5 (AG00-AG05). */
    index?: number;
}

type ActionInstance = KeyAction<AgentButtonSettings> | DialAction<AgentButtonSettings>;

/**
 * Agent Button action — maps a Stream Deck key to Codex Micro AG00-AG05.
 * The key image reflects the thread status color from the daemon.
 */
@action({ UUID: "com.micro-emu.codex.agent" })
export class AgentButtonAction extends SingletonAction<AgentButtonSettings> {
    private readonly ctx: PluginContext;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<AgentButtonSettings>): void {
        this.refresh(ev.action as ActionInstance, ev.payload.settings);
    }

    onKeyDown(ev: KeyDownEvent<AgentButtonSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const index = Number(ev.payload.settings.index ?? 0);
        this.ctx.daemon.sendButton(index, true);
    }

    onKeyUp(ev: KeyUpEvent<AgentButtonSettings>): void {
        const index = Number(ev.payload.settings.index ?? 0);
        this.ctx.daemon.sendButton(index, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) => this.refresh(action, settings));
        }
    }

    private refresh(action: ActionInstance, settings: AgentButtonSettings): void {
        const index = Number(settings.index ?? 0);
        if (!this.ctx.isConnected()) {
            action.setImage(renderDisconnectedImage(`AG0${index}`));
            return;
        }
        const slot = this.ctx.getSlot(index);
        const status = (slot?.status as string) ?? "";
        action.setImage(renderKeyImage(`AG0${index}`, status, index));
    }
}
