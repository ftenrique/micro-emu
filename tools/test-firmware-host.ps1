[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
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

$environmentLines = & cmd.exe /d /s /c `
    "`"$vsDevCmd`" -no_logo -arch=x64 >nul && set"
if ($LASTEXITCODE -ne 0) {
    throw "Visual Studio developer environment initialization failed."
}
foreach ($line in $environmentLines) {
    $separator = $line.IndexOf("=")
    if ($separator -gt 0) {
        [Environment]::SetEnvironmentVariable(
            $line.Substring(0, $separator),
            $line.Substring($separator + 1),
            "Process"
        )
    }
}

$outputDirectory = Join-Path $repoRoot "artifacts\firmware-host-test"
if (-not (Test-Path $outputDirectory)) {
    New-Item -ItemType Directory -Path $outputDirectory | Out-Null
}
$executable = Join-Path $outputDirectory "bridge_protocol_host_test.exe"
$testSource = Join-Path $repoRoot "tests\firmware\bridge_protocol_host_test.c"
$protocolSource = Join-Path $repoRoot "firmware\rp2040-zero\src\bridge_protocol.c"
$includeDirectory = Join-Path $repoRoot "firmware\rp2040-zero\src"

& cl.exe /nologo /TC /std:c11 /W4 /WX `
    "/I$includeDirectory" `
    $testSource `
    $protocolSource `
    "/Fe:$executable" `
    "/Fo:$outputDirectory\"
if ($LASTEXITCODE -ne 0) {
    throw "Firmware host test compilation failed with exit code $LASTEXITCODE."
}

& $executable
if ($LASTEXITCODE -ne 0) {
    throw "Firmware host tests failed with exit code $LASTEXITCODE."
}
