[CmdletBinding()]
param(
    [switch]$AcknowledgeRebootAndSecurityImpact
)

$ErrorActionPreference = "Stop"
if (-not $AcknowledgeRebootAndSecurityImpact) {
    throw (
        "Test-signing changes Windows boot policy and requires a reboot. " +
        "Re-run with -AcknowledgeRebootAndSecurityImpact only on the test machine."
    )
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]$identity
if (-not $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Changing boot policy requires an elevated PowerShell."
}

bcdedit /set testsigning on
if ($LASTEXITCODE -ne 0) {
    throw (
        "bcdedit failed. Secure Boot may prevent test-signing mode; do not " +
        "disable Secure Boot without reviewing the machine's recovery setup."
    )
}
Write-Host "Test-signing was enabled. Reboot Windows before installing the driver."
