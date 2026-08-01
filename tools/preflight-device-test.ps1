[CmdletBinding()]
param(
    [string]$OutputPath = ".\artifacts\device-test-preflight.json",
    [switch]$Strict
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$inventoryScript = Join-Path $PSScriptRoot "inventory-windows.ps1"
$toolchainScript = Join-Path $PSScriptRoot "check-rp2040-toolchain.ps1"
$inventoryPath = Join-Path $repoRoot "artifacts\inventory-connected.json"

function Invoke-CheckedCommand {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][scriptblock]$Command
    )

    & $Command *> $null
    [ordered]@{
        name = $Name
        passed = ($LASTEXITCODE -eq 0)
        exitCode = $LASTEXITCODE
    }
}

Push-Location $repoRoot
try {
    $inventory = & $inventoryScript -OutputPath $inventoryPath | ConvertFrom-Json
    $toolchain = & $toolchainScript | ConvertFrom-Json

    $checks = @(
        Invoke-CheckedCommand "protocol-tests" { npm test }
        Invoke-CheckedCommand "firmware-host-tests" {
            npm run firmware:host-test
        }
        Invoke-CheckedCommand "rp2040-artifact" {
            npm run rp2040:verify
        }
        Invoke-CheckedCommand "ajazz-hardware-test-build" {
            cargo build --manifest-path tools/ajazz-hardware-test/Cargo.toml `
                --release --offline --quiet
        }
        Invoke-CheckedCommand "rp2040-bridge-tests" {
            cargo test --manifest-path tools/rp2040-bridge/Cargo.toml `
                --offline --quiet
        }
        Invoke-CheckedCommand "rp2040-bridge-build" {
            cargo build --manifest-path tools/rp2040-bridge/Cargo.toml `
                --release --offline --quiet
        }
    )
} finally {
    Pop-Location
}

$knownAjazzCount = @($inventory.pnp.ajazzCandidates).Count
$unverifiedCount = @($inventory.pnp.unverifiedCandidates).Count
$codexMicroCount = @($inventory.pnp.codexMicroReference).Count
$failedChecks = @($checks | Where-Object { -not $_.passed })
$blockers = @()

if ($knownAjazzCount -eq 0) {
    $blockers += "No known AKP03-family VID/PID is currently present."
}
if ($codexMicroCount -eq 0) {
    $blockers += "No flashed RP2040 Codex Micro HID is currently present."
}
if (-not $toolchain.ready -and $codexMicroCount -eq 0) {
    $blockers += "The RP2040 firmware toolchain is incomplete."
}
if ($failedChecks.Count -gt 0) {
    $blockers += "One or more repository checks failed."
}

$result = [ordered]@{
    schemaVersion = 2
    capturedAtUtc = [DateTime]::UtcNow.ToString("o")
    readyForHardwareProtocolTest = ($knownAjazzCount -gt 0 -and $failedChecks.Count -eq 0)
    readyToBuildRp2040Firmware = [bool]$toolchain.ready
    readyToRunBridge = ($knownAjazzCount -gt 0 -and
        $codexMicroCount -gt 0 -and
        $failedChecks.Count -eq 0)
    readyForEndToEndTest = ($knownAjazzCount -gt 0 -and
        $codexMicroCount -gt 0 -and
        $failedChecks.Count -eq 0)
    devices = [ordered]@{
        knownAjazzCollections = $knownAjazzCount
        unverifiedCollections = $unverifiedCount
        codexMicroCollections = $codexMicroCount
    }
    checks = $checks
    toolchain = $toolchain
    blockers = $blockers
}

$json = $result | ConvertTo-Json -Depth 8
$resolvedOutput = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath(
    $OutputPath
)
$parent = Split-Path -Parent $resolvedOutput
if ($parent -and -not (Test-Path $parent)) {
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
}
Set-Content -LiteralPath $resolvedOutput -Value $json -Encoding UTF8
$json

if ($Strict -and -not $result.readyForEndToEndTest) {
    exit 2
}
