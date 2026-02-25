#!/bin/bash
# scripts/build/build_kexec.sh — Build kexec module + redir.so + kexec-tools for BG2CDP
#
# Runs in WSL (Ubuntu). Requires: arm-linux-gnueabihf-gcc, make, bc, flex, bison
#
# Usage:
#   ./scripts/build/build_kexec.sh              # build everything
#   ./scripts/build/build_kexec.sh module       # build kexec-mod.ko only
#   ./scripts/build/build_kexec.sh redir        # build redir.so only
#   ./scripts/build/build_kexec.sh tools        # build kexec-tools only
#   ./scripts/build/build_kexec.sh clean        # clean all
#
# Output:
#   tools/kexec-module/kernel/kexec-mod.ko
#   tools/kexec-module/user/redir.so
#   tools/kexec-tools/build/sbin/kexec
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VENDOR_KERNEL="$REPO_ROOT/vendor/kernel"
KEXEC_MOD="$REPO_ROOT/tools/kexec-module"
KEXEC_TOOLS_DIR="$REPO_ROOT/tools/kexec-tools"

ARCH=arm
CROSS_COMPILE=arm-linux-gnueabihf-
export ARCH CROSS_COMPILE

# Verify cross-compiler
if ! command -v "${CROSS_COMPILE}gcc" &>/dev/null; then
    echo "ERROR: ${CROSS_COMPILE}gcc not found"
    echo "       Install: sudo apt install gcc-arm-linux-gnueabihf"
    exit 1
fi

build_module() {
    echo "=== Building kexec-mod.ko ==="
    echo "  Vendor kernel: $VENDOR_KERNEL"

    cd "$KEXEC_MOD/kernel"

    # Create orig/ symlinks to vendor kernel source
    mkdir -p orig
    ln -sf "$VENDOR_KERNEL/arch/arm/kernel/machine_kexec.c" orig/machine_kexec.c
    ln -sf "$VENDOR_KERNEL/kernel/kexec.c" orig/kexec.c

    # Only create relocate_kernel.S symlink if no real file exists
    # (we may have a custom version for kexec debugging/fixes)
    if [ ! -f relocate_kernel.S ] || [ -L relocate_kernel.S ]; then
        ln -sf "$VENDOR_KERNEL/arch/arm/kernel/relocate_kernel.S" relocate_kernel.S
    fi

    # Build the module
    # CFLAGS_MODULE needed because gnueabihf compiler's cc-option test for
    # -march=armv7-a fails (hard-float default conflicts), causing fallback
    # to -march=armv5t which can't emit dmb/ldrex/strex instructions
    make -C "$VENDOR_KERNEL" M="$KEXEC_MOD/kernel" ARCH=arm \
        CROSS_COMPILE="$CROSS_COMPILE" \
        'CFLAGS_MODULE=-march=armv7-a -msoft-float -mfloat-abi=soft -fno-pic -fno-pie' \
        modules

    if [ -f kexec-mod.ko ]; then
        SIZE=$(stat -c%s kexec-mod.ko 2>/dev/null || wc -c < kexec-mod.ko)
        echo ""
        echo "  kexec-mod.ko: $SIZE bytes"
        # Show module info
        modinfo kexec-mod.ko 2>/dev/null || true
    else
        echo "ERROR: kexec-mod.ko not built"
        exit 1
    fi
}

build_redir() {
    echo "=== Building redir.so ==="
    cd "$KEXEC_MOD/user"
    make CC="${CROSS_COMPILE}gcc"

    if [ -f redir.so ]; then
        SIZE=$(stat -c%s redir.so 2>/dev/null || wc -c < redir.so)
        echo "  redir.so: $SIZE bytes"
    else
        echo "ERROR: redir.so not built"
        exit 1
    fi
}

build_tools() {
    echo "=== Building kexec-tools 2.0.15 ==="

    KEXEC_TOOLS_VER="2.0.15"
    KEXEC_TOOLS_TAR="kexec-tools-${KEXEC_TOOLS_VER}.tar.xz"
    KEXEC_TOOLS_URL="https://www.kernel.org/pub/linux/utils/kernel/kexec/${KEXEC_TOOLS_TAR}"

    # Download if not present
    if [ ! -d "$KEXEC_TOOLS_DIR/kexec-tools-${KEXEC_TOOLS_VER}" ]; then
        mkdir -p "$KEXEC_TOOLS_DIR"
        cd "$KEXEC_TOOLS_DIR"

        if [ ! -f "$KEXEC_TOOLS_TAR" ]; then
            echo "  Downloading kexec-tools ${KEXEC_TOOLS_VER}..."
            wget -q "$KEXEC_TOOLS_URL" -O "$KEXEC_TOOLS_TAR" || {
                echo "ERROR: Failed to download kexec-tools"
                echo "       Manual download: $KEXEC_TOOLS_URL"
                exit 1
            }
        fi

        echo "  Extracting..."
        tar xf "$KEXEC_TOOLS_TAR"
    fi

    cd "$KEXEC_TOOLS_DIR/kexec-tools-${KEXEC_TOOLS_VER}"

    # Configure for ARM cross-compilation
    if [ ! -f Makefile ] || [ ! -f config.status ]; then
        echo "  Configuring..."
        ./configure --host=arm-linux-gnueabihf \
            --without-lzma --without-xen \
            LDFLAGS="-static" \
            CFLAGS="-g -O2 -fcommon" 2>&1 | tail -5
    fi

    echo "  Building..."
    # GCC 10+ needs -fcommon (my_debug multiple definition)
    make -j"$(nproc 2>/dev/null || echo 2)" 2>&1 | tail -5

    # Strip debug symbols
    "${CROSS_COMPILE}strip" build/sbin/kexec 2>/dev/null || true

    KEXEC_BIN="build/sbin/kexec"

    echo ""
    echo "  NOTE: kexec-tools patched to use /dev/kexec ioctl"
    echo "  (no LD_PRELOAD/redir.so needed — works with static binary)"
    if [ -f "$KEXEC_BIN" ]; then
        SIZE=$(stat -c%s "$KEXEC_BIN" 2>/dev/null || wc -c < "$KEXEC_BIN")
        echo ""
        echo "  kexec: $SIZE bytes"
        file "$KEXEC_BIN" || true
    else
        echo "ERROR: kexec binary not built"
        exit 1
    fi
}

ACTION="${1:-all}"

case "$ACTION" in
    module)
        build_module
        ;;
    redir)
        build_redir
        ;;
    tools)
        build_tools
        ;;
    all)
        build_module
        echo ""
        build_redir
        echo ""
        build_tools
        echo ""
        echo "=== All kexec components built ==="
        echo ""
        echo "Deploy to device:"
        echo "  scp kexec-mod.ko redir.so kexec root@DEVICE:/lsync/kexec/"
        echo ""
        echo "On device:"
        echo "  insmod /lsync/kexec/kexec-mod.ko"
        echo "  LD_PRELOAD=/lsync/kexec/redir.so /lsync/kexec/kexec -l /tmp/zImage-dtb --dtb=/tmp/dtb"
        echo "  LD_PRELOAD=/lsync/kexec/redir.so /lsync/kexec/kexec -e"
        ;;
    clean)
        echo "=== Cleaning ==="
        cd "$KEXEC_MOD/kernel" && make clean 2>/dev/null; rm -rf orig relocate_kernel.S
        cd "$KEXEC_MOD/user" && make clean 2>/dev/null
        echo "Done"
        ;;
    *)
        echo "Usage: $0 [module|redir|tools|all|clean]"
        exit 1
        ;;
esac
