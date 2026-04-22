#!/bin/bash
# Wait for device to come back after kexec/reboot
# Usage: wait_reboot.sh [IP] [MAX_SECONDS]
. "$(dirname "$0")/../common.sh"
IP="${1:-$ENCORE_DEVICE_IP}"
MAX="${2:-120}"
echo "Waiting for $IP (max ${MAX}s)..."
elapsed=0
while [ "$elapsed" -lt "$MAX" ]; do
    sleep 3
    elapsed=$((elapsed + 3))
    if ping -c 1 -W 1 "$IP" 2>/dev/null | grep -q "64 bytes"; then
        echo "BACK after ${elapsed}s"
        exit 0
    fi
    echo "  ${elapsed}s..."
done
echo "TIMEOUT after ${MAX}s"
exit 1
