#!/bin/bash
# Poll for device to come back up after reboot
. "$(dirname "$0")/../common.sh"
HOST="${1:-$ENCORE_DEVICE_IP}"
SSH_CMD="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS -o ConnectTimeout=3 root@$HOST"
for i in $(seq 1 40); do
    sleep 3
    if $SSH_CMD echo UP 2>/dev/null; then
        echo "Device back after $((i*3))s"
        exit 0
    fi
    echo "try $i..."
done
echo "Timeout waiting for device"
exit 1
