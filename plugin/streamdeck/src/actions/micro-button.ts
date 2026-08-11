import {
    SingletonAction,
    type DidReceiveSettingsEvent,
    type DialAction,
    type KeyAction,
    type KeyDownEvent,
    type KeyUpEvent,
    type WillAppearEvent,
} from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderControlKeyImage } from "../button-icons";

export interface MicroButtonSettings {
    [key: string]: JsonValue;
    /** Micro button index 0-8 (AG00-AG05 or ACT06-ACT08). */
    index?: number;
    /** Visual icon only; behavior is assigned in Codex. */
    icon?: string;
}

type ActionInstance = KeyAction<MicroButtonSettings> | DialAction<MicroButtonSettings>;

const DEFAULT_ICONS: Record<number, string> = {
    0: "agent",
    1: "agent",
    2: "agent",
    3: "agent",
    4: "agent",
    5: "agent",
    6: "new-chat",
    7: "retry",
    8: "stop",
};

/** Shared implementation for the unified control and the hidden legacy alias. */
export abstract class MicroButtonAction extends SingletonAction<MicroButtonSettings> {
    constructor(
        private readonly ctx: PluginContext,
        private readonly defaultIndex: number,
    ) {
        super();
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<MicroButtonSettings>): void {
        this.refresh(ev.action as ActionInstance, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<MicroButtonSettings>): void {
        this.refresh(ev.action as ActionInstance, ev.payload.settings);
    }

    onKeyDown(ev: KeyDownEvent<MicroButtonSettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        this.ctx.daemon.sendMicroKey(this.getCode(ev.payload.settings), true);
    }

    onKeyUp(ev: KeyUpEvent<MicroButtonSettings>): void {
        this.ctx.daemon.sendMicroKey(this.getCode(ev.payload.settings), false);
    }

    private getIndex(settings: MicroButtonSettings): number {
        const index = Number(settings.index ?? this.defaultIndex);
        return Number.isInteger(index) && index >= 0 && index <= 8 ? index : this.defaultIndex;
    }

    private getCode(settings: MicroButtonSettings): string {
        const index = this.getIndex(settings);
        return index <= 5
            ? `AG${String(index).padStart(2, "0")}`
            : `ACT${String(index).padStart(2, "0")}`;
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) => this.refresh(action, settings));
        }
    }

    private refresh(action: ActionInstance, settings: MicroButtonSettings): void {
        const index = this.getIndex(settings);
        const icon = typeof settings.icon === "string" && settings.icon
            ? settings.icon
            : DEFAULT_ICONS[index] ?? "action";
        action.setImage(renderControlKeyImage(icon, this.ctx.isConnected()));
    }
}
