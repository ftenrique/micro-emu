import streamDeck from "@elgato/streamdeck";
import { DaemonClient } from "./daemon-client";
import { PluginContext } from "./context";
import { AgentButtonAction } from "./actions/agent-button";
import { ActionButtonAction } from "./actions/action-button";
import { TaskCardAction } from "./actions/task-card";
import { KnobDialAction } from "./actions/knob-dial";
import { CruxHorizontalAction } from "./actions/crux-horizontal";
import { CruxVerticalAction } from "./actions/crux-vertical";
import { MicAction } from "./actions/mic";
import { SendAction } from "./actions/send";
import { ArrowKeyAction } from "./actions/arrow-key";
import { ContextKeyAction } from "./actions/context";
import { CodexActionExecutor } from "./codex-action-executor";
import { resolveBridgeExecutable } from "./bridge-path";
import { TASK_SLOT_COUNT } from "./task-slots";
const SUSPEND_GAP_MS = 60_000;
const HOST_WAKE_GRACE_MS = 20_000;
const WATCHDOG_INTERVAL_MS = 15_000;

// --- Daemon client setup ---
// Stream Deck does not reliably inherit the user's shell environment, so use
// the installed LOCALAPPDATA copy (with release/dev fallbacks) when the
// explicit override is absent. Plugin mode must not open the physical HID
// controller owned by the official Stream Deck application.
const bridgeExe = resolveBridgeExecutable();
const daemonArgs = [
    "--port",
    process.env.MICRO_EMU_PORT || "auto",
    "--controller",
    process.env.MICRO_EMU_CONTROLLER || "none",
];

const daemon = new DaemonClient({
    bridgeExe,
    daemonArgs,
    cwd: process.env.MICRO_EMU_CWD,
});

daemon.on("error", (error: Error) => {
    streamDeck.logger.warn(`Bridge connection failed: ${error.message}`);
});
daemon.on("log", (message: string) => {
    streamDeck.logger.info(message);
});

let lastWatchdogTickAt = Date.now();
let lastSystemWakeAt = 0;
let hostRecoveryTimer: ReturnType<typeof setTimeout> | null = null;

streamDeck.system.onSystemDidWakeUp(() => {
    lastSystemWakeAt = Date.now();
    if (hostRecoveryTimer) {
        clearTimeout(hostRecoveryTimer);
        hostRecoveryTimer = null;
    }
    daemon.reconnect("Stream Deck system wake");
});

setInterval(() => {
    const previousTickAt = lastWatchdogTickAt;
    const now = Date.now();
    lastWatchdogTickAt = now;
    const gapMs = now - previousTickAt;
    if (gapMs < SUSPEND_GAP_MS) return;
    if (lastSystemWakeAt >= previousTickAt) return;
    daemon.reconnect(`event-loop gap of ${gapMs}ms`);
    if (hostRecoveryTimer) return;
    streamDeck.logger.warn(
        `System resumed after ${gapMs}ms without a Stream Deck wake event; awaiting host recovery`,
    );
    hostRecoveryTimer = setTimeout(() => {
        streamDeck.logger.error(
            "Stream Deck host connection did not recover after resume; restarting plugin",
        );
        daemon.stop();
        process.exit(1);
    }, HOST_WAKE_GRACE_MS);
}, WATCHDOG_INTERVAL_MS);
const codex = new CodexActionExecutor((message) => streamDeck.logger.info(message));
const ctx = new PluginContext(daemon, codex);

// --- Action registration ---
streamDeck.actions.registerAction(new AgentButtonAction(ctx));
streamDeck.actions.registerAction(new ActionButtonAction(ctx));
streamDeck.actions.registerAction(new TaskCardAction(ctx));
streamDeck.actions.registerAction(new KnobDialAction(ctx));
streamDeck.actions.registerAction(new CruxHorizontalAction(ctx));
streamDeck.actions.registerAction(new CruxVerticalAction(ctx));
streamDeck.actions.registerAction(new MicAction(ctx));
streamDeck.actions.registerAction(new SendAction(ctx));
streamDeck.actions.registerAction(new ArrowKeyAction(ctx));
streamDeck.actions.registerAction(new ContextKeyAction(ctx));
// --- Connect to the Stream Deck and the daemon ---
// Stream Deck+ exposes eight physical key slots (4x2). The ninth logical
// Task Card remains available on larger layouts, pages, and desktop-only
// profiles, so report the full logical capacity to the daemon.
daemon.setTaskSlots(TASK_SLOT_COUNT);
streamDeck.connect();
daemon.start();

streamDeck.logger.info("Codex Micro plugin started");
