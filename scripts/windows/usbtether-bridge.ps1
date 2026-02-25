Write-Host "=== Invoke USB Tether Bridge ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "This script prevents USB tethering from taking over your PC's" -ForegroundColor White
Write-Host "existing network connections while bridging to the Invoke's AP." -ForegroundColor White
Write-Host ""
Write-Host "Prerequisites:" -ForegroundColor Yellow
Write-Host "  1. Connect your phone to the Invoke's WiFi AP"
Write-Host "     - Android will show a 'Sign in to <AP name>' popup" -ForegroundColor DarkYellow
Write-Host "     - Tap the menu (top-right) and select 'Use this network as-is'" -ForegroundColor DarkYellow
Write-Host "  2. Connect your phone to your PC via USB"
Write-Host "  3. Enable USB tethering on your phone"
Write-Host ""

# Detect USB tether adapter
$usbAdapter = Get-NetAdapter | Where-Object { $_.InterfaceDescription -like "*Remote NDIS*" }
if (-not $usbAdapter) {
    Write-Host "[FAIL] USB tether not detected. Make sure your phone is connected and USB tethering is enabled." -ForegroundColor Red
    Read-Host "Press Enter to exit"
    exit 1
}
Write-Host "[OK] USB tether detected: $($usbAdapter.InterfaceDescription) (Index: $($usbAdapter.ifIndex))" -ForegroundColor Green

# Get USB tether config
$usbConfig = Get-NetIPConfiguration -InterfaceIndex $usbAdapter.ifIndex
$usbGateway = $usbConfig.IPv4DefaultGateway.NextHop
if (-not $usbGateway) {
    Write-Host "[FAIL] USB tether has no gateway. Try toggling USB tethering on your phone." -ForegroundColor Red
    Read-Host "Press Enter to exit"
    exit 1
}
Write-Host "[OK] USB tether gateway: $usbGateway" -ForegroundColor Green

# Save original metric and sanity check
$originalMetric = (Get-NetIPInterface -InterfaceIndex $usbAdapter.ifIndex -AddressFamily IPv4).InterfaceMetric
if ($originalMetric -ge 9999) {
    Write-Host "[WARN] USB metric is already $originalMetric (possibly from a previous run). Will restore to default." -ForegroundColor DarkYellow
    $originalMetric = 25
} else {
    Write-Host "[OK] Saved original USB metric: $originalMetric" -ForegroundColor Green
}

# Apply routing changes
Write-Host ""
Write-Host "Applying routing changes..." -ForegroundColor Yellow
Set-NetIPInterface -InterfaceIndex $usbAdapter.ifIndex -InterfaceMetric 9999
route delete 192.168.43.1 >$null 2>&1
route add 192.168.43.1 mask 255.255.255.255 $usbGateway | Out-Null

# Verify Invoke is reachable
$connected = $false
$firstAttempt = $true
while (-not $connected) {
    Write-Host ""
    Write-Host "Testing connection to Invoke at 192.168.43.1..." -ForegroundColor Yellow
    $ping = Test-Connection -ComputerName 192.168.43.1 -Count 3 -Quiet
    if ($ping) {
        Write-Host "[OK] Invoke is reachable at 192.168.43.1" -ForegroundColor Green
        $connected = $true
    } else {
        Write-Host "[FAIL] Invoke not reachable at 192.168.43.1" -ForegroundColor Red
        if ($firstAttempt) {
            Write-Host ""
            Write-Host "Troubleshooting:" -ForegroundColor Yellow
            Write-Host "  - Is your phone connected to the Invoke's WiFi AP?"
            Write-Host "  - Is the Invoke powered on and broadcasting its AP?"
            Write-Host "  - Try disconnecting and reconnecting your phone's WiFi to the Invoke"
            $firstAttempt = $false
        }
        Write-Host ""
        $retry = Read-Host "Retry? (y/n)"
        if ($retry -notmatch '^(y|yes)$') {
            Write-Host ""
            Write-Host "Restoring routing..." -ForegroundColor Yellow
            route delete 192.168.43.1 >$null 2>&1
            Set-NetIPInterface -InterfaceIndex $usbAdapter.ifIndex -InterfaceMetric $originalMetric
            Write-Host "[OK] Routing restored." -ForegroundColor Green
            Read-Host "Press Enter to exit"
            exit 1
        }
    }
}

Write-Host ""
Write-Host "=== Bridge Active ===" -ForegroundColor Cyan
Write-Host "  Invoke GUI: http://192.168.43.1" -ForegroundColor White
Write-Host "  Your existing network connections are unaffected" -ForegroundColor White
Write-Host ""
$restore = ""
while ($restore -notmatch '^(y|yes)$') {
    $restore = Read-Host "Restore normal routing? (y/n)"
}

# Teardown
Write-Host ""
Write-Host "Restoring routing..." -ForegroundColor Yellow
route delete 192.168.43.1 | Out-Null
Set-NetIPInterface -InterfaceIndex $usbAdapter.ifIndex -InterfaceMetric $originalMetric

Write-Host "[OK] Routing restored. USB tether back to normal priority (metric: $originalMetric)." -ForegroundColor Green
