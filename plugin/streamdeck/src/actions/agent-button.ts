import { action } from "@elgato/streamdeck";
import { PluginContext } from "../context";
import { MicroButtonAction, type MicroButtonSettings } from "./micro-button";

/** Settings retained for existing Agent Button instances. */
export type AgentButtonSettings = MicroButtonSettings;

/** Hidden compatibility alias for profiles using the former Agent Button UUID. */
@action({ UUID: "com.micro-emu.codex.agent" })
export class AgentButtonAction extends MicroButtonAction {
    constructor(ctx: PluginContext) {
        super(ctx, 0);
    }
}
