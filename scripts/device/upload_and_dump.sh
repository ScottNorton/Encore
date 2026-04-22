#!/bin/bash
set -euo pipefail
. "$(dirname "$0")/../common.sh"
SSH="$ENCORE_SSH"
SCRIPTS="/mnt/g/HKInvoke/scripts/device"
sed 's/\r$//' "$SCRIPTS/devmem.py" | $SSH "cat > /lsync/scripts/devmem.py; chmod +x /lsync/scripts/devmem.py"
echo "Uploaded devmem.py"
$SSH "/lsync/scripts/py devmem.py i2c"
