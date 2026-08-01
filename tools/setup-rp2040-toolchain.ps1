[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$toolsRoot = Join-Path $repoRoot ".toolchains"
$lockPath = Join-Path $PSScriptRoot "rp2040-toolchain.lock.json"
$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json
$sdkPath = Join-Path $toolsRoot "pico-sdk-$($lock.picoSdk.version)"
$picotoolSource = Join-Path $toolsRoot "picotool-source-$($lock.picotool.version)"
$compiler = Join-Path $toolsRoot "bin\arm-none-eabi-gcc.exe"
$archive = Join-Path $toolsRoot "arm-gnu-toolchain-$($lock.armGnuToolchain.version).zip"

if (-not (Test-Path $toolsRoot)) {
    New-Item -ItemType Directory -Path $toolsRoot | Out-Null
}

if (-not (Test-Path $sdkPath)) {
    & git clone --branch $lock.picoSdk.version --depth 1 `
        $lock.picoSdk.repository $sdkPath
    if ($LASTEXITCODE -ne 0) {
        throw "Pico SDK clone failed with exit code $LASTEXITCODE."
    }
}

$sdkCommit = (& git -c "safe.directory=$sdkPath" -C $sdkPath rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sdkCommit -ne $lock.picoSdk.commit) {
    throw "Pico SDK commit does not match the lock file."
}

$tinyUsbPath = Join-Path $sdkPath "lib\tinyusb"
if (-not (Test-Path (Join-Path $tinyUsbPath "hw\bsp\rp2040"))) {
    & git -c "safe.directory=$sdkPath" -C $sdkPath `
        submodule update --init --depth 1 lib/tinyusb
    if ($LASTEXITCODE -ne 0) {
        throw "TinyUSB submodule checkout failed with exit code $LASTEXITCODE."
    }
}
$tinyUsbCommit = (& git -c "safe.directory=$tinyUsbPath" -C $tinyUsbPath `
    rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $tinyUsbCommit -ne $lock.tinyUsb.commit) {
    throw "TinyUSB commit does not match the lock file."
}

if (-not (Test-Path $picotoolSource)) {
    & git clone --branch $lock.picotool.version --depth 1 `
        $lock.picotool.repository $picotoolSource
    if ($LASTEXITCODE -ne 0) {
        throw "picotool clone failed with exit code $LASTEXITCODE."
    }
}
$picotoolCommit = (& git -c "safe.directory=$picotoolSource" `
    -C $picotoolSource rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $picotoolCommit -ne $lock.picotool.commit) {
    throw "picotool commit does not match the lock file."
}

if (-not (Test-Path $compiler)) {
    Invoke-WebRequest -Uri $lock.armGnuToolchain.url -OutFile $archive
    $actualHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    if ($actualHash -ne $lock.armGnuToolchain.sha256) {
        throw "Arm GNU Toolchain SHA-256 does not match the lock file."
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $toolsRoot -Force
    Remove-Item -LiteralPath $archive
}

& (Join-Path $PSScriptRoot "check-rp2040-toolchain.ps1")
