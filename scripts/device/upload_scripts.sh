#!/bin/bash
# Upload Python scripts to device. Run in WSL.
set -euo pipefail
. "$(dirname "$0")/../common.sh"

DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@$DEVICE_IP"
SCRIPTS="$(cd "$(dirname "$0")" && pwd)"

$SSH "mkdir -p /lsync/scripts"

echo "Uploading mcu.py..."
sed 's/\r$//' "$SCRIPTS/mcu.py" | $SSH "cat > /lsync/scripts/mcu.py"
$SSH "chmod +x /lsync/scripts/mcu.py"

echo "Uploading bt_discover.py..."
sed 's/\r$//' "$SCRIPTS/bt_discover.py" | $SSH "cat > /lsync/scripts/bt_discover.py"
$SSH "chmod +x /lsync/scripts/bt_discover.py"

echo "Creating wrapper..."
$SSH 'echo "#!/bin/sh" > /lsync/scripts/py'
$SSH 'echo "exec /lsync/python/bin/python3 /lsync/scripts/\"\$@\"" >> /lsync/scripts/py'
$SSH "chmod +x /lsync/scripts/py"

echo ""
echo "Files on device:"
$SSH "ls -la /lsync/scripts/"

echo ""
echo "Testing mcu.py version command..."
$SSH "/lsync/scripts/py mcu.py version"

echo ""
echo "DONE"
