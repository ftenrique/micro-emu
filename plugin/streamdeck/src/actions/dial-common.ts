import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";

/**
 * Click assignment values shared by the crux dial actions.
 * - "native": press the dial's own encoder button (original Codex Micro action)
 * - "ag0".."ag5": agent buttons AG00-AG05 (button indices 0-5)
 * - "act6".."act8": action buttons ACT06-ACT08 (button indices 6-8)
 * - "mic": ACT10 (encoder button 2)
 * - "send": ACT12 (encoder button 0)
 * - "rotor": ENC_CLK (encoder button 1)
 */
export type ClickAssignment = string;

/** Settings shared by the crux dial actions. */
export interface CruxDialSettings {
    [key: string]: JsonValue;
    click?: ClickAssignment;
}

/** Short label describing a click assignment, shown on the touch strip. */
export function clickLabel(click: ClickAssignment | undefined, nativeLabel: string): string {
    switch (click ?? "native") {
        case "native": return nativeLabel;
        case "mic": return "MIC";
        case "send": return "SEND";
        case "rotor": return "CLK";
        default: {
            const match = /^(ag|act)(\d+)$/.exec(click ?? "");
            if (match) {
                return `${match[1].toUpperCase()}${match[2].padStart(2, "0")}`;
            }
            return nativeLabel;
        }
    }
}

/**
 * Dispatches a click assignment press/release to the daemon.
 * `nativeEncoder` is the dial's own encoder index used for "native".
 */
export function sendClick(
    ctx: PluginContext,
    click: ClickAssignment | undefined,
    nativeEncoder: number,
    pressed: boolean,
): void {
    switch (click ?? "native") {
        case "native":
            ctx.daemon.sendEncoderButton(nativeEncoder, pressed);
            return;
        case "send":
            ctx.daemon.sendEncoderButton(0, pressed);
            return;
        case "rotor":
            ctx.daemon.sendEncoderButton(1, pressed);
            return;
        case "mic":
            ctx.daemon.sendEncoderButton(2, pressed);
            return;
        default: {
            const match = /^(?:ag|act)(\d+)$/.exec(click ?? "");
            if (match) {
                ctx.daemon.sendButton(Number(match[1]), pressed);
            } else {
                ctx.daemon.sendEncoderButton(nativeEncoder, pressed);
            }
        }
    }
}
