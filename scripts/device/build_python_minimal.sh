#!/bin/bash
# Build a minimal Python 3.10 package for ARMv7 (HK Invoke).
# Run in WSL: bash /mnt/g/HKInvoke/scripts/device/build_python_minimal.sh
set -euo pipefail

BUILD=/home/scott/python-armv7-build
mkdir -p "$BUILD"

# Download
TAR="$BUILD/python-armv7.tar.gz"
URL="https://github.com/astral-sh/python-build-standalone/releases/download/20260211/cpython-3.10.19+20260211-armv7-unknown-linux-gnueabihf-install_only_stripped.tar.gz"

if [ ! -f "$TAR" ] || [ "$(stat -c%s "$TAR")" -lt 1000000 ]; then
    echo "Downloading Python 3.10 for ARMv7..."
    wget -q --show-progress -O "$TAR" "$URL"
fi
echo "Tarball: $(du -sh "$TAR" | cut -f1)"

# Extract full
FULL="$BUILD/full"
if [ ! -d "$FULL/python/bin" ]; then
    echo "Extracting..."
    rm -rf "$FULL"
    mkdir -p "$FULL"
    tar xzf "$TAR" -C "$FULL"
fi
echo "Full: $(du -sh "$FULL" | cut -f1)"
file "$FULL/python/bin/python3.10"

# Build minimal
echo ""
echo "Building minimal package..."
MIN="$BUILD/minimal"
rm -rf "$MIN"
SRC="$FULL/python"

mkdir -p "$MIN/bin"
mkdir -p "$MIN/lib/python3.10/lib-dynload"
mkdir -p "$MIN/lib/python3.10/encodings"
mkdir -p "$MIN/lib/python3.10/collections"
mkdir -p "$MIN/lib/python3.10/importlib"

# Binary
cp "$SRC/bin/python3.10" "$MIN/bin/python3.10"
ln -sf python3.10 "$MIN/bin/python3"

# Shared libs (libpython, etc.)
for lib in "$SRC"/lib/lib*.so*; do
    [ -e "$lib" ] && cp -a "$lib" "$MIN/lib/" || true
done

# Essential stdlib .py files
S="$SRC/lib/python3.10"
D="$MIN/lib/python3.10"

for f in os.py posixpath.py stat.py genericpath.py \
    _collections_abc.py abc.py io.py \
    site.py _sitebuiltins.py \
    struct.py socket.py selectors.py \
    subprocess.py signal.py threading.py \
    _threading_local.py contextlib.py \
    functools.py operator.py keyword.py \
    types.py enum.py \
    sre_compile.py sre_parse.py sre_constants.py \
    re.py copyreg.py reprlib.py warnings.py \
    weakref.py _weakrefset.py \
    codecs.py traceback.py linecache.py \
    tokenize.py token.py; do
    [ -f "$S/$f" ] && cp "$S/$f" "$D/$f"
done

for f in encodings/__init__.py encodings/aliases.py \
    encodings/utf_8.py encodings/ascii.py encodings/latin_1.py \
    collections/__init__.py collections/abc.py \
    importlib/__init__.py importlib/abc.py \
    importlib/_bootstrap.py importlib/_bootstrap_external.py \
    importlib/machinery.py importlib/util.py; do
    [ -f "$S/$f" ] && cp "$S/$f" "$D/$f"
done

# C extensions we need
for mod in fcntl _socket _struct select _posixsubprocess; do
    found=$(find "$S/lib-dynload/" -name "${mod}*.so" 2>/dev/null | head -1)
    [ -n "$found" ] && cp "$found" "$D/lib-dynload/"
done

echo "Minimal: $(du -sh "$MIN" | cut -f1)"
echo "Files: $(find "$MIN" -type f | wc -l)"
ls -la "$D/lib-dynload/"

# Tarball
cd "$MIN"
tar czf "$BUILD/python-minimal.tar.gz" .
echo "Upload tarball: $(du -sh "$BUILD/python-minimal.tar.gz" | cut -f1)"
echo ""
echo "BUILD DONE"
