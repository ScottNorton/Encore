#!/bin/bash
# Generate unified diffs of the USB gadget kernel patches vs the pristine Harman
# Invoke-kernel source (vendor/kernel is git-ignored, so these are how the patches
# are tracked/reproduced). Rebuild: apply to vendor/kernel, build with Linaro 4.9.4
# via build_usb_gadget_modules.sh.
P=/home/scott/HKHacking/workspace/source/kernel/Invoke-kernel/drivers/usb/gadget
V=/mnt/g/HKInvoke/vendor/kernel/drivers/usb/gadget
OUT=/mnt/g/HKInvoke/scripts/device/usb-gadget-patches
mkdir -p "$OUT"
for f in composite.c f_rndis.c ether.c u_ether.c mv_udc_core.c; do
  diff -u "$P/$f" "$V/$f" > "$OUT/$f.patch"
  echo "$f.patch: $(grep -c '^[+-]' "$OUT/$f.patch") +/- lines"
done
ls -la "$OUT"
