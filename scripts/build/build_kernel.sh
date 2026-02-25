#!/bin/bash
# scripts/build/build_kernel.sh — Cross-compile Linux 6.1 LTS for BG2CDP (88DE3006)
#
# Runs in WSL (Ubuntu). Requires: arm-linux-gnueabihf-gcc, make, bc, flex, bison,
# libssl-dev, libelf-dev
#
# Usage:
#   ./scripts/build/build_kernel.sh              # build zImage + DTBs
#   ./scripts/build/build_kernel.sh defconfig    # regenerate .config from defconfig
#   ./scripts/build/build_kernel.sh menuconfig   # interactive config
#   ./scripts/build/build_kernel.sh clean        # clean build artifacts
#   ./scripts/build/build_kernel.sh modules      # build loadable modules
#
# Output:
#   firmware/kernel_test/<kernel-dir>/zImage-dtb  (kernel + appended DTB)
set -euo pipefail

# ── Auto-detect paths ──
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

KERNEL_TEST="$REPO_ROOT/firmware/kernel_test"

# Find the 6.1 kernel source directory
KERNEL_DIR=""
for d in "$KERNEL_TEST"/linux-6.1.*/; do
    if [ -d "$d" ]; then
        KERNEL_DIR="${d%/}"
        break
    fi
done

if [ -z "$KERNEL_DIR" ]; then
    echo "ERROR: No linux-6.1.* directory found in $KERNEL_TEST"
    echo "       Run: cd $KERNEL_TEST && tar xf linux-6.1-latest.tar.xz"
    exit 1
fi

KERNEL_VER="$(basename "$KERNEL_DIR")"
echo "=== Linux Kernel Build for BG2CDP ==="
echo "  Source:  $KERNEL_DIR"
echo "  Version: $KERNEL_VER"
echo ""

# ── Cross-compiler ──
ARCH=arm
CROSS_COMPILE=arm-linux-gnueabihf-
export ARCH CROSS_COMPILE

# Verify cross-compiler is available
if ! command -v "${CROSS_COMPILE}gcc" &>/dev/null; then
    echo "ERROR: ${CROSS_COMPILE}gcc not found"
    echo "       Install: sudo apt install gcc-arm-linux-gnueabihf"
    exit 1
fi

NPROC=$(nproc 2>/dev/null || echo 2)

# ── Verify required files exist ──
verify_sources() {
    local ok=1

    # mach-berlin
    for f in berlin.c Kconfig Makefile platsmp.c headsmp.S; do
        if [ ! -f "$KERNEL_DIR/arch/arm/mach-berlin/$f" ]; then
            echo "ERROR: Missing mach-berlin/$f"
            ok=0
        fi
    done

    # Device tree
    if [ ! -f "$KERNEL_DIR/arch/arm/boot/dts/berlin2cdp.dtsi" ]; then
        echo "ERROR: Missing arch/arm/boot/dts/berlin2cdp.dtsi"
        ok=0
    fi
    if [ ! -f "$KERNEL_DIR/arch/arm/boot/dts/berlin2cdp-hk-invoke.dts" ]; then
        echo "ERROR: Missing arch/arm/boot/dts/berlin2cdp-hk-invoke.dts"
        ok=0
    fi

    # Defconfig
    if [ ! -f "$KERNEL_DIR/arch/arm/configs/berlin2cdp_hkinvoke_defconfig" ]; then
        echo "ERROR: Missing arch/arm/configs/berlin2cdp_hkinvoke_defconfig"
        ok=0
    fi

    # DTS Makefile entry
    if ! grep -q 'berlin2cdp-hk-invoke' "$KERNEL_DIR/arch/arm/boot/dts/Makefile"; then
        echo "  Adding berlin2cdp-hk-invoke.dtb to DTS Makefile..."
        sed -i 's/berlin2q-marvell-dmp.dtb$/berlin2q-marvell-dmp.dtb \\\n\tberlin2cdp-hk-invoke.dtb/' \
            "$KERNEL_DIR/arch/arm/boot/dts/Makefile"
    fi

    if [ "$ok" -eq 0 ]; then
        echo ""
        echo "Required source files are missing from the 6.1 tree."
        echo "These need to be created manually — see docs/kernel-porting.md"
        exit 1
    fi
}

# ── Main ──
cd "$KERNEL_DIR"

ACTION="${1:-build}"

case "$ACTION" in
    defconfig)
        verify_sources
        make berlin2cdp_hkinvoke_defconfig
        echo ""
        echo "=== Config generated. Run './scripts/build/build_kernel.sh' to build ==="
        ;;

    menuconfig)
        make menuconfig
        ;;

    clean)
        # Documentation/Kbuild is a directory in 6.1, which causes a non-fatal
        # error during clean. Ignore it — all build artifacts still get cleaned.
        make clean || true
        echo "=== Clean complete ==="
        ;;

    modules)
        make -j"$NPROC" modules
        echo "=== Modules built ==="
        ;;

    build|"")
        verify_sources

        # Generate .config if not present
        if [ ! -f .config ]; then
            echo "=== Generating .config from defconfig ==="
            make berlin2cdp_hkinvoke_defconfig
        fi

        echo "=== Building kernel (${NPROC} jobs) ==="
        make -j"$NPROC" zImage dtbs

        # Find the DTB
        DTB=""
        for path in \
            arch/arm/boot/dts/berlin2cdp-hk-invoke.dtb \
            arch/arm/boot/dts/synaptics/berlin2cdp-hk-invoke.dtb; do
            if [ -f "$path" ]; then
                DTB="$path"
                break
            fi
        done

        if [ -z "$DTB" ]; then
            echo "ERROR: DTB not found after build"
            echo "  Check that berlin2cdp-hk-invoke.dtb is in the DTS Makefile"
            exit 1
        fi

        ZIMAGE="arch/arm/boot/zImage"
        OUTPUT="$KERNEL_DIR/zImage-dtb"

        # Append DTB to zImage (kexec loads this as a single blob)
        cat "$ZIMAGE" "$DTB" > "$OUTPUT"

        ZIMAGE_SIZE=$(stat -c%s "$ZIMAGE" 2>/dev/null || wc -c < "$ZIMAGE")
        DTB_SIZE=$(stat -c%s "$DTB" 2>/dev/null || wc -c < "$DTB")
        TOTAL_SIZE=$(stat -c%s "$OUTPUT" 2>/dev/null || wc -c < "$OUTPUT")

        echo ""
        echo "=== Build complete ==="
        echo "  zImage:     $ZIMAGE ($ZIMAGE_SIZE bytes)"
        echo "  DTB:        $DTB ($DTB_SIZE bytes)"
        echo "  zImage-dtb: $OUTPUT ($TOTAL_SIZE bytes)"

        if [ "$TOTAL_SIZE" -gt 4194304 ]; then
            echo "  WARNING: zImage-dtb exceeds 4 MB"
        fi
        ;;

    *)
        echo "Usage: $0 [defconfig|menuconfig|clean|modules|build]"
        exit 1
        ;;
esac
