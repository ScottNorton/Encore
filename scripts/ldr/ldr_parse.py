#!/usr/bin/env python3
"""ADSP-21489 .ldr file block parser.

Parses the block stream following the 1536-byte boot kernel in a SHARC .ldr file.
Block headers are 3 x LE 32-bit words: (type, count, address).

Data size formulas (empirically validated, byte-exact to EOF):
  Type 0  (FINAL):       1536 bytes (384 x 32-bit DMA, hardcoded in boot kernel)
  Type 1  (ZERO_DM32):   0 bytes
  Type 2  (ZERO_DM48):   0 bytes
  Type 3  (LOAD_DM32):   ceil(count/2) * 4 bytes
  Type 4  (LOAD_DM8):    count * 4 bytes
  Type 5  (LOAD_PM48):   ceil(count/2) * 12 bytes
  Type 6  (LOAD_PM_PX):  count * 8 bytes (estimated, not seen in test file)
  Type 7  (alias->1):    0 bytes
  Type 8  (alias->1):    0 bytes
  Type 9  (alias->4):    count * 4 bytes
  Type 10 (alias->4):    count * 4 bytes

Usage:
  python scripts/ldr_parse.py <dsp-img.ldr>          # summary
  python scripts/ldr_parse.py <dsp-img.ldr> --all     # all blocks
  python scripts/ldr_parse.py <dsp-img.ldr> --disasm   # disassemble PM blocks
"""
import struct, sys, math, argparse

BOOT_KERNEL_SIZE = 256 * 6  # 1536 bytes
FINAL_DATA_SIZE = 384 * 4   # 1536 bytes (hardcoded DMA count in boot kernel)

TYPE_NAMES = {
    0: "FINAL", 1: "ZERO_DM32", 2: "ZERO_DM48", 3: "LOAD_DM32",
    4: "LOAD_DM8", 5: "LOAD_PM48", 6: "LOAD_PM_PX",
    7: "ZERO_DM32a", 8: "ZERO_DM32b", 9: "LOAD_DM8a", 10: "LOAD_DM8b",
}


def data_size(typ, count):
    """Compute data payload size in bytes for a given block type and count."""
    eff = typ
    if typ in (7, 8): eff = 1
    elif typ in (9, 10): eff = 4

    if eff == 0:   return FINAL_DATA_SIZE
    if eff in (1, 2): return 0
    if eff == 3:   return math.ceil(count / 2) * 4
    if eff == 4:   return count * 4
    if eff == 5:   return math.ceil(count / 2) * 12
    if eff == 6:   return count * 8  # estimated
    return None


def parse_ldr(data):
    """Parse .ldr file into list of block dicts."""
    offset = BOOT_KERNEL_SIZE
    blocks = []

    while offset + 12 <= len(data):
        w0, w1, w2 = struct.unpack_from('<III', data, offset)
        typ, count, addr = w0, w1, w2

        if typ > 10:
            break

        ds = data_size(typ, count)
        if ds is None:
            break

        blocks.append({
            'num': len(blocks),
            'offset': offset,
            'type': typ,
            'count': count,
            'addr': addr,
            'data_size': ds,
            'data_offset': offset + 12,
        })

        offset += 12 + ds

    return blocks, offset


def main():
    parser = argparse.ArgumentParser(description='ADSP-21489 .ldr block parser')
    parser.add_argument('ldr_file', help='Path to dsp-img.ldr')
    parser.add_argument('--all', action='store_true', help='Show all blocks')
    parser.add_argument('--disasm', action='store_true', help='Disassemble PM code blocks')
    args = parser.parse_args()

    with open(args.ldr_file, 'rb') as f:
        data = f.read()

    blocks, consumed = parse_ldr(data)
    pct = consumed / len(data) * 100

    print(f"File: {args.ldr_file} ({len(data)} bytes)")
    print(f"Boot kernel: {BOOT_KERNEL_SIZE} bytes (256 x 48-bit instructions)")
    print(f"Parsed: {len(blocks)} blocks, {consumed}/{len(data)} bytes ({pct:.1f}%)")
    if consumed != len(data):
        print(f"  WARNING: {len(data) - consumed} bytes unparsed!")
    print()

    # Per-type summary
    by_type = {}
    for b in blocks:
        by_type.setdefault(b['type'], []).append(b)

    print("Block type summary:")
    print(f"  {'Type':>4s} {'Name':>12s} {'Blocks':>6s} {'Headers':>8s} {'Data':>8s} {'Total':>8s}")
    print("  " + "-" * 52)
    total_hdr = 0
    total_data = 0
    for t in sorted(by_type.keys()):
        blist = by_type[t]
        name = TYPE_NAMES.get(t, f"TYPE_{t}")
        hdr_bytes = len(blist) * 12
        dat_bytes = sum(b['data_size'] for b in blist)
        total_hdr += hdr_bytes
        total_data += dat_bytes
        print(f"  {t:4d} {name:>12s} {len(blist):6d} {hdr_bytes:8d} {dat_bytes:8d} {hdr_bytes+dat_bytes:8d}")
    print("  " + "-" * 52)
    print(f"  {'':>4s} {'TOTAL':>12s} {len(blocks):6d} {total_hdr:8d} {total_data:8d} "
          f"{total_hdr+total_data:8d}")
    print()

    # Memory map — group contiguous regions
    print("Memory regions loaded:")
    for space, types in [("PM", (2, 5, 6)), ("DM", (1, 3, 4, 7, 8, 9, 10))]:
        relevant = [b for b in blocks if b['type'] in types and b['count'] > 0]
        if not relevant:
            continue
        # Group by high nibble of address to show separate regions
        regions = {}
        for b in relevant:
            key = b['addr'] >> 16
            regions.setdefault(key, []).append(b)
        for key in sorted(regions.keys()):
            rblocks = regions[key]
            lo = min(b['addr'] for b in rblocks)
            hi = max(b['addr'] + b['count'] - 1 for b in rblocks)
            total = sum(b['count'] for b in rblocks)
            load_count = sum(1 for b in rblocks if b['type'] not in (1, 2, 7, 8))
            zero_count = sum(1 for b in rblocks if b['type'] in (1, 2, 7, 8))
            print(f"  {space}: {lo:#08x} - {hi:#08x} ({total:6d} words, "
                  f"{load_count} loads + {zero_count} fills)")

    # FINAL blocks (boot kernel re-entry points)
    final_blocks = [b for b in blocks if b['type'] == 0]
    if final_blocks:
        print(f"\nFINAL blocks ({len(final_blocks)} re-entry points):")
        for b in final_blocks:
            print(f"  Block {b['num']:3d} @ offset 0x{b['offset']:05X} "
                  f"(DMA 384 words to PM 0x92000)")

    # All blocks table
    if args.all:
        print(f"\n{'='*80}")
        print(f"{'#':>4s} {'Offset':>8s} {'Type':>4s} {'Name':>12s} {'Count':>7s} "
              f"{'Address':>10s} {'DataSz':>7s}")
        print("-" * 80)
        for b in blocks:
            name = TYPE_NAMES.get(b['type'], f"T{b['type']}")
            print(f"{b['num']:4d} {b['offset']:#08x}   {b['type']:2d} {name:>12s} "
                  f"{b['count']:7d} {b['addr']:#010x} {b['data_size']:7d}")

    # Disassemble PM blocks
    if args.disasm:
        print(f"\n{'='*80}")
        print("PM CODE BLOCKS (48-bit instructions)")
        print("=" * 80)
        for b in blocks:
            if b['type'] not in (5, 6):
                continue
            print(f"\n--- Block {b['num']}: {b['count']} words @ PM {b['addr']:#08x} ---")
            payload = data[b['data_offset']:b['data_offset'] + b['data_size']]
            # Decode 48-bit words from payload
            # Type 5: _spi_poll reads 3x32-bit words per pair of PM words
            # Data is stored as ceil(count/2) groups of 12 bytes (3 words each)
            # Each group of 3 words encodes 2 PM words via DM-to-PX transfer
            addr = b['addr']
            for i in range(0, len(payload), 12):
                if i + 12 > len(payload):
                    break
                w0, w1, w2 = struct.unpack_from('<III', payload, i)
                # The 3 words from SPI are stored at DM 0x92003-0x92005
                # Then PX = DM(0x8C002) and PX = DM(0x8C003) read them
                # This is due to SHARC memory aliasing in Block 0 SRAM
                # For now, just show the raw words
                print(f"  {addr:#08x}: [{w0:#010x} {w1:#010x} {w2:#010x}]")
                addr += 2  # 2 PM words per group


if __name__ == '__main__':
    main()
