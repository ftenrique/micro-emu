/** Status colors matching the HID path's `v.oai.thstatus` palette. */
const STATUS_COLORS: Record<string, string> = {
    idle: "#37474f",
    running: "#1565c0",
    working: "#1565c0",
    active: "#1565c0",
    thinking: "#6a1b9a",
    waiting: "#ef6c00",
    error: "#b71c1c",
    completed: "#1b5e20",
    complete: "#1b5e20",
    done: "#1b5e20",
    ready: "#0277bd",
};

const DEFAULT_COLOR = "#263238";

/** Encode generated SVGs in the data-URI format accepted by Stream Deck.
 * Uses base64 encoding — percent-encoded SVGs render backgrounds but not
 * text elements in Stream Deck's rasterizer. */
function svgDataUrl(svg: string): string {
    return `data:image/svg+xml;base64,${Buffer.from(svg, "utf-8").toString("base64")}`;
}

/** 5x3 pixel font for digits 0-9, matching the task-card numbering style. */
const DIGITS: number[][] = [
    [0, 1, 0, 1, 0, 1, 1, 0, 1, 1, 0, 1, 0, 1, 0], // 0
    [1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1], // 1
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1], // 2
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0], // 3
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1], // 4
    [1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0], // 5
    [0, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0], // 6
    [1, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0], // 7
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1], // 8
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0], // 9
];

/** Renders a big pixel-font digit (0-9) as SVG rects. */
function renderDigit(digit: number, originX: number, originY: number, cell: number, color: string): string {
    if (digit < 0 || digit >= DIGITS.length) return "";
    const glyph = DIGITS[digit];
    let rects = "";
    for (let row = 0; row < 5; row++) {
        for (let col = 0; col < 3; col++) {
            if (glyph[row * 3 + col] === 0) continue;
            const x = originX + col * cell;
            const y = originY + row * cell;
            rects += `<rect x="${x + 2}" y="${y + 2}" width="${cell - 4}" height="${cell - 4}" fill="${color}"/>`;
        }
    }
    return rects;
}

/** Generates a colored key image as a Stream Deck image data URI. */
export function renderKeyImage(
    label: string,
    status?: string,
    index?: number,
    colorOverride?: unknown,
): string {
    const color = normalizeColor(colorOverride) ?? (status ? (STATUS_COLORS[status.toLowerCase()] ?? DEFAULT_COLOR) : DEFAULT_COLOR);
    const w = 144;
    const h = 144;
    const radius = 12;

    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
  <rect x="4" y="4" width="${w - 8}" height="${h - 8}" rx="${radius}" fill="${color}" stroke="#000" stroke-opacity="0.2" stroke-width="1"/>
  ${index !== undefined ? `<text x="${w / 2}" y="28" font-family="sans-serif" font-size="14" font-weight="bold" fill="#fff" fill-opacity="0.5" text-anchor="middle">${index}</text>` : ""}
  <text x="${w / 2}" y="${h / 2 + 6}" font-family="sans-serif" font-size="18" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(label)}</text>
  ${status ? `<text x="${w / 2}" y="${h - 16}" font-family="sans-serif" font-size="11" fill="#fff" fill-opacity="0.7" text-anchor="middle">${escapeXml(status.toUpperCase())}</text>` : ""}
</svg>`);
}

/** Renders a task-card key with agent label on top and a big pixel-font digit. */
export function renderTaskCardImage(
    slot: number,
    agent: string | null | undefined,
    status: string,
    colorOverride?: unknown,
    startedAtMs?: number,
    finishedAtMs?: number,
    selected = false,
): string {
    const normalizedStatus = status.toLowerCase();
    // Codex can send a white legacy `c` value with an idle thstatus entry.
    // Idle task buttons retain their initial dark-grey state regardless of it.
    const override = normalizeColor(colorOverride);
    const isWorking = ["running", "working", "active", "thinking"].includes(normalizedStatus);
    const isFinished = ["completed", "complete", "done"].includes(normalizedStatus);
    const color = normalizedStatus === "idle"
        ? STATUS_COLORS.idle
        : isFinished
            ? STATUS_COLORS.completed
        // Legacy status messages sometimes carry white as a placeholder `c`.
        // It must not replace the semantic task-state background.
        : override === "#ffffff"
            ? (status ? (STATUS_COLORS[normalizedStatus] ?? DEFAULT_COLOR) : DEFAULT_COLOR)
            : override ?? (status ? (STATUS_COLORS[normalizedStatus] ?? DEFAULT_COLOR) : DEFAULT_COLOR);
    const w = 144;
    const h = 144;
    // Keep the task number compact and leave room for the enlarged timer.
    const cell = 12;
    const digitW = 3 * cell;
    const digitH = 5 * cell;
    const originX = Math.floor((w - digitW) / 2);
    const originY = 42;
    const agentLabel = agent ? agent.toUpperCase() : "";
    const elapsed = isWorking || isFinished
        ? formatElapsed(startedAtMs, finishedAtMs, isFinished)
        : undefined;
    const timerColor = isFinished ? "#a5d6a7" : "#90caf9";

    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
  <rect x="4" y="4" width="${w - 8}" height="${h - 8}" rx="12" fill="${color}" stroke="#000" stroke-opacity="0.2" stroke-width="1"/>
  ${selected ? `<rect x="${w - 9}" y="12" width="5" height="${h - 24}" rx="2.5" fill="#fff"/>` : ""}
  <text x="${w / 2}" y="19" font-family="sans-serif" font-size="14" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(agentLabel)}</text>
  ${renderDigit(slot + 1, originX, originY, cell, "#c4c2ff")}
  ${elapsed ? `<text x="${w / 2}" y="135" font-family="monospace" font-size="26" font-weight="bold" fill="${timerColor}" text-anchor="middle">${elapsed}</text>` : ""}
</svg>`);
}

/** Formats elapsed task time as minutes and seconds. */
function formatElapsed(startedAtMs: number | undefined, finishedAtMs: number | undefined, finished: boolean): string | undefined {
    if (!startedAtMs) return undefined;
    const end = finished && finishedAtMs ? finishedAtMs : Date.now();
    const seconds = Math.max(0, Math.floor((end - startedAtMs) / 1_000));
    const minutes = Math.floor(seconds / 60);
    return `${String(minutes).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
}

/** Renders a disconnected/greyed key image as a Stream Deck image data URI. */
export function renderDisconnectedImage(label: string): string {
    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
  <rect x="4" y="4" width="136" height="136" rx="12" fill="#1a1a1a" stroke="#333" stroke-width="1"/>
  <text x="72" y="78" font-family="sans-serif" font-size="16" font-weight="bold" fill="#666" text-anchor="middle">${escapeXml(label)}</text>
</svg>`);
}

/** Fixed modes supported by the non-Plus Context key. */
export type ContextKeyMode = "task" | "model" | "usage";

/** Renders one LCD-strip context screen as a square Stream Deck key. */
export function renderContextKeyImage(mode: ContextKeyMode, ctx: StripContext, connected = true, showResetTimes = false): string {
    const title = mode === "task" ? "TASK INFO" : mode === "model" ? "MODEL INFO" : "USAGE INFO";
    if (!connected) {
        return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
  <rect x="4" y="4" width="136" height="136" rx="12" fill="#161b1f" stroke="#333"/>
  <text x="72" y="50" font-family="sans-serif" font-size="16" font-weight="bold" fill="#777" text-anchor="middle">${title}</text>
  <text x="72" y="88" font-family="sans-serif" font-size="18" font-weight="bold" fill="#555" text-anchor="middle">OFFLINE</text>
</svg>`);
    }
    if (isPendingApproval(ctx)) return renderApprovalContextImage(mode, ctx);
    const body = mode === "task" ? renderContextTaskBody(ctx)
        : mode === "model" ? renderContextModelBody(ctx)
            : renderContextUsageBody(ctx, showResetTimes);
    // Tag the usage screen with the reporting agent so the source is
    // visible at a glance (codex/zcode).
    const agentTag = mode === "usage" && ctx.usage_agent
        ? `<text x="132" y="24" font-family="sans-serif" font-size="10" font-weight="bold" letter-spacing="0.6" fill="#6366f1" text-anchor="end">${escapeXml(ctx.usage_agent.toUpperCase())}</text>`
        : "";
    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
  <rect x="4" y="4" width="136" height="136" rx="12" fill="#10161a" stroke="#000" stroke-opacity="0.4"/>
  <text x="12" y="24" font-family="sans-serif" font-size="13" font-weight="bold" letter-spacing="1.2" fill="#78909c">${title}</text>
  ${agentTag}
  ${body}
</svg>`);
}

function renderContextTaskBody(ctx: StripContext): string {
    const taskId = truncate(String(ctx.task_number ?? ctx.task_id ?? "—"), 9);
    const project = truncate(ctx.project ?? "—", 18);
    const isWaiting = ctx.status?.toLowerCase() === "waiting";
    const task = truncate(isWaiting && ctx.prompt ? ctx.prompt : (ctx.task ?? "—"), 20);
    const taskLabel = isWaiting && ctx.prompt ? "APPROVAL" : "SELECTED TASK";
    return `<text x="12" y="56" font-family="monospace" font-size="26" font-weight="bold" fill="#90caf9">#${escapeXml(String(taskId))}</text>
  <text x="12" y="78" font-family="sans-serif" font-size="12" fill="#a5d6a7">${escapeXml(project)}</text>
  <text x="12" y="111" font-family="sans-serif" font-size="16" font-weight="bold" fill="#fff">${escapeXml(task)}</text>
  <text x="12" y="130" font-family="sans-serif" font-size="9" fill="#607d8b">${taskLabel}</text>`;
}

function isPendingApproval(ctx: StripContext): boolean {
    return ctx.status?.toLowerCase() === "waiting" && ctx.wait_reason?.toLowerCase() === "approval";
}

function renderApprovalContextImage(mode: ContextKeyMode, ctx: StripContext): string {
    if (mode === "task") {
        const prompt = truncate(ctx.prompt ?? "Approval required in Codex", 42);
        return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
  <rect x="4" y="4" width="136" height="136" rx="12" fill="#ef6c00" stroke="#ffcc80" stroke-width="2"/>
  <text x="72" y="27" font-family="sans-serif" font-size="13" font-weight="bold" letter-spacing="1" fill="#fff3e0" text-anchor="middle">APPROVAL</text>
  <text x="72" y="64" font-family="sans-serif" font-size="12" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(prompt)}</text>
  <text x="72" y="116" font-family="sans-serif" font-size="10" fill="#fff3e0" text-anchor="middle">CHOOSE ON CODEX</text>
</svg>`);
    }
    const approve = mode === "model";
    const color = approve ? "#1b5e20" : "#b71c1c";
    const label = approve ? "APPROVE" : "DENY";
    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
  <rect x="4" y="4" width="136" height="136" rx="12" fill="${color}" stroke="#fff" stroke-opacity="0.45" stroke-width="2"/>
  <text x="72" y="54" font-family="sans-serif" font-size="22" font-weight="bold" fill="#fff" text-anchor="middle">${label}</text>
  <text x="72" y="81" font-family="sans-serif" font-size="11" font-weight="bold" fill="#fff3e0" text-anchor="middle">OPEN CODEX</text>
  <text x="72" y="112" font-family="sans-serif" font-size="9" fill="#fff3e0" text-anchor="middle">DECIDE IN THE PROMPT</text>
</svg>`);
}

function renderContextModelBody(ctx: StripContext): string {
    const hasModel = ctx.model != null && ctx.model !== "";
    const hasEffort = ctx.effort != null && ctx.effort !== "";
    const status = ctx.status ?? "—";
    const progress = ctx.progress != null ? `${ctx.progress}%` : "—";
    const topLabel = hasModel ? "MODEL" : "STATUS";
    const topValue = truncate(hasModel ? (ctx.model as string) : status.toUpperCase(), 17);
    const topColor = hasModel ? "#ce93d8" : STATUS_COLORS[status.toLowerCase()] ?? "#ce93d8";
    const bottomLabel = hasEffort ? "EFFORT" : "PROGRESS";
    const bottomValue = truncate(hasEffort ? (ctx.effort as string) : progress, 14);
    const bottomColor = hasEffort ? "#ffcc80" : "#90caf9";
    return `<text x="12" y="47" font-family="sans-serif" font-size="9" fill="#607d8b">${topLabel}</text>
  <text x="12" y="70" font-family="sans-serif" font-size="17" font-weight="bold" fill="${topColor}">${escapeXml(topValue)}</text>
  <text x="12" y="92" font-family="sans-serif" font-size="9" fill="#607d8b">${bottomLabel}</text>
  <text x="12" y="116" font-family="sans-serif" font-size="16" font-weight="bold" fill="${bottomColor}">${escapeXml(bottomValue)}</text>`;
}

function renderContextUsageBody(ctx: StripContext, showResetTimes: boolean): string {
    const hasUsage = ctx.five_hour_remaining != null || ctx.weekly_remaining != null;
    const hasResetTimes = ctx.five_hour_reset_at != null || ctx.weekly_reset_at != null;
    if (showResetTimes && hasResetTimes) {
        return `<text x="12" y="47" font-family="sans-serif" font-size="9" fill="#607d8b">5H RESET</text>
  <text x="12" y="70" font-family="monospace" font-size="19" font-weight="bold" fill="#90caf9">${escapeXml(formatResetClock(ctx.five_hour_reset_at))}</text>
  <text x="12" y="93" font-family="sans-serif" font-size="9" fill="#607d8b">WEEKLY RESET</text>
  <text x="12" y="116" font-family="monospace" font-size="16" font-weight="bold" fill="#a5d6a7">${escapeXml(formatResetAt(ctx.weekly_reset_at))}</text>`;
    }
    if (hasUsage) return `${usageBarKey("5H", ctx.five_hour_remaining, 42)}\n  ${usageBarKey("WK", ctx.weekly_remaining, 82)}`;
    const status = (ctx.status ?? "idle").toUpperCase();
    const progress = ctx.progress != null ? `${ctx.progress}%` : "\u2014";
    const statusColor = STATUS_COLORS[(ctx.status ?? "idle").toLowerCase()] ?? "#37474f";
    return `<text x="12" y="52" font-family="sans-serif" font-size="9" fill="#607d8b">STATUS</text>
  <text x="12" y="75" font-family="sans-serif" font-size="17" font-weight="bold" fill="${statusColor}">${escapeXml(status)}</text>
  <text x="12" y="98" font-family="sans-serif" font-size="9" fill="#607d8b">PROGRESS</text>
  <text x="12" y="121" font-family="sans-serif" font-size="16" font-weight="bold" fill="#90caf9">${escapeXml(progress)}</text>`;
}

function usageBarKey(label: string, remaining: number | null | undefined, y: number): string {
    const barX = 34;
    const barW = 66;
    const pct = remaining ?? null;
    const width = pct === null ? 0 : Math.round((barW * Math.max(0, Math.min(100, pct))) / 100);
    const color = pct === null ? "#333" : pct <= 10 ? "#b71c1c" : pct <= 25 ? "#ef6c00" : "#2e7d32";
    const text = pct === null ? "—" : `${pct}%`;
    return `<text x="12" y="${y + 12}" font-family="monospace" font-size="11" font-weight="bold" fill="#90a4ae">${label}</text>
  <rect x="${barX}" y="${y}" width="${barW}" height="14" rx="7" fill="#333"/>
  <rect x="${barX}" y="${y}" width="${width}" height="14" rx="7" fill="${color}"/>
  <text x="132" y="${y + 13}" font-family="monospace" font-size="18" font-weight="bold" fill="#fff" text-anchor="end">${escapeXml(text)}</text>`;
}

// --- Touch strip canvases (200x100, one encoder slot on the Stream Deck+) ---

const STRIP_W = 200;
const STRIP_H = 100;

/** Wraps strip body markup in the standard 200x100 canvas frame.
 * Returns a base64 data URI (data:image/svg+xml;base64,...) — the pixmap
 * layout item requires either a file path, a base64 data URI with declared
 * mime type, or raw SVG; percent-encoded URIs render as a blank/white box. */
function stripSvg(body: string): string {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${STRIP_W}" height="${STRIP_H}" viewBox="0 0 ${STRIP_W} ${STRIP_H}">
  <rect x="0" y="0" width="${STRIP_W}" height="${STRIP_H}" fill="#000"/>
  ${body}
</svg>`;
    return `data:image/svg+xml;base64,${Buffer.from(svg, "utf-8").toString("base64")}`;
}

/** Renders an offline/disconnected strip canvas. */
export function renderStripOffline(label: string): string {
    return stripSvg(`<text x="${STRIP_W / 2}" y="40" font-family="sans-serif" font-size="12" font-weight="bold" fill="#555" text-anchor="middle">${escapeXml(label)}</text>
  <text x="${STRIP_W / 2}" y="66" font-family="sans-serif" font-size="14" font-weight="bold" fill="#777" text-anchor="middle">OFFLINE</text>`);
}

/** Strip context fields shared by the dial canvases. */
export interface StripContext {
    agent?: string | null;
    project?: string | null;
    task?: string | null;
    model?: string | null;
    effort?: string | null;
    status?: string | null;
    wait_reason?: string | null;
    prompt?: string | null;
    interaction_id?: string | null;
    short_action?: string | null;
    long_action?: string | null;
    progress?: number | null;
    task_id?: string | null;
    task_number?: number | null;
    weekly_remaining?: number | null;
    five_hour_remaining?: number | null;
    weekly_reset_at?: number | null;
    five_hour_reset_at?: number | null;
    /** Agent the usage fields belong to ("codex" | "zcode"). */
    usage_agent?: string | null;
}

/** Knob strip: task number, project, shortened task name, and owning agent. */
export function renderKnobStrip(ctx: StripContext): string {
    if (isPendingApproval(ctx)) {
        const prompt = truncate(ctx.prompt ?? "Approval required in Codex", 34);
        return stripSvg(`<rect width="${STRIP_W}" height="${STRIP_H}" fill="#5f3700"/>
  <text x="10" y="22" font-family="sans-serif" font-size="10" font-weight="bold" letter-spacing="1" fill="#ffb74d">APPROVAL WAITING</text>
  <text x="${STRIP_W / 2}" y="53" font-family="sans-serif" font-size="13" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(prompt)}</text>
  <text x="${STRIP_W / 2}" y="84" font-family="sans-serif" font-size="12" font-weight="bold" fill="#ffe0b2" text-anchor="middle">PRESS: OPEN CODEX</text>`);
    }
    const taskId = ctx.task_number ?? ctx.task_id ?? "—";
    const agent = truncate((ctx.agent ?? agentFromTaskId(ctx.task_id) ?? "—").toUpperCase(), 12);
    const project = truncate(ctx.project ?? "—", 18);
    const task = truncate(ctx.task ?? "—", 24);
    return stripSvg(`<text x="10" y="26" font-family="monospace" font-size="13" font-weight="bold" fill="#90caf9">#${escapeXml(String(taskId))}</text>
  <text x="${STRIP_W - 10}" y="26" font-family="sans-serif" font-size="12" fill="#a5d6a7" text-anchor="end">${escapeXml(project)}</text>
  <text x="${STRIP_W / 2}" y="58" font-family="sans-serif" font-size="14" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(task)}</text>
  <text x="${STRIP_W / 2}" y="84" font-family="sans-serif" font-size="16" font-weight="bold" fill="#6366f1" text-anchor="middle">${escapeXml(agent)}</text>`);
}

/** Crux horizontal strip: model / effort, or status / progress as fallback. */
export function renderCruxHStrip(ctx: StripContext, clickLabel: string): string {
    if (isPendingApproval(ctx)) {
        const prompt = truncate(ctx.prompt ?? "Approval required", 32);
        return stripSvg(`<rect width="${STRIP_W}" height="${STRIP_H}" fill="#123d21"/>
  <text x="10" y="22" font-family="sans-serif" font-size="10" font-weight="bold" letter-spacing="1" fill="#81c784">PENDING APPROVAL</text>
  <text x="${STRIP_W / 2}" y="53" font-family="sans-serif" font-size="13" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(prompt)}</text>
  <text x="${STRIP_W / 2}" y="84" font-family="sans-serif" font-size="15" font-weight="bold" fill="#a5d6a7" text-anchor="middle">PRESS: APPROVE</text>`);
    }
    const hasModel = ctx.model != null && ctx.model !== "";
    const hasEffort = ctx.effort != null && ctx.effort !== "";
    const status = ctx.status ?? "—";
    const progress = ctx.progress != null ? `${ctx.progress}%` : "—";
    const topLabel = hasModel ? "MODEL" : "STATUS";
    const topValue = truncate(hasModel ? (ctx.model as string) : status.toUpperCase(), 20);
    const topColor = hasModel ? "#ce93d8" : STATUS_COLORS[status.toLowerCase()] ?? "#ce93d8";
    const bottomLabel = hasEffort ? "EFFORT" : "PROGRESS";
    const bottomValue = truncate(hasEffort ? (ctx.effort as string) : progress, 14);
    const bottomColor = hasEffort ? "#ffcc80" : "#90caf9";
    return stripSvg(`<text x="10" y="24" font-family="sans-serif" font-size="10" fill="#666">${topLabel}</text>
  <text x="${STRIP_W / 2}" y="46" font-family="sans-serif" font-size="15" font-weight="bold" fill="${topColor}" text-anchor="middle">${escapeXml(topValue)}</text>
  <text x="10" y="70" font-family="sans-serif" font-size="10" fill="#666">${bottomLabel}</text>
  <text x="${STRIP_W / 2}" y="88" font-family="sans-serif" font-size="13" font-weight="bold" fill="${bottomColor}" text-anchor="middle">${escapeXml(bottomValue)}</text>
  <text x="${STRIP_W - 8}" y="16" font-family="monospace" font-size="9" fill="#546e7a" text-anchor="end">◄► ${escapeXml(clickLabel)}</text>`);
}

/** Crux vertical strip: usage bars, or status/progress as fallback. */
export function renderCruxVStrip(ctx: StripContext, clickLabel: string, showResetTimes = false): string {
    if (isPendingApproval(ctx)) {
        const prompt = truncate(ctx.prompt ?? "Approval required", 32);
        return stripSvg(`<rect width="${STRIP_W}" height="${STRIP_H}" fill="#5b1515"/>
  <text x="10" y="22" font-family="sans-serif" font-size="10" font-weight="bold" letter-spacing="1" fill="#ef9a9a">PENDING APPROVAL</text>
  <text x="${STRIP_W / 2}" y="53" font-family="sans-serif" font-size="13" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(prompt)}</text>
  <text x="${STRIP_W / 2}" y="84" font-family="sans-serif" font-size="15" font-weight="bold" fill="#ef9a9a" text-anchor="middle">PRESS: DENY</text>`);
    }
    const hasUsage = ctx.five_hour_remaining != null || ctx.weekly_remaining != null;
    const hasResetTimes = ctx.five_hour_reset_at != null || ctx.weekly_reset_at != null;
    // The reporting agent (codex/zcode) rides along in the top-left corner.
    const agentTag = ctx.usage_agent
        ? `<text x="10" y="16" font-family="sans-serif" font-size="10" font-weight="bold" letter-spacing="0.6" fill="#6366f1">${escapeXml(ctx.usage_agent.toUpperCase())}</text>`
        : "";
    if (showResetTimes && hasResetTimes) {
        return stripSvg(`${agentTag}
  <text x="10" y="28" font-family="sans-serif" font-size="9" fill="#666">5H RESET</text>
  <text x="${STRIP_W / 2}" y="47" font-family="monospace" font-size="17" font-weight="bold" fill="#90caf9" text-anchor="middle">${escapeXml(formatResetClock(ctx.five_hour_reset_at))}</text>
  <text x="10" y="65" font-family="sans-serif" font-size="9" fill="#666">WEEKLY RESET</text>
  <text x="${STRIP_W / 2}" y="84" font-family="monospace" font-size="14" font-weight="bold" fill="#a5d6a7" text-anchor="middle">${escapeXml(formatResetAt(ctx.weekly_reset_at))}</text>
  <text x="${STRIP_W - 8}" y="16" font-family="monospace" font-size="9" fill="#546e7a" text-anchor="end">â–²â–¼ ${escapeXml(clickLabel)}</text>`);
    }
    if (hasUsage) {
        return stripSvg(`${agentTag}
  ${usageBar("5H", ctx.five_hour_remaining, 26)}
  ${usageBar("WK", ctx.weekly_remaining, 62)}
  <text x="${STRIP_W - 8}" y="16" font-family="monospace" font-size="9" fill="#546e7a" text-anchor="end">â–²â–¼ ${escapeXml(clickLabel)}</text>`);
    }
    // Fallback: show status and progress as text when no usage data.
    const status = (ctx.status ?? "idle").toUpperCase();
    const progress = ctx.progress != null ? `${ctx.progress}%` : "—";
    const statusColor = STATUS_COLORS[(ctx.status ?? "idle").toLowerCase()] ?? "#37474f";
    return stripSvg(`${agentTag}
  <text x="10" y="30" font-family="sans-serif" font-size="10" fill="#666">STATUS</text>
  <text x="${STRIP_W / 2}" y="50" font-family="sans-serif" font-size="16" font-weight="bold" fill="${statusColor}" text-anchor="middle">${escapeXml(status)}</text>
  <text x="10" y="72" font-family="sans-serif" font-size="10" fill="#666">PROGRESS</text>
  <text x="${STRIP_W / 2}" y="90" font-family="sans-serif" font-size="14" font-weight="bold" fill="#90caf9" text-anchor="middle">${escapeXml(progress)}</text>
  <text x="${STRIP_W - 8}" y="16" font-family="monospace" font-size="9" fill="#546e7a" text-anchor="end">▲▼ ${escapeXml(clickLabel)}</text>`);
}

/** Formats a reset timestamp in the user local timezone. */
function formatResetAt(resetAt: number | null | undefined): string {
    if (resetAt == null || !Number.isFinite(resetAt)) return "\u2014";
    return new Intl.DateTimeFormat(undefined, {
        month: "short",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
    }).format(new Date(resetAt * 1_000));
}

/** Formats a reset timestamp as 24-hour HH:MM in the local timezone. The
 * 5-hour window always resets within hours, so the clock time alone is
 * enough and stays compact at larger font sizes. */
function formatResetClock(resetAt: number | null | undefined): string {
    if (resetAt == null || !Number.isFinite(resetAt)) return "\u2014";
    const date = new Date(resetAt * 1_000);
    return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** Renders a labelled usage bar (remaining percentage) at the given y. */
function usageBar(label: string, remaining: number | null | undefined, y: number): string {
    const barX = 34;
    // Leave enough room for the enlarged percentage value, including "100%".
    const barW = STRIP_W - barX - 72;
    const pct = remaining ?? null;
    const width = pct === null ? 0 : Math.round((barW * Math.max(0, Math.min(100, pct))) / 100);
    const color = pct === null ? "#333" : pct <= 10 ? "#b71c1c" : pct <= 25 ? "#ef6c00" : "#2e7d32";
    const text = pct === null ? "—" : `${pct}%`;
    return `<text x="10" y="${y + 9}" font-family="monospace" font-size="11" font-weight="bold" fill="#90a4ae">${escapeXml(label)}</text>
  <rect x="${barX}" y="${y}" width="${barW}" height="12" rx="6" fill="#333"/>
  <rect x="${barX}" y="${y}" width="${width}" height="12" rx="6" fill="${color}"/>
  <text x="${STRIP_W - 8}" y="${y + 10}" font-family="monospace" font-size="22" font-weight="bold" fill="#fff" text-anchor="end">${escapeXml(text)}</text>`;
}

// --- Action button icons ---

/** Icon glyphs (drawn in a 40x40 box centered in the key). */
const ICON_GLYPHS: Record<string, string> = {
    action: `<path d="M22 4 L10 24 h8 l-4 12 L28 18 h-8 z" fill="#fff"/>`,
    send: `<path d="M4 20 L34 6 L24 36 L18 24 z" fill="#fff"/>`,
    mic: `<rect x="14" y="4" width="12" height="20" rx="6" fill="#fff"/><path d="M8 20 a12 12 0 0 0 24 0 M20 32 v6 M12 38 h16" stroke="#fff" stroke-width="3" fill="none"/>`,
    stop: `<rect x="8" y="8" width="24" height="24" rx="4" fill="#fff"/>`,
    "new-chat": `<path d="M20 6 v28 M6 20 h28" stroke="#fff" stroke-width="5" stroke-linecap="round"/>`,
    retry: `<path d="M32 20 a12 12 0 1 1 -4 -9" stroke="#fff" stroke-width="4" fill="none"/><path d="M30 4 v8 h-8 z" fill="#fff"/>`,
    copy: `<rect x="6" y="6" width="20" height="24" rx="3" fill="none" stroke="#fff" stroke-width="3"/><rect x="14" y="12" width="20" height="24" rx="3" fill="#1a1a1a" stroke="#fff" stroke-width="3"/>`,
    up: `<path d="M20 6 L34 26 h-9 v8 h-10 v-8 h-9 z" fill="#fff"/>`,
    down: `<path d="M20 34 L6 14 h9 V6 h10 v8 h9 z" fill="#fff"/>`,
    left: `<path d="M6 20 L26 6 v9 h8 v10 h-8 v9 z" fill="#fff"/>`,
    right: `<path d="M34 20 L14 34 v-9 H6 V15 h8 V6 z" fill="#fff"/>`,
    rotor: `<circle cx="20" cy="20" r="12" stroke="#fff" stroke-width="4" fill="none" stroke-dasharray="50 12"/><path d="M32 8 v8 h-8 z" fill="#fff"/>`,
    task: `<path d="M8 10 h6 M8 20 h6 M8 30 h6 M18 10 h14 M18 20 h14 M18 30 h14" stroke="#fff" stroke-width="4" stroke-linecap="round"/>`,
};

/** Names of the available action-button icons. */
export const ACTION_ICONS = Object.keys(ICON_GLYPHS);

/** Renders a key image with an action icon glyph, label, and status color.
 * The glyph is drawn in a 40x40 box; `scale` enlarges it (e.g. 1.8 = 72x72).
 * `bgColor` overrides the default dark-grey background. */
export function renderActionKeyImage(
    label: string,
    icon: string,
    index?: number,
    scale = 1,
    bgColor?: string,
): string {
    const glyph = ICON_GLYPHS[icon] ?? ICON_GLYPHS.action;
    const w = 144;
    const h = 144;
    const box = 40 * scale;
    const glyphY = index !== undefined ? 32 : (h - box) / 2 - 8;
    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
  <rect x="4" y="4" width="${w - 8}" height="${h - 8}" rx="12" fill="${bgColor ?? DEFAULT_COLOR}" stroke="#000" stroke-opacity="0.2" stroke-width="1"/>
  ${index !== undefined ? `<text x="${w / 2}" y="26" font-family="sans-serif" font-size="14" font-weight="bold" fill="#fff" fill-opacity="0.5" text-anchor="middle">${index}</text>` : ""}
  <g transform="translate(${(w - box) / 2}, ${glyphY}) scale(${scale})">${glyph}</g>
  <text x="${w / 2}" y="${h - 18}" font-family="sans-serif" font-size="14" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(label)}</text>
</svg>`);
}

function normalizeColor(value: unknown): string | undefined {
    if (typeof value === "number" && Number.isFinite(value)) {
        const rgb = Math.max(0, Math.min(0xffffff, Math.round(value)));
        return `#${rgb.toString(16).padStart(6, "0")}`;
    }
    if (typeof value === "string") {
        const text = value.trim();
        if (/^#[0-9a-f]{6}$/i.test(text)) return text;
        if (/^[0-9a-f]{6}$/i.test(text)) return `#${text}`;
        if (/^\d+$/.test(text)) {
            const rgb = Math.max(0, Math.min(0xffffff, Number(text)));
            return `#${rgb.toString(16).padStart(6, "0")}`;
        }
    }
    return undefined;
}
function escapeXml(text: string): string {
    return text
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&apos;");
}

function truncate(text: string, maxLen: number): string {
    if (text.length <= maxLen) return text;
    return text.slice(0, maxLen - 1) + "…";
}

function agentFromTaskId(taskId: string | null | undefined): string | undefined {
    const prefix = taskId?.match(/^([^:]+):/u)?.[1];
    return prefix ? prefix : undefined;
}
