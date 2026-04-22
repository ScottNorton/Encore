#!/bin/bash
# Deploy Python 3 + discovery scripts to the Invoke's /lsync partition.
#
# Run from WSL (Ubuntu):
#   bash /mnt/g/HKInvoke/scripts/device/deploy_python.sh [DEVICE_IP]
#
# After deployment, SSH to device and run:
#   /lsync/python/bin/python3 /lsync/scripts/mcu.py monitor
#   /lsync/python/bin/python3 /lsync/scripts/bt_discover.py load

set -euo pipefail
. "$(dirname "$0")/../common.sh"

DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@$DEVICE_IP"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Use home dir for build work (WSL /tmp is only 32MB tmpfs)
BUILD_DIR="$HOME/python-armv7-build"
PYTHON_URL="https://github.com/astral-sh/python-build-standalone/releases/download/20260211/cpython-3.10.19+20260211-armv7-unknown-linux-gnueabihf-install_only_stripped.tar.gz"
PYTHON_TAR="${BUILD_DIR}/python-armv7.tar.gz"

echo "=== HK Invoke Python Deployment ==="
echo "Device: ${DEVICE_IP}"
echo "Build dir: ${BUILD_DIR}"
echo ""

mkdir -p "${BUILD_DIR}"

# Step 1: Download if not cached
if [ ! -f "${PYTHON_TAR}" ]; then
    echo "Downloading Python 3.10 for ARMv7..."
    wget -q --show-progress -O "${PYTHON_TAR}" "${PYTHON_URL}"
else
    echo "Using cached ${PYTHON_TAR}"
fi

# Step 2: Extract full install
PYTHON_FULL="${BUILD_DIR}/full"
if [ ! -f "${PYTHON_FULL}/python/bin/python3" ]; then
    echo "Extracting full install..."
    rm -rf "${PYTHON_FULL}"
    mkdir -p "${PYTHON_FULL}"
    tar xzf "${PYTHON_TAR}" -C "${PYTHON_FULL}"
else
    echo "Using cached full extraction"
fi

echo "Python binary: $(file "${PYTHON_FULL}/python/bin/python3")"

# Step 3: Build minimal Python with only what our scripts need
echo ""
echo "Building minimal Python package..."
MINIMAL="${BUILD_DIR}/minimal"
rm -rf "${MINIMAL}"
mkdir -p "${MINIMAL}/bin" "${MINIMAL}/lib/python3.10/lib-dynload"

SRC="${PYTHON_FULL}/python"

# Binary
cp "${SRC}/bin/python3.10" "${MINIMAL}/bin/python3.10"
ln -sf python3.10 "${MINIMAL}/bin/python3"

# Shared libs the binary links against
if [ -d "${SRC}/lib" ]; then
    for lib in "${SRC}"/lib/lib*.so*; do
        [ -f "$lib" ] && cp -a "$lib" "${MINIMAL}/lib/" 2>/dev/null || true
    done
fi

# Essential stdlib .py files (what our scripts import)
STDLIB="${MINIMAL}/lib/python3.10"
for f in \
    os.py posixpath.py stat.py genericpath.py \
    _collections_abc.py abc.py io.py \
    site.py _sitebuiltins.py \
    struct.py socket.py selectors.py \
    subprocess.py signal.py threading.py \
    _threading_local.py contextlib.py \
    functools.py operator.py keyword.py \
    types.py enum.py sre_compile.py sre_parse.py sre_constants.py \
    re.py copyreg.py reprlib.py warnings.py \
    weakref.py _weakrefset.py \
    codecs.py encodings/__init__.py encodings/aliases.py \
    encodings/utf_8.py encodings/ascii.py encodings/latin_1.py \
    traceback.py linecache.py tokenize.py token.py \
    collections/__init__.py collections/abc.py \
    importlib/__init__.py importlib/abc.py importlib/_bootstrap.py \
    importlib/_bootstrap_external.py importlib/machinery.py importlib/util.py \
    ; do
    dir=$(dirname "$f")
    mkdir -p "${STDLIB}/${dir}"
    if [ -f "${SRC}/lib/python3.10/${f}" ]; then
        cp "${SRC}/lib/python3.10/${f}" "${STDLIB}/${f}"
    fi
done

# C extension modules our scripts need (fcntl, socket, struct, select)
for mod in fcntl _socket _struct select _posixsubprocess; do
    found=$(find "${SRC}/lib/python3.10/lib-dynload/" -name "${mod}*.so" 2>/dev/null | head -1)
    if [ -n "$found" ]; then
        cp "$found" "${STDLIB}/lib-dynload/"
    fi
done

echo "Minimal Python size: $(du -sh "${MINIMAL}" | cut -f1)"
echo "Files: $(find "${MINIMAL}" -type f | wc -l)"

# Step 4: Create upload tarball
UPLOAD_TAR="${BUILD_DIR}/python-minimal.tar.gz"
cd "${MINIMAL}"
tar czf "${UPLOAD_TAR}" .
echo "Upload tarball: $(du -sh "${UPLOAD_TAR}" | cut -f1)"

# Step 5: Verify device is reachable
echo ""
echo "Checking device..."
if ! ${SSH} "echo ok" 2>/dev/null; then
    echo "ERROR: Cannot reach device at ${DEVICE_IP}"
    exit 1
fi

${SSH} "/lib/libc.so.6 2>&1 | head -1 || true"
echo "Device /lsync space:"
${SSH} "df -h /lsync"

# Step 6: Upload Python
echo ""
echo "Uploading Python to device..."
${SSH} "mkdir -p /tmp/py"
cat "${UPLOAD_TAR}" | ${SSH} "cat > /tmp/py/py.tar.gz"
${SSH} "cd /tmp/py && tar xzf py.tar.gz && rm py.tar.gz"
${SSH} "rm -rf /lsync/python && cp -a /tmp/py /lsync/python && rm -rf /tmp/py"

# Test it
echo "Testing Python on device..."
${SSH} '/lsync/python/bin/python3 -c "import sys; print(\"Python \" + sys.version)"' || {
    echo ""
    echo "ERROR: Python binary failed to run!"
    echo "Likely glibc incompatibility. Check: /lib/libc.so.6"
    exit 1
}

# Test the modules we need
echo "Testing required modules..."
${SSH} '/lsync/python/bin/python3 -c "import os, fcntl, struct, socket, time; print(\"All modules OK\")"' || {
    echo "WARNING: Some modules failed to import. Check output above."
}

# Step 7: Upload scripts
echo ""
echo "Uploading discovery scripts..."
${SSH} "mkdir -p /lsync/scripts"

for script in mcu.py bt_discover.py; do
    sed 's/\r$//' "${SCRIPT_DIR}/${script}" | ${SSH} "cat > /lsync/scripts/${script}"
    ${SSH} "chmod +x /lsync/scripts/${script}"
done

# Convenience wrapper
${SSH} 'cat > /lsync/scripts/py << "EOF"
#!/bin/sh
exec /lsync/python/bin/python3 /lsync/scripts/"$@"
EOF
chmod +x /lsync/scripts/py'

echo ""
echo "=== Deployment Complete ==="
echo ""
echo "Usage (SSH to device):"
echo "  /lsync/scripts/py mcu.py monitor        # Watch button events"
echo "  /lsync/scripts/py mcu.py btled on        # BT LED on"
echo "  /lsync/scripts/py mcu.py led 0 50 100    # Set LEDs blue"
echo "  /lsync/scripts/py bt_discover.py load     # Load BT module"
echo "  /lsync/scripts/py bt_discover.py info     # Adapter info"
echo "  /lsync/scripts/py bt_discover.py up       # Power on + discoverable"
echo "  /lsync/scripts/py bt_discover.py scan     # Scan for devices"
echo "  /lsync/scripts/py bt_discover.py monitor  # Watch BT events"
