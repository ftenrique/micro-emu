# Code Cleanup Report: Bloat, Dead Code, and Over-Engineering

Audit of the micro-emu repository (2026-08-10). Findings are grouped into phases,
ordered from safest to riskiest. Each item has an ID, an exact action, and a
verification step. Work through phases in order; run the phase's verification
before moving on.

**Ground rules for whoever executes this:**

- Do ONE item at a time. Run the verification command after each item.
- If a verification fails, revert that item (`git checkout -- <file>`) and skip it.
- Never touch anything in the "DO NOT TOUCH" section at the bottom.
- Baseline verification commands (run before starting, all must pass):

```powershell
npm test
cargo test --manifest-path tools/rp2040-bridge/Cargo.toml --offline
npm run plugin:build
```

---

## Phase 1 — Delete dead files and directories (zero risk to runtime)

### 1.1 Delete the entire VHF driver spike (largest single win)

The README states the driver is not required for the working RP2040 path, and
ADR `docs/decisions/0004-rp2040-usb-gadget.md` records it as a historical spike.

- **Action:** Delete the directory `driver\` (contains `vhf-spike\` with 3 C
  files, 8 PowerShell scripts, `.sln`, `.vcxproj`, `.inf`).
- **Also:** Remove these two lines from root `package.json` (lines 41-42):
  `"driver:check"` and `"driver:build"`.
- **Also:** Delete the test `"KMDF spike embeds the exact same BLE descriptor bytes"`
  in `tests\protocol\descriptor.test.js` (approx. lines 29-47) — it reads
  `driver/vhf-spike/driver/descriptor.c` and will fail once the directory is gone.
- **Also:** `tools\protocol-monitor\` talks to the driver's device channel
  (`DriverChannel.cs`). With the driver gone it is dead. Delete
  `tools\protocol-monitor\` and remove `"monitor:build"` and `"monitor:self-test"`
  from `package.json` (lines 23-24).
- **Verify:** `npm test` passes; `Select-String -Path package.json -Pattern "driver:|monitor:"` returns nothing.

### 1.2 Delete one-off diagnostic scripts with hardcoded device IDs

These contain PnP instance IDs from a single debugging session on one machine
and are referenced by no npm script or doc.

- **Action:** Delete `tools\check-rp2040-props.ps1` and
  `tools\compare-hid-properties.ps1`.
- **Verify:** `Select-String -Path package.json,README.md,DEPLOYMENT.md -Pattern "check-rp2040-props|compare-hid-properties"` returns nothing.

### 1.3 Delete stale hardware profiles

- **Action:** Delete `hardware\profiles\ajazz-akp03.pending.json` (an empty
  never-filled template) and `hardware\profiles\usb-candidate-04b4-1007.observed.json`
  (a different SONiX device explicitly noted as unrelated to the AKP03).
  Keep `ajazz-akp03e-rev2-0300-3002.verified.json` — it is the real device.
- **Verify:** `npm test` still passes; grep the repo for the two deleted filenames to confirm nothing references them.

### 1.4 Delete session/IDE artifacts and untracked build dirs

- **Action:** Delete `.zcode\` directory. Add `.zcode/` and `target-next/` to
  `.gitignore` (the existing `**/target/` pattern does not match
  `tools\rp2040-bridge\target-next\`). Delete `tools\rp2040-bridge\target-next\`.
- **Verify:** `git status --short` no longer lists `.zcode/` or `target-next/`.

---

## Phase 2 — Documentation cleanup (zero risk to runtime)

The repo has three generations of docs layered on top of each other. Keep the
README, the ADRs, and the current user guides; delete the historical plans.

| ID | Action | Reason |
|----|--------|--------|
| 2.1 | Delete `plan-emulacion-codex-micro-ajazz-akp03.md` (root, 784 lines) | Superseded phased plan; project is past all phases |
| 2.2 | Delete `docs\Hermes-integration-plan.md` | The feature is implemented; `docs\Hermes_integration.md` is the live guide |
| 2.3 | Delete `docs\ZCode-integration-plan.md` | Same pattern; keep `docs\ZCode_integration.md` |
| 2.4 | Delete `docs\windows-handshake.md` and `docs\wdk-setup.md` | Both are self-marked "Ruta histórica" and document the deleted VHF driver path |
| 2.5 | Delete `docs\phase2-feasibility-report.md` | Historical decision already captured by ADR 0004 |
| 2.6 | Delete `docs\windows-environment.md` | Stale one-time environment snapshot from July 2026 |
| 2.7 | Delete `DEPLOYMENT.md` | Near-duplicate of README build/flash/run sections; two sources of truth drift |
| 2.8 | Delete `hardware\schema\device-profile.schema.json` and remove any `$schema` key from the remaining profile JSON | Full JSON Schema for what is now 1 data file; nothing validates against it at runtime |

- **After all of Phase 2, verify:** `npm test` passes, and
  `Select-String -Path README.md -Pattern "DEPLOYMENT|windows-handshake|wdk-setup"`
  returns nothing (fix README links if it does).

---

## Phase 3 — package.json and script cleanup (low risk)

| ID | Action | Reason |
|----|--------|--------|
| 3.1 | Remove `"test:protocol"` script from `package.json` (line 13) | Byte-for-byte duplicate of `"test"` |
| 3.2 | In `tools\inventory-windows.ps1`, remove the `Get-ToolchainInventory` function (approx. lines 253-298) and its call site | Checks for WDK/VHF toolchain that only the deleted driver needed |
| 3.3 | In `tools\preflight-device-test.ps1`, remove WDK/toolchain readiness checks | Same reason; keep the protocol/bridge/artifact checks |
| 3.4 | Delete `firmware\rp2040-zero\src\codex_micro_descriptor.inc` and remove the write to it in `tools\generate-rp2040-descriptor.mjs` (line 17) | The firmware never `#include`s it; the descriptor bytes live in `usb_descriptors.c` and are already cross-checked by `tests\protocol\descriptor.test.js` |

- **Verify:** `npm test`, `npm run inventory`, `npm run verify:descriptor` all pass.

---

## Phase 4 — Duplication (medium risk: mechanical refactors, tests exist)

### Rust bridge (`tools\rp2040-bridge\src\`)

| ID | Finding | Fix |
|----|---------|-----|
| 4.1 | `DIGITS` 5x3 bitmask arrays duplicated in `streamdeck.rs:27-36` and `ajazz.rs:20-27` | Create `src\glyphs.rs` with one `DIGITS` constant; import from both files |
| 4.2 | `draw_agent_label` + `agent_glyph` duplicated in `streamdeck.rs:1359-1383` and `ajazz.rs:440-464` | Move both functions into the same new `glyphs.rs` module; delete the copies |
| 4.3 | AJAZZ HID code→event mapping duplicated between `ajazz.rs:237-290` (`events_from_code`) and `tools\ajazz-hardware-test\src\main.rs:143-195` | Acceptable if `ajazz-hardware-test` is kept as a standalone diagnostic; otherwise delete the whole `tools\ajazz-hardware-test\` crate and its `hardware:build`/`hardware:test` npm scripts (it duplicates what `bridge:run` proves) |

### Stream Deck plugin (`plugin\streamdeck\src\actions\`) — biggest TS win

The 10 action files share ~80% of their code (constructor registering a
context listener, `refreshAll()` loop, `isConnected()` + `showAlert()` guard).

| ID | Finding | Fix |
|----|---------|-----|
| 4.4 | `mic.ts` and `send.ts` are identical except encoder index (2 vs 0) and label | Merge into one `encoder-button.ts` action class parameterized by `{index, label}`; keep two thin exported subclasses so manifest UUIDs stay unchanged |
| 4.5 | `crux-horizontal.ts` and `crux-vertical.ts` are identical except encoder index (0 vs 2) and strip renderer | Same treatment: one shared base class, two thin subclasses |
| 4.6 | `agent-button.ts` and `action-button.ts` repeat the same listener/refresh/keyDown skeleton | Extract a shared abstract base class (e.g. `base-button.ts`) with the constructor, `refreshAll`, and connection guard; subclasses keep only their unique `refresh` logic |
| 4.7 | `StripContext` in `images.ts:146-157` is an exact duplicate of `DisplayContext` in `daemon-client.ts` | Delete `StripContext`; import `DisplayContext` instead |
| 4.8 | `renderKnobStrip` / `renderCruxHStrip` / `renderCruxVStrip` in `images.ts:159-296` repeat the extract-truncate-wrap-in-`stripSvg` pattern | Extract a helper that takes rows of `{text, x, y, style}`; keep the three functions as thin wrappers |

- **IMPORTANT constraint for 4.4-4.6:** Do not change action UUIDs in
  `manifest.json` or the exported class names registered in `plugin.ts`.
- **Verify:** `npm run plugin:build` compiles; `cargo test --manifest-path tools/rp2040-bridge/Cargo.toml --offline` passes.

---

## Phase 5 — Dead code and pointless guards (medium risk; verify each)

### Rust bridge

| ID | File:lines | Finding | Fix |
|----|-----------|---------|-----|
| 5.1 | `serial.rs:31-34, 188-191, 221-224` | `#[cfg(not(windows))]` stubs in a Windows-only project (README requires Windows; nothing else builds cross-platform) | Delete all `#[cfg(not(windows))]` blocks and the `#[cfg(windows)]` attributes on their counterparts |
| 5.2 | `routing.rs:159` | `_ => unreachable!(...)` after arms 0/1/2/3 in a match on a value already bounded to 0..=3 | Restructure so the bound is expressed in the match (or leave — cosmetic) |
| 5.3 | `ajazz.rs:245-253` (and similar at 271, 280) | Inner `match code {...  _ => unreachable!()}` inside an outer match arm that already restricts `code` to those exact values | Compute the index directly in the outer arm; remove the inner match |
| 5.4 | `codex.rs:185` | `.clamp(0.0, 1.0)` on a value already limited to {0.0, 0.5, 1.0} by `min(2)` at line 179 | Remove the clamp |
| 5.5 | `controller.rs:22-36` | Whitelist loop rejecting unknown `DisplayContext` fields, followed by per-field parsers that ignore unknown fields anyway | Remove the whitelist loop |
| 5.6 | `main.rs` `--legacy` mode (lines ~44, 64, 136, 176, and `run_legacy`) | Pre-daemon operating mode kept alongside MCP/daemon modes | Only remove if you confirm `--legacy` is unused in your own workflow; the README smoke test (`--no-ajazz --listen ... --emit ...`) runs through this path, so removing legacy mode also requires updating README line 130 and `docs\rp2040-first-connection.md` |

### JS protocol (`protocol\`)

| ID | File:lines | Finding | Fix |
|----|-----------|---------|-----|
| 5.7 | `messages.js:36-60` | `standardJsonRpc` option produces standard JSON-RPC envelopes; only ever used by its own test (`tests\protocol\messages.test.js:27`) | Remove the option, both `if (options.standardJsonRpc...)` branches, and the test assertions for it |
| 5.8 | `messages.js:62-83` | `parseRpcMessage` normalizes a "standard" JSON-RPC shape (`method`/`params`) that no producer in this repo emits, and returns an unused `style` field | Keep only the compact (`m`/`p`) path; delete `style`; update the one test that asserts `style === "standard"` |
| 5.9 | `messages.js:220-250` | `methodNotFoundResponse(requestOrId)` accepts either a request object or a bare id | Change the parameter to a bare `id`; update call sites |
| 5.10 | `messages.js:265-289` | `normalizeColor` accepts int, `#RGB`, `#RRGGBB`, `0x…`, and `[r,g,b]` — callers in this repo use one format | Check what the bridge/tests actually pass (`grep -r normalizeColor`), keep that format plus int, delete the rest, update the color test |
| 5.11 | `framing.js:15-29, 31-42` | `asBytes` accepts `ArrayBuffer`/`Array` and `serializePayload` accepts strings; all internal callers pass `Uint8Array`/objects | Simplify both to the single supported type. Do NOT remove the length/opcode/UTF-8/JSON guards elsewhere in `framing.js` — those validate real bytes from hardware |

### Stream Deck plugin

| ID | File:lines | Finding | Fix |
|----|-----------|---------|-----|
| 5.12 | `daemon-client.ts:210-236` | `try/catch` wrapping `spawn()` whose failures are delivered via the `error` event, not thrown | Remove the try/catch; keep the `on("error")` handler |
| 5.13 | `daemon-client.ts` `DaemonClientOptions` | Confirm `daemonArgs` and `cwd` options are actually set anywhere (`grep -r daemonArgs plugin/`) | If unused, delete them and the code that reads them |

### Firmware (`firmware\rp2040-zero\src\`)

| ID | File:lines | Finding | Fix |
|----|-----------|---------|-----|
| 5.14 | `bridge_protocol.h:38` + call sites | `void *context` parameter on `bridge_frame_callback_t` is always `NULL` | Remove the parameter from the typedef, `bridge_protocol.c`, `main.c`, and the host test |
| 5.15 | `main.c:127, 211-213, 222, 228` | NULL checks on pointers TinyUSB guarantees valid in its own callbacks | Remove; rebuild firmware (`npm run rp2040:build && npm run rp2040:verify`) |
| 5.16 | `main.c:232-278` | `codex_payload_contains` / `codex_payload_parse_id` do string-scanning "JSON parsing" in the firmware transport layer | Only touch this if you know the `device.status` reply id must echo the request id; if so this parsing is load-bearing — otherwise leave it. Mark LOW priority |

- **Verify after each Rust item:** `cargo test --manifest-path tools/rp2040-bridge/Cargo.toml --offline`
- **Verify after each JS item:** `npm test`
- **Verify after each firmware item:** `npm run firmware:host-test`, then `npm run rp2040:build` and `npm run rp2040:verify`

---

## Phase 6 — Structural (higher effort; do last, only if still needed)

| ID | Finding | Fix |
|----|---------|-----|
| 6.1 | `main.rs` is 2,361 lines mixing CLI parsing, bridge runtime, MCP handling, ChatGPT usage fetch, and 400+ lines of tests | Split into `cli.rs` (Options + parsing), `bridge.rs` (runtime), `usage.rs` (ChatGPT usage fetch); keep `fn main` thin. Pure moves, no behavior change |
| 6.2 | `streamdeck.rs` is 1,717 lines mixing HID packets, JPEG rendering, and device logic | Split into `streamdeck\hid.rs`, `streamdeck\render.rs`, `streamdeck\device.rs`. Pure moves |
| 6.3 | `daemon.rs` is 1,325 lines (TCP server + sessions + tools + ZCode polling) | Split into `daemon\server.rs`, `daemon\session.rs`, `daemon\tools.rs`. Pure moves |
| 6.4 | Root `package.json` has 34 scripts spanning 4 toolchains (Node, Rust, .NET, PowerShell); `ajazz-doctor` (.NET) duplicates diagnostics the Rust bridge already performs | After Phase 1 removes protocol-monitor, evaluate deleting `tools\ajazz-doctor\` + its `doctor:*` scripts too; that removes the .NET toolchain requirement entirely |

- **Verify:** full baseline suite (`npm test`, `cargo test`, `npm run plugin:build`, `npm run firmware:host-test`).

---

## DO NOT TOUCH (investigated and found justified)

These look like bloat but are load-bearing. Do not "clean" them:

1. **Firmware keyboard HID interface** (`usb_descriptors.c:36-111`): deliberately
   present so Windows `kbdhid.sys` binds interface 1 and leaves the vendor
   collection free for ChatGPT. Removing it breaks the Windows handshake.
2. **`AjazzDevice::clear_display`** (`ajazz.rs:187`): used at `ajazz.rs:221`.
3. **Input guards in `framing.js` decode path** (length, opcode, Report ID,
   chunk-length, UTF-8, JSON-shape checks): these validate raw bytes arriving
   from real USB hardware — a genuine trust boundary.
4. **`ureq` dependency and ChatGPT usage fetching** (`main.rs:~779-809`):
   newest feature (commit `16cc2e3`); the `.clamp(0.0,100.0)` there guards an
   external API response and is fine.
5. **`#[cfg(test)] mod tests` blocks inside Rust source files**: idiomatic Rust
   unit-test placement. Do not move them out.
6. **`PhysicalController` trait** (`controller.rs:211-240`): four real
   implementers (AJAZZ, Stream Deck, plugin controller, test double) with dyn
   dispatch at runtime. Converting to an enum is a taste call, not a cleanup —
   skip unless separately requested.
7. **`tasks.rs` / `routing.rs` state machines**: multi-agent task board and
   3-agent routing are active features per recent commits. Simplify only with
   explicit product decisions, not as mechanical cleanup.
8. **`plugin_controller.rs`**: backs the shipped Stream Deck plugin mode
   documented in the README. Not vestigial.

---

## Expected impact

- Phase 1-2: ~40 files / ~3,500 lines deleted (driver spike, protocol-monitor,
  stale docs/profiles/scripts) with zero runtime risk.
- Phase 3-5: ~400-600 lines of dead options, duplicate constants, and no-op
  guards removed across Rust/JS/TS/C.
- Phase 4 plugin dedup: 10 action files (~660 lines) shrink to roughly half.
- Phase 6: no line-count change, but the three giant Rust files become navigable.
