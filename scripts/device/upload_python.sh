#!/bin/bash
# Upload pre-built minimal Python + scripts to the Invoke.
# Run in WSL: bash /mnt/g/HKInvoke/scripts/device/upload_python.sh [IP]
set -euo pipefail
. "$(dirname "$0")/../common.sh"

DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@$DEVICE_IP"
BUILD="/home/scott/python-armv7-build"
SCRIPTS="/mnt/g/HKInvoke/scripts/device"
TAR="$BUILD/python-minimal.tar.gz"

if [ ! -f "$TAR" ]; then
    echo "ERROR: Build first: bash build_python_minimal.sh"
    exit 1
fi

echo "Tarball: $(du -sh "$TAR" | cut -f1)"

echo "Checking device..."
$SSH "echo ok" 2>/dev/null || { echo "Device unreachable at $DEVICE_IP"; exit 1; }
$SSH "df -h /lsync | tail -1"

echo "Uploading Python..."
$SSH "rm -rf /tmp/py; mkdir -p /tmp/py"
cat "$TAR" | $SSH "cat > /tmp/py/py.tar.gz"
echo "Extracting on device..."
$SSH "cd /tmp/py && tar xzf py.tar.gz && rm py.tar.gz"
echo "Moving to /lsync/python..."
$SSH "rm -rf /lsync/python; cp -a /tmp/py /lsync/python; rm -rf /tmp/py; sync"

echo "Testing Python..."
$SSH "/lsync/python/bin/python3 -c 'import sys; print(\"Python \" + sys.version)'" || {
    echo "FAILED! glibc too old?"
    exit 1
}
$SSH "/lsync/python/bin/python3 -c 'import os,fcntl,struct,socket,time; print(\"All modules OK\")'" || {
    echo "WARNING: Module import failed"
}

echo ""
echo "Uploading scripts..."
$SSH "mkdir -p /lsync/scripts"
for script in mcu.py bt_discover.py; do
    sed 's/\r$//' "$SCRIPTS/$script" | $SSH "cat > /lsync/scripts/$script; chmod +x /lsync/scripts/$script"
done

$SSH "cat > /lsync/scripts/py << 'PYEOF'
#!/bin/sh
exec /lsync/python/bin/python3 /lsync/scripts/\"\$@\"
PYEOF
chmod +x /lsync/scripts/py"

echo ""
echo "=== DONE ==="
echo "SSH in and try:"
echo "  /lsync/scripts/py mcu.py monitor"
echo "  /lsync/scripts/py mcu.py btled on"
echo "  /lsync/scripts/py bt_discover.py load"
