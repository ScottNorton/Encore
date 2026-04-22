#!/bin/bash
set -euo pipefail
. "$(dirname "$0")/../common.sh"

DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@$DEVICE_IP"
SCP="$ENCORE_SCP"
SCRIPT_DIR="/mnt/g/HKInvoke/scripts/device"

echo "=== Uploading i2c_init.py to device ==="
cat "${SCRIPT_DIR}/i2c_init.py" | $SSH 'cat > /tmp/i2c_init.py'
$SSH 'cp /tmp/i2c_init.py /lsync/scripts/i2c_init.py && sync'

echo "=== Running i2c_init.py ==="
$SSH '/lsync/python/bin/python3 -u /lsync/scripts/i2c_init.py'
