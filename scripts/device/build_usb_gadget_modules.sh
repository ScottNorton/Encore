#!/bin/bash
# Rebuild the USB gadget kernel modules with the period-correct Linaro 4.9.4
# cross-compiler. The running kernel is gcc 5.3.0 / soft-float; modules built
# with a modern host gcc load but then fault the kernel (float-ABI / codegen
# mismatch), so we pin the old Linaro toolchain and force soft-float. We reuse
# the vendor kernel tree (same Harman config as the stock Invoke kernel, host
# tools already built, LOCALVERSION = -yocto-standard) and only swap the cross
# compiler for the gadget objects.
#
# These modules are GPL-2.0 kernel code. LEGAL.md says where the corresponding
# source is: Harman's kernel tree, the patches in scripts/device/usb-gadget-patches/,
# and this script.
#
# Inputs, set as environment variables when the defaults do not fit:
#   KDIR       the Invoke vendor kernel tree, Linux 3.8.13 (default: <repo>/vendor/kernel).
#              Prepare it once with scripts/build/build_kernel_vendor.sh: "defconfig",
#              then "modules_prepare".
#   TOOLCHAIN  the Linaro GCC 4.9.4 toolchain folder, the one that contains
#              bin/arm-linux-gnueabihf-gcc (default:
#              <repo>/vendor/toolchains/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf).
#   OUT        where the built .ko files are collected (default: <repo>/build/usb_gadget_modules).
#
# The source changes are applied to KDIR here; a patch that is already applied
# is left alone. After building, copy the .ko files into rootfs/usr/lib/usbgadget/
# and rebuild the firmware.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
KDIR="${KDIR:-$REPO/vendor/kernel}"
TOOLCHAIN="${TOOLCHAIN:-$REPO/vendor/toolchains/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf}"
OUT="${OUT:-$REPO/build/usb_gadget_modules}"
PATCH_DIR="$REPO/scripts/device/usb-gadget-patches"

if [ ! -x "$TOOLCHAIN/bin/arm-linux-gnueabihf-gcc" ]; then
    echo "ERROR: no arm-linux-gnueabihf-gcc in $TOOLCHAIN/bin" >&2
    echo "       Set TOOLCHAIN to the gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf folder." >&2
    echo "       A modern gcc is not a substitute: its modules load but fault the 3.8.13 kernel." >&2
    exit 1
fi
if [ ! -f "$KDIR/Makefile" ]; then
    echo "ERROR: no kernel tree at $KDIR" >&2
    echo "       Set KDIR to the unpacked Invoke vendor kernel (LEGAL.md says where to get it)." >&2
    exit 1
fi

export PATH="$TOOLCHAIN/bin:$PATH"
export ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf-
# WSL's /tmp is a small tmpfs, so keep temporary files inside the build folder.
export TMPDIR="${TMPDIR:-$REPO/build/wsl-tmp}"
mkdir -p "$TMPDIR" "$OUT"

echo "=== cross toolchain ==="; arm-linux-gnueabihf-gcc --version | head -1

echo "=== apply the gadget patches to $KDIR ==="
for p in "$PATCH_DIR"/*.patch; do
    name="$(basename "$p")"
    if patch -p1 -R --dry-run -s -f -d "$KDIR" < "$p" >/dev/null 2>&1; then
        echo "  already applied: $name"
    elif patch -p1 --dry-run -s -f -d "$KDIR" < "$p" >/dev/null 2>&1; then
        patch -p1 -s -f -d "$KDIR" < "$p"
        echo "  applied: $name"
    else
        echo "ERROR: $name does not apply to $KDIR and is not already applied there." >&2
        echo "       The tree must be the vendor kernel (Linux 3.8.13), changed only by these patches." >&2
        exit 1
    fi
done

cd "$KDIR"

echo "=== ensure config (idempotent): LOCALVERSION + gadget=m ==="
./scripts/config --set-str LOCALVERSION "-yocto-standard"
./scripts/config --module USB_GADGET --module USB_MV_UDC --module USB_LIBCOMPOSITE --module USB_ETH
grep -E '^CONFIG_LOCALVERSION=|^CONFIG_USB_GADGET=|^CONFIG_USB_MV_UDC=|^CONFIG_USB_ETH=' .config

echo "=== verify kernel.release (host tools already built; should be current) ==="
REL=$(cat include/config/kernel.release)
echo "kernel.release=$REL"
[ "$REL" = "3.8.13-yocto-standard" ] || { echo "FAIL: release != 3.8.13-yocto-standard"; exit 1; }

echo "=== clean gadget dir so it recompiles with Linaro (drops any stale objects) ==="
make M=drivers/usb/gadget clean

echo "=== build gadget modules with Linaro 4.9.4 (soft-float forced) ==="
make -j"$(nproc)" M=drivers/usb/gadget modules \
  CFLAGS_MODULE='-march=armv7-a -msoft-float -mfloat-abi=soft -fno-pic -fno-pie'

echo "=== collect ==="
rm -f "$OUT"/*.ko
for m in udc-core mv_udc libcomposite g_ether; do
  f="drivers/usb/gadget/$m.ko"
  [ -f "$f" ] && cp -v "$f" "$OUT/"
done

echo "=== vermagic check (want: 3.8.13-yocto-standard SMP preempt mod_unload ARMv7) ==="
for f in "$OUT"/*.ko; do
  vm=$(arm-linux-gnueabihf-objcopy -O binary --only-section=.modinfo "$f" /dev/stdout 2>/dev/null | tr '\0' '\n' | grep '^vermagic=' | cut -d= -f2-)
  dep=$(arm-linux-gnueabihf-objcopy -O binary --only-section=.modinfo "$f" /dev/stdout 2>/dev/null | tr '\0' '\n' | grep '^depends=' | cut -d= -f2-)
  printf '%-16s vermagic=[%s] depends=[%s]\n' "$(basename "$f")" "$vm" "$dep"
done
echo "--- cross-built with gcc $(arm-linux-gnueabihf-gcc -dumpversion) ---"
ls -la "$OUT"
