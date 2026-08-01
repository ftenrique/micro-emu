# Protocol monitor

Console bridge for the Codex Micro VHF spike. It opens the driver's control
interface, decodes 63-byte output fragments and can submit 63-byte input
fragments.

## Offline validation

```powershell
npm run monitor:build
npm run monitor:self-test
```

The self-test exercises fragmented `device.status` traffic and an `AG00`
input report without requiring the driver.

## Connected VHF session

Run from an elevated PowerShell after installing the test driver:

```powershell
npm run monitor -- --serve --stats
```

In another elevated terminal, emit one synthetic `AG00` press/release:

```powershell
npm run monitor -- --emit AG00
```

Add `--verbose` only when raw protocol payloads are needed. The default output
redacts message bodies and is safer to attach to bug reports.
