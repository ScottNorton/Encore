#!/bin/bash
set -euo pipefail
. "$(dirname "$0")/../common.sh"
SSH="$ENCORE_SSH"

# Upload a diagnostic script and run it
$SSH 'cat > /tmp/diag.sh' << 'SCRIPT'
#!/bin/sh
echo "=== I2C adapters ==="
ls /sys/class/i2c-adapter/

echo ""
echo "=== I2C-0 name ==="
cat /sys/class/i2c-adapter/i2c-0/name 2>/dev/null || echo "N/A"

echo ""
echo "=== All I2C adapter names ==="
for d in /sys/class/i2c-adapter/i2c-*; do
    name=$(cat $d/name 2>/dev/null || echo "?")
    echo "  $(basename $d): $name"
done

echo ""
echo "=== I2C device nodes ==="
ls -la /dev/i2c*

echo ""
echo "=== dmesg i2c/twsi ==="
dmesg | grep -iE 'i2c|twsi|designware|synopsys' | head -30

echo ""
echo "=== /dev/i2c-0 symlink target ==="
readlink -f /sys/class/i2c-adapter/i2c-0 2>/dev/null || echo "N/A"
SCRIPT

$SSH 'chmod +x /tmp/diag.sh; /tmp/diag.sh'
