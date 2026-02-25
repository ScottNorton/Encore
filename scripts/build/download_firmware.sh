#!/bin/bash
set -euo pipefail

# =================================================================
# Download and verify stock Harman Kardon Invoke firmware (83_IMAGE)
#
# Downloads the official OTA2 package from archive.org (Harman's
# final public firmware update), extracts 83_IMAGE, and verifies
# its integrity via SHA256.
#
# The stock 83_IMAGE contains the encrypted kernel, bootloader,
# and original rootfs. We need it as a base for building custom
# firmware since the kernel is OTP-encrypted and cannot be replaced.
#
# Usage:
#   bash scripts/build/download_firmware.sh
# =================================================================

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT="$REPO_ROOT/firmware/83_IMAGE_stock"

# Harman's final public firmware update (archive.org mirror)
OTA2_URL="https://archive.org/download/harman.-kardon.-invoke.-driver.-ota-2/Harman.Kardon.INVOKE.Driver.OTA2.zip"
INNER_PATH="OTA2/83_IMAGE"

# Verification
EXPECTED_SIZE=69481562
EXPECTED_SHA256="b2e12178f98a0c0904cb1e6e2ba933de0c0fef8be7c24e7852bc9933294850e8"

echo '=== Harman Kardon Invoke — Stock Firmware Download ==='

# Skip if already present and valid
if [ -f "$OUTPUT" ]; then
    ACTUAL_SIZE=$(wc -c < "$OUTPUT" | tr -d ' ')
    if [ "$ACTUAL_SIZE" -eq "$EXPECTED_SIZE" ]; then
        ACTUAL_SHA=$(sha256sum "$OUTPUT" | cut -d' ' -f1)
        if [ "$ACTUAL_SHA" = "$EXPECTED_SHA256" ]; then
            echo "Stock image already exists and verified: $OUTPUT"
            exit 0
        fi
    fi
    echo "WARNING: Existing image failed verification, re-downloading..."
    rm -f "$OUTPUT"
fi

mkdir -p "$REPO_ROOT/firmware"

TMPZIP=$(mktemp "${TMPDIR:-/tmp}/ota2_XXXXXX.zip")
trap 'rm -f "$TMPZIP"' EXIT

echo "Downloading OTA2 package (~214 MB)..."
echo "  URL: $OTA2_URL"
curl -fSL --progress-bar -o "$TMPZIP" "$OTA2_URL"

echo "Extracting $INNER_PATH..."
unzip -jo "$TMPZIP" "$INNER_PATH" -d "$REPO_ROOT/firmware/"

# unzip strips the directory prefix, so the file lands as 83_IMAGE
EXTRACTED="$REPO_ROOT/firmware/83_IMAGE"
if [ -f "$EXTRACTED" ] && [ "$EXTRACTED" != "$OUTPUT" ]; then
    mv "$EXTRACTED" "$OUTPUT"
fi

echo "Verifying..."
ACTUAL_SIZE=$(wc -c < "$OUTPUT" | tr -d ' ')
if [ "$ACTUAL_SIZE" -ne "$EXPECTED_SIZE" ]; then
    echo "ERROR: Size mismatch ($ACTUAL_SIZE != $EXPECTED_SIZE)"
    rm -f "$OUTPUT"
    exit 1
fi

ACTUAL_SHA=$(sha256sum "$OUTPUT" | cut -d' ' -f1)
if [ "$ACTUAL_SHA" != "$EXPECTED_SHA256" ]; then
    echo "ERROR: SHA256 mismatch"
    echo "  Expected: $EXPECTED_SHA256"
    echo "  Got:      $ACTUAL_SHA"
    rm -f "$OUTPUT"
    exit 1
fi

echo "Verified: $OUTPUT ($ACTUAL_SIZE bytes, SHA256 OK)"
echo "Done!"
