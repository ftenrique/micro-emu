[CmdletBinding()]
param(
    [string]$PicoSdkPath = $env:PICO_SDK_PATH,
    [string]$ArmToolchainPath = $env:PICO_TOOLCHAIN_PATH
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$localToolsRoot = Join-Path $repoRoot ".toolchains"
if (-not $PicoSdkPath) {
    $localSdk = Join-Path $localToolsRoot "pico-sdk-2.3.0"
    if (Test-Path $localSdk) {
        $PicoSdkPath = $localSdk
    }
}
if (-not $ArmToolchainPath) {
    $localArmGcc = Join-Path $localToolsRoot "bin\arm-none-eabi-gcc.exe"
    if (Test-Path $localArmGcc) {
        $ArmToolchainPath = $localToolsRoot
    }
}

$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$visualStudio = if (Test-Path $vsWhere) {
    & $vsWhere -latest -products * -property installationPath
} else {
    $null
}

$cmake = Get-Command cmake -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty Source
if (-not $cmake -and $visualStudio) {
    $candidate = Join-Path $visualStudio `
        "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
    if (Test-Path $candidate) {
        $cmake = $candidate
    }
}

$ninja = Get-Command ninja -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty Source
if (-not $ninja -and $visualStudio) {
    $candidate = Join-Path $visualStudio `
        "Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe"
    if (Test-Path $candidate) {
        $ninja = $candidate
    }
}

$armGccCommand = Get-Command arm-none-eabi-gcc -ErrorAction SilentlyContinue |
    Select-Object -First 1
$armGcc = if ($armGccCommand) {
    $armGccCommand.Source
} else {
    $null
}
if (-not $armGcc -and $ArmToolchainPath) {
    $candidate = Join-Path $ArmToolchainPath "bin\arm-none-eabi-gcc.exe"
    if (Test-Path $candidate) {
        $armGcc = (Resolve-Path $candidate).Path
    }
}

$resolvedSdk = $null
if ($PicoSdkPath -and (Test-Path $PicoSdkPath)) {
    $resolvedSdk = (Resolve-Path $PicoSdkPath).Path
}
$sdkImport = if ($resolvedSdk) {
    Join-Path $resolvedSdk "external\pico_sdk_import.cmake"
} else {
    $null
}
$sdkReady = $sdkImport -and (Test-Path $sdkImport)
$tinyUsbPath = if ($resolvedSdk) {
    Join-Path $resolvedSdk "lib\tinyusb\hw\bsp\rp2040"
} else {
    $null
}
$tinyUsbReady = $tinyUsbPath -and (Test-Path $tinyUsbPath)
$picotoolSourcePath = Join-Path $localToolsRoot "picotool-source-2.3.0"
$picotoolSourceReady = Test-Path (Join-Path $picotoolSourcePath "CMakeLists.txt")

$missing = @()
if (-not $cmake) { $missing += "CMake" }
if (-not $ninja) { $missing += "Ninja" }
if (-not $armGcc) { $missing += "GNU Arm Embedded compiler (arm-none-eabi-gcc)" }
if (-not $sdkReady) { $missing += "Raspberry Pi Pico SDK (PICO_SDK_PATH)" }
if ($sdkReady -and -not $tinyUsbReady) {
    $missing += "Pico SDK TinyUSB submodule (lib/tinyusb)"
}
if ($sdkReady -and -not $picotoolSourceReady) {
    $missing += "picotool 2.3.0 source"
}

[pscustomobject]@{
    ready = $missing.Count -eq 0
    cmake = $cmake
    ninja = $ninja
    armGcc = $armGcc
    picoSdkPath = $resolvedSdk
    tinyUsbPath = if ($tinyUsbReady) { $tinyUsbPath } else { $null }
    picotoolSourcePath = if ($picotoolSourceReady) {
        $picotoolSourcePath
    } else {
        $null
    }
    missing = $missing
} | ConvertTo-Json -Depth 3
