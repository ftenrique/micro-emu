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