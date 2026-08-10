import { action, SingletonAction, type KeyAction, type DialAction, type WillAppearEvent, type DidReceiveSettingsEvent, type KeyDownEvent, type KeyUpEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderActionKeyImage, renderDisconnectedImage } from "../images";

/** Default icon per action index, mirroring the Codex Micro defaults. */
const DEFAULT_ICONS: Record<number, string> = {
    6: "new-chat",
    7: "retry",
    8: "stop",
};

/** Settings for the Action Button action. */
export interface ActionButtonSettings {
    [key: string]: JsonValue;
    /** Action index 6-8 (ACT06-ACT08). */
    index?: number;
    /** Icon name indicating the assigned action (see ACTION_ICONS). */
    icon?: string;
}

type ActionInstance = KeyAction<ActionButtonSettings> | DialAction<ActionButtonSettings>;

/**
 * Action Button action — maps a Stream Deck key to Codex Micro ACT06-ACT08.
 */
@action({ UUID: "com.micro-emu.codex.action" })
export class ActionButtonAction extends SingletonAction<ActionButtonSettings> {
    private readonly ctx: PluginContext;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<ActionButtonSettings>): void {
        this.refresh(ev.action as ActionInstance, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<ActionButtonSettings>): void {
        this.refresh(ev.action as ActionInstance, ev.payload.settings);
    }

    onKeyDown(ev: KeyDownEvent<ActionButtonSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const index = Number(ev.payload.settings.index ?? 6);
        this.ctx.daemon.sendButton(index, true);
    }

    onKeyUp(ev: KeyUpEvent<ActionButtonSettings>): void {
        const index = Number(ev.payload.settings.index ?? 6);
        this.ctx.daemon.sendButton(index, false);
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) => this.refresh(action, settings));
        }
    }

    private refresh(action: ActionInstance, settings: ActionButtonSettings): void {
        const index = Number(settings.index ?? 6);
        const label = `ACT${String(index).padStart(2, "0")}`;
        if (!this.ctx.isConnected()) {
            action.setImage(renderDisconnectedImage(label));
            return;
        }
        const icon = (settings.icon as string) || DEFAULT_ICONS[index] || "action";
        action.setImage(renderActionKeyImage(label, icon, index));
    }
}
