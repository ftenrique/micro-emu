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
import { resolveCatalogAction } from "../action-catalog";
import { catalogActionNeedsBridge, dispatchCatalogAction } from "../action-dispatch";

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
        if (catalogActionNeedsBridge(item) && !this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        try {
            await dispatchCatalogAction(this.ctx, item, true);
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
        const item = resolveCatalogAction(ev.payload.settings.actionId, ev.payload.settings.index);
        void dispatchCatalogAction(this.ctx, item, false);
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
