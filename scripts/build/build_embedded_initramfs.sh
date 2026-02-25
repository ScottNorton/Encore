#!/bin/bash
# scripts/build_embedded_initramfs.sh — Build kernel with embedded initramfs
#
# Creates a minimal initramfs and embeds it directly in the kernel via
# CONFIG_INITRAMFS_SOURCE. This eliminates the --initrd kexec parameter.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BUSYBOX="$REPO_ROOT/tools/busybox-1.36.1/busybox"
TINY="$REPO_ROOT/firmware/initramfs_tiny"
APPLET_TABLE="$REPO_ROOT/tools/busybox-1.36.1/include/applet_tables.h"
KERNEL_DIR=""
for d in "$REPO_ROOT/firmware/kernel_test"/linux-6.1.*/; do
    [ -d "$d" ] && KERNEL_DIR="${d%/}" && break
done

if [ -z "$KERNEL_DIR" ]; then
    echo "ERROR: No kernel source found"
    exit 1
fi

echo "=== Building embedded initramfs ==="

# Clean and create directory structure
rm -rf "$TINY"
mkdir -p "$TINY/bin" "$TINY/dev" "$TINY/proc" "$TINY/sys" "$TINY/sbin"

# Install BusyBox
cp "$BUSYBOX" "$TINY/bin/busybox"
chmod 755 "$TINY/bin/busybox"

# Create minimal symlinks from applet table
APPLETS=$(grep '^"' "$APPLET_TABLE" | sed 's/^"//; s/" *"\\0"$//; s/".*$//')
for applet in $APPLETS; do
    [ "$applet" = "busybox" ] && continue
    ln -sf busybox "$TINY/bin/$applet"
done

# Move sbin commands
for cmd in init mount umount halt reboot poweroff; do
    [ -L "$TINY/bin/$cmd" ] && mv "$TINY/bin/$cmd" "$TINY/sbin/$cmd"
done

NAPPLETS=$(echo "$APPLETS" | wc -l)
echo "  BusyBox: $NAPPLETS applets"

# Create init script
cat > "$TINY/init" << 'INITEOF'
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev

echo "=========================================="
echo "  LINUX 6.1 BOOT SUCCESS"
echo "=========================================="
echo ""
echo "Kernel: $(uname -r)"
echo "Cmdline: $(cat /proc/cmdline)"
echo ""
echo "Memory:"
cat /proc/meminfo | head -3
echo ""
echo "CPUs:"
cat /proc/cpuinfo | head -10
echo ""
echo "Interrupts:"
cat /proc/interrupts 2>/dev/null | head -10
echo ""
echo "dmesg (last 40 lines):"
dmesg | tail -40
echo ""
echo "Dropping to shell..."
exec /bin/sh
INITEOF
chmod 755 "$TINY/init"

echo "  initramfs: $(find "$TINY" | wc -l) entries"

# Update kernel config to embed the initramfs
cd "$KERNEL_DIR"

# Set INITRAMFS_SOURCE to point to our directory
sed -i 's|^CONFIG_INITRAMFS_SOURCE=.*|CONFIG_INITRAMFS_SOURCE="'"$TINY"'"|' .config
# Verify
if ! grep -q "CONFIG_INITRAMFS_SOURCE=\"$TINY\"" .config; then
    echo "CONFIG_INITRAMFS_SOURCE=\"$TINY\"" >> .config
fi

echo "  INITRAMFS_SOURCE set to $TINY"

# Build
NPROC=$(nproc 2>/dev/null || echo 2)
echo ""
echo "=== Building kernel with embedded initramfs ($NPROC jobs) ==="
make -j"$NPROC" ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- zImage dtbs

# Find DTB
DTB=""
for path in arch/arm/boot/dts/berlin2cdp-hk-invoke.dtb \
            arch/arm/boot/dts/synaptics/berlin2cdp-hk-invoke.dtb; do
    [ -f "$path" ] && DTB="$path" && break
done

ZIMAGE="arch/arm/boot/zImage"
ZIMAGE_SIZE=$(stat -c%s "$ZIMAGE")
DTB_SIZE=$(stat -c%s "$DTB")

echo ""
echo "=== Build complete ==="
echo "  zImage:  $ZIMAGE ($ZIMAGE_SIZE bytes)"
echo "  DTB:     $DTB ($DTB_SIZE bytes)"

if [ "$ZIMAGE_SIZE" -gt 8388608 ]; then
    echo "  WARNING: zImage exceeds 8 MB — might be too large for kexec"
fi

echo ""
echo "  To boot (no --initrd needed):"
echo "    kexec -l /lsync/kexec/zImage --dtb=/lsync/kexec/berlin2cdp-hk-invoke.dtb \\"
echo "      --command-line='console=ttyS0,115200 earlyprintk loglevel=8 panic=30'"
echo "    kexec -e"
