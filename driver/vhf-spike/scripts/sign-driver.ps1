[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackageDirectory,
    [Parameter(Mandatory)]
    [string]$CertificateThumbprint
)

$ErrorActionPreference = "Stop"
$package = (Resolve-Path $PackageDirectory).Path
$kitsRoot = "${env:ProgramFiles(x86)}\Windows Kits\10"
$signTool = Get-ChildItem (Join-Path $kitsRoot "bin") -Recurse `
    -Filter "signtool.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\x64\\" } |
    Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName
$inf2Cat = Get-ChildItem (Join-Path $kitsRoot "bin") -Recurse `
    -Filter "Inf2Cat.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\(x86|x64)\\" } |
    Sort-Object `
        @{ Expression = { if ($_.FullName -match "\\x64\\") { 0 } else { 1 } } }, `
        @{ Expression = { $_.FullName }; Descending = $true } |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $signTool) {
    throw "signtool.exe (x64) is missing from the Windows SDK/WDK."
}
if (-not $inf2Cat) {
    throw "Inf2Cat.exe (x86 or x64) is missing from the WDK."
}

$infFiles = @(Get-ChildItem -LiteralPath $package -File -Filter "*.inf")
$sysFiles = @(Get-ChildItem -LiteralPath $package -File -Filter "*.sys")
if ($infFiles.Count -eq 0 -or $sysFiles.Count -eq 0) {
    throw "The package directory must contain at least one .inf and one .sys file: $package"
}

$certificate = Get-ChildItem -LiteralPath "Cert:\CurrentUser\My\$CertificateThumbprint" `
    -ErrorAction SilentlyContinue
if (-not $certificate) {
    throw "Certificate $CertificateThumbprint was not found in Cert:\CurrentUser\My."
}
if (-not $certificate.HasPrivateKey) {
    throw "Certificate $CertificateThumbprint does not have an accessible private key."
}

Write-Host "SignTool: $signTool"
Write-Host "Inf2Cat:  $inf2Cat"
Write-Host "Package:  $package"

# Sign the binaries first: their final hashes must be recorded in the catalog.
foreach ($file in $sysFiles) {
    & $signTool sign /v /fd SHA256 /sha1 $CertificateThumbprint $file.FullName
    if ($LASTEXITCODE -ne 0) {
        throw "Signing $($file.Name) failed with exit code $LASTEXITCODE."
    }
}

& $inf2Cat /driver:"$package" /os:10_X64 /uselocaltime
if ($LASTEXITCODE -ne 0) {
    throw "Inf2Cat failed with exit code $LASTEXITCODE."
}

$catFiles = @(Get-ChildItem -LiteralPath $package -File -Filter "*.cat")
if ($catFiles.Count -eq 0) {
    throw "Inf2Cat did not create a .cat file in $package."
}
foreach ($file in $catFiles) {
    & $signTool sign /v /fd SHA256 /sha1 $CertificateThumbprint $file.FullName
    if ($LASTEXITCODE -ne 0) {
        throw "Signing $($file.Name) failed with exit code $LASTEXITCODE."
    }
}

$signedFiles = @($sysFiles) + @($catFiles)
foreach ($file in $signedFiles) {
    $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
    if (-not $signature.SignerCertificate) {
        throw "No Authenticode signature was found on $($file.Name)."
    }
    Write-Host "$($file.Name): signature present ($($signature.SignerCertificate.Thumbprint))"
}
