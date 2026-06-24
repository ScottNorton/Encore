#!/bin/bash
# scripts/build/build_encore.sh — Build Encore ARM firmware binary
#
# Works natively on Windows (Git Bash) with cargo-zigbuild + zig,
# or in WSL with arm-linux-gnueabihf-gcc.
#
# The web dashboard is a WASM app built by 'make wasm' (wasm-pack).
# The Makefile's encore target depends on wasm, so it runs automatically.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ENCORE_DIR="$REPO_DIR/encore"
TARGET="armv7-unknown-linux-musleabihf"

echo "=== Building Encore firmware ==="

# Ensure target is installed
rustup target add "$TARGET" 2>/dev/null || true

cd "$ENCORE_DIR"

# ── Stamp service worker version (before cargo build so rust-embed picks it up) ──
BUILD_VERSION="${BUILD_VERSION:-$(date +%s 2>/dev/null || echo "0")}"
SW_STAMPED=false
ORIGINAL_VERSION=""
if [ -f "web/sw.js" ]; then
    # Portable extraction — Git Bash grep lacks -P/PCRE, which silently fell back
    # to "0" and reset the committed base version on every build (dirtying the tree).
    ORIGINAL_VERSION=$(sed -n "s/.*CACHE_VERSION = '\([^']*\)'.*/\1/p" "web/sw.js" | head -1)
    [ -z "$ORIGINAL_VERSION" ] && ORIGINAL_VERSION="0"
    sed -i "s/CACHE_VERSION = '[^']*'/CACHE_VERSION = '$BUILD_VERSION'/" "web/sw.js"
    SW_STAMPED=true
    echo "=== Stamped sw.js with version $BUILD_VERSION ==="
fi

# Ensure sw.js version is restored even if build fails
restore_sw() {
    if [ "$SW_STAMPED" = true ] && [ -f "web/sw.js" ]; then
        sed -i "s/CACHE_VERSION = '[^']*'/CACHE_VERSION = '$ORIGINAL_VERSION'/" "web/sw.js"
    fi
}
trap restore_sw EXIT

# ── Build firmware (embeds web/ via rust-embed) ──
echo "=== Building firmware binary ==="

if command -v cargo-zigbuild &>/dev/null; then
    # Native build with cargo-zigbuild (Windows or Linux)
    # Zig bundles a cross-compiler — no GCC sysroot needed
    cargo zigbuild --package encore-firmware --target "$TARGET" --release
else
    # Fallback: plain cargo build (requires arm-linux-gnueabihf-gcc in WSL)
    cargo build --package encore-firmware --target "$TARGET" --release
fi

BINARY="$ENCORE_DIR/target/$TARGET/release/encore-firmware"

# ── Strip binary ──
# Try native tools first, then WSL as fallback (Windows has no cross-strip)
if command -v llvm-strip &>/dev/null; then
    llvm-strip "$BINARY"
elif command -v arm-linux-gnueabihf-strip &>/dev/null; then
    arm-linux-gnueabihf-strip "$BINARY"
elif command -v wsl.exe &>/dev/null; then
    WSL_PATH=$(wsl.exe -d Ubuntu -- bash -c "wslpath '$BINARY'" 2>/dev/null)
    if [ -n "$WSL_PATH" ]; then
        if ! wsl.exe -d Ubuntu -- bash -c "llvm-strip '$WSL_PATH' 2>/dev/null || arm-linux-gnueabihf-strip '$WSL_PATH' 2>/dev/null || strip '$WSL_PATH' 2>/dev/null"; then
            echo "NOTE: strip unavailable, shipping unstripped binary"
        fi
    fi
else
    echo "=== No strip tool found, binary unstripped ==="
fi

SIZE=$(stat -c%s "$BINARY" 2>/dev/null || stat -f%z "$BINARY" 2>/dev/null || wc -c < "$BINARY")
echo "=== Built: $BINARY ($SIZE bytes) ==="

# Copy to build output
mkdir -p "$REPO_DIR/build"
cp "$BINARY" "$REPO_DIR/build/encore"
chmod 755 "$REPO_DIR/build/encore"
echo "=== Installed to build/encore ==="
