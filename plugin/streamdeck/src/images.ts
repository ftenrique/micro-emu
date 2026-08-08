/** Status colors matching the HID path's `v.oai.thstatus` palette. */
const STATUS_COLORS: Record<string, string> = {
    idle: "#37474f",
    working: "#1565c0",
    thinking: "#6a1b9a",
    waiting: "#ef6c00",
    error: "#b71c1c",
    done: "#2e7d32",
    ready: "#0277bd",
};

const DEFAULT_COLOR = "#263238";

/** Encode generated SVGs in the data-URI format accepted by Stream Deck. */
function svgDataUrl(svg: string): string {
    return `data:image/svg+xml,${encodeURIComponent(svg)}`;
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

/** Renders a disconnected/greyed key image as a Stream Deck image data URI. */
export function renderDisconnectedImage(label: string): string {
    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
  <rect x="4" y="4" width="136" height="136" rx="12" fill="#1a1a1a" stroke="#333" stroke-width="1"/>
  <text x="72" y="78" font-family="sans-serif" font-size="16" font-weight="bold" fill="#666" text-anchor="middle">${escapeXml(label)}</text>
</svg>`);
}

// --- Touch strip canvases (200x100, one encoder slot on the Stream Deck+) ---

const STRIP_W = 200;
const STRIP_H = 100;

/** Wraps strip body markup in the standard 200x100 canvas frame. */
function stripSvg(body: string): string {
    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="${STRIP_W}" height="${STRIP_H}" viewBox="0 0 ${STRIP_W} ${STRIP_H}">
  <rect x="2" y="2" width="${STRIP_W - 4}" height="${STRIP_H - 4}" rx="8" fill="#1a1a1a"/>
  ${body}
</svg>`);
}

/** Renders an offline/disconnected strip canvas. */
export function renderStripOffline(label: string): string {
    return stripSvg(`<text x="${STRIP_W / 2}" y="40" font-family="sans-serif" font-size="12" font-weight="bold" fill="#555" text-anchor="middle">${escapeXml(label)}</text>
  <text x="${STRIP_W / 2}" y="66" font-family="sans-serif" font-size="14" font-weight="bold" fill="#777" text-anchor="middle">OFFLINE</text>`);
}

/** Strip context fields shared by the dial canvases. */
export interface StripContext {
    project?: string | null;
    task?: string | null;
    model?: string | null;
    effort?: string | null;
    status?: string | null;
    progress?: number | null;
    task_id?: string | null;
    weekly_remaining?: number | null;
    five_hour_remaining?: number | null;
}

/** Knob strip: task number, project, shortened task name. */
export function renderKnobStrip(ctx: StripContext): string {
    const taskId = ctx.task_id ?? "—";
    const project = truncate(ctx.project ?? "—", 18);
    const task = truncate(ctx.task ?? "—", 24);
    return stripSvg(`<text x="10" y="26" font-family="monospace" font-size="13" font-weight="bold" fill="#90caf9">#${escapeXml(String(taskId))}</text>
  <text x="${STRIP_W - 10}" y="26" font-family="sans-serif" font-size="12" fill="#a5d6a7" text-anchor="end">${escapeXml(project)}</text>
  <text x="${STRIP_W / 2}" y="58" font-family="sans-serif" font-size="14" font-weight="bold" fill="#fff" text-anchor="middle">${escapeXml(task)}</text>
  <text x="${STRIP_W / 2}" y="84" font-family="sans-serif" font-size="10" fill="#666" text-anchor="middle">KNOB</text>`);
}

/** Crux horizontal strip: model / effort of the selected task. */
export function renderCruxHStrip(ctx: StripContext, clickLabel: string): string {
    const model = truncate(ctx.model ?? "—", 20);
    const effort = truncate(ctx.effort ?? "—", 14);
    return stripSvg(`<text x="10" y="24" font-family="sans-serif" font-size="10" fill="#666">MODEL</text>
  <text x="${STRIP_W / 2}" y="46" font-family="sans-serif" font-size="15" font-weight="bold" fill="#ce93d8" text-anchor="middle">${escapeXml(model)}</text>
  <text x="10" y="70" font-family="sans-serif" font-size="10" fill="#666">EFFORT</text>
  <text x="${STRIP_W / 2}" y="88" font-family="sans-serif" font-size="13" font-weight="bold" fill="#ffcc80" text-anchor="middle">${escapeXml(effort)}</text>
  <text x="${STRIP_W - 8}" y="16" font-family="monospace" font-size="9" fill="#546e7a" text-anchor="end">◄► ${escapeXml(clickLabel)}</text>`);
}

/** Crux vertical strip: 5-hour and weekly usage limits as bars + percent. */
export function renderCruxVStrip(ctx: StripContext, clickLabel: string): string {
    return stripSvg(`${usageBar("5H", ctx.five_hour_remaining, 26)}
  ${usageBar("WK", ctx.weekly_remaining, 62)}
  <text x="${STRIP_W - 8}" y="16" font-family="monospace" font-size="9" fill="#546e7a" text-anchor="end">▲▼ ${escapeXml(clickLabel)}</text>`);
}

/** Renders a labelled usage bar (remaining percentage) at the given y. */
function usageBar(label: string, remaining: number | null | undefined, y: number): string {
    const barX = 34;
    const barW = STRIP_W - barX - 48;
    const pct = remaining ?? null;
    const width = pct === null ? 0 : Math.round((barW * Math.max(0, Math.min(100, pct))) / 100);
    const color = pct === null ? "#333" : pct <= 10 ? "#b71c1c" : pct <= 25 ? "#ef6c00" : "#2e7d32";
    const text = pct === null ? "—" : `${pct}%`;
    return `<text x="10" y="${y + 9}" font-family="monospace" font-size="11" font-weight="bold" fill="#90a4ae">${escapeXml(label)}</text>
  <rect x="${barX}" y="${y}" width="${barW}" height="12" rx="6" fill="#333"/>
  <rect x="${barX}" y="${y}" width="${width}" height="12" rx="6" fill="${color}"/>
  <text x="${STRIP_W - 8}" y="${y + 10}" font-family="monospace" font-size="11" font-weight="bold" fill="#fff" text-anchor="end">${escapeXml(text)}</text>`;
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

/** Renders a key image with an action icon glyph, label, and status color. */
export function renderActionKeyImage(label: string, icon: string, index?: number): string {
    const glyph = ICON_GLYPHS[icon] ?? ICON_GLYPHS.action;
    const w = 144;
    const h = 144;
    return svgDataUrl(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
  <rect x="4" y="4" width="${w - 8}" height="${h - 8}" rx="12" fill="${DEFAULT_COLOR}" stroke="#000" stroke-opacity="0.2" stroke-width="1"/>
  ${index !== undefined ? `<text x="${w / 2}" y="26" font-family="sans-serif" font-size="14" font-weight="bold" fill="#fff" fill-opacity="0.5" text-anchor="middle">${index}</text>` : ""}
  <g transform="translate(${(w - 40) / 2}, 40)">${glyph}</g>
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

