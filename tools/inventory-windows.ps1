[CmdletBinding()]
param(
    [string]$OutputPath,
    [switch]$IncludeAllHid
)

$ErrorActionPreference = "Stop"
$knownAjazzPattern = "VID_0300&PID_(?:1001|1002|1003|3002|3003)"
$unverifiedCandidatePattern = "VID_04B4&PID_1007"

function Get-RegistryOperatingSystem {
    $current = Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion"
    [ordered]@{
        productName = $current.ProductName
        displayVersion = $current.DisplayVersion
        build = "$($current.CurrentBuild).$($current.UBR)"
        architecture = $env:PROCESSOR_ARCHITECTURE
    }
}

function Get-ChatGptPackages {
    $packages = @()
    try {
        $packages = @(
            Get-AppxPackage -ErrorAction Stop |
                Where-Object {
                    $_.Name -match "ChatGPT|OpenAI|Codex" -or
                    $_.PackageFullName -match "ChatGPT|OpenAI|Codex"
                } |
                ForEach-Object {
                    [ordered]@{
                        name = $_.Name
                        version = "$($_.Version)"
                        architecture = "$($_.Architecture)"
                        packageFullName = $_.PackageFullName
                    }
                }
        )
    } catch {
        $packages = @()
    }

    if ($packages.Count -eq 0) {
        $packageRoot = Join-Path $env:LOCALAPPDATA "Packages"
        if (Test-Path $packageRoot) {
            $packages = @(
                Get-ChildItem $packageRoot -Directory -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name -match "ChatGPT|OpenAI|Codex" } |
                    ForEach-Object {
                        [ordered]@{
                            name = $_.Name
                            version = $null
                            architecture = $null
                            packageFullName = $null
                        }
                    }
            )
        }
    }

    $packages
}

function Convert-PnpDevice {
    param([Parameter(Mandatory)]$Device)

    $properties = @()
    try {
        $properties = @(
            Get-PnpDeviceProperty -InstanceId $Device.InstanceId -ErrorAction Stop |
                Where-Object {
                    $_.KeyName -in @(
                        "DEVPKEY_Device_BusReportedDeviceDesc",
                        "DEVPKEY_Device_Manufacturer",
                        "DEVPKEY_Device_HardwareIds",
                        "DEVPKEY_Device_CompatibleIds",
                        "DEVPKEY_Device_ContainerId",
                        "DEVPKEY_Device_Service"
                    )
                } |
                ForEach-Object {
                    [ordered]@{
                        key = $_.KeyName
                        value = $_.Data
                    }
                }
        )
    } catch {
        $properties = @()
    }

    [ordered]@{
        status = "$($Device.Status)"
        class = "$($Device.Class)"
        friendlyName = "$($Device.FriendlyName)"
        instanceId = "$($Device.InstanceId)"
        properties = $properties
    }
}

function Get-PnpInventory {
    $devices = @()
    try {
        $devices = @(Get-PnpDevice -PresentOnly -ErrorAction Stop)
    } catch {
        return Get-PnpUtilInventory -OriginalError $_.Exception.Message
    }

    $ajazz = @(
        $devices |
            Where-Object {
                $_.FriendlyName -match "AJAZZ|AKP03" -or
                $_.InstanceId -match "AJAZZ|AKP03" -or
                $_.InstanceId -match $knownAjazzPattern
            } |
            ForEach-Object { Convert-PnpDevice $_ }
    )
    $codex = @(
        $devices |
            Where-Object { $_.InstanceId -match "VID_303A&PID_8360" } |
            ForEach-Object { Convert-PnpDevice $_ }
    )
    $unverified = @(
        $devices |
            Where-Object { $_.InstanceId -match $unverifiedCandidatePattern } |
            ForEach-Object { Convert-PnpDevice $_ }
    )
    $allHid = @()
    if ($IncludeAllHid) {
        $allHid = @(
            $devices |
                Where-Object { $_.Class -eq "HIDClass" } |
                ForEach-Object { Convert-PnpDevice $_ }
        )
    }

    [ordered]@{
        available = $true
        source = "Get-PnpDevice"
        error = $null
        ajazzCandidates = $ajazz
        unverifiedCandidates = $unverified
        codexMicroReference = $codex
        allHid = $allHid
    }
}

function Get-PnpUtilInventory {
    param([string]$OriginalError)

    $pnpUtil = Get-Command "pnputil.exe" -ErrorAction SilentlyContinue
    if (-not $pnpUtil) {
        return [ordered]@{
            available = $false
            source = $null
            error = $OriginalError
            ajazzCandidates = @()
            unverifiedCandidates = @()
            codexMicroReference = @()
            allHid = @()
        }
    }

    try {
        $text = (& $pnpUtil.Source /enum-devices /connected /class HIDClass 2>&1) -join "`n"
        $matches = [regex]::Matches(
            $text,
            "(?im)^(?:Instance ID|Id\.\s+de\s+instancia)\s*:\s*(?<id>[^\r\n]+)"
        )
        $ids = @(
            $matches |
                ForEach-Object { $_.Groups["id"].Value.Trim() } |
                Sort-Object -Unique
        )
        $ajazz = @(
            $ids |
                Where-Object {
                    $_ -match $knownAjazzPattern -or $_ -match "AJAZZ|AKP03"
                } |
                ForEach-Object {
                    [ordered]@{
                        status = "present"
                        class = "HIDClass"
                        friendlyName = $null
                        instanceId = $_
                        properties = @()
                    }
                }
        )
        $codex = @(
            $ids |
                Where-Object { $_ -match "VID_303A&PID_8360" } |
                ForEach-Object {
                    [ordered]@{
                        status = "present"
                        class = "HIDClass"
                        friendlyName = $null
                        instanceId = $_
                        properties = @()
                    }
                }
        )
        $unverified = @(
            $ids |
                Where-Object { $_ -match $unverifiedCandidatePattern } |
                ForEach-Object {
                    [ordered]@{
                        status = "present"
                        class = "HIDClass"
                        friendlyName = $null
                        instanceId = $_
                        properties = @()
                    }
                }
        )
        $allHid = @()
        if ($IncludeAllHid) {
            $allHid = @(
                $ids |
                    ForEach-Object {
                        [ordered]@{
                            status = "present"
                            class = "HIDClass"
                            friendlyName = $null
                            instanceId = $_
                            properties = @()
                        }
                    }
            )
        }
        return [ordered]@{
            available = $true
            source = "pnputil"
            error = $OriginalError
            ajazzCandidates = $ajazz
            unverifiedCandidates = $unverified
            codexMicroReference = $codex
            allHid = $allHid
        }
    } catch {
        return [ordered]@{
            available = $false
            source = "pnputil"
            error = "$OriginalError; pnputil: $($_.Exception.Message)"
            ajazzCandidates = @()
            unverifiedCandidates = @()
            codexMicroReference = @()
            allHid = @()
        }
    }
}

$inventory = [ordered]@{
    schemaVersion = 1
    capturedAtUtc = [DateTime]::UtcNow.ToString("o")
    readOnlyProbe = $true
    operatingSystem = Get-RegistryOperatingSystem
    chatGptPackages = @(Get-ChatGptPackages)
    pnp = Get-PnpInventory
}

$json = $inventory | ConvertTo-Json -Depth 12

if ($OutputPath) {
    $resolvedParent = Split-Path -Parent $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath)
    if ($resolvedParent -and -not (Test-Path $resolvedParent)) {
        New-Item -ItemType Directory -Path $resolvedParent -Force | Out-Null
    }
    Set-Content -LiteralPath $OutputPath -Value $json -Encoding UTF8
}

$json
