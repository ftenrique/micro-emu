[CmdletBinding()]
param(
    [string]$PicoSdkPath = $env:PICO_SDK_PATH,
    [string]$ArmToolchainPath = $env:PICO_TOOLCHAIN_PATH,
    [string]$Board = "waveshare_rp2040_zero"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$firmware = Join-Path $root "firmware\rp2040-zero"
$build = Join-Path $firmware "build"
$picotoolInstall = Join-Path $root ".toolchains\picotool-2.3.0"
$picotoolExecutable = Join-Path $picotoolInstall "picotool\picotool.exe"
$forcePicotoolBuild = if (Test-Path $picotoolExecutable) { "0" } else { "1" }

$toolchainJson = & (Join-Path $PSScriptRoot "check-rp2040-toolchain.ps1") `
    -PicoSdkPath $PicoSdkPath `
    -ArmToolchainPath $ArmToolchainPath
$toolchain = $toolchainJson | ConvertFrom-Json
if (-not $toolchain.ready) {
    throw "RP2040 toolchain is incomplete: $($toolchain.missing -join ', ')"
}

$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$visualStudio = if (Test-Path $vsWhere) {
    & $vsWhere -latest -products * -property installationPath
} else {
    $null
}
$vsDevCmd = if ($visualStudio) {
    Join-Path $visualStudio "Common7\Tools\VsDevCmd.bat"
} else {
    $null
}
if (-not $vsDevCmd -or -not (Test-Path $vsDevCmd)) {
    throw "Visual Studio native compiler environment was not found."
}

# picotool is a Windows host executable. Import the native MSVC environment
# into this process before prepending the bare-metal Arm compiler for the
# firmware target.
$environmentLines = & cmd.exe /d /s /c `
    "`"$vsDevCmd`" -no_logo -arch=x64 >nul && set"
if ($LASTEXITCODE -ne 0) {
    throw "Visual Studio developer environment initialization failed."
}
foreach ($line in $environmentLines) {
    $separator = $line.IndexOf("=")
    if ($separator -gt 0) {
        $name = $line.Substring(0, $separator)
        $value = $line.Substring($separator + 1)
        [Environment]::SetEnvironmentVariable($name, $value, "Process")
    }
}

$ninjaDirectory = Split-Path -Parent $toolchain.ninja
$armGccDirectory = Split-Path -Parent $toolchain.armGcc
$picotoolInstallCmake = $picotoolInstall.Replace("\", "/")
$picotoolSourceCmake = $toolchain.picotoolSourcePath.Replace("\", "/")
$oldPath = $env:Path
$oldGitConfigCount = [Environment]::GetEnvironmentVariable(
    "GIT_CONFIG_COUNT",
    "Process"
)
$oldGitConfigKey = [Environment]::GetEnvironmentVariable(
    "GIT_CONFIG_KEY_0",
    "Process"
)
$oldGitConfigValue = [Environment]::GetEnvironmentVariable(
    "GIT_CONFIG_VALUE_0",
    "Process"
)
try {
    $env:Path = "$armGccDirectory;$ninjaDirectory;$env:Path"
    $env:GIT_CONFIG_COUNT = "1"
    $env:GIT_CONFIG_KEY_0 = "safe.directory"
    $env:GIT_CONFIG_VALUE_0 = "$picotoolSourceCmake/.git"
    $cmakeArguments = @(
        "--fresh",
        "-S", $firmware,
        "-B", $build,
        "-G", "Ninja",
        "-DPICO_SDK_PATH=$($toolchain.picoSdkPath)",
        "-DPICO_TOOLCHAIN_PATH=$(Split-Path -Parent $armGccDirectory)",
        "-DPICOTOOL_FETCH_FROM_GIT_PATH=$picotoolInstallCmake",
        "-DPICOTOOL_FORCE_FETCH_FROM_GIT=$forcePicotoolBuild",
        "-DPICO_BOARD=$Board"
    )
    if ($forcePicotoolBuild -eq "1") {
        $cmakeArguments += @(
            "-DPICOTOOL_GIT_REPOSITORY_URL=$picotoolSourceCmake",
            "-DPICOTOOL_GIT_BRANCH=2.3.0"
        )
    }
    & $toolchain.cmake @cmakeArguments
    if ($LASTEXITCODE -ne 0) {
        throw "RP2040 CMake configure failed with exit code $LASTEXITCODE."
    }

    & $toolchain.cmake --build $build --config Release
    if ($LASTEXITCODE -ne 0) {
        throw "RP2040 build failed with exit code $LASTEXITCODE."
    }
} finally {
    $env:Path = $oldPath
    [Environment]::SetEnvironmentVariable(
        "GIT_CONFIG_COUNT",
        $oldGitConfigCount,
        "Process"
    )
    [Environment]::SetEnvironmentVariable(
        "GIT_CONFIG_KEY_0",
        $oldGitConfigKey,
        "Process"
    )
    [Environment]::SetEnvironmentVariable(
        "GIT_CONFIG_VALUE_0",
        $oldGitConfigValue,
        "Process"
    )
}

$uf2 = Join-Path $build "codex_micro_rp2040_bridge.uf2"
if (-not (Test-Path $uf2)) {
    throw "The build completed but did not produce $uf2."
}
Write-Host "Firmware ready: $uf2"
