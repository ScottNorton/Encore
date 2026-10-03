#!/bin/bash
# Regenerate the USB gadget kernel patches as unified diffs against the pristine
# Harman Invoke kernel source. The kernel tree (vendor/kernel) is git-ignored, so
# these patches are how the changes are tracked. The headers use kernel-relative
# paths (a/drivers/usb/gadget/...), so they apply with `patch -p1` from the
# tree root; build_usb_gadget_modules.sh does that for you.
#
#   PRISTINE  an unmodified copy of the Invoke vendor kernel tree (required)
#   KDIR      the tree that holds your changes (default: <repo>/vendor/kernel)
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
PRISTINE="${PRISTINE:?set PRISTINE to an unmodified copy of the Invoke vendor kernel tree}"
KDIR="${KDIR:-$REPO/vendor/kernel}"
OUT="$REPO/scripts/device/usb-gadget-patches"
G=drivers/usb/gadget

mkdir -p "$OUT"
for f in composite.c f_rndis.c ether.c u_ether.c mv_udc_core.c; do
  # diff exits 1 when the files differ, which is the normal case here
  diff -u --label "a/$G/$f" --label "b/$G/$f" "$PRISTINE/$G/$f" "$KDIR/$G/$f" > "$OUT/$f.patch" || [ $? -eq 1 ]
  echo "$f.patch: $(grep -c '^[+-]' "$OUT/$f.patch") +/- lines"
done
ls -la "$OUT"
