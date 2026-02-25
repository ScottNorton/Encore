#!/bin/bash
# scripts/build/build_kernel_vendor.sh — Build/clean the vendor 3.8.13 kernel
#
# Runs in WSL (Ubuntu). Requires: arm-linux-gnueabihf-gcc, make, bc
#
# Usage:
#   ./scripts/build/build_kernel_vendor.sh              # build zImage + modules
#   ./scripts/build/build_kernel_vendor.sh defconfig     # regenerate .config from defconfig
#   ./scripts/build/build_kernel_vendor.sh menuconfig    # interactive config
#   ./scripts/build/build_kernel_vendor.sh clean         # clean build artifacts (keeps .config)
#   ./scripts/build/build_kernel_vendor.sh modules_prepare  # prepare for out-of-tree module builds
#
# This is the stock Marvell BG2CDP kernel (3.8.13). It cannot be flashed to the
# device (OTP-encrypted boot chain), but its build tree is needed for:
#   - Out-of-tree module builds (kexec-mod.ko)
#   - Kernel header reference
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

KERNEL_DIR="$REPO_ROOT/vendor/kernel"

if [ ! -f "$KERNEL_DIR/Makefile" ]; then
    echo "ERROR: Vendor kernel not found at $KERNEL_DIR"
    exit 1
fi

KERNEL_VER=$(head -4 "$KERNEL_DIR/Makefile" | awk '/^VERSION/{v=$3} /^PATCHLEVEL/{p=$3} /^SUBLEVEL/{s=$3} END{print v"."p"."s}')
echo "=== Vendor Kernel Build (${KERNEL_VER}) ==="
echo "  Source: $KERNEL_DIR"
echo ""

# Cross-compiler
ARCH=arm
CROSS_COMPILE=arm-linux-gnueabihf-
export ARCH CROSS_COMPILE

if ! command -v "${CROSS_COMPILE}gcc" &>/dev/null; then
    echo "ERROR: ${CROSS_COMPILE}gcc not found"
    echo "       Install: sudo apt install gcc-arm-linux-gnueabihf"
    exit 1
fi

NPROC=$(nproc 2>/dev/null || echo 2)
DEFCONFIG=berlin2cdp_amp_defconfig

cd "$KERNEL_DIR"

ACTION="${1:-build}"

case "$ACTION" in
    defconfig)
        make "$DEFCONFIG"
        echo ""
        echo "=== Config generated from $DEFCONFIG ==="
        ;;

    menuconfig)
        make menuconfig
        ;;

    clean)
        make clean || true
        echo "=== Clean complete ==="
        ;;

    modules_prepare)
        if [ ! -f .config ]; then
            echo "=== Generating .config from $DEFCONFIG ==="
            make "$DEFCONFIG"
        fi
        make -j"$NPROC" modules_prepare
        echo "=== modules_prepare complete (ready for out-of-tree builds) ==="
        ;;

    build|"")
        if [ ! -f .config ]; then
            echo "=== Generating .config from $DEFCONFIG ==="
            make "$DEFCONFIG"
        fi

        echo "=== Building kernel (${NPROC} jobs) ==="
        make -j"$NPROC" zImage modules

        ZIMAGE="arch/arm/boot/zImage"
        if [ -f "$ZIMAGE" ]; then
            SIZE=$(stat -c%s "$ZIMAGE" 2>/dev/null || wc -c < "$ZIMAGE")
            echo ""
            echo "=== Build complete ==="
            echo "  zImage: $ZIMAGE ($SIZE bytes)"
            echo ""
            echo "  NOTE: This kernel cannot be flashed (OTP-encrypted boot)."
            echo "  Use 'make kexec' to build modules against this tree."
        else
            echo "ERROR: zImage not built"
            exit 1
        fi
        ;;

    *)
        echo "Usage: $0 [defconfig|menuconfig|clean|modules_prepare|build]"
        exit 1
        ;;
esac
