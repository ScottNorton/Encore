#!/bin/bash
# Emergency diagnostic — run on device immediately after connecting
# Usage: cat this | ssh root@DEVICE_IP 'bash -s'
set -x

echo "=== PROCESSES ==="
ps | grep -E 'encore|hostapd|dnsmasq|python|librespot'

echo "=== INTERFACES ==="
ifconfig p2p0 2>&1 | head -3
ifconfig wlan0 2>&1 | head -3

echo "=== CONFIG ==="
cat /lsync/encore/config.toml 2>/dev/null | grep -E 'ap_|wifi_ssid'

echo "=== ENCORE LOG (last 30 network lines) ==="
grep -iE 'network|ap |stop_ap|keep_alive|mdns' /lsync/encore/encore.log 2>/dev/null | tail -30

echo "=== BOOT LOG ==="
cat /tmp/start_ap.log 2>/dev/null

echo "=== DISABLE ENCORE (prevent restart loop) ==="
killall encore 2>/dev/null
touch /lsync/encore/disabled
echo "Encore disabled. AP should recover on next manual start_ap.sh"

echo "=== RESTART AP ==="
killall hostapd 2>/dev/null
if [ -f /tmp/dnsmasq-ap.pid ]; then kill $(cat /tmp/dnsmasq-ap.pid) 2>/dev/null; fi
sleep 1
/sbin/start_ap.sh
sleep 3
pgrep -a hostapd
echo "=== DONE ==="
