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

// --- Daemon client setup ---
// The bridge exe path can be configured via the MICRO_EMU_BRIDGE_EXE
// environment variable; defaults to the release build in the repo.
const bridgeExe = process.env.MICRO_EMU_BRIDGE_EXE ?? undefined;
const daemonArgs: string[] = [];
if (process.env.MICRO_EMU_PORT) {
    daemonArgs.push("--port", process.env.MICRO_EMU_PORT);
}
if (process.env.MICRO_EMU_CONTROLLER) {
    daemonArgs.push("--controller", process.env.MICRO_EMU_CONTROLLER);
}

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
// Stream Deck+ exposes eight physical key slots (4x2). Report this before
// connecting so the daemon can route Task Card presses immediately.
daemon.setTaskSlots(8);
streamDeck.connect();
daemon.start();

streamDeck.logger.info("Codex Micro plugin started");
