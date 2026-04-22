#!/bin/bash
# emergency_disable_encore.sh — Poll SSH rapidly to disable Encore during crash loop
#
# When Encore is crash-looping and the watchdog reboots the device each cycle,
# there's a brief window (~10-20s) where WiFi + SSH are up but the watchdog
# hasn't fired yet. This script polls every 0.5s to catch that window.
#
# Usage (from WSL):
#   bash scripts/device/emergency_disable_encore.sh [ip]

. "$(dirname "$0")/../common.sh"
IP="${1:-$ENCORE_DEVICE_IP}"
SSH_OPTS="$ENCORE_SSH_OPTS -o ConnectTimeout=1 -o ServerAliveInterval=1"
CMD='touch /lsync/encore/disabled && echo 0 > /lsync/encore/boot_crashes && echo DISABLED'

echo "=== Emergency Encore Disable ==="
echo "Target: $IP"
echo "Polling every 0.5s — waiting for SSH window..."
echo ""

ATTEMPT=0
while true; do
    ATTEMPT=$((ATTEMPT + 1))
    RESULT=$(sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@"$IP" "$CMD" 2>/dev/null)
    if [ "$RESULT" = "DISABLED" ]; then
        echo ""
        echo "=== SUCCESS on attempt $ATTEMPT ==="
        echo "Encore disabled + crash counter cleared."
        echo "Device will boot into safe mode on next reboot."
        echo ""
        echo "Verifying..."
        sleep 1
        sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@"$IP" \
            "ls -la /lsync/encore/disabled; cat /lsync/encore/boot_crashes; ps | grep -E 'encore|python|librespot' | grep -v grep" 2>/dev/null
        exit 0
    fi
    # Print dot every 10 attempts (5 seconds) to show progress
    if [ $((ATTEMPT % 10)) -eq 0 ]; then
        printf "." >&2
    fi
    sleep 0.5
done
