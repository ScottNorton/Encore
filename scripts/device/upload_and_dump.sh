#!/bin/bash
set -euo pipefail
. "$(dirname "$0")/../common.sh"
SSH="$ENCORE_SSH"
SCRIPTS="$(cd "$(dirname "$0")" && pwd)"
sed 's/\r$//' "$SCRIPTS/devmem.py" | $SSH "cat > /lsync/scripts/devmem.py; chmod +x /lsync/scripts/devmem.py"
echo "Uploaded devmem.py"
$SSH "/lsync/scripts/py devmem.py i2c"
