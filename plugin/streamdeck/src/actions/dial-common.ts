import type { JsonValue } from "@elgato/utils";
import streamDeck from "@elgato/streamdeck";
import type { DialAction } from "@elgato/streamdeck";
import {
    findCatalogAction,
    resolveCatalogAction,
    type ActionCatalogItem,
} from "../action-catalog";
import { catalogActionNeedsBridge, dispatchCatalogAction } from "../action-dispatch";
import type { PluginContext } from "../context";

/**
 * Click assignment values shared by the crux dial actions. A value is
 * either "native" (the dial's own encoder press, the original Codex Micro
 * action), a legacy pre-catalog token ("ag0".."ag5", "act6".."act8",
 * "mic", "send", "rotor"), or the id of an entry from the shared action
 * catalog — the same set the Action Button offers.
 */
export type ClickAssignment = string;

/** Settings shared by the crux dial actions. */
export interface CruxDialSettings {
    [key: string]: JsonValue;
    click?: ClickAssignment;
}

/** Pre-catalog click tokens and their catalog equivalents. */
const LEGACY_CLICK_IDS: Readonly<Record<string, string>> = {
    mic: "micro.mic",
    send: "micro.send",
    rotor: "micro.rotor-click",
    act6: "micro.act06",
    act7: "micro.act07",
    act8: "micro.act08",
};

/** What a dial press resolves to: the dial's own encoder button or a
 * shared catalog action. */
export type ClickTarget =
    | { kind: "native"; encoder: number }
    | { kind: "catalog"; item: ActionCatalogItem };

/** Resolves a persisted click setting; unknown values fall back to the
 * dial's native encoder press. */
export function resolveClick(
    click: ClickAssignment | undefined,
    nativeEncoder: number,
): ClickTarget {
    const value = click ?? "native";
    if (value === "native") return { kind: "native", encoder: nativeEncoder };
    const legacyId = LEGACY_CLICK_IDS[value];
    if (legacyId) {
        const item = findCatalogAction(legacyId);
        if (item) return { kind: "catalog", item };
    }
    const agMatch = /^ag([0-5])$/.exec(value);
    if (agMatch) {
        return { kind: "catalog", item: resolveCatalogAction(undefined, Number(agMatch[1])) };
    }
    const item = findCatalogAction(value);
    if (item) return { kind: "catalog", item };
    return { kind: "native", encoder: nativeEncoder };
}

/** True when the target is routed through the rp2040 bridge and needs an
 * active daemon connection. */
export function clickNeedsBridge(target: ClickTarget): boolean {
    return target.kind === "native" || catalogActionNeedsBridge(target.item);
}

/**
 * Dispatches a click press/release to the daemon. Catalog targets use the
 * same dispatch path as the Action Button; one-shot actions fire on press
 * only and reject on failure.
 */
export function sendClick(
    ctx: PluginContext,
    target: ClickTarget,
    pressed: boolean,
): Promise<void> {
    if (target.kind === "native") {
        ctx.daemon.sendEncoderButton(target.encoder, pressed);
        return Promise.resolve();
    }
    return dispatchCatalogAction(ctx, target.item, pressed);
}

/** Short label describing a click assignment, shown on the touch strip. */
export function clickLabel(click: ClickAssignment | undefined, nativeLabel: string): string {
    const target = resolveClick(click, -1);
    return target.kind === "native" ? nativeLabel : target.item.title;
}

/**
 * Shared dial-press handling for the crux actions: resolves the click
 * assignment and dispatches it like the Action Button, alerting when the
 * bridge is required but offline or when the action fails. Dials have no
 * success flash (showOk is keypad-only), so Codex workflow actions only
 * surface failures.
 */
export async function handleDialDown<S extends CruxDialSettings>(
    ctx: PluginContext,
    action: DialAction<S>,
    click: ClickAssignment | undefined,
    nativeEncoder: number,
): Promise<void> {
    const target = resolveClick(click, nativeEncoder);
    if (clickNeedsBridge(target) && !ctx.isConnected()) {
        await action.showAlert();
        return;
    }
    try {
        await sendClick(ctx, target, true);
    } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        const id = target.kind === "catalog" ? target.item.id : `native-${nativeEncoder}`;
        streamDeck.logger.error(`Crux dial click ${id} failed: ${message}`);
        await action.showAlert();
    }
}
