import { action, SingletonAction, type KeyAction, type DialAction, type WillAppearEvent, type KeyDownEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderKeyImage, renderDisconnectedImage } from "../images";

/** Settings for the Arrow Key action. */
export interface ArrowKeySettings {
    [key: string]: JsonValue;
    /**
     * Arrow direction. Each direction maps to an encoder turn event,
     * mirroring the XL virtual layout:
     * - "up"    → EncoderTurn index 2, delta +1
     * - "down"  → EncoderTurn index 2, delta -1
     * - "left"  → EncoderTurn index 0, delta -1
     * - "right" → EncoderTurn index 0, delta +1
     * - "rotor-ccw" → EncoderTurn index 1, delta -1
     * - "rotor-cw"  → EncoderTurn index 1, delta +1
     */
    direction?: string;
}

type ArrowKeyInstance = KeyAction<ArrowKeySettings> | DialAction<ArrowKeySettings>;

/** Maps a direction string to (encoderIndex, delta). */
const DIRECTION_MAP: Record<string, { index: number; delta: number }> = {
    up: { index: 2, delta: 1 },
    down: { index: 2, delta: -1 },
    left: { index: 0, delta: -1 },
    right: { index: 0, delta: 1 },
    "rotor-ccw": { index: 1, delta: -1 },
    "rotor-cw": { index: 1, delta: 1 },
};

/**
 * Arrow Key action — emits encoder turn events on key press, mirroring the
 * XL virtual layout. Allows keypad-only decks to navigate without dials.
 */
@action({ UUID: "com.micro-emu.codex.arrow" })
export class ArrowKeyAction extends SingletonAction<ArrowKeySettings> {
    private readonly ctx: PluginContext;

    constructor(ctx: PluginContext) {
        super();
        this.ctx = ctx;
        this.ctx.addListener(() => this.refreshAll());
    }

    onWillAppear(ev: WillAppearEvent<ArrowKeySettings>): void {
        this.refresh(ev.action as ArrowKeyInstance, ev.payload.settings);
    }

    onKeyDown(ev: KeyDownEvent<ArrowKeySettings>): void {
        if (!this.ctx.isConnected()) {
            ev.action.showAlert();
            return;
        }
        const direction = ev.payload.settings.direction ?? "up";
        const mapping = DIRECTION_MAP[direction];
        if (mapping) {
            this.ctx.daemon.sendEncoderTurn(mapping.index, mapping.delta);
        }
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) => this.refresh(action, settings));
        }
    }

    private refresh(action: ArrowKeyInstance, settings: ArrowKeySettings): void {
        const direction = settings.direction ?? "up";
        if (!this.ctx.isConnected()) {
            action.setImage(renderDisconnectedImage(direction.toUpperCase()));
            return;
        }
        action.setImage(renderKeyImage(getArrowChar(direction), undefined));
    }
}

function getArrowChar(direction: string): string {
    switch (direction) {
        case "up": return "↑";
        case "down": return "↓";
        case "left": return "←";
        case "right": return "→";
        case "rotor-ccw": return "↺";
        case "rotor-cw": return "↻";
        default: return direction;
    }
}
