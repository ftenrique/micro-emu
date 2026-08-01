[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$InfPath,
    [switch]$AcknowledgeTestDriverRisk
)

$ErrorActionPreference = "Stop"
if (-not $AcknowledgeTestDriverRisk) {
    throw (
        "Installation changes the kernel driver store. Re-run with " +
        "-AcknowledgeTestDriverRisk after reviewing docs/windows-handshake.md."
    )
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]$identity
if (-not $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Driver installation requires an elevated PowerShell."
}

$resolvedInf = (Resolve-Path $InfPath).Path
$devCon = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\Tools" `
    -Recurse -Filter "devcon.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\x64\\" } |
    Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $devCon) {
    throw "devcon.exe x64 is missing. Install the complete WDK tools."
}

& $devCon install $resolvedInf "ROOT\CODEXMICROVHF"
if ($LASTEXITCODE -ne 0) {
    throw "devcon install failed with exit code $LASTEXITCODE."
}
