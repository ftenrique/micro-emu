[CmdletBinding()]
param(
    [string]$Version = "1.0.0",
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$artifactsRoot = Join-Path $repositoryRoot "artifacts"
$bundleName = "micro-emu-v$Version-windows-x64"
$bundleRoot = Join-Path $artifactsRoot $bundleName
$zipPath = Join-Path $artifactsRoot "$bundleName.zip"
$pluginPath = Join-Path $artifactsRoot "com.micro-emu.codex.streamDeckPlugin"
$bridgeManifest = Join-Path $repositoryRoot "tools\rp2040-bridge\Cargo.toml"
$bridgeTargetRoot = Join-Path $artifactsRoot "cargo-target"
$bridgePath = Join-Path $bridgeTargetRoot "release\rp2040-bridge.exe"

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
    New-Item -ItemType Directory -Path $artifactsRoot -Force | Out-Null

    if (-not $SkipTests) {
        Invoke-Step "Run protocol and plugin tests" { & npm test }
        Invoke-Step "Run bridge tests" { & npm run bridge:test }
    }

    Invoke-Step "Build Stream Deck plugin" { & npm run plugin:build }
    Remove-Item -LiteralPath $pluginPath -Force -ErrorAction SilentlyContinue
    Invoke-Step "Package Stream Deck plugin" { & npm run plugin:pack }
    Invoke-Step "Build Windows bridge" {
        & cargo build --manifest-path $bridgeManifest --release --offline --target-dir $bridgeTargetRoot
    }

    if (-not (Test-Path -LiteralPath $pluginPath)) {
        throw "Plugin packaging did not produce $pluginPath."
    }
    if (-not (Test-Path -LiteralPath $bridgePath)) {
        throw "Bridge build did not produce $bridgePath."
    }

    Remove-Item -LiteralPath $bundleRoot -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $zipPath -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $bundleRoot | Out-Null

    Copy-Item -LiteralPath $bridgePath -Destination $bundleRoot
    Copy-Item -LiteralPath $pluginPath -Destination $bundleRoot
    Copy-Item -Path (Join-Path $repositoryRoot "release\windows\*") -Destination $bundleRoot -Recurse
    Copy-Item -LiteralPath (Join-Path $repositoryRoot "LICENSE") -Destination $bundleRoot
    Copy-Item -LiteralPath (Join-Path $repositoryRoot "NOTICE") -Destination $bundleRoot

    Compress-Archive -Path (Join-Path $bundleRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal

    $checksumFiles = @($zipPath, $pluginPath)
    $checksumLines = foreach ($file in $checksumFiles) {
        $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $file
        "$($hash.Hash.ToLowerInvariant())  $([IO.Path]::GetFileName($file))"
    }
    $checksumPath = Join-Path $artifactsRoot "SHA256SUMS.txt"
    Set-Content -LiteralPath $checksumPath -Value $checksumLines -Encoding Ascii

    Write-Host "Release bundle: $zipPath"
    Write-Host "Plugin package: $pluginPath"
    Write-Host "Checksums: $checksumPath"
}
finally {
    Pop-Location
}
