#!/bin/bash
set -euo pipefail

# =================================================================
# Harman Kardon Invoke Community Firmware — Build Script
#
# Builds a custom firmware image from a stock 83_IMAGE by:
#   1. Extracting the stock rootfs (SquashFS)
#   2. Removing bloat and proprietary binaries
#   3. Applying the rootfs overlay (Encore binary, scripts, web UI)
#   4. Building a new SquashFS and packaging into 83_IMAGE
#
# Prerequisites:
#   - Stock 83_IMAGE in firmware/ (USB-boot files live in uboot/)
#   - Encore binary built (run: make encore)
#
# Usage:
#   ./scripts/build/build_firmware.sh [options]
#
# Options:
#   --work DIR          Working directory (default: ~/hkinvoke-build)
# =================================================================

# ── Auto-detect repo root ──
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ── Configurable paths (override via env or CLI) ──
WORK="${WORK:-${HOME:-/root}/hkinvoke-build}"
STOCK_IMG="${STOCK_IMG:-$REPO_ROOT/firmware/83_IMAGE_stock}"

# ── Parse CLI args ──
while [ $# -gt 0 ]; do
    case "$1" in
        --work)
            [ -z "${2:-}" ] && echo "ERROR: --work requires a value" >&2 && exit 1
            WORK="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

HYBRID="$WORK/hybrid_rootfs"
OUTPUT_SQFS="$WORK/rootfs.squashfs"
BUILD_DIR="$REPO_ROOT/build"
ENCORE_BIN="$BUILD_DIR/encore"

echo '============================================'
echo '  Harman Kardon Invoke Community Firmware Builder'
echo '============================================'
echo "  Repo:     $REPO_ROOT"
echo "  Work:     $WORK"
echo "  Stock:    $STOCK_IMG"
echo "  Encore:   $ENCORE_BIN"
echo ''

# ── Preflight checks ──
[ ! -f "$STOCK_IMG" ] && echo "ERROR: Stock image not found: $STOCK_IMG" && exit 1

mkdir -p "$WORK" "$BUILD_DIR/firmware"

# =================================================================
extract_stock_rootfs() {
    echo '=== Extracting stock rootfs ==='
    python3 "$REPO_ROOT/scripts/build/extract_rootfs.py" "$STOCK_IMG" "$WORK/stock.squashfs"
    rm -rf "$HYBRID"
    unsquashfs -d "$HYBRID" "$WORK/stock.squashfs" 2>&1 | tail -3
    rm "$WORK/stock.squashfs"
}

# =================================================================
remove_bloat() {
    echo ''
    echo '=== Removing bloat ==='
    du -sh "$HYBRID" | awk '{print "Before: "$1}'

    # Content and docs
    rm -rf "$HYBRID/usr/share/sounds"
    rm -rf "$HYBRID/usr/share/cortana"
    rm -rf "$HYBRID/usr/include"
    rm -rf "$HYBRID/usr/share/dpkg" "$HYBRID/usr/share/man" "$HYBRID/usr/share/info"
    rm -rf "$HYBRID/usr/share/dict" "$HYBRID/usr/share/fonts"
    rm -rf "$HYBRID/usr/games" "$HYBRID/usr/src"

    # Python 2.7 — stock firmware never uses it (all Harman services are C/C++)
    rm -rf "$HYBRID/usr/lib/python2.7"
    rm -f  "$HYBRID/usr/bin/python"*
    rm -f  "$HYBRID/usr/lib/libpython2.7.so"*

    # Perl — unused
    rm -rf "$HYBRID/usr/lib/perl"

    # Stock dnsmasq DNS cacher — listens on 127.0.0.1:53 with empty config,
    # serves no purpose. Encore's AP dnsmasq on 192.168.43.1 is separate.
    rm -f "$HYBRID/etc/init.d/dnsmasq"
    rm -f "$HYBRID/etc/rc"*".d/"[SK]"20dnsmasq"
    rm -f "$HYBRID/etc/dnsmasq.conf" "$HYBRID/etc/dnsmasq-default.conf"
    rm -f "$HYBRID/etc/dnsmasq-resolv.conf" "$HYBRID/etc/dnsmasq-hosts"

    du -sh "$HYBRID" | awk '{print "After bloat removal: "$1}'
}

# =================================================================
remove_harman_binaries() {
    echo ''
    echo '=== Removing Harman proprietary binaries ==='

    # /usr/bin/ — Harman service binaries (ALL replaced by Encore)
    for bin in dsp-client mcu-interface system-manager audio-ui cortana \
               cortana-harness spotify music-source-manager connection-manager \
               bluetooth factory-test engine client; do
        rm -f "$HYBRID/usr/bin/$bin"
    done
    echo '  Removed Harman service binaries from /usr/bin/'

    # LUCI (Microsoft cloud)
    rm -f "$HYBRID/system/bin/luci_service"
    rm -f "$HYBRID/usr/lib/libluci_helper.so"

    # WAMP infrastructure
    rm -f "$HYBRID/usr/bin/bonefish"
    rm -f "$HYBRID/usr/lib/libwamp_framework.so"

    # Boost libraries (only needed by Harman C++ binaries)
    rm -f "$HYBRID/usr/lib/libboost_system.so"*
    rm -f "$HYBRID/usr/lib/libboost_thread.so"*
    rm -f "$HYBRID/usr/lib/libboost_filesystem.so"*
    rm -f "$HYBRID/usr/lib/libboost_program_options.so"*

    # Spotify embedded SDK (replaced by open-source librespot in Encore)
    rm -f "$HYBRID/usr/lib/libspotify_embedded_shared.so"

    # Crash uploaders and factory scripts
    rm -f "$HYBRID/usr/bin/crash-uploader-HK.sh"
    rm -f "$HYBRID/usr/bin/crash-uploader-MS.sh"
    rm -f "$HYBRID/usr/bin/device_auto_recovery.sh"

    du -sh "$HYBRID" | awk '{print "After Harman removal: "$1}'
}

# =================================================================
apply_overlay() {
    echo ''
    echo '=== Applying rootfs overlay ==='

    # TODO: Marvell WLAN SDK config files currently ship in
    # rootfs/usr/share/factory/misc_config/. They live on a separate stock
    # NAND partition (not in 83_IMAGE) and should ideally be extracted from
    # the user's own stock image at build time rather than distributed
    # in-tree. See rootfs/usr/share/factory/misc_config/README.md.

    # Copy all overlay files
    cp -a "$REPO_ROOT/rootfs/"* "$HYBRID/"

    # Strip CRLF from shell scripts (Windows editors create \r\n, BusyBox chokes)
    find "$HYBRID/sbin" "$HYBRID/etc/init.d" "$HYBRID/usr/bin" \
        -type f \( -name '*.sh' -o -name 'S[0-9]*' \) \
        -exec sed -i 's/\r$//' {} +
    echo '  CRLF stripped from shell scripts'

    # Set permissions — init scripts
    chmod 755 "$HYBRID/etc/init.d/S00audiomute" "$HYBRID/etc/init.d/S01firewall" \
              "$HYBRID/etc/init.d/S02bootlog"

    # Set permissions — boot scripts
    chmod 755 "$HYBRID/sbin/mount_partition.sh" "$HYBRID/sbin/auto_wifi_firewall.sh" \
              "$HYBRID/sbin/start_ap.sh" "$HYBRID/sbin/encore_supervisor.sh"

    # USB RNDIS gadget scripts (modules ship in usr/lib/usbgadget/)
    chmod 755 "$HYBRID/sbin/usb_gadget.sh" "$HYBRID/sbin/usb_gadget_monitor.sh" 2>/dev/null || true

    # Install Encore binary from build output
    if [ -f "$ENCORE_BIN" ]; then
        # Cheap ELF magic check: first 4 bytes must be 7f 45 4c 46
        MAGIC=$(od -An -tx1 -N4 "$ENCORE_BIN" | tr -d ' \n')
        [ "$MAGIC" != "7f454c46" ] && echo "ERROR: build/encore is not an ELF binary" >&2 && exit 1
        cp "$ENCORE_BIN" "$HYBRID/usr/bin/encore"
        chmod 755 "$HYBRID/usr/bin/encore"
        echo "  Installed Encore binary from build/encore"
    elif [ -f "$HYBRID/usr/bin/encore" ]; then
        chmod 755 "$HYBRID/usr/bin/encore"
        echo "  WARNING: Using Encore binary from rootfs overlay (build/encore not found)"
    else
        echo "ERROR: No Encore binary found at $ENCORE_BIN — run 'make encore' first" >&2
        exit 1
    fi

    # TLS fallback pair. Created in build/tls/ (git-ignored) the first time and
    # kept after that, so it can be replaced with your own. The firmware loads it
    # from /usr/share/encore/tls only if it cannot make its own per-device
    # certificate. See "TLS fallback pair" in docs/build-guide.md.
    TLS_SRC="${TLS_DIR:-$BUILD_DIR/tls}"
    bash "$REPO_ROOT/scripts/build/gen_tls_fallback.sh"
    install -d -m 755 "$HYBRID/usr/share/encore/tls"
    install -m 644 "$TLS_SRC/cert.pem" "$HYBRID/usr/share/encore/tls/cert.pem"
    install -m 600 "$TLS_SRC/key.pem" "$HYBRID/usr/share/encore/tls/key.pem"
    echo "  Installed TLS fallback pair from $TLS_SRC"

    # Set permissions — binaries
    chmod 755 "$HYBRID/usr/bin/run-podium.sh" "$HYBRID/usr/bin/i2c_mute"

    # Configure ALSA resampler (44.1kHz → 48kHz for Spotify)
    if ! grep -q 'speexrate_best' "$HYBRID/etc/asound.conf" 2>/dev/null; then
        echo 'defaults.pcm.rate_converter "speexrate_best"' >> "$HYBRID/etc/asound.conf"
    fi

    echo '  Overlay applied'
}

# =================================================================
set_root_password() {
    echo ''
    echo '=== Setting root password ==='
    if grep -q '^root:\*:' "$HYBRID/etc/shadow" 2>/dev/null; then
        sed -i 's|^root:\*:|root:$6$E0tZksKNC.U7Fond$b46C4HIgD3NTVgGJT4pSBeWkk8twlsyWf538.sYTNcZeK8lzWIJpI2nGZc/HA2gZdaJsw0HRp0Mv0El5.a4nn.:|' "$HYBRID/etc/shadow"
        echo '  Root password set (ridiculous)'
    else
        echo '  Password already set'
    fi
}

# =================================================================
fix_ownership() {
    echo ''
    echo '=== Fixing ownership ==='
    # Overlay files copied from Windows may have wrong UIDs
    chown root:root "$HYBRID/etc/init.d/S00audiomute" "$HYBRID/etc/init.d/S01firewall" \
                    "$HYBRID/etc/init.d/S02bootlog" 2>/dev/null || true
    chown root:root "$HYBRID/sbin/auto_wifi_firewall.sh" "$HYBRID/sbin/start_ap.sh" \
                    "$HYBRID/sbin/mount_partition.sh" "$HYBRID/sbin/encore_supervisor.sh" 2>/dev/null || true
    chown root:root "$HYBRID/usr/bin/run-podium.sh" "$HYBRID/usr/bin/i2c_mute" 2>/dev/null || true
    chown root:root "$HYBRID/usr/bin/encore" 2>/dev/null || true
    chown root:root "$HYBRID/sbin/usb_gadget.sh" "$HYBRID/sbin/usb_gadget_monitor.sh" 2>/dev/null || true
    chown -R root:root "$HYBRID/usr/lib/usbgadget" 2>/dev/null || true
    chown -R root:root "$HYBRID/usr/share/encore" 2>/dev/null || true
    chown root:root "$HYBRID/system/etc/wpa_supplicant.conf.in" 2>/dev/null || true
    echo '  Ownership fixed'
}

# =================================================================
build_squashfs() {
    echo ''
    echo '=== Building SquashFS ==='
    du -sh "$HYBRID" | awk '{print "Rootfs size (uncompressed): "$1}'
    rm -f "$OUTPUT_SQFS"
    # -all-root: force every file to root:root. This appliance runs everything as
    # root, and it lets the image build correctly without sudo/fakeroot (WSL builds
    # as a normal user, so preserved ownership would otherwise bake the build user).
    mksquashfs "$HYBRID" "$OUTPUT_SQFS" -comp gzip -b 131072 -noappend -all-root 2>&1 | tail -10

    echo ''
    python3 -c "
import struct, os
with open('$OUTPUT_SQFS', 'rb') as f:
    f.seek(24); flags = struct.unpack('<H', f.read(2))[0]
    f.seek(40); bu = struct.unpack('<Q', f.read(8))[0]
fsize = os.path.getsize('$OUTPUT_SQFS')
print(f'Flags: 0x{flags:04X} (need 0x00C0)')
print(f'SquashFS size: {fsize:,} bytes ({fsize/1024/1024:.1f} MB)')
print(f'Partition limit: 84,824,064 bytes (80.9 MB)')
if fsize > 84824064:
    print('ERROR: TOO LARGE!')
    exit(1)
print(f'Headroom: {(84824064-fsize):,} bytes ({(84824064-fsize)/1024/1024:.1f} MB)')
print('OK - fits!')
"
}

# =================================================================
package_image() {
    echo ''
    echo '=== Packaging into 83_IMAGE ==='
    python3 "$REPO_ROOT/scripts/build/package_rootfs.py" \
        "$OUTPUT_SQFS" "$BUILD_DIR/firmware/83_IMAGE" \
        --stock "$STOCK_IMG"

    # Keep SquashFS for web UI OTA flashing (flash_image rootfs <file>)
    cp "$OUTPUT_SQFS" "$BUILD_DIR/firmware/rootfs.squashfs"
    echo "  OTA image: build/firmware/rootfs.squashfs ($(wc -c < "$BUILD_DIR/firmware/rootfs.squashfs") bytes)"
}

# =================================================================
# Main build sequence
# =================================================================
extract_stock_rootfs
remove_bloat
remove_harman_binaries
apply_overlay
set_root_password
fix_ownership
build_squashfs
package_image

# Cleanup
rm -rf "$HYBRID" "$OUTPUT_SQFS"

IMGSIZE=$(wc -c < "$BUILD_DIR/firmware/83_IMAGE")
SQFSSIZE=$(wc -c < "$BUILD_DIR/firmware/rootfs.squashfs")

echo ''
echo '============================================'
echo '  BUILD COMPLETE'
echo '============================================'
echo ''
echo "USB boot:  build/firmware/83_IMAGE ($IMGSIZE bytes)"
echo "  Flash:   tftp2nand -d $IMGSIZE 0x7000000"
echo ''
echo "Web UI:    build/firmware/rootfs.squashfs ($SQFSSIZE bytes)"
echo "  Upload via web dashboard Update tab, or:"
echo "  scp build/firmware/rootfs.squashfs root@encore.local:/run/"
echo "  ssh root@encore.local flash_image rootfs /run/rootfs.squashfs"
echo ''
echo 'ALL CHECKS PASSED'
