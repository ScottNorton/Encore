#!/usr/bin/env python3
"""Complete ADSP-21489 DSP firmware disassembly from .ldr file.

Extracts ALL code/data blocks from the .ldr file, reconstructs the memory
image at each target address, interprets DM data as packed PM code (3 DM32
words → 2 PM48 words using 6-byte LE packing), and disassembles everything.

Also disassembles the boot kernel and FINAL block entry point.

Usage:
  python scripts/dsp/disasm_ldr_full.py                         # full output
  python scripts/dsp/disasm_ldr_full.py --region B2000:B2100     # specific DM range
  python scripts/dsp/disasm_ldr_full.py -o vendor/dsp_full_disasm.txt
"""
import sys, os, struct, math, argparse, collections

if sys.platform == "win32":
    import io
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sharc_disasm

LDR_PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                         "vendor", "firmware", "squashfs-root", "usr", "share", "dsp", "dsp-img.ldr")

BOOT_KERNEL_SIZE = 256 * 6  # 1536 bytes
FINAL_DATA_SIZE = 384 * 4   # 1536 bytes

TYPE_NAMES = {
    0: "FINAL", 1: "ZERO_DM32", 2: "ZERO_DM48", 3: "LOAD_DM32",
    4: "LOAD_DM8", 5: "LOAD_PM48", 6: "LOAD_PM_PX",
    7: "ZERO_DM32a", 8: "ZERO_DM32b", 9: "LOAD_DM8a", 10: "LOAD_DM8b",
}


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
    return 0


def parse_ldr(path):
    """Parse .ldr file into boot kernel + block list."""
    with open(path, "rb") as f:
        ldr = f.read()

    boot_kernel = ldr[:BOOT_KERNEL_SIZE]
    blocks = []
    off = BOOT_KERNEL_SIZE

    while off + 12 <= len(ldr):
        typ, count, addr = struct.unpack_from("<III", ldr, off)
        off += 12
        ds = data_size(typ, count)
        payload = ldr[off:off+ds] if ds > 0 else b""
        blocks.append({
            "type": typ,
            "count": count,
            "addr": addr,
            "data": payload,
            "name": TYPE_NAMES.get(typ, f"TYPE_{typ}"),
        })
        off += ds

    return boot_kernel, blocks


def reconstruct_dm_image(blocks):
    """Reconstruct the DM memory image from .ldr blocks.

    Returns dict: {dm_addr: value} for all loaded/zeroed addresses.
    """
    dm = {}

    for blk in blocks:
        typ = blk["type"]
        count = blk["count"]
        addr = blk["addr"]
        payload = blk["data"]
        eff = typ
        if typ in (7, 8): eff = 1
        elif typ in (9, 10): eff = 4

        if eff == 0:
            # FINAL: 384 DM32 words loaded to hardcoded DM 0x92000
            words = struct.unpack_from(f"<{len(payload)//4}I", payload)
            for i, w in enumerate(words):
                dm[0x92000 + i] = w

        elif eff in (1,):
            # ZERO_DM32: zero-fill count DM words starting at addr
            for i in range(count):
                dm[addr + i] = 0

        elif eff == 2:
            # ZERO_DM48: zero-fill (48-bit DM words = same addressing)
            for i in range(count):
                dm[addr + i] = 0

        elif eff == 3:
            # LOAD_DM32: packed data, ceil(count/2)*4 bytes
            # Each 4 bytes of payload contains data for 2 consecutive DM addresses
            # Actually: the boot kernel handler reads pairs of SPI words
            # and stores R8 (the SPI-read header word) to destination.
            # The data format is: for count DM words, the payload has
            # ceil(count/2)*4 bytes = ceil(count/2) 32-bit words.
            # This means ~2 DM values per 4-byte payload word.
            #
            # Looking at the boot kernel LOAD_DM32 handler more carefully:
            # The handler calls SPI_read to get data, then does:
            #   DM(I0,M6) = R8   (store data word)
            # So each SPI read yields one DM word.
            # But the data size formula says ceil(count/2)*4, which means
            # the SPI stream has half as many words as the destination count.
            # This implies the SPI data is packed: each 32-bit SPI word
            # contains two 16-bit values, or the handler unpacks somehow.
            #
            # For now, just store raw 32-bit words sequentially.
            words = struct.unpack_from(f"<{len(payload)//4}I", payload)
            for i, w in enumerate(words):
                if addr + i < addr + count:
                    dm[addr + i] = w

        elif eff == 4:
            # LOAD_DM8: count*4 bytes, one 32-bit value per DM address
            words = struct.unpack_from(f"<{len(payload)//4}I", payload)
            for i, w in enumerate(words):
                if i < count:
                    dm[addr + i] = w

    return dm


def dm_to_pm_words(dm_image, dm_start, dm_count):
    """Convert consecutive DM32 words to PM48 words using 6-byte LE packing.

    3 DM32 words (12 bytes) → 2 PM48 words (12 bytes).
    """
    # Build raw byte stream from DM words
    raw = bytearray()
    for i in range(dm_count):
        addr = dm_start + i
        w = dm_image.get(addr, 0)
        raw += struct.pack("<I", w)

    # Interpret as 6-byte LE PM words
    pm_words = []
    for i in range(0, len(raw) - 5, 6):
        chunk = raw[i:i+6]
        w = int.from_bytes(chunk, "little")
        pm_words.append(w)

    return pm_words


def score_as_code(pm_words):
    """Score how likely a sequence of PM48 words is to be code vs data.

    Returns (score, total) where score is the number of plausible instructions.
    """
    if not pm_words:
        return 0, 0

    plausible = 0
    total = 0
    for w in pm_words:
        if w == 0:
            continue  # NOP is ambiguous
        total += 1
        try:
            asm = sharc_disasm.disasm(w, 0)
            # If it decoded without "UNKNOWN" or "TYPE?" and isn't just a compute fallback
            if "UNKNOWN" not in asm and "TYPE?" not in asm:
                plausible += 1
        except:
            pass

    return plausible, total


def find_contiguous_regions(dm_image):
    """Find contiguous loaded address ranges in the DM image."""
    if not dm_image:
        return []

    addrs = sorted(dm_image.keys())
    regions = []
    start = addrs[0]
    prev = addrs[0]

    for a in addrs[1:]:
        if a != prev + 1:
            regions.append((start, prev, prev - start + 1))
            start = a
        prev = a
    regions.append((start, prev, prev - start + 1))

    return regions


def main():
    parser = argparse.ArgumentParser(description="Complete SHARC DSP firmware disassembly from .ldr")
    parser.add_argument("--ldr", default=LDR_PATH, help="Path to dsp-img.ldr")
    parser.add_argument("--region", help="Specific DM range (hex), e.g. B2000:B2100")
    parser.add_argument("-o", "--output", help="Output file")
    parser.add_argument("--data-regions", action="store_true", help="Include data regions too")
    args = parser.parse_args()

    print(f"Parsing {args.ldr}...", file=sys.stderr)
    boot_kernel_data, blocks = parse_ldr(args.ldr)
    print(f"  {len(blocks)} blocks", file=sys.stderr)

    # Reconstruct DM image
    print("Reconstructing DM memory image...", file=sys.stderr)
    dm_image = reconstruct_dm_image(blocks)
    print(f"  {len(dm_image)} DM words loaded", file=sys.stderr)

    # Find regions
    regions = find_contiguous_regions(dm_image)
    print(f"\nDM memory regions ({len(regions)}):", file=sys.stderr)
    for start, end, length in regions:
        # Try to interpret as PM code
        pm_words = dm_to_pm_words(dm_image, start, min(length, 300))
        score, total = score_as_code(pm_words)
        pct = score / max(total, 1) * 100
        tag = "CODE" if pct > 60 and total > 10 else "DATA" if pct < 30 else "MIXED"
        print(f"  DM 0x{start:06X}-0x{end:06X} ({length:6d} words) "
              f"[{tag}: {score}/{total} instructions = {pct:.0f}%]", file=sys.stderr)

    # Open output
    out = open(args.output, "w", encoding="utf-8") if args.output else sys.stdout

    out.write("; ADSP-21489 SHARC DSP Firmware — Complete .ldr Disassembly\n")
    out.write(f"; Source: {os.path.basename(args.ldr)} ({len(blocks)} blocks)\n")
    out.write("; Packing: 6-byte little-endian (3 DM32 → 2 PM48)\n")
    out.write(";\n")
    out.write("; Memory regions loaded by .ldr:\n")
    for start, end, length in regions:
        out.write(f";   DM 0x{start:06X}-0x{end:06X} ({length} words)\n")
    out.write(";\n\n")

    # ── Section 1: Boot Kernel ──
    out.write("; " + "=" * 68 + "\n")
    out.write("; BOOT KERNEL (PM 0x8C000-0x8C0FF, from .ldr header)\n")
    out.write("; " + "=" * 68 + "\n\n")

    # Decode boot kernel words (bit-reversed + byte-swapped from raw .ldr)
    bk_words = []
    for i in range(256):
        chunk = boot_kernel_data[i*6:(i+1)*6]
        # Reverse byte order (swap) and bit-reverse each byte (raw)
        reversed_chunk = bytes(chunk[j] for j in range(5, -1, -1))
        bitrev = bytes(int(f'{b:08b}'[::-1], 2) for b in reversed_chunk)
        w = int.from_bytes(bitrev, "big")
        bk_words.append(w)

    bk_targets = sharc_disasm.collect_targets(bk_words, 0x8C000)
    bk_labels = sharc_disasm.generate_labels(bk_targets, 0x8C000, 256)

    for i, w in enumerate(bk_words):
        addr = 0x8C000 + i
        if addr in bk_labels:
            out.write(f"\n{bk_labels[addr]}:\n")
        if w == 0:
            continue
        asm = sharc_disasm.disasm(w, addr)
        for ta, tl in bk_labels.items():
            for fmt in [f"0x{ta:05X}", f"0x0{ta:05X}"]:
                if fmt in asm:
                    asm = asm.replace(fmt, tl)
        out.write(f"  {addr:05X}  {w:012X}  {asm}\n")

    # ── Section 2: FINAL Block (Entry Point / IVT) ──
    out.write("\n\n; " + "=" * 68 + "\n")
    out.write("; FINAL BLOCK — Application Entry Point / Interrupt Vector Table\n")
    out.write("; Loaded by boot kernel DMA to PM 0x8C000 (overwrites boot kernel)\n")
    out.write("; " + "=" * 68 + "\n\n")

    # Extract FINAL block data
    final_data = None
    for blk in blocks:
        if blk["type"] == 0:
            final_data = blk["data"]
            break

    if final_data:
        final_pm = []
        for i in range(0, len(final_data) - 5, 6):
            chunk = final_data[i:i+6]
            w = int.from_bytes(chunk, "little")
            final_pm.append(w)

        final_targets = sharc_disasm.collect_targets(final_pm, 0x8C000)
        final_labels = sharc_disasm.generate_labels(final_targets, 0x8C000, len(final_pm))

        for i, w in enumerate(final_pm):
            addr = 0x8C000 + i
            if addr in final_labels:
                out.write(f"\n{final_labels[addr]}:\n")
            if w == 0:
                continue
            asm = sharc_disasm.disasm(w, addr)
            for ta, tl in final_labels.items():
                for fmt in [f"0x{ta:05X}", f"0x0{ta:05X}"]:
                    if fmt in asm:
                        asm = asm.replace(fmt, tl)
            out.write(f"  {addr:05X}  {w:012X}  {asm}\n")

    # ── Section 3: Main Firmware Code/Data ──
    total_instr = 0
    total_labels = 0

    # Filter to requested region if specified
    if args.region:
        parts = args.region.split(":")
        r_start = int(parts[0], 16)
        r_end = int(parts[1], 16) if len(parts) > 1 else r_start + 512
        target_regions = [(s, e, l) for s, e, l in regions if e >= r_start and s <= r_end]
    else:
        target_regions = regions

    for start, end, length in target_regions:
        # Skip the FINAL block region (already handled above)
        if start == 0x92000 and length == 384:
            continue

        pm_words = dm_to_pm_words(dm_image, start, length)
        score, total = score_as_code(pm_words)
        pct = score / max(total, 1) * 100
        is_code = pct > 40 and total > 5

        out.write(f"\n\n; {'=' * 68}\n")
        out.write(f"; DM 0x{start:06X}-0x{end:06X} ({length} DM words → "
                  f"{len(pm_words)} PM words)\n")
        if is_code:
            out.write(f"; Classification: CODE ({score}/{total} = {pct:.0f}% plausible)\n")
        else:
            out.write(f"; Classification: DATA ({score}/{total} = {pct:.0f}% plausible)\n")
        out.write(f"; {'=' * 68}\n\n")

        if not is_code and not args.data_regions:
            # For data, just show a hex summary
            out.write(f";   (Data region — use --data-regions to disassemble)\n")
            # Show first 16 DM words
            out.write(f";   First 16 DM words:\n")
            for i in range(min(16, length)):
                w = dm_image.get(start + i, 0)
                out.write(f";     DM[0x{start+i:06X}] = 0x{w:08X}\n")
            if length > 16:
                out.write(f";     ... ({length - 16} more words)\n")
            continue

        # Generate labels
        targets = sharc_disasm.collect_targets(pm_words, start)
        labels = sharc_disasm.generate_labels(targets, start, len(pm_words))
        total_labels += len(labels)

        # Disassemble
        for i, w in enumerate(pm_words):
            pm_addr = start + i
            if pm_addr in labels:
                out.write(f"\n{labels[pm_addr]}:\n")
            if w == 0:
                continue
            total_instr += 1
            try:
                asm = sharc_disasm.disasm(w, pm_addr)
            except Exception as e:
                asm = f"<error: {e}>"
            for ta, tl in labels.items():
                for fmt in [f"0x{ta:05X}", f"0x0{ta:05X}"]:
                    if fmt in asm:
                        asm = asm.replace(fmt, tl)
            out.write(f"  {pm_addr:05X}  {w:012X}  {asm}\n")

    out.write(f"\n; {'=' * 68}\n")
    out.write(f"; Summary: {total_instr} instructions, {total_labels} labels\n")
    out.write(f"; {'=' * 68}\n")

    if args.output:
        out.close()
        print(f"\nWrote to {args.output}", file=sys.stderr)

    print(f"\nDone: {total_instr} instructions, {total_labels} labels", file=sys.stderr)


if __name__ == "__main__":
    main()
