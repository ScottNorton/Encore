$port = New-Object System.IO.Ports.SerialPort COM3,115200,None,8,One
$port.NewLine = "`n"
$port.ReadTimeout = 5000
$port.WriteTimeout = 1000
$port.Open()
Start-Sleep -Milliseconds 200

# Wake console
$port.WriteLine("")
$port.WriteLine("")
Start-Sleep -Milliseconds 1000
$buf = $port.ReadExisting()
Write-Host "--- INITIAL ---"
Write-Host $buf

# Send command
$port.WriteLine("echo HELLO_FROM_SERIAL")
Start-Sleep -Milliseconds 2000
$buf = $port.ReadExisting()
Write-Host "--- RESPONSE ---"
Write-Host $buf

$port.Close()
