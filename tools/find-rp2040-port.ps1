[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$matches = @(
    Get-PnpDevice -PresentOnly -Class Ports -ErrorAction SilentlyContinue |
        Where-Object InstanceId -Match "VID_303A&PID_8360" |
        ForEach-Object {
            $port = if ($_.FriendlyName -match "\((COM\d+)\)") {
                $Matches[1]
            } else {
                $null
            }
            [pscustomobject]@{
                port = $port
                friendlyName = $_.FriendlyName
                instanceId = $_.InstanceId
                status = $_.Status
            }
        }
)

[pscustomobject]@{
    found = $matches.Count
    ports = $matches
} | ConvertTo-Json -Depth 4
