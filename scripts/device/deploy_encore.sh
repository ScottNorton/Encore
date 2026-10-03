#!/bin/bash
set -euo pipefail
. "$(dirname "$0")/../common.sh"
DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS -o ConnectTimeout=10 root@$DEVICE_IP"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="$REPO_ROOT/encore/target/armv7-unknown-linux-musleabihf/release/encore-firmware"
echo "=== Uploading encore binary ($(wc -c < $BINARY) bytes) ==="
cat "$BINARY" | $SSH 'cat > /run/encore_next'
echo "=== Staging as encore_next ==="
$SSH 'rm -f /lsync/encore/encore_next; cp /run/encore_next /lsync/encore/encore_next && sync && ls -la /lsync/encore/encore_next'
echo "=== Rebooting ==="
$SSH 'reboot' || true
echo "Done. Device will boot with new binary."
