$rpInstanceId = 'HID\VID_303A&PID_8360&MI_00&COL04\7&8C2DEB2&0&0003'
$props = Get-PnpDeviceProperty -InstanceId $rpInstanceId
foreach ($p in $props) {
    $val = if ($p.Data -is [array]) {
        ($p.Data | ForEach-Object { $_.ToString() }) -join '; '
    } else {
        if ($null -ne $p.Data) { $p.Data.ToString() } else { '' }
    }
    if ($p.KeyName -match 'HardwareIds|Address|Status|Driver|DeviceDesc|Parent') {
        Write-Output ($p.KeyName + ' = ' + $val)
    }
}
