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
$mcpServerName = "micro_emu_bridge"

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

function Register-CodexMcpServer {
    $codexCommand = Get-Command -Name "codex" -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1

    if (-not $codexCommand) {
        Write-Warning "Codex CLI was not found. The bridge is installed, but MCP registration was skipped. Install Codex CLI and run 'codex mcp add $mcpServerName -- `"$installedBridge`" --mcp-proxy --agent codex --autostart'."
        return
    }

    # Reinstalling should update the entry if an older release already added it.
    & $codexCommand.Source mcp remove $mcpServerName 2>&1 | Out-Null

    & $codexCommand.Source mcp add $mcpServerName -- $installedBridge --mcp-proxy --agent codex --autostart 2>&1 | ForEach-Object {
        Write-Verbose $_
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "The bridge is installed, but Codex MCP registration failed (exit code $LASTEXITCODE). Run 'codex mcp add $mcpServerName -- `"$installedBridge`" --mcp-proxy --agent codex --autostart' manually."
        return
    }

    Write-Host "Registered $mcpServerName with Codex MCP."
}

Register-CodexMcpServer

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
