#!/bin/bash
# scripts/build_minimal_kexec.sh — Build absolutely minimal kernel for kexec boot testing
# Uses embedded initramfs, CONFIG_SMP=n, minimal drivers
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

echo "=== Phase 1: Build minimal initramfs ==="

rm -rf "$TINY"
mkdir -p "$TINY/bin" "$TINY/dev" "$TINY/proc" "$TINY/sys" "$TINY/sbin"

cp "$BUSYBOX" "$TINY/bin/busybox"
chmod 755 "$TINY/bin/busybox"

APPLETS=$(grep '^"' "$APPLET_TABLE" | sed 's/^"//; s/" *"\\0"$//; s/".*$//')
for applet in $APPLETS; do
    [ "$applet" = "busybox" ] && continue
    ln -sf busybox "$TINY/bin/$applet"
done

for cmd in init mount umount halt reboot poweroff; do
    [ -L "$TINY/bin/$cmd" ] && mv "$TINY/bin/$cmd" "$TINY/sbin/$cmd"
done

NAPPLETS=$(echo "$APPLETS" | wc -l)
echo "  BusyBox: $NAPPLETS applets"

# Create minimal init script
cat > "$TINY/init" << 'INITEOF'
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev

echo "=========================================="
echo "  LINUX 6.1 MINIMAL BOOT SUCCESS"
echo "=========================================="
echo ""
echo "Kernel: $(uname -r)"
echo "Cmdline: $(cat /proc/cmdline)"
echo ""
echo "Memory:"
cat /proc/meminfo | head -3
echo ""
echo "CPUs online:"
cat /proc/cpuinfo | grep -c processor || echo "unknown"
echo ""
echo "Interrupts:"
cat /proc/interrupts 2>/dev/null | head -10
echo ""
echo "dmesg (last 50 lines):"
dmesg | tail -50
echo ""
echo "Dropping to shell..."
exec /bin/sh
INITEOF
chmod 755 "$TINY/init"

echo "  initramfs: $(find "$TINY" | wc -l) entries"

echo ""
echo "=== Phase 2: Configure minimal kernel ==="
cd "$KERNEL_DIR"

# Start from minimal defconfig
make ARCH=arm berlin2cdp_minimal_defconfig

# Set INITRAMFS_SOURCE to embed the initramfs
sed -i 's|^CONFIG_INITRAMFS_SOURCE=.*|CONFIG_INITRAMFS_SOURCE="'"$TINY"'"|' .config
if ! grep -q "CONFIG_INITRAMFS_SOURCE=\"$TINY\"" .config; then
    echo "CONFIG_INITRAMFS_SOURCE=\"$TINY\"" >> .config
fi

# Run olddefconfig to resolve dependencies
make ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- olddefconfig

# Verify critical configs
echo ""
echo "  Verifying config..."
check_config() {
    if grep -q "^$1" .config; then
        echo "    OK: $1"
    else
        echo "    MISSING: $1"
    fi
}
check_config "CONFIG_BLK_DEV_INITRD=y"
check_config "CONFIG_USE_OF=y"
check_config "CONFIG_ARM_ARCH_TIMER=y"
check_config "CONFIG_DW_APB_ICTL=y"
check_config "CONFIG_SERIAL_8250=y"
check_config "CONFIG_SERIAL_8250_CONSOLE=y"
check_config "CONFIG_SERIAL_8250_DW=y"
check_config "CONFIG_DEBUG_LL=y"
check_config "CONFIG_EARLY_PRINTK=y"

# Verify SMP is off
if grep -q "CONFIG_SMP=y" .config; then
    echo "    WARNING: SMP still enabled!"
else
    echo "    OK: SMP disabled"
fi

# Verify initramfs source
if grep -q "CONFIG_INITRAMFS_SOURCE=" .config; then
    IRSRC=$(grep "CONFIG_INITRAMFS_SOURCE=" .config)
    echo "    OK: $IRSRC"
fi

echo ""
echo "=== Phase 3: Build kernel ==="
NPROC=$(nproc 2>/dev/null || echo 2)
echo "  Building with $NPROC jobs..."
make -j"$NPROC" ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- zImage dtbs

# Find outputs
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
    echo "  WARNING: zImage exceeds 8 MB"
fi

echo ""
echo "  To test via kexec:"
echo "    scripts/kexec_embedded.sh"
