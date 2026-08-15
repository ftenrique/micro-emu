import type { ActionCatalogItem } from "./action-catalog";
import type { PluginContext } from "./context";

/** True when an action is delivered through the rp2040 bridge and therefore
 * needs an active daemon connection. Codex workflow actions run locally and
 * retired tombstones fail on their own when pressed. */
export function catalogActionNeedsBridge(item: ActionCatalogItem): boolean {
    return item.dispatch.kind !== "codex-action" && item.dispatch.kind !== "unsupported";
}

/**
 * Dispatches a catalog action the same way for every control (Action
 * Button keys and Crux dial presses): Micro keys and encoder buttons keep
 * their press/release pairing, while one-shot entries (encoder turns,
 * bridge catalog actions, Codex workflow actions) fire on press only and
 * report failures by rejecting.
 */
export async function dispatchCatalogAction(
    ctx: PluginContext,
    item: ActionCatalogItem,
    pressed: boolean,
): Promise<void> {
    switch (item.dispatch.kind) {
        case "micro-key":
            ctx.daemon.sendMicroKey(item.dispatch.key, pressed);
            break;
        case "encoder-button":
            ctx.daemon.sendEncoderButton(item.dispatch.index, pressed);
            break;
        case "encoder-turn":
            if (pressed) ctx.daemon.sendEncoderTurn(item.dispatch.index, item.dispatch.delta);
            break;
        case "catalog-action":
            if (pressed) ctx.daemon.sendCatalogAction(item.id);
            break;
        case "codex-action":
            if (pressed) await ctx.executeSelectedAgentAction(item.id);
            break;
        case "unsupported":
            if (pressed) throw new Error(item.dispatch.reason);
            break;
    }
}
