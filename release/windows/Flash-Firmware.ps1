[CmdletBinding(SupportsShouldProcess, ConfirmImpact = "Low")]
param()

$ErrorActionPreference = "Stop"

# The firmware ships next to this script in the release bundle, so the path is
# resolved relative to the script location, not the working directory. This keeps
# the release self-contained: no source checkout or build toolchain is required.
$bundleRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$resolvedUf2 = Join-Path $bundleRoot "codex_micro_rp2040_bridge.uf2"

if (-not (Test-Path -LiteralPath $resolvedUf2)) {
    throw (
        "The firmware image was not found in the release bundle: $resolvedUf2. " +
        "Re-extract the complete ZIP (including codex_micro_rp2040_bridge.uf2) and try again."
    )
}

$resolvedUf2 = (Resolve-Path -LiteralPath $resolvedUf2).Path
$uf2File = Get-Item -LiteralPath $resolvedUf2

if ($uf2File.PSIsContainer) {
    throw "The firmware path points to a directory: $resolvedUf2"
}

$uf2Bytes = [System.IO.File]::ReadAllBytes($resolvedUf2)

# Each UF2 block is exactly 512 bytes.
$uf2BlockSize = 512

if ($uf2Bytes.Length -lt $uf2BlockSize) {
    throw "The firmware file is too short to contain a UF2 block."
}

if (($uf2Bytes.Length % $uf2BlockSize) -ne 0) {
    throw (
        "The firmware file size is not a multiple of 512 bytes. " +
        "Size: $($uf2Bytes.Length) bytes."
    )
}

# Build the magic values as UInt32 from hex text. This avoids the signed Int32
# interpretation that PowerShell 5.1 would otherwise apply to integer literals.
$expectedMagic0 = [Convert]::ToUInt32("0A324655", 16)
$expectedMagic1 = [Convert]::ToUInt32("9E5D5157", 16)
$expectedMagicEnd = [Convert]::ToUInt32("0AB16F30", 16)

$blockCount = [int]($uf2Bytes.Length / $uf2BlockSize)

# Validate every block, not only the first header, so a truncated or corrupt
# image is rejected before anything is written to the board.
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
            "found 0x{4:X8}, 0x{5:X8}, 0x{6:X8}. " +
            "The firmware image appears to be corrupt; re-download the release."
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
    # Build the message first, then apply -f. Doing it inline as
    # "a{0}" + "b" -f $count would bind -f only to the last fragment.
    $volumeMessage = (
        "Expected exactly one RPI-RP2 boot volume; found {0}. " +
        "Disconnect the board, then reconnect it while holding BOOTSEL " +
        "until the RPI-RP2 drive appears."
    )
    throw ($volumeMessage -f $bootVolumes.Count)
}

$driveLetter = $bootVolumes[0].DriveLetter
$destination = "{0}:\{1}" -f (
    $driveLetter,
    (Split-Path -Leaf $resolvedUf2)
)

if ($PSCmdlet.ShouldProcess($destination, "Flash RP2040 UF2")) {
    Write-Host "Copying firmware to the RP2040:"
    Write-Host "  Source:      $resolvedUf2"
    Write-Host "  Destination: $destination"
    Write-Host "  Size:        $($uf2Bytes.Length) bytes"
    Write-Host "  Blocks:      $blockCount"

    Copy-Item `
        -LiteralPath $resolvedUf2 `
        -Destination $destination `
        -Force

    Write-Host ""
    Write-Host "Firmware copied successfully."
    Write-Host "The RP2040 reboots and the RPI-RP2 drive disappears automatically."
    Write-Host "Restart the bridge (or relaunch Codex Micro) for it to detect the board."
}
