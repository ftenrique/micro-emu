[CmdletBinding()]
param(
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $PSScriptRoot "rp2040-bridge\Cargo.toml"
$releaseBinary = Join-Path $PSScriptRoot "rp2040-bridge\target\release\rp2040-bridge.exe"

function Invoke-Step {
    param(
        [Parameter(Mandatory)] [string]$Name,
        [Parameter(Mandatory)] [scriptblock]$Action
    )

    Write-Host "==> $Name"
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE."
    }
}

Push-Location $repositoryRoot
try {
    if (-not $SkipTests) {
        Invoke-Step "Run bridge tests" {
            & cargo test --manifest-path $manifest --offline
        }
    }

    Invoke-Step "Build release bridge" {
        & cargo build --manifest-path $manifest --release --offline
    }

    if (-not (Test-Path -LiteralPath $releaseBinary)) {
        throw "Bridge build completed but did not produce $releaseBinary."
    }
    Write-Host "Bridge ready: $releaseBinary"
}
finally {
    Pop-Location
}