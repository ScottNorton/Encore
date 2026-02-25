#!/bin/bash
# scripts/kexec/upload_kexec.sh — Upload kexec boot files to device
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DEVICE_IP="${1:-192.168.43.1}"
SSH_OPTS="-o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa -o StrictHostKeyChecking=no -o ConnectTimeout=5"
SSH="sshpass -p ridiculous ssh $SSH_OPTS root@$DEVICE_IP"
SCP="sshpass -p ridiculous scp $SSH_OPTS"

KERNEL_DIR="$REPO_ROOT/firmware/kernel_test"
KDIR=""
for d in "$KERNEL_DIR"/linux-6.1.*/; do
    [ -d "$d" ] && KDIR="${d%/}" && break
done

echo "=== Uploading kexec boot files to $DEVICE_IP ==="

# Ensure target directory exists
$SSH "mkdir -p /lsync/kexec"

# Upload initramfs
INITRAMFS="$REPO_ROOT/firmware/initramfs.cpio.gz"
if [ -f "$INITRAMFS" ]; then
    echo "Uploading initramfs.cpio.gz..."
    cat "$INITRAMFS" | $SSH "cat > /lsync/kexec/initramfs.cpio.gz"
    LOCAL=$(stat -c%s "$INITRAMFS")
    REMOTE=$($SSH "stat -c%s /lsync/kexec/initramfs.cpio.gz")
    echo "  Local: $LOCAL  Remote: $REMOTE"
    [ "$LOCAL" = "$REMOTE" ] && echo "  OK" || echo "  MISMATCH!"
fi

# Upload zImage (if not already there or newer)
ZIMAGE="$KDIR/arch/arm/boot/zImage"
if [ -f "$ZIMAGE" ]; then
    echo "Uploading zImage..."
    cat "$ZIMAGE" | $SSH "cat > /lsync/kexec/zImage"
    LOCAL=$(stat -c%s "$ZIMAGE")
    REMOTE=$($SSH "stat -c%s /lsync/kexec/zImage")
    echo "  Local: $LOCAL  Remote: $REMOTE"
    [ "$LOCAL" = "$REMOTE" ] && echo "  OK" || echo "  MISMATCH!"
fi

# Upload DTB
DTB=""
for path in "$KDIR/arch/arm/boot/dts/berlin2cdp-hk-invoke.dtb" \
            "$KDIR/arch/arm/boot/dts/synaptics/berlin2cdp-hk-invoke.dtb"; do
    [ -f "$path" ] && DTB="$path" && break
done
if [ -n "$DTB" ]; then
    echo "Uploading DTB..."
    cat "$DTB" | $SSH "cat > /lsync/kexec/berlin2cdp-hk-invoke.dtb"
    LOCAL=$(stat -c%s "$DTB")
    REMOTE=$($SSH "stat -c%s /lsync/kexec/berlin2cdp-hk-invoke.dtb")
    echo "  Local: $LOCAL  Remote: $REMOTE"
    [ "$LOCAL" = "$REMOTE" ] && echo "  OK" || echo "  MISMATCH!"
fi

echo ""
echo "=== Files on device ==="
$SSH "ls -la /lsync/kexec/"

echo ""
echo "=== Test kexec load ==="
$SSH "test -c /dev/kexec || insmod /lsync/kexec/kexec-mod.ko 2>/dev/null || true"
RESULT=$($SSH "/lsync/kexec/kexec -l /lsync/kexec/zImage --dtb=/lsync/kexec/berlin2cdp-hk-invoke.dtb --initrd=/lsync/kexec/initramfs.cpio.gz --command-line='console=ttyS0,115200 earlyprintk loglevel=8 panic=30' 2>&1; echo RETVAL=\$?")
echo "  $RESULT"
if echo "$RESULT" | grep -q "RETVAL=0"; then
    echo "  PASS: kexec load succeeded (with initramfs)"
    $SSH "/lsync/kexec/kexec -u"
    echo "  Unloaded (safe)"
else
    echo "  FAIL: kexec load failed"
fi
