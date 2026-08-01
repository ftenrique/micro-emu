[CmdletBinding()]
param(
    [switch]$Json,
    [switch]$NoFail
)

$ErrorActionPreference = "Stop"
$requiredKitVersion = "10.0.26100.0"

$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$installationPath = if (Test-Path $vsWhere) {
    & $vsWhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
} else {
    $null
}
$msBuild = if ($installationPath) {
    Join-Path $installationPath "MSBuild\Current\Bin\amd64\MSBuild.exe"
} else {
    $null
}

$kitsRoot = "${env:ProgramFiles(x86)}\Windows Kits\10"
$vhfHeader = Get-ChildItem (Join-Path $kitsRoot "Include\$requiredKitVersion") `
    -Recurse -Filter "vhf.h" -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
$vhfLibrary = Get-ChildItem (Join-Path $kitsRoot "Lib\$requiredKitVersion") `
    -Recurse -Filter "VhfKm.lib" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\x64\\" } |
    Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName
$inf2Cat = Get-ChildItem (Join-Path $kitsRoot "bin\$requiredKitVersion") -Recurse `
    -Filter "Inf2Cat.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\(?:x64|x86)\\" } |
    Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName
$signTool = Get-ChildItem (Join-Path $kitsRoot "bin\$requiredKitVersion") -Recurse `
    -Filter "signtool.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\x64\\" } |
    Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName
$devCon = Get-ChildItem (Join-Path $kitsRoot "Tools") -Recurse `
    -Filter "devcon.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\x64\\" } |
    Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName
$driverToolset = if ($installationPath) {
    @(
        Get-ChildItem (Join-Path $installationPath "MSBuild") -Recurse `
            -Directory -Filter "WindowsKernelModeDriver10.0" `
            -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match "\\Platforms\\x64\\" } |
            Select-Object -First 1 -ExpandProperty FullName
    )
} else {
    @()
}

$result = [ordered]@{
    requiredKitVersion = $requiredKitVersion
    readyToBuild = [bool](
        $msBuild -and (Test-Path $msBuild) -and
        $vhfHeader -and
        $vhfLibrary -and
        $driverToolset.Count -gt 0
    )
    visualStudio = if ($installationPath) { "$installationPath" } else { $null }
    msBuild = if ($msBuild) { "$msBuild" } else { $null }
    vhfHeader = if ($vhfHeader) { "$vhfHeader" } else { $null }
    vhfLibrary = if ($vhfLibrary) { "$vhfLibrary" } else { $null }
    driverPlatformToolset = if ($driverToolset.Count -gt 0) {
        "$($driverToolset | Select-Object -First 1)"
    } else {
        $null
    }
    inf2Cat = if ($inf2Cat) { "$inf2Cat" } else { $null }
    signTool = if ($signTool) { "$signTool" } else { $null }
    devCon = if ($devCon) { "$devCon" } else { $null }
}

if ($Json) {
    $result | ConvertTo-Json -Depth 4
} else {
    [pscustomobject]$result | Format-List
}

if (-not $result.readyToBuild -and -not $NoFail) {
    Write-Error (
        "WDK kernel components are incomplete. Install the Windows Driver Kit " +
        "$requiredKitVersion integrated with Visual Studio, including VhfKm.lib and the " +
        "WindowsKernelModeDriver10.0 platform toolset."
    )
}
