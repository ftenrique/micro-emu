[CmdletBinding(SupportsShouldProcess, ConfirmImpact = "Low")]
param(
    [string]$Uf2Path
)

$ErrorActionPreference = "Stop"

# La ruta predeterminada se calcula respecto al propio script,
# no respecto al directorio desde el que se ejecuta PowerShell.
$repositoryRoot = Split-Path -Parent $PSScriptRoot

if ([string]::IsNullOrWhiteSpace($Uf2Path)) {
    $Uf2Path = Join-Path `
        $repositoryRoot `
        "firmware\rp2040-zero\build\codex_micro_rp2040_bridge.uf2"
}
elseif (-not [System.IO.Path]::IsPathRooted($Uf2Path)) {
    # Las rutas proporcionadas explícitamente se interpretan
    # respecto al directorio de trabajo actual.
    $Uf2Path = Join-Path (Get-Location).Path $Uf2Path
}

$resolvedUf2 = (Resolve-Path -LiteralPath $Uf2Path).Path
$uf2File = Get-Item -LiteralPath $resolvedUf2

if ($uf2File.PSIsContainer) {
    throw "The selected UF2 path points to a directory: $resolvedUf2"
}

$uf2Bytes = [System.IO.File]::ReadAllBytes($resolvedUf2)

# Cada bloque UF2 ocupa exactamente 512 bytes.
$uf2BlockSize = 512

if ($uf2Bytes.Length -lt $uf2BlockSize) {
    throw "The selected file is too short to contain a UF2 block."
}

if (($uf2Bytes.Length % $uf2BlockSize) -ne 0) {
    throw (
        "The selected file size is not a multiple of 512 bytes. " +
        "Size: $($uf2Bytes.Length) bytes."
    )
}

# Se construyen los valores como UInt32 desde texto hexadecimal.
# Esto evita la interpretación signed Int32 de PowerShell 5.1.
$expectedMagic0 = [Convert]::ToUInt32("0A324655", 16)
$expectedMagic1 = [Convert]::ToUInt32("9E5D5157", 16)
$expectedMagicEnd = [Convert]::ToUInt32("0AB16F30", 16)

$blockCount = [int]($uf2Bytes.Length / $uf2BlockSize)

# Validamos todos los bloques, no solamente el encabezado del primero.
for ($blockIndex = 0; $blockIndex -lt $blockCount; $blockIndex++) {
    $blockOffset = $blockIndex * $uf2BlockSize

    $magic0 = [BitConverter]::ToUInt32($uf2Bytes, $blockOffset)
    $magic1 = [BitConverter]::ToUInt32($uf2Bytes, $blockOffset + 4)
    $magicEnd = [BitConverter]::ToUInt32($uf2Bytes, $blockOffset + 508)

    if (
        $magic0 -ne $expectedMagic0 -or
        $magic1 -ne $expectedMagic1 -or
        $magicEnd -ne $expectedMagicEnd
    ) {
        $message = (
            "Invalid UF2 block {0}. " +
            "Expected magic values 0x{1:X8}, 0x{2:X8}, 0x{3:X8}; " +
            "found 0x{4:X8}, 0x{5:X8}, 0x{6:X8}."
        ) -f (
            $blockIndex,
            $expectedMagic0,
            $expectedMagic1,
            $expectedMagicEnd,
            $magic0,
            $magic1,
            $magicEnd
        )

        throw $message
    }
}

Write-Verbose (
    "Valid UF2 image: {0} bytes, {1} blocks, path: {2}" -f
    $uf2Bytes.Length,
    $blockCount,
    $resolvedUf2
)

$bootVolumes = @(
    Get-Volume -FileSystemLabel "RPI-RP2" -ErrorAction SilentlyContinue |
        Where-Object { $null -ne $_.DriveLetter }
)

if ($bootVolumes.Count -ne 1) {
    throw (
        "Expected exactly one RPI-RP2 boot volume; found {0}. " +
        "Connect the RP2040 while holding BOOTSEL." -f
        $bootVolumes.Count
    )
}

$driveLetter = $bootVolumes[0].DriveLetter
$destination = "{0}:\{1}" -f (
    $driveLetter,
    (Split-Path -Leaf $resolvedUf2)
)

if ($PSCmdlet.ShouldProcess($destination, "Flash RP2040 UF2")) {
    Write-Host "Copying UF2:"
    Write-Host "  Source:      $resolvedUf2"
    Write-Host "  Destination: $destination"
    Write-Host "  Size:        $($uf2Bytes.Length) bytes"
    Write-Host "  Blocks:      $blockCount"

    Copy-Item `
        -LiteralPath $resolvedUf2 `
        -Destination $destination `
        -Force

    Write-Host "UF2 copied successfully."
    Write-Host "The RP2040 should reboot and detach RPI-RP2 automatically."
}