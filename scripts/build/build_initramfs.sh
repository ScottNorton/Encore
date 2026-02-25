#!/bin/bash
# scripts/build_initramfs.sh — Build minimal initramfs for 6.1 kexec boot
#
# Creates a cpio.gz initramfs with BusyBox for kernel testing.
# Must run as root in WSL (needs mknod for device nodes).
#
# Output:
#   firmware/initramfs_tiny.cpio.gz (embedded in kernel via CONFIG_INITRAMFS_SOURCE)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

SRC="$REPO_ROOT/firmware/initramfs_tiny"
STAGING=/tmp/initramfs_build
OUT="$REPO_ROOT/firmware/initramfs_tiny.cpio.gz"

if [ "$(id -u)" != "0" ]; then
    echo "ERROR: Must run as root (need mknod for device nodes)"
    exit 1
fi

if [ ! -f "$SRC/bin/busybox" ]; then
    echo "ERROR: BusyBox not found at $SRC/bin/busybox"
    exit 1
fi

echo "=== Building initramfs ==="

# Clean and create staging on native Linux filesystem (not NTFS)
rm -rf "$STAGING"
mkdir -p "$STAGING"/{bin,dev,proc,sys,sbin,tmp}

# Copy busybox (static ARM binary)
cp "$SRC/bin/busybox" "$STAGING/bin/busybox"
chmod 755 "$STAGING/bin/busybox"

# Copy mcu_pet (MCU heartbeat tool)
if [ -f "$SRC/bin/mcu_pet" ]; then
    cp "$SRC/bin/mcu_pet" "$STAGING/bin/mcu_pet"
    chmod 755 "$STAGING/bin/mcu_pet"
    echo "  Included mcu_pet"
fi

# Copy init and fix CRLF
cp "$SRC/init" "$STAGING/init"
chmod 755 "$STAGING/init"
sed -i 's/\r$//' "$STAGING/init"

# Create essential device nodes (can't exist on NTFS!)
mknod -m 600 "$STAGING/dev/console" c 5 1
mknod -m 666 "$STAGING/dev/null" c 1 3
mknod -m 666 "$STAGING/dev/zero" c 1 5
mknod -m 666 "$STAGING/dev/mem" c 1 1

# Create busybox symlinks for essential commands
cd "$STAGING/bin"
for cmd in sh ash cat echo grep head mount sleep uname dmesg devmem \
           i2cset i2cget i2cdump i2cdetect ifconfig hostname mknod mkdir \
           ls cp rm mv ln chmod chown vi less free ps top kill reboot \
           ping wget tar gzip gunzip dd hexdump stty; do
    ln -sf busybox "$cmd"
done

# sbin links
cd "$STAGING/sbin"
ln -sf ../bin/busybox reboot
ln -sf ../bin/busybox poweroff
ln -sf ../bin/busybox halt
ln -sf ../bin/busybox mount

# Package as cpio.gz
cd "$STAGING"
find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$OUT"

echo ""
echo "=== Build complete ==="
ls -la "$OUT"
echo ""
echo "Contents:"
zcat "$OUT" | cpio -t 2>/dev/null | head -40
echo ""
echo "Device nodes:"
ls -la "$STAGING/dev/"
echo ""
echo "Update kernel config:"
echo "  CONFIG_INITRAMFS_SOURCE=\"$OUT\""
