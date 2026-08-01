[CmdletBinding()]
param(
    [string]$PublishedInf,
    [switch]$AcknowledgeRemoval
)

$ErrorActionPreference = "Stop"
if (-not $AcknowledgeRemoval) {
    throw "Re-run with -AcknowledgeRemoval to remove the test device."
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]$identity
if (-not $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Driver removal requires an elevated PowerShell."
}

pnputil /remove-device "ROOT\CODEXMICROVHF\0000"
if ($PublishedInf) {
    pnputil /delete-driver $PublishedInf /uninstall /force
}
