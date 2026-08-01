[CmdletBinding()]
param(
    [string]$Name = "micro-emu Codex Micro Test",
    [string]$OutputDirectory = ".\artifacts\driver-signing",
    [switch]$InstallTrust
)

$ErrorActionPreference = "Stop"
$resolved = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath(
    $OutputDirectory
)
New-Item -ItemType Directory -Path $resolved -Force | Out-Null

$certificate = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject "CN=$Name" `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -HashAlgorithm SHA256 `
    -KeyExportPolicy Exportable `
    -NotAfter (Get-Date).AddYears(1)

$cerPath = Join-Path $resolved "codex-micro-test.cer"
Export-Certificate -Cert $certificate -FilePath $cerPath | Out-Null

if ($InstallTrust) {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]$identity
    if (-not $principal.IsInRole(
            [Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "-InstallTrust must run from an elevated PowerShell."
    }
    Import-Certificate -FilePath $cerPath `
        -CertStoreLocation "Cert:\LocalMachine\Root" | Out-Null
    Import-Certificate -FilePath $cerPath `
        -CertStoreLocation "Cert:\LocalMachine\TrustedPublisher" | Out-Null
}

[pscustomobject]@{
    thumbprint = $certificate.Thumbprint
    certificate = $cerPath
    trustedForMachine = [bool]$InstallTrust
} | ConvertTo-Json
