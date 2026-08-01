# Deployment

This project is deployed as a combination of firmware for an RP2040 board and
a Rust bridge that runs on Windows. There is no web server or npm package to
publish.

## Requirements

- Windows PowerShell 5.1 or later.
- Node.js 20 or later.
- Rust 1.85 or later with Cargo.
- An RP2040 Zero board and a USB cable.
- Approximately 1.3 GiB of free space on `D:` for the isolated RP2040 toolchain.

## From a clean checkout

```powershell
git clone https://github.com/ftenrique/micro-emu.git
Set-Location .\micro-emu
npm test
npm run verify:descriptor
npm run bridge:test
npm run bridge:build
```

These commands validate the protocol core and build the bridge without
accessing the device.

## Build and verify the firmware

The first setup run installs the RP2040 toolchain in the isolated project
location:

```powershell
npm run rp2040:setup
npm run rp2040:check
npm run rp2040:build
npm run rp2040:verify
```

`rp2040:verify` checks that the generated artifact matches the expected
descriptor. Do not continue if any of these checks fail.

## Flash the board

1. Disconnect the RP2040 board.
2. Hold the `BOOTSEL` button while connecting the USB cable.
3. Run `npm run rp2040:flash` and follow the script prompts.
4. Disconnect and reconnect the board normally.
5. Run `npm run rp2040:port` to locate the serial port.

The flashing script targets an RP2040 Zero board. Check the board model and the
detected drive before confirming any write operation.

## Run the bridge

Start the bridge using the serial port reported by the previous step:

```powershell
npm run bridge:run -- -- --port COM7
```

Replace `COM7` with the actual port. The bridge exposes the HID+CDC interface
and transports protocol messages; it must remain open during the test session.

## Connect Codex through MCP

The bridge also provides a local Model Context Protocol (MCP) server over
STDIO. Codex launches it as a child process and communicates through standard
input and output; no HTTP server or additional network port is needed.

Build the bridge and find the RP2040 CDC port:

```powershell
npm run bridge:build
npm run rp2040:port
```

Register it with Codex CLI. Replace `COM7` and the project path as needed:

```powershell
codex mcp add micro-emu-rp2040 `
  -- npm.cmd --silent --prefix D:\Programming\micro-emu `
  run bridge:run -- -- --port COM7 --mcp
```

The equivalent `%USERPROFILE%\.codex\config.toml` entry is:

```toml
[mcp_servers.micro_emu_rp2040]
command = "npm.cmd"
args = ["--silent", "run", "bridge:run", "--", "--", "--port", "COM7", "--mcp"]
cwd = "D:\\Programming\\micro-emu"
startup_timeout_sec = 20
tool_timeout_sec = 60
enabled = true
```

If Codex cannot find `npm.cmd`, use its absolute path or point the configuration
to the compiled executable:

```toml
[mcp_servers.micro_emu_rp2040]
command = "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe"
args = ["--port", "COM7", "--mcp"]
cwd = "D:\\Programming\\micro-emu"
```

Verify the server:

```powershell
codex mcp list
```

In the Codex TUI, `/mcp` shows the active server. Ask Codex to call
`bridge_status` first; it reports the firmware, COM port, and AJAZZ connection.
The available MCP tools are `bridge_status`, `emit_key`,
`send_codex_message`, `set_thread_status`, `set_rgb_config`, and
`device_status`.

Codex owns the MCP bridge process. Do not run another bridge instance against
the same COM port at the same time.
## Optional hardware validation

With the keyboard OEM software closed, validate the AJAZZ device with:

```powershell
npm run hardware:test -- --listen 45
```

This command writes six tiles to the LCDs and reads keys, encoders, and encoder
clicks. It is a hardware test and should not be run against a device other than
the documented compatible profile.

## Troubleshooting

- **The port does not appear:** reconnect the board in normal mode and run
  `npm run rp2040:port` again.
- **The firmware build fails:** run `npm run rp2040:check` and repeat
  `npm run rp2040:setup` if the toolchain is missing.
- **The keyboard does not react:** close the OEM software and confirm that the
  vendor collection `MI_00 / FFA0:0001` is being used, as described in
  [docs/hardware-profile.md](docs/hardware-profile.md).
- **The bridge does not connect:** check the port, USB cable, and that no other
  process has opened the serial connection.

## Publish a new version

Run all local checks before creating a tag:

```powershell
npm test
npm run verify:descriptor
npm run bridge:test
npm run bridge:build
npm run rp2040:build
npm run rp2040:verify
git tag v0.1.0
git push origin main --follow-tags
```

Update the tag version when appropriate. Hardware artifacts and local inventory
files should not be included in a commit unless they are explicitly documented
and reproducible.