# Compara las propiedades PnP de las interfaces HID del AKP03E y del RP2040
# para localizar diferencias que expliquen por qué ChatGPT reconoce el
# AKP03E pero no el RP2040.

$ErrorActionPreference = 'Stop'

$akpInstanceId = 'HID\VID_0300&PID_3002&MI_00\9&32ABE067&0&0000'
$rpInstanceId = 'HID\VID_303A&PID_8360&MI_00&COL04\7&8C2DEB2&0&0003'

Write-Output "AKP03E: $akpInstanceId"
Write-Output "RP2040: $rpInstanceId"
Write-Output ""

$akpProps = Get-PnpDeviceProperty -InstanceId $akpInstanceId
$rpProps = Get-PnpDeviceProperty -InstanceId $rpInstanceId

Write-Output ("AKP03E property count: " + $akpProps.Count)
Write-Output ("RP2040 property count: " + $rpProps.Count)

$akpDict = @{}
foreach ($p in $akpProps) {
    $val = if ($p.Data -is [array]) {
        ($p.Data | ForEach-Object { $_.ToString() }) -join '; '
    } else {
        if ($null -ne $p.Data) { $p.Data.ToString() } else { '' }
    }
    $akpDict[$p.KeyName] = $val
}

$rpDict = @{}
foreach ($p in $rpProps) {
    $val = if ($p.Data -is [array]) {
        ($p.Data | ForEach-Object { $_.ToString() }) -join '; '
    } else {
        if ($null -ne $p.Data) { $p.Data.ToString() } else { '' }
    }
    $rpDict[$p.KeyName] = $val
}

$allKeys = ($akpDict.Keys + $rpDict.Keys) | Sort-Object -Unique

Write-Output ""
Write-Output "=== COMPARISON (only differing keys) ==="
Write-Output ""

$rows = foreach ($k in $allKeys) {
    $a = if ($akpDict.ContainsKey($k)) { $akpDict[$k] } else { '(missing)' }
    $r = if ($rpDict.ContainsKey($k)) { $rpDict[$k] } else { '(missing)' }
    if ($a -ne $r) {
        [PSCustomObject]@{
            Key    = $k
            AKP03E = $a
            RP2040 = $r
        }
    }
}

$rows | Format-Table -AutoSize -Wrap | Out-String -Width 200

# Exportar a CSV
$csvPath = "D:\Programming\micro-emu\artifacts\hid-comparison.csv"
$rows | Export-Csv -Path $csvPath -NoTypeInformation -Encoding UTF8
Write-Output ""
Write-Output "Differences exported to: $csvPath"
Write-Output ("Total differing keys: " + $rows.Count)

# Exportar todas las propiedades también
$allRows = foreach ($k in $allKeys) {
    $a = if ($akpDict.ContainsKey($k)) { $akpDict[$k] } else { '(missing)' }
    $r = if ($rpDict.ContainsKey($k)) { $rpDict[$k] } else { '(missing)' }
    [PSCustomObject]@{
        Key    = $k
        AKP03E = $a
        RP2040 = $r
        Same   = if ($a -eq $r) { 'YES' } else { 'NO' }
    }
}
$allCsv = "D:\Programming\micro-emu\artifacts\hid-comparison-all.csv"
$allRows | Export-Csv -Path $allCsv -NoTypeInformation -Encoding UTF8
Write-Output "All properties exported to: $allCsv"
