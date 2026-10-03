#!/bin/bash
# Deploy dsp_init_stock.py to the device. Run from Git Bash (Windows).
set -euo pipefail
. "$(dirname "$0")/../common.sh"

DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@$DEVICE_IP"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# This folder as WSL sees it: Git Bash says /g/..., WSL says /mnt/g/... (already right inside WSL).
WSL_SCRIPT_DIR="$(printf '%s' "$SCRIPT_DIR" | sed -E 's#^/([a-zA-Z])/#/mnt/\L\1/#')"

echo "=== Deploying DSP init proxy ==="

# Upload script (strip CRLF)
wsl.exe -d Ubuntu -e bash -c "
  SSH='sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@${DEVICE_IP}'
  \$SSH 'mkdir -p /lsync/scripts'
  sed 's/\r\$//' '${WSL_SCRIPT_DIR}/dsp_init_stock.py' | \$SSH 'cat > /lsync/scripts/dsp_init_stock.py'
  \$SSH 'chmod +x /lsync/scripts/dsp_init_stock.py'
  echo 'Uploaded dsp_init_stock.py'
  \$SSH 'ls -la /lsync/scripts/dsp_init_stock.py'
"

echo ""
echo "=== Deployed ==="
echo "To run:"
echo "  1. Disable Encore: ssh root@$DEVICE_IP 'touch /lsync/encore/disabled && sync && reboot'"
echo "  2. Wait for reboot, reconnect"
echo "  3. Run: ssh root@$DEVICE_IP '/lsync/scripts/py dsp_init_stock.py'"
echo "  4. Or just dump registers: ssh root@$DEVICE_IP '/lsync/scripts/py dsp_init_stock.py --dump'"
