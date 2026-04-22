#!/bin/bash
# Dump and parse U-Boot environment partitions from the HK Invoke.
# Run from WSL with device accessible via SSH.
#
# U-Boot env format: [4-byte CRC32] [key=value\0 key=value\0 ... \0\0]
# Each partition is 128KB (0x20000).
#
# Partitions:
#   mtd1  = pre-bootloader (1MB, env at offset 0)
#   mtd13 = cenv (common environment, 128KB)
#   mtd14 = senv (secure environment, 128KB)

set -euo pipefail
. "$(dirname "$0")/../common.sh"

DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@$DEVICE_IP"
OUTDIR="$(cd "$(dirname "$0")/../.." && pwd)/firmware/env_dumps"
mkdir -p "$OUTDIR"

echo "=== Dumping U-Boot environment partitions from $DEVICE_IP ==="

for mtd in 1 13 14; do
    name="mtd${mtd}"
    case $mtd in
        1)  label="pre-bootloader" ;;
        13) label="cenv" ;;
        14) label="senv" ;;
    esac

    echo ""
    echo "--- $name ($label) ---"
    outfile="$OUTDIR/${label}_mtd${mtd}.bin"

    # Dump raw partition (first 128KB = env size per fw_env.config)
    $SSH "dd if=/dev/mtd${mtd} bs=128k count=1 2>/dev/null" > "$outfile"
    size=$(stat -c%s "$outfile" 2>/dev/null || stat -f%z "$outfile" 2>/dev/null)
    echo "Dumped $size bytes to $outfile"

    # Parse: skip 4-byte CRC, print key=value pairs (null-separated)
    echo "Environment variables:"
    # Use python to parse (handles nulls properly)
    python3 -c "
import struct, sys
data = open('$outfile', 'rb').read()
if len(data) < 5:
    print('  (empty or too small)')
    sys.exit(0)
crc = struct.unpack('<I', data[:4])[0]
print(f'  CRC32: 0x{crc:08x}')
# Check for flags byte (redundant env): if byte 4 looks like a flag (0x00 or 0x01)
# and byte 5 is printable ASCII, there's a flags byte
env_start = 4
if len(data) > 5 and data[4] in (0, 1) and 0x20 <= data[5] < 0x7f:
    env_start = 5
    print(f'  Flags: 0x{data[4]:02x}')
env = data[env_start:]
# Find double-null terminator
end = env.find(b'\x00\x00')
if end < 0:
    end = len(env)
env = env[:end]
pairs = env.split(b'\x00')
count = 0
for pair in pairs:
    if not pair:
        continue
    try:
        text = pair.decode('ascii', errors='replace')
        print(f'  {text}')
        count += 1
    except:
        print(f'  (binary: {pair[:40].hex()}...)')
        count += 1
print(f'  --- {count} variables ---')
" 2>&1 || echo "  (parse failed, check raw dump with: hexdump -C $outfile | head -40)"

    echo ""
done

echo "=== Done. Raw dumps in $OUTDIR ==="
echo "To examine manually: hexdump -C $OUTDIR/cenv_mtd13.bin | head -80"
