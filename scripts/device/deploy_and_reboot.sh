#!/bin/bash
set -euo pipefail
. "$(dirname "$0")/../common.sh"
DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS -o ConnectTimeout=10 root@$DEVICE_IP"

echo "=== Uploading dsp_init_stock.py ==="
$SSH 'mkdir -p /lsync/scripts'
sed 's/\r$//' /mnt/g/HKInvoke/scripts/device/dsp_init_stock.py | $SSH 'cat > /lsync/scripts/dsp_init_stock.py'
$SSH 'chmod +x /lsync/scripts/dsp_init_stock.py && ls -la /lsync/scripts/dsp_init_stock.py'

echo "=== Disabling Encore ==="
$SSH 'touch /lsync/encore/disabled && sync'

echo "=== Rebooting ==="
$SSH 'reboot' || true
echo "Device rebooting... wait ~30s for it to come back."
