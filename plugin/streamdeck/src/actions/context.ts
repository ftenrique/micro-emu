import { action, SingletonAction, type DidReceiveSettingsEvent, type KeyAction, type KeyDownEvent, type WillAppearEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderContextKeyImage, type ContextKeyMode } from "../images";

export interface ContextKeySettings {
    [key: string]: JsonValue;
    mode?: ContextKeyMode;
}

/** Shows one LCD-strip context screen on a regular Stream Deck key. */
@action({ UUID: "com.micro-emu.codex.context" })
export class ContextKeyAction extends SingletonAction<ContextKeySettings> {
    private readonly showResetTimes = new WeakMap<object, boolean>();
    constructor(private readonly ctx: PluginContext) {
        super();
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<ContextKeySettings>): void {
        this.refresh(ev.action as KeyAction<ContextKeySettings>, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<ContextKeySettings>): void {
        this.refresh(ev.action as KeyAction<ContextKeySettings>, ev.payload.settings);
    }

    onKeyDown(ev: KeyDownEvent<ContextKeySettings>): void {
        const mode = normalizeMode(ev.payload.settings.mode);
        if (mode === "usage") {
            if (!this.ctx.isConnected()) {
                ev.action.showAlert();
                return;
            }
            const action = ev.action as KeyAction<ContextKeySettings>;
            this.showResetTimes.set(action, !(this.showResetTimes.get(action) ?? false));
            this.refresh(action, ev.payload.settings);
            return;
        }
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        if (mode === "task") this.ctx.daemon.sendCatalogAction("agent.search");
        else this.ctx.daemon.sendModelCycle();
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) =>
                this.refresh(action as KeyAction<ContextKeySettings>, settings));
        }
    }

    private refresh(action: KeyAction<ContextKeySettings>, settings: ContextKeySettings): void {
        const mode = normalizeMode(settings.mode);
        action.setImage(renderContextKeyImage(mode, this.ctx.getSelectedDisplayContext(), this.ctx.isConnected(), this.showResetTimes.get(action) ?? false));
        // The key already carries the mode in its header; avoid a redundant Stream Deck title beneath it.
        action.setTitle("");
    }
}

export function normalizeMode(value: unknown): ContextKeyMode {
    return value === "model" || value === "usage" ? value : "task";
}
