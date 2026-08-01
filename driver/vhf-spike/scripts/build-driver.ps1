[CmdletBinding()]
param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Release"
)

$ErrorActionPreference = "Stop"
$requiredKitVersion = "10.0.26100.0"

$projectRoot = Split-Path -Parent $PSScriptRoot
$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vsWhere)) {
    throw "vswhere.exe was not found."
}
$visualStudio = & $vsWhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath
if (-not $visualStudio) {
    throw "Visual Studio with the x64 C++ toolchain was not found."
}
$msBuild = Join-Path $visualStudio "MSBuild\Current\Bin\amd64\MSBuild.exe"
$vhfLibrary = Get-ChildItem (
    "${env:ProgramFiles(x86)}\Windows Kits\10\Lib\$requiredKitVersion"
    ) `
    -Recurse -Filter "VhfKm.lib" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\x64\\" } |
    Select-Object -First 1
if (-not $vhfLibrary) {
    throw (
        "VhfKm.lib for $requiredKitVersion is missing. Install the matching " +
        "WDK kernel driver components first."
    )
}
$driverToolset = Get-ChildItem (Join-Path $visualStudio "MSBuild") -Recurse `
    -Directory -Filter "WindowsKernelModeDriver10.0" `
    -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $driverToolset) {
    throw (
        "The WindowsKernelModeDriver10.0 MSBuild toolset is missing. " +
        "Install/integrate the WDK with this Visual Studio instance."
    )
}

$effectivePath = [Environment]::GetEnvironmentVariable("Path", "Process")
[Environment]::SetEnvironmentVariable("PATH", $null, "Process")
[Environment]::SetEnvironmentVariable("Path", $null, "Process")
[Environment]::SetEnvironmentVariable("Path", $effectivePath, "Process")

& $msBuild (Join-Path $projectRoot "CodexMicroVhf.sln") `
    /m `
    /t:Build `
    /p:Configuration=$Configuration `
    /p:Platform=x64 `
    /p:SpectreMitigation=false `
    /p:SignMode=Off
if ($LASTEXITCODE -ne 0) {
    throw "Driver build failed with exit code $LASTEXITCODE."
}
