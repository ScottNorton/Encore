Get-WmiObject Win32_PnPEntity | Where-Object { $_.DeviceID -like '*USB*' } | Select-Object Name,DeviceID | Format-Table -AutoSize
