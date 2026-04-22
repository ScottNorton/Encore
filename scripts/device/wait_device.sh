#!/bin/bash
. "$(dirname "$0")/../common.sh"
IP="${1:-$ENCORE_DEVICE_IP}"
for i in $(seq 1 12); do
    if ping -c 1 -W 2 "$IP" > /dev/null 2>&1; then
        echo "Device is up after $((i*5)) seconds"
        exit 0
    fi
    echo "Attempt $i: no response, waiting..."
    sleep 5
done
echo "Device $IP still unreachable after 60s"
exit 1
