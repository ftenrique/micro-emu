import { action, SingletonAction, type DidReceiveSettingsEvent, type KeyAction, type KeyDownEvent, type WillAppearEvent } from "@elgato/streamdeck";
import type { JsonValue } from "@elgato/utils";
import { PluginContext } from "../context";
import { renderContextKeyImage, type ContextKeyMode } from "../images";

/** Agent with a usage source the bridge can report on. */
export type PinnedUsageAgent = "codex" | "zcode" | "claude";

/** Usage source setting: "auto" follows the last pressed agent (the
 * bridge's selection); any other value pins the key to that agent. */
export type UsageAgent = "auto" | PinnedUsageAgent;

export interface ContextKeySettings {
    [key: string]: JsonValue;
    mode?: ContextKeyMode;
    usageAgent?: UsageAgent;
    usageAgentVersion?: number;
}

/** Settings version that introduced "Last pressed agent". */
export const USAGE_AGENT_SETTINGS_VERSION = 2;

/**
 * Keys and dials saved before "Last pressed agent" existed carry the old
 * panel default "codex" without anyone having chosen it, which pins them to
 * Codex. Moves that default to "auto" once and stamps the version, so a
 * Codex pin chosen afterwards sticks. Deliberate non-default pins (ZCode)
 * are kept. Returns null when the settings are already current.
 */
export function migrateUsageAgentSettings<T extends { usageAgent?: unknown; usageAgentVersion?: unknown }>(
    settings: T,
): T | null {
    if (settings.usageAgentVersion === USAGE_AGENT_SETTINGS_VERSION) return null;
    return {
        ...settings,
        usageAgent: settings.usageAgent === "codex" || settings.usageAgent == null
            ? "auto"
            : settings.usageAgent,
        usageAgentVersion: USAGE_AGENT_SETTINGS_VERSION,
    };
}

/** Applies the migration on appear: persists it and returns the settings to
 * render with. */
export function upgradeUsageAgentSettings<T extends { usageAgent?: unknown; usageAgentVersion?: unknown }>(
    action: { setSettings(settings: T): Promise<void> },
    settings: T,
): T {
    const migrated = migrateUsageAgentSettings(settings);
    if (!migrated) return settings;
    void action.setSettings(migrated).catch(() => undefined);
    return migrated;
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
        const action = ev.action as KeyAction<ContextKeySettings>;
        this.refresh(action, upgradeUsageAgentSettings(action, ev.payload.settings));
    }

    onDidReceiveSettings(ev: DidReceiveSettingsEvent<ContextKeySettings>): void {
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
            // Auto follows the last pressed agent; a pinned key renders its
            // own source from the bridge's per-agent snapshots.
            const agent = this.ctx.resolveUsageAgent(normalizeUsageAgent(settings.usageAgent));
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

/** Following the last pressed agent is the default; only known agents pin. */
export function normalizeUsageAgent(value: unknown): UsageAgent {
    return value === "codex" || value === "zcode" || value === "claude" ? value : "auto";
}
