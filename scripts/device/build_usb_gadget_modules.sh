#!/bin/bash
# Rebuild the USB gadget kernel modules with the period-correct Linaro 4.9.4
# cross-compiler. The running kernel is gcc 5.3.0 / soft-float; modules built
# with a modern host gcc load but then fault the kernel (float-ABI / codegen
# mismatch), so we pin the old Linaro toolchain and force soft-float. We reuse
# vendor/kernel (same Harman config as the stock Invoke kernel, host tools
# already built, LOCALVERSION already = -yocto-standard) and only swap the cross
# compiler for the gadget objects.
#
# Source patches applied to vendor/kernel before building live in
# scripts/device/usb-gadget-patches/ (regenerate the diffs with
# gen_gadget_patches.sh). After building, copy the .ko files into
# rootfs/usr/lib/usbgadget/ and rebuild the firmware.
set -euo pipefail

TOOLCHAIN=/home/scott/HKHacking/workspace/tools/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf
KDIR=/mnt/g/HKInvoke/vendor/kernel
OUT=/mnt/g/HKInvoke/build/usb_gadget_modules
export PATH="$TOOLCHAIN/bin:$PATH"
export ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf-
export TMPDIR=/mnt/g/HKInvoke/build/wsl-tmp
mkdir -p "$TMPDIR" "$OUT"

echo "=== cross toolchain ==="; arm-linux-gnueabihf-gcc --version | head -1
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
