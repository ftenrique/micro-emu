# Codex Micro for AJAZZ AKP03

A working Windows integration that connects an AJAZZ AKP03E to Codex Micro
through a low-cost RP2040 USB bridge. The project includes the portable
protocol implementation, the Windows user-space bridge, the RP2040 HID+CDC
firmware, hardware validation tools, and reproducible build scripts.

The supported path is:

```text
AJAZZ AKP03E  <->  Rust user-space bridge  <->  USB CDC  <->  RP2040
                                                        <->  USB HID
                                                        <->  Windows / ChatGPT
```

The implementation is operational with an AJAZZ AKP03E revision 2
(`0300:3002`) and an RP2040 Zero-class board.

## Why a low-cost RP2040 board is required

The RP2040 is the USB adapter between the AJAZZ device and Windows. Its
firmware presents the Codex Micro HID interface and a CDC channel for the
user-space bridge, while the bridge handles the AJAZZ vendor HID interface.

This inexpensive board is deliberate:

- Windows uses its built-in HID and USB CDC class drivers.
- The project does not require a custom kernel-mode driver or a signed driver
  package.
- Secure Boot remains enabled and Windows test-signing mode is not required.
- Firmware updates are reversible through the board's `BOOTSEL` mass-storage
  mode and affect only the RP2040.
- The hardware cost is kept low because no dedicated USB controller or custom
  Windows hardware is needed.

The repository contains an optional driver implementation for compatibility
research, but it is not required for the working RP2040 path.

## Requirements

- Windows with PowerShell 5.1 or later.
- Node.js 20 or later.
- Rust 1.85 or later with Cargo.
- A low-cost RP2040 Zero or compatible RP2040 board.
- An AJAZZ AKP03E revision 2 (`0300:3002`) for the physical integration.
- A USB data cable and approximately 1.3 GiB available on `D:` for the pinned
  RP2040 toolchain.

No npm dependencies are required.

## Quick start

From a clean checkout:

```powershell
git clone https://github.com/ftenrique/micro-emu.git
Set-Location .\micro-emu

npm test
npm run verify:descriptor
npm run bridge:test
npm run bridge:build
npm run firmware:host-test
npm run rp2040:setup
npm run rp2040:check
npm run rp2040:build
npm run rp2040:verify
```

The RP2040 firmware artifact is generated at:

```text
firmware\rp2040-zero\build\codex_micro_rp2040_bridge.uf2
```

## Flash the RP2040

1. Disconnect the RP2040 board.
2. Hold `BOOTSEL` while connecting the USB cable.
3. Wait for the `RPI-RP2` drive to appear.
4. Run:

   ```powershell
   npm run rp2040:flash
   ```

5. Disconnect and reconnect the board normally.
6. Find the CDC port:

   ```powershell
   npm run rp2040:port
   ```

The flash script validates the UF2 image and requires exactly one `RPI-RP2`
drive before copying. It changes only the firmware on the RP2040 and does not
modify Windows boot configuration, drivers, or Secure Boot.

## Run the bridge

Close the official AJAZZ software before starting the bridge. Replace `COM7`
with the port reported by `npm run rp2040:port`:

```powershell
npm run bridge:run -- -- --port COM7
```

The bridge opens the AJAZZ vendor collection `MI_00 / FFA0:0001`, translates
its controls into Codex Micro events, and forwards the protocol through the
RP2040 HID interface.

For a firmware-only smoke test without the AJAZZ connected:

```powershell
npm run bridge:run -- -- --port COM7 --no-ajazz --listen 120 --emit AG00 --emit-after 10
```

## Integrate with Codex through MCP

The bridge includes a local Model Context Protocol (MCP) server over STDIO.
Codex starts the bridge as a child process, sends JSON-RPC messages through
standard input, and receives tool results through standard output. No HTTP
server or additional network port is required.

Build the bridge first and identify the RP2040 CDC port:

```powershell
npm run bridge:build
npm run rp2040:port
```

Register the bridge with Codex CLI. Replace `COM7` and the project path when
necessary:

```powershell
codex mcp add micro-emu-rp2040 `
  -- npm.cmd --silent --prefix D:\Programming\micro-emu `
  run bridge:run -- -- --port COM7 --mcp
```

The two `--` separators after `bridge:run` are intentional: one is consumed by
npm and the other is forwarded to the bridge script. The explicit `--mcp` flag
starts the STDIO MCP transport.

The same server can be configured directly in `%USERPROFILE%\.codex\config.toml`
or in a trusted project-scoped `.codex/config.toml`:

```toml
[mcp_servers.micro_emu_rp2040]
command = "npm.cmd"
args = ["--silent", "run", "bridge:run", "--", "--", "--port", "COM7", "--mcp"]
cwd = "D:\\Programming\\micro-emu"
startup_timeout_sec = 20
tool_timeout_sec = 60
enabled = true
```

If `npm.cmd` is not available in Codex's `PATH`, use the absolute path returned
by `Get-Command npm.cmd`, or run the compiled bridge directly:

```toml
[mcp_servers.micro_emu_rp2040]
command = "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe"
args = ["--port", "COM7", "--mcp"]
cwd = "D:\\Programming\\micro-emu"
```

Verify the registration with:

```powershell
codex mcp list
```

In the Codex TUI, use `/mcp` to inspect the connected server. The Codex app,
CLI, and IDE extension share the same MCP configuration on the host. Once
connected, ask Codex to call `bridge_status` first. The bridge exposes these
tools:

- `bridge_status` — report firmware, serial port, and AJAZZ connection state.
- `emit_key` — emit a synthetic Codex Micro key press/release.
- `send_codex_message` — send one Codex Micro JSON message.
- `set_thread_status` — update the six AJAZZ LCD status slots.
- `set_rgb_config` — send `v.oai.rgbcfg` configuration.
- `device_status` — request `device.status` from the RP2040 firmware.

When Codex owns the MCP process, do not start a second bridge process against
the same COM port. Close any manually started `bridge:run` process before
using the MCP configuration.
## Implemented functionality

- Codex Micro HID reports with Report ID 6 and 63-byte input, output, and
  feature reports.
- USB CDC transport between the bridge and the RP2040, including framing,
  sequence numbers, payload lengths, and CRC16-CCITT.
- Portable JavaScript framing, message validation, fixtures, and safe handling
  of unknown methods.
- `device.status`, `v.oai.thstatus`, `v.oai.rgbcfg`, radial controls, and key
  events.
- Six LCD keys mapped to `AG00`-`AG05`.
- Lower keys mapped to `ACT06`-`ACT08`.
- Encoder rotation, direction, and click events.
- AJAZZ LCD updates with color, brightness, and clearing behavior.
- Correlated ACK responses for RPC calls and no responses for notifications.
- Windows inventory, preflight, hardware diagnostics, firmware host tests, and
  reproducible RP2040 artifact verification.

## Hardware validation

With the AJAZZ software closed, run the physical test utility:

```powershell
npm run hardware:test -- --listen 45
```

The test writes six numbered tiles to the LCDs and reads the keys, encoders,
and encoder clicks. The verified hardware profile is documented in
[docs/hardware-profile.md](docs/hardware-profile.md).

## Protocol API

```js
import {
  FrameDecoder,
  frameJson,
  createRequest,
  keyEvent,
} from "./protocol/index.js";

const reports = frameJson(createRequest("device.status", undefined, 1));

const decoder = new FrameDecoder();
for (const report of reports) {
  const { messages, errors } = decoder.feed(report);
  // Process messages and record errors without stopping the transport.
}

const press = keyEvent("AG00", 1, 0);
```

## Documentation

- [Deployment](DEPLOYMENT.md) — build, flash, run, validate, and publish.
- [RP2040 bridge details](docs/rp2040-bridge.md) — firmware and transport
  architecture.
- [Hardware profile](docs/hardware-profile.md) — verified AJAZZ interface and
  controls.
- [Windows environment](docs/windows-environment.md) — inventory and system
  diagnostics.

## Security model

The portable protocol core does not access hardware, the network, or the
filesystem. The recommended RP2040 path uses Windows inbox USB class drivers
and does not install kernel code, alter Secure Boot, enable test signing, or
modify the Windows driver store. Firmware flashing is isolated to the RP2040
board.

## License and attribution

Project code is released under the [MIT License](LICENSE). Protocol and
interoperability information adapted from FreeMicro is attributed in
[NOTICE](NOTICE), with the original license reproduced at
[docs/third-party/freemicro-LICENSE.txt](docs/third-party/freemicro-LICENSE.txt).

Codex, Codex Micro, ChatGPT, AJAZZ, and AKP03 may be trademarks of their
respective owners. This project is independent and is not endorsed by OpenAI,
AJAZZ, or the FreeMicro authors.