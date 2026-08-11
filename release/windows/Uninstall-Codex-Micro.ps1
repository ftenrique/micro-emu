[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$installRoot = Join-Path $env:LOCALAPPDATA "micro-emu"
$startupFolder = [Environment]::GetFolderPath("Startup")
$startupCommand = Join-Path $startupFolder "Codex Micro Bridge.cmd"

Get-Process -Name "rp2040-bridge" -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item -LiteralPath $startupCommand -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $installRoot -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "The Codex Micro bridge and automatic startup entry were removed."
Write-Host "Remove the Codex Micro plugin from the Stream Deck app if you no longer want it."
