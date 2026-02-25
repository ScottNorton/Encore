#!/bin/bash
# scripts/kexec/deploy_kexec.sh — Deploy kexec components to device
#
# Uploads kexec-mod.ko, redir.so, and kexec binary to /lsync/kexec/
# Then loads the module and verifies it's working.
#
# Usage:
#   ./scripts/kexec/deploy_kexec.sh [DEVICE_IP]
#   ./scripts/kexec/deploy_kexec.sh 192.168.7.235
#   ./scripts/kexec/deploy_kexec.sh 192.168.43.1
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DEVICE_IP="${1:-192.168.7.235}"
SSH_OPTS="-o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa -o StrictHostKeyChecking=no -o ConnectTimeout=5"
SSH_CMD="sshpass -p ridiculous ssh $SSH_OPTS root@$DEVICE_IP"

KEXEC_MOD="$REPO_ROOT/tools/kexec-module/kernel/kexec-mod.ko"
REDIR_SO="$REPO_ROOT/tools/kexec-module/user/redir.so"
KEXEC_BIN="$REPO_ROOT/tools/kexec-tools/kexec-tools-2.0.15/build/sbin/kexec"

echo "=== Deploying kexec to $DEVICE_IP ==="

# Verify builds exist
for f in "$KEXEC_MOD" "$REDIR_SO" "$KEXEC_BIN"; do
    if [ ! -f "$f" ]; then
        echo "ERROR: $f not found — run scripts/build/build_kexec.sh first"
        exit 1
    fi
done

# Test SSH connectivity
echo "  Testing SSH..."
if ! $SSH_CMD "echo connected" >/dev/null 2>&1; then
    echo "ERROR: Cannot SSH to $DEVICE_IP"
    exit 1
fi

echo "  Creating /lsync/kexec/..."
$SSH_CMD "mkdir -p /lsync/kexec"

echo "  Uploading kexec-mod.ko ($(stat -c%s "$KEXEC_MOD" 2>/dev/null || wc -c < "$KEXEC_MOD") bytes)..."
cat "$KEXEC_MOD" | $SSH_CMD "cat > /lsync/kexec/kexec-mod.ko"

echo "  Uploading redir.so ($(stat -c%s "$REDIR_SO" 2>/dev/null || wc -c < "$REDIR_SO") bytes)..."
cat "$REDIR_SO" | $SSH_CMD "cat > /lsync/kexec/redir.so"

echo "  Uploading kexec ($(stat -c%s "$KEXEC_BIN" 2>/dev/null || wc -c < "$KEXEC_BIN") bytes)..."
cat "$KEXEC_BIN" | $SSH_CMD "cat > /lsync/kexec/kexec && chmod +x /lsync/kexec/kexec"

echo ""
echo "  Loading kexec module..."
$SSH_CMD "rmmod kexec-mod 2>/dev/null; insmod /lsync/kexec/kexec-mod.ko" 2>&1 || {
    echo "  Module load failed — checking dmesg..."
    $SSH_CMD "dmesg | tail -10"
    exit 1
}

echo "  Verifying /dev/kexec..."
$SSH_CMD "ls -la /dev/kexec" 2>&1 || {
    echo "  WARNING: /dev/kexec not created"
}

echo ""
$SSH_CMD "dmesg | grep kexec-mod | tail -5"

echo ""
echo "=== Deployment complete ==="
echo ""
echo "Next steps:"
echo "  1. Upload a kernel:  cat zImage-dtb | ssh ... 'cat > /tmp/zImage-dtb'"
echo "  2. Load kernel:      LD_PRELOAD=/lsync/kexec/redir.so /lsync/kexec/kexec -l /tmp/zImage-dtb"
echo "  3. Execute kexec:    LD_PRELOAD=/lsync/kexec/redir.so /lsync/kexec/kexec -e"
