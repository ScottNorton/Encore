#!/usr/bin/env python3
"""Extract and disassemble all PM (Program Memory) code from DSP firmware.

Parses the .ldr file, extracts PM code from:
  1. Type 5 (LOAD_PM48) blocks: 3-word groups -> 2 PM48 words via PX register
  2. Type 0 (FINAL) block: 384 DM32 words DMA'd to PM 0x92000

DM32 -> PM48 packing (3 DM words -> 2 PM words):
  The 96-bit group (3 x 32-bit LE) splits into two 48-bit PM words.
  Two possible packings are tried; the one producing valid instructions wins.

Usage:
  python scripts/ldr_disasm_pm.py <dsp-img.ldr>
  python scripts/ldr_disasm_pm.py <dsp-img.ldr> --raw-words   # show raw hex too
"""
import struct, sys, math, argparse, os

# Add scripts dir to path so we can import from sharc_disasm
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sharc_disasm import disasm, collect_targets, generate_labels

BOOT_KERNEL_SIZE = 256 * 6  # 1536 bytes
FINAL_DATA_SIZE = 384 * 4   # 1536 bytes
FINAL_PM_ADDR = 0x92000


def data_size(typ, count):
    eff = typ
    if typ in (7, 8): eff = 1
    elif typ in (9, 10): eff = 4
    if eff == 0: return FINAL_DATA_SIZE
    if eff in (1, 2): return 0
    if eff == 3: return math.ceil(count / 2) * 4
    if eff == 4: return count * 4
    if eff == 5: return math.ceil(count / 2) * 12
    if eff == 6: return count * 8
    return None


def parse_ldr(data):
    offset = BOOT_KERNEL_SIZE
    blocks = []
    while offset + 12 <= len(data):
        w0, w1, w2 = struct.unpack_from('<III', data, offset)
        typ, count, addr = w0, w1, w2
        if typ > 10: break
        ds = data_size(typ, count)
        if ds is None: break
        blocks.append({
            'num': len(blocks), 'offset': offset, 'type': typ,
            'count': count, 'addr': addr, 'data_size': ds,
            'data_offset': offset + 12,
        })
        offset += 12 + ds
    return blocks


def unpack_pm48_A(payload, count):
    """Packing A: natural LE 96-bit split.
    pm0 = w0 | ((w1 & 0xFFFF) << 32)
    pm1 = (w1 >> 16) | (w2 << 16)
    """
    pm = []
    groups = math.ceil(count / 2)
    for g in range(groups):
        off = g * 12
        if off + 12 > len(payload):
            break
        w0, w1, w2 = struct.unpack_from('<III', payload, off)
        pm0 = w0 | ((w1 & 0xFFFF) << 32)
        pm.append(pm0)
        if len(pm) < count:
            pm1 = ((w1 >> 16) & 0xFFFF) | (w2 << 16)
            pm.append(pm1)
    return pm


def unpack_pm48_B(payload, count):
    """Packing B: reversed — first PM word in high bits.
    pm0 = (w0 << 16) | (w1 >> 16)
    pm1 = ((w1 & 0xFFFF) << 32) | w2
    """
    pm = []
    groups = math.ceil(count / 2)
    for g in range(groups):
        off = g * 12
        if off + 12 > len(payload):
            break
        w0, w1, w2 = struct.unpack_from('<III', payload, off)
        pm0 = (w0 << 16) | ((w1 >> 16) & 0xFFFF)
        pm.append(pm0)
        if len(pm) < count:
            pm1 = ((w1 & 0xFFFF) << 32) | w2
            pm.append(pm1)
    return pm


def unpack_pm48_C(payload, count):
    """Packing C: treat as 96 bits BE.
    Raw bytes from 3 LE words: b0..b3 | b4..b7 | b8..b11
    As 12 bytes LE: [b0,b1,...,b11]
    PM0 = bytes[0:6] as 48-bit LE, PM1 = bytes[6:12] as 48-bit LE.
    """
    pm = []
    groups = math.ceil(count / 2)
    for g in range(groups):
        off = g * 12
        if off + 12 > len(payload):
            break
        raw = payload[off:off+12]
        # PM0 from first 6 bytes, little-endian
        pm0 = 0
        for i in range(6):
            pm0 |= raw[i] << (i * 8)
        pm.append(pm0)
        if len(pm) < count:
            pm1 = 0
            for i in range(6):
                pm1 |= raw[6+i] << (i * 8)
            pm.append(pm1)
    return pm


def unpack_pm48_D(payload, count):
    """Packing D: 6 bytes each, big-endian.
    PM0 = bytes[0:6] as 48-bit BE, PM1 = bytes[6:12] as 48-bit BE.
    """
    pm = []
    groups = math.ceil(count / 2)
    for g in range(groups):
        off = g * 12
        if off + 12 > len(payload):
            break
        raw = payload[off:off+12]
        pm0 = 0
        for i in range(6):
            pm0 = (pm0 << 8) | raw[i]
        pm.append(pm0)
        if len(pm) < count:
            pm1 = 0
            for i in range(6):
                pm1 = (pm1 << 8) | raw[6+i]
            pm.append(pm1)
    return pm


def score_packing(pm_words, base_addr):
    """Score a packing by how many instructions decode successfully."""
    good = 0
    nop_count = 0
    for i, w in enumerate(pm_words):
        text = disasm(w, base_addr + i)
        if text and not text.startswith('???'):
            good += 1
        if w == 0:
            nop_count += 1
    return good, nop_count


def disassemble_pm(pm_words, base_addr, labels=None):
    """Disassemble a list of PM48 words and return text."""
    lines = []
    for i, w in enumerate(pm_words):
        addr = base_addr + i
        text = disasm(w, addr)
        if not text:
            text = f"??? 0x{w:012X}"

        # Substitute labels
        if labels:
            label_str = labels.get(addr, '')
            if label_str:
                lines.append(f"\n{label_str}:")
        else:
            label_str = ''

        lines.append(f"  0x{addr:06X}: {w:012X}  {text}")
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description='Extract and disassemble PM code from .ldr')
    parser.add_argument('ldr_file', help='Path to dsp-img.ldr')
    parser.add_argument('--raw-words', action='store_true', help='Show raw hex words')
    parser.add_argument('--packing', choices=['A', 'B', 'C', 'D', 'auto'], default='auto',
                        help='DM32->PM48 packing to use (default: auto-detect)')
    args = parser.parse_args()

    with open(args.ldr_file, 'rb') as f:
        data = f.read()

    blocks = parse_ldr(data)

    pm5_blocks = [b for b in blocks if b['type'] == 5]
    final_blocks = [b for b in blocks if b['type'] == 0]

    print(f"File: {args.ldr_file} ({len(data)} bytes)")
    print(f"PM blocks: {len(pm5_blocks)} LOAD_PM48 + {len(final_blocks)} FINAL")

    all_sections = []

    # Collect all PM sections to decode
    for b in pm5_blocks:
        payload = data[b['data_offset']:b['data_offset'] + b['data_size']]
        all_sections.append(('LOAD_PM48', b, payload, b['addr'], b['count']))

    for b in final_blocks:
        payload = data[b['data_offset']:b['data_offset'] + b['data_size']]
        pm_count = (384 // 3) * 2  # 256 PM words
        all_sections.append(('FINAL', b, payload, FINAL_PM_ADDR, pm_count))

    unpackers = {
        'A': unpack_pm48_A,
        'B': unpack_pm48_B,
        'C': unpack_pm48_C,
        'D': unpack_pm48_D,
    }

    for section_name, block, payload, base_addr, pm_count in all_sections:
        print(f"\n{'='*80}")
        print(f"{section_name} Block {block['num']}: {pm_count} PM words @ PM 0x{base_addr:06X}")
        print(f"  File offset: 0x{block['data_offset']:05X}, payload: {block['data_size']} bytes")
        if section_name == 'FINAL':
            print(f"  (384 DM32 words -> {pm_count} PM48 words via 3:2 packing)")
        print(f"{'='*80}")

        if args.packing == 'auto':
            # Try all packings and pick the best
            best_pack = None
            best_score = -1
            for name, unpacker in unpackers.items():
                pm_words = unpacker(payload, pm_count)
                good, nops = score_packing(pm_words, base_addr)
                total = len(pm_words)
                non_nop = total - nops
                score = good - nops  # prefer valid non-NOP instructions
                print(f"  Packing {name}: {good}/{total} decoded ({nops} NOPs, {non_nop} non-NOP)")
                if score > best_score:
                    best_score = score
                    best_pack = name
            print(f"  -> Best: packing {best_pack}")
            pm_words = unpackers[best_pack](payload, pm_count)
        else:
            pm_words = unpackers[args.packing](payload, pm_count)

        # Generate labels
        targets = collect_targets(pm_words, base_addr)
        labels = generate_labels(targets, base_addr, len(pm_words))

        if args.raw_words:
            print(f"\n  Raw PM48 words:")
            for i, w in enumerate(pm_words[:32]):
                addr = base_addr + i
                print(f"    PM(0x{addr:06X}) = 0x{w:012X}")
            if len(pm_words) > 32:
                print(f"    ... ({len(pm_words) - 32} more)")
            print()

        # Disassemble
        output = disassemble_pm(pm_words, base_addr, labels)
        print(output)

        # Summary
        good = sum(1 for i, w in enumerate(pm_words) if not disasm(w, base_addr+i).startswith('???') if disasm(w, base_addr+i))
        total = len(pm_words)
        print(f"\n; {section_name}: {good}/{total} instructions decoded")


if __name__ == '__main__':
    main()
