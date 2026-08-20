import { action, SingletonAction, type DidReceiveSettingsEvent, type KeyAction, type KeyDownEvent, type WillAppearEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderContextKeyImage, type ContextKeyMode } from "../images";

/** Agent whose usage limits the Usage mode reports. */
export type UsageAgent = "codex" | "zcode";

export interface ContextKeySettings {
    [key: string]: JsonValue;
    mode?: ContextKeyMode;
    usageAgent?: UsageAgent;
}

export function isPendingApproval(ctx: { status?: string | null; wait_reason?: string | null }): boolean {
    return ctx.status?.toLowerCase() === "waiting"
        && ctx.wait_reason?.toLowerCase() === "approval";
}

/** Send the same Micro action configured for approval or denial. */
export function sendApprovalDecision(ctx: PluginContext, decision: "approve" | "deny"): void {
    const card = ctx.getSelectedTaskCard();
    const slot = Number(card?.source_slot ?? card?.id ?? card?.slot ?? card?.i);
    const taskId = typeof card?.task_id === "string" ? card.task_id : undefined;
    if (card?.interaction != null && Number.isFinite(slot) && taskId) {
        ctx.daemon.sendTaskAction(slot, decision === "approve" ? "short" : "long", taskId);
        return;
    }
    const key = decision === "approve" ? "ACT06" : "ACT07";
    ctx.daemon.sendMicroKey(key, true);
    ctx.daemon.sendMicroKey(key, false);
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
        this.syncUsageAgent(ev.payload.settings);
        this.refresh(ev.action as KeyAction<ContextKeySettings>, ev.payload.settings);
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<ContextKeySettings>): void {
        this.syncUsageAgent(ev.payload.settings);
        this.refresh(ev.action as KeyAction<ContextKeySettings>, ev.payload.settings);
    }

    async onKeyDown(ev: KeyDownEvent<ContextKeySettings>): Promise<void> {
        const mode = normalizeMode(ev.payload.settings.mode);
        const selected = this.ctx.getSelectedDisplayContext();
        if (!this.ctx.isConnected()) {
            await ev.action.showAlert();
            return;
        }
        if (isPendingApproval(selected)) {
            try {
                if (mode === "task") await this.ctx.executeSelectedAgentAction("task.open");
                else sendApprovalDecision(this.ctx, mode === "model" ? "approve" : "deny");
                await ev.action.showOk();
            } catch {
                await ev.action.showAlert();
            }
            return;
        }
        if (mode === "usage") {
            const action = ev.action as KeyAction<ContextKeySettings>;
            this.showResetTimes.set(action, !(this.showResetTimes.get(action) ?? false));
            this.refresh(action, ev.payload.settings);
            return;
        }
        if (mode === "task") await this.ctx.executeSelectedAgentAction("task.open");
        else this.ctx.daemon.sendModelCycle();
    }
    /** Pushes the configured usage source to the daemon whenever a key in
     * Usage mode appears or its settings change. Task/Model keys leave the
     * current selection untouched. */
    private syncUsageAgent(settings: ContextKeySettings): void {
        if (normalizeMode(settings.mode) !== "usage") return;
        this.ctx.setUsageAgent(normalizeUsageAgent(settings.usageAgent));
    }

    private refreshAll(): void {
        for (const action of this.actions) {
            action.getSettings().then((settings) =>
                this.refresh(action as KeyAction<ContextKeySettings>, settings));
        }
    }

    private refresh(action: KeyAction<ContextKeySettings>, settings: ContextKeySettings): void {
        const mode = normalizeMode(settings.mode);
        let ctx = this.ctx.getSelectedDisplayContext();
        if (mode === "usage") {
            // This key always renders its own configured source, independent
            // of the global selection it pushes to the daemon.
            const agent = normalizeUsageAgent(settings.usageAgent);
            ctx = { ...ctx, ...this.ctx.getUsageFields(agent), usage_agent: agent };
        }
        action.setImage(renderContextKeyImage(mode, ctx, this.ctx.isConnected(), this.showResetTimes.get(action) ?? false));
        // The key already carries the mode in its header; avoid a redundant Stream Deck title beneath it.
        action.setTitle("");
    }
}

export function normalizeMode(value: unknown): ContextKeyMode {
    return value === "model" || value === "usage" ? value : "task";
}

/** Codex is the default usage source; anything else resolves to it. */
export function normalizeUsageAgent(value: unknown): UsageAgent {
    return value === "zcode" ? "zcode" : "codex";
}
