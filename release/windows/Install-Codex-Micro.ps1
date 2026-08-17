[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$bundleRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$sourceBridge = Join-Path $bundleRoot "rp2040-bridge.exe"
$sourcePlugin = Join-Path $bundleRoot "com.micro-emu.codex.streamDeckPlugin"
$installRoot = Join-Path $env:LOCALAPPDATA "micro-emu"
$installedBridge = Join-Path $installRoot "rp2040-bridge.exe"
$startupFolder = [Environment]::GetFolderPath("Startup")
$startupCommand = Join-Path $startupFolder "Codex Micro Bridge.cmd"

if (-not (Test-Path -LiteralPath $sourceBridge)) {
    throw "The release bundle is missing rp2040-bridge.exe. Extract the complete ZIP and try again."
}
if (-not (Test-Path -LiteralPath $sourcePlugin)) {
    throw "The release bundle is missing the Stream Deck plugin package. Extract the complete ZIP and try again."
}

Get-Process -Name "rp2040-bridge" -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Path $installRoot -Force | Out-Null
Copy-Item -LiteralPath $sourceBridge -Destination $installedBridge -Force

$startupContents = @(
    "@echo off"
    "start `"Codex Micro Bridge`" /min `"$installedBridge`" --daemon --port auto --controller none"
) -join "`r`n"
Set-Content -LiteralPath $startupCommand -Value $startupContents -Encoding Ascii

Start-Process -FilePath $installedBridge -ArgumentList @("--daemon", "--port", "auto", "--controller", "none") -WindowStyle Hidden

try {
    Start-Process -FilePath $sourcePlugin
    Write-Host "Stream Deck will ask you to confirm the Codex Micro plugin installation."
}
catch {
    Write-Warning "The bridge is installed, but the Stream Deck plugin could not be opened automatically. Double-click $sourcePlugin after installing Stream Deck."
}

Write-Host ""
Write-Host "Codex Micro 1.2.0 is installed."
Write-Host "Bridge: $installedBridge"
Write-Host "Automatic startup: $startupCommand"
