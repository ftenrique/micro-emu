[CmdletBinding()]
param(
    [switch]$SkipHostTest,
    [switch]$SkipBuild,
    [switch]$WhatIf
)

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$buildScript = Join-Path $PSScriptRoot "build-rp2040.ps1"
$verifyScript = Join-Path $repositoryRoot "tools\verify-rp2040-artifact.mjs"
$flashScript = Join-Path $PSScriptRoot "flash-rp2040.ps1"

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
    if (-not $SkipHostTest) {
        Invoke-Step "Run firmware host tests" {
            & npm.cmd run firmware:host-test
        }
    }

    if (-not $SkipBuild) {
        Invoke-Step "Build RP2040 firmware" {
            & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $buildScript
        }
    }

    Invoke-Step "Verify UF2 artifact" {
        & node.exe $verifyScript
    }

    $flashArguments = @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-File", $flashScript
    )
    if ($WhatIf) {
        $flashArguments += "-WhatIf"
    }

    Invoke-Step "Flash RP2040 firmware" {
        & powershell.exe @flashArguments
    }
}
finally {
    Pop-Location
}