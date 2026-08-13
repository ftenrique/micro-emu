# Codex Micro Emulator for Stream Deck and AJAZZ AKP03

`micro-emu` turns a Stream Deck or an AJAZZ AKP03E into a controller for Codex Micro. It provides a standard Stream Deck plugin for everyday use and an RP2040-backed hardware emulator for the AJAZZ device.

The project is aimed at a Windows workstation running Codex/ChatGPT. Its local bridge translates controller input into the existing Codex Micro HID protocol; it does not introduce a new Codex protocol.

## What it includes

- **Stream Deck plugin** â€” use Codex Micro controls alongside normal Stream Deck profiles and plugins. The official Stream Deck app keeps ownership of the device while the plugin talks to the local bridge daemon over loopback.
- **AJAZZ AKP03E emulator** â€” maps the AKP03E's six LCD keys, lower keys, and three encoders to Codex Micro events. An RP2040 Zero presents the required USB HID device to Windows/ChatGPT and connects to the bridge over USB CDC.
- **Local bridge and MCP server** â€” exposes status, controller input, task cards, and display context to Codex. Daemon mode can coordinate Codex, ZCode, and Hermes simultaneously.

```text
Stream Deck plugin -- TCP loopback --+
                                     +-- bridge daemon -- USB CDC -- RP2040 -- USB HID -- Codex Micro
AJAZZ AKP03E ------ vendor HID ------+
```

The Stream Deck plugin is the recommended Stream Deck integration. Direct HID support for Stream Deck hardware also exists, but it requires closing the official Stream Deck app.

## Install the Windows release

Download `micro-emu-v1.1.0-windows-x64.zip` from the GitHub release, extract it, and double-click `Install.cmd`. The installer does not need administrator access. It installs the bridge under `%LOCALAPPDATA%\micro-emu`, starts it automatically when you sign in, and opens the bundled Stream Deck plugin for confirmation in the Stream Deck app.

To remove it, double-click `Uninstall.cmd` from the same extracted folder. The standalone `.streamDeckPlugin` release asset is also available for users who only want to install or update the plugin.

If you have an RP2040 Zero board, the same bundle also ships the prebuilt firmware (`codex_micro_rp2040_bridge.uf2`) plus a `Flash-Firmware.cmd` helper to flash it; see `README.txt` in the extracted folder. The firmware is also published as a standalone `.uf2` release asset.


## Stream Deck plugin

The plugin coexists with other Stream Deck plugins and profiles. Build and link it for development:

```powershell
npm run plugin:install
npm run plugin:build
npm run plugin:link
```

Start the bridge daemon:

```powershell
npm run bridge:daemon -- --port auto
```

For a distributable plugin package, run `npm run plugin:pack`; it writes a `.streamDeckPlugin` file to `artifacts/`.

### Available actions

| Action | Purpose |
|---|---|
| Action Button | Choose a direct Micro control, daemon task-navigation command, or extended agent workflow action |
| Task Card | Render an assigned task; tap to select, or hold to show/minimize Codex |
| Knob | Codex Micro encoder turn and press |
| Crux Horizontal / Vertical | Radial controls with assignable dial presses; tap the horizontal strip to cycle Sol, Terra, and Luna |
| Mic / Send | `ACT10` microphone and `ACT12` send controls |
| Arrow Key | Keypad-only directional or virtual-rotor control |

The Action Button property inspector includes a physical Codex Micro map, automatic action-specific icons and titles, and manual overrides. Task Cards, Micro commands, and extended catalog actions use distinct daemon events, so a task slot can never swallow a catalog action. Existing profiles that use the former Agent Button keep working through a hidden compatibility action.

The complete catalog and the adapter event contract are documented in [docs/streamdeck-action-catalog.md](docs/streamdeck-action-catalog.md).

The daemon renders task status, RGB configuration, and optional display context back to the plugin. On Stream Deck + hardware, the touch strip can show task, project, model, effort, progress, and resource-usage information.

## AJAZZ AKP03E hardware emulator

The verified hardware path uses an **AJAZZ AKP03E revision 2** (`0300:3002`) and an RP2040 Zero-class board. The bridge opens the AJAZZ vendor HID collection (`MI_00`, Usage Page `FFA0`, Usage `0001`) and the RP2040 firmware exposes the Codex Micro HID interface plus a CDC channel.

```text
AJAZZ AKP03E -- vendor HID -- Rust bridge -- USB CDC -- RP2040 -- Codex Micro HID -- Windows / ChatGPT
```

The RP2040 approach uses Windows' built-in USB class drivers: it requires no kernel driver, driver signing, Secure Boot change, or test-signing mode.

### Build and flash

Requirements: Windows, Node.js 20+, Rust/Cargo, an RP2040 Zero-class board, and an AKP03E for the physical controller path.

```powershell
npm test
npm run bridge:build
npm run rp2040:setup
npm run rp2040:build
npm run rp2040:verify
```

Put the RP2040 into BOOTSEL mode, then flash it:

```powershell
npm run rp2040:flash
npm run rp2040:port
```

Close the AJAZZ OEM software before running the bridge, then use the detected port (or let the daemon discover it):

```powershell
npm run bridge:run -- -- --port COM7
# or
npm run bridge:daemon -- --port auto
```

The AJAZZ LCD keys map to agent buttons, its lower keys map to actions, and its encoders emit the existing radial and encoder events. The bridge can also render six task/status tiles on the AJAZZ displays.

## Codex MCP setup

Build the bridge and register its proxy in `.codex/config.toml`:

```toml
[mcp_servers.micro_emu_bridge]
command = "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe"
args = ["--mcp-proxy", "--agent", "codex", "--autostart"]
cwd = "D:\\Programming\\micro-emu"
```

The proxy starts or attaches to the loopback-only daemon. Key tools include `bridge_status`, `poll_events`, `publish_tasks`, `set_thread_status`, `set_display_context`, and `set_rgb_config`. Hardware-specific commands are unavailable when the daemon runs in standalone mode (`--port none`).

## Multi-controller task board

The daemon combines task slots across active controllers and assigns published tasks using stable IDs. An AKP03E supplies six slots; Stream Deck + and XL controllers supply eight by default. It also supports dynamic key and LCD-slot partitioning for Codex, ZCode, and Hermes sessions.

See [the multi-agent guide](docs/multi-agent-coexistence.md) for the session and partitioning model.

## Development commands

```powershell
npm test
npm run bridge:test
npm run firmware:host-test
npm run hardware:test -- --listen 45
npm run plugin:watch
```

## Documentation

- [RP2040 bridge architecture](docs/rp2040-bridge.md)
- [First RP2040 connection](docs/rp2040-first-connection.md)
- [Verified AJAZZ hardware profile](docs/hardware-profile.md)
- [ZCode integration](docs/ZCode_integration.md)
- [Hermes integration](docs/Hermes_integration.md)

## License

Released under the [MIT License](LICENSE). Codex, ChatGPT, Stream Deck, AJAZZ, and AKP03 are trademarks of their respective owners. This is an independent, unendorsed project.
