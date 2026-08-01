[CmdletBinding()]
param(
    [string]$CertificatePath = ".\artifacts\driver-signing\codex-micro-test.cer",
    [switch]$AcknowledgeMachineTrustChange
)

$ErrorActionPreference = "Stop"
if (-not $AcknowledgeMachineTrustChange) {
    throw (
        "Trusting a test certificate changes the machine certificate stores. " +
        "Re-run with -AcknowledgeMachineTrustChange only on the test machine."
    )
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]$identity
if (-not $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Installing machine certificate trust requires an elevated PowerShell."
}

$resolvedCertificate = (Resolve-Path $CertificatePath).Path
$certificate = Get-PfxCertificate -FilePath $resolvedCertificate

Import-Certificate -FilePath $resolvedCertificate `
    -CertStoreLocation "Cert:\LocalMachine\Root" | Out-Null
Import-Certificate -FilePath $resolvedCertificate `
    -CertStoreLocation "Cert:\LocalMachine\TrustedPublisher" | Out-Null

$root = Get-ChildItem -LiteralPath "Cert:\LocalMachine\Root\$($certificate.Thumbprint)" `
    -ErrorAction SilentlyContinue
$publisher = Get-ChildItem `
    -LiteralPath "Cert:\LocalMachine\TrustedPublisher\$($certificate.Thumbprint)" `
    -ErrorAction SilentlyContinue
if (-not $root -or -not $publisher) {
    throw "The certificate was not found in both required machine trust stores."
}

[pscustomobject]@{
    thumbprint = $certificate.Thumbprint
    certificate = $resolvedCertificate
    trustedRoot = [bool]$root
    trustedPublisher = [bool]$publisher
} | ConvertTo-Json
