import {
    action,
    SingletonAction,
    type DidReceiveSettingsEvent,
    type DialAction,
    type KeyAction,
    type KeyDownEvent,
    type KeyUpEvent,
    type WillAppearEvent,
} from "@elgato/streamdeck";
import streamDeck from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderControlKeyImage } from "../button-icons";
import { resolveCatalogAction, type ActionCatalogItem } from "../action-catalog";

/** Settings for a catalog-driven Action Button. */
export interface ActionButtonSettings {
    [key: string]: JsonValue;
    actionId?: string;
    /** Empty means the catalog action's automatic icon. */
    icon?: string;
    /** Empty means the catalog action's automatic short title. */
    title?: string;
    /** Pre-catalog compatibility; resolved but never written by the new PI. */
    index?: number;
}

type ActionInstance = KeyAction<ActionButtonSettings> | DialAction<ActionButtonSettings>;

/** Selects and executes an entry from the stable action catalog. */
@action({ UUID: "com.micro-emu.codex.action" })
export class ActionButtonAction extends SingletonAction<ActionButtonSettings> {
    constructor(private readonly ctx: PluginContext) {
        super();
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<ActionButtonSettings>): void {
        this.refresh(ev.action as ActionInstance, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<ActionButtonSettings>): void {
        this.refresh(ev.action as ActionInstance, ev.payload.settings);
    }

    async onKeyDown(ev: KeyDownEvent<ActionButtonSettings>): Promise<void> {
        const item = resolveCatalogAction(ev.payload.settings.actionId, ev.payload.settings.index);
        const needsBridge = item.dispatch.kind !== "codex-action"
            && item.dispatch.kind !== "unsupported";
        if (needsBridge && !this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        try {
            await this.dispatch(item, true);
            if (item.dispatch.kind === "codex-action") {
                await ev.action.showOk();
            }
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            streamDeck.logger.error(`Action ${item.id} failed: ${message}`);
            await ev.action.showAlert();
        }
    }

    onKeyUp(ev: KeyUpEvent<ActionButtonSettings>): void {
        this.dispatch(resolveCatalogAction(ev.payload.settings.actionId, ev.payload.settings.index), false);
    }

    private async dispatch(item: ActionCatalogItem, pressed: boolean): Promise<void> {
        switch (item.dispatch.kind) {
            case "micro-key":
                this.ctx.daemon.sendMicroKey(item.dispatch.key, pressed);
                break;
            case "encoder-button":
                this.ctx.daemon.sendEncoderButton(item.dispatch.index, pressed);
                break;
            case "encoder-turn":
                if (pressed) this.ctx.daemon.sendEncoderTurn(item.dispatch.index, item.dispatch.delta);
                break;
            case "catalog-action":
                if (pressed) this.ctx.daemon.sendCatalogAction(item.id);
                break;
            case "codex-action":
                if (pressed) {
                    await this.ctx.executeSelectedAgentAction(item.id);
                }
                break;
            case "unsupported":
                if (pressed) throw new Error(item.dispatch.reason);
                break;
        }
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) => this.refresh(action, settings));
        }
    }

    private refresh(action: ActionInstance, settings: ActionButtonSettings): void {
        const item = resolveCatalogAction(settings.actionId, settings.index);
        const icon = typeof settings.icon === "string" && settings.icon
            ? settings.icon
            : item.icon;
        const customTitle = typeof settings.title === "string" ? settings.title.trim() : "";
        const available = item.dispatch.kind === "codex-action"
            || (item.dispatch.kind !== "unsupported" && this.ctx.isConnected());
        action.setImage(renderControlKeyImage(icon, available));
        action.setTitle(customTitle || item.title);
    }
}
