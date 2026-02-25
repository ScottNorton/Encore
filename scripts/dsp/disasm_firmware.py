#!/usr/bin/env python3
"""Complete ADSP-21489 DSP firmware disassembly from SRAM dump.

Reads vendor/dsp_dump.bin (640 KB = 320 pages × 2048 bytes), interprets the
raw bytes as consecutive 6-byte little-endian PM48 words starting at PM 0x80000,
and produces a full annotated disassembly.

Memory mapping:
  Physical SRAM is shared between DM (32-bit) and PM (48-bit) ports.
  DM address D → byte offset (D - 0x80000) × 4
  PM address P → byte offset (P - 0x80000) × 6
  Therefore: DM 0x92000 → byte 0x48000 → PM 0x8C000

  The boot kernel at PM 0x8C000 overwrites itself with the FINAL block,
  so the application entry point is also at PM 0x8C000.

Usage:
  python scripts/disasm_firmware.py                    # full disassembly
  python scripts/disasm_firmware.py --code-only         # skip data regions
  python scripts/disasm_firmware.py --region 8C000:8C100  # specific PM range
"""
import sys, os, struct, argparse

# Force UTF-8 on Windows
if sys.platform == "win32":
    import io
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")

# Import the SHARC disassembler
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sharc_disasm

DUMP_PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                          "vendor", "dsp_dump.bin")
PM_BASE = 0x80000
DM_BASE = 0x80000

# Known data regions (PM address ranges) — skip disassembly for these
# Derived from ldr_parse analysis and dump analysis
# DM addr → PM addr: PM = 0x80000 + (DM - 0x80000) * 2/3
def dm_to_pm(dm_addr):
    """Convert DM address to PM address via shared physical SRAM."""
    dm_off = dm_addr - DM_BASE
    # 3 DM words (12 bytes) = 2 PM words (12 bytes)
    pm_off = (dm_off * 2) // 3
    return PM_BASE + pm_off

def pm_to_dm(pm_addr):
    """Convert PM address to DM address via shared physical SRAM."""
    pm_off = pm_addr - PM_BASE
    dm_off = (pm_off * 3) // 2
    return DM_BASE + dm_off

def pm_to_byte_offset(pm_addr):
    """Convert PM address to byte offset in the dump."""
    return (pm_addr - PM_BASE) * 6

def byte_offset_to_pm(off):
    """Convert byte offset to PM address."""
    return PM_BASE + off // 6


def load_pm_words(dump_path):
    """Load dump and extract all PM48 words as 6-byte LE integers."""
    with open(dump_path, "rb") as f:
        data = f.read()

    num_pm = len(data) // 6
    remainder = len(data) % 6

    words = []
    for i in range(num_pm):
        chunk = data[i*6 : i*6+6]
        w = int.from_bytes(chunk, "little")
        words.append(w)

    return words, num_pm, remainder


def classify_regions(words, num_words):
    """Classify PM address ranges as code, data, or empty.

    Returns list of (start_pm, end_pm, region_type, description) tuples.
    """
    regions = []

    # Known regions from .ldr analysis (converted from DM addresses):
    # Boot kernel / FINAL entry: DM 0x92000 (384 words) → PM 0x8C000
    # Beamforming matrix: DM ~0x98600-0xA4BFF → PM ~0x90400-0x96DFF

    # Scan for non-empty runs
    in_run = False
    run_start = 0
    nop_count = 0
    code_count = 0

    for i in range(num_words):
        w = words[i]
        if w != 0:
            if not in_run:
                run_start = i
                in_run = True
                nop_count = 0
                code_count = 0

            # Heuristic: check if it looks like code
            grp = (w >> 45) & 0x7
            if grp in (0, 1, 2, 3, 4, 5, 6, 7):
                code_count += 1
            nop_count = 0
        else:
            if in_run:
                nop_count += 1
                # End run after 16+ consecutive NOPs
                if nop_count >= 16:
                    run_end = i - nop_count
                    if run_end > run_start:
                        length = run_end - run_start + 1
                        pm_start = PM_BASE + run_start
                        pm_end = PM_BASE + run_end
                        regions.append((pm_start, pm_end, length))
                    in_run = False

    # Close final run
    if in_run:
        run_end = num_words - 1
        while run_end > run_start and words[run_end] == 0:
            run_end -= 1
        if run_end > run_start:
            length = run_end - run_start + 1
            pm_start = PM_BASE + run_start
            pm_end = PM_BASE + run_end
            regions.append((pm_start, pm_end, length))

    return regions


def disassemble_region(words, pm_start, pm_end, all_labels=None):
    """Disassemble a PM address range. Returns list of (addr, hex, mnemonic) tuples."""
    lines = []
    start_idx = pm_start - PM_BASE
    end_idx = pm_end - PM_BASE

    for i in range(start_idx, end_idx + 1):
        addr = PM_BASE + i
        w = words[i]

        if w == 0:
            mnemonic = "NOP;"
        else:
            try:
                mnemonic = sharc_disasm.disasm(w, addr)
            except Exception as e:
                mnemonic = f"<decode error: {e}>"

        # Add label if present
        label = ""
        if all_labels and addr in all_labels:
            label = all_labels[addr]

        lines.append((addr, w, mnemonic, label))

    return lines


def main():
    parser = argparse.ArgumentParser(description="Complete SHARC DSP firmware disassembly from SRAM dump")
    parser.add_argument("--dump", default=DUMP_PATH, help="Path to dsp_dump.bin")
    parser.add_argument("--code-only", action="store_true", help="Only disassemble likely code regions")
    parser.add_argument("--region", help="Specific PM range to disassemble (hex), e.g. 8C000:8C100")
    parser.add_argument("--labels", action="store_true", default=True, help="Generate labels (default: on)")
    parser.add_argument("--no-labels", action="store_true", help="Disable label generation")
    parser.add_argument("-o", "--output", help="Output file (default: stdout)")
    parser.add_argument("--stats", action="store_true", help="Print statistics only")
    args = parser.parse_args()

    if args.no_labels:
        args.labels = False

    print(f"Loading {args.dump}...", file=sys.stderr)
    words, num_pm, remainder = load_pm_words(args.dump)
    print(f"  {num_pm} PM48 words (PM 0x{PM_BASE:05X} to 0x{PM_BASE+num_pm-1:05X}), "
          f"{remainder} trailing bytes", file=sys.stderr)

    # Map key addresses
    print(f"\nAddress mapping:", file=sys.stderr)
    for dm_name, dm_addr in [("Boot kernel / FINAL entry", 0x92000),
                              ("Beamforming start (~pg195)", DM_BASE + 195*512),
                              ("Beamforming end (~pg293)", DM_BASE + 293*512),
                              ("Block 1 start", 0xA0000),
                              ("Dump end", DM_BASE + 320*512 - 1)]:
        pm = dm_to_pm(dm_addr)
        print(f"  DM 0x{dm_addr:06X} → PM 0x{pm:05X}", file=sys.stderr)

    # Classify regions
    regions = classify_regions(words, num_pm)
    print(f"\nFound {len(regions)} non-empty regions:", file=sys.stderr)
    total_nonzero = 0
    for pm_start, pm_end, length in regions:
        dm_start = pm_to_dm(pm_start)
        dm_end = pm_to_dm(pm_end)
        total_nonzero += length

        # Check if this overlaps known data regions
        bf_pm_start = dm_to_pm(DM_BASE + 195*512)  # beamforming start
        bf_pm_end = dm_to_pm(DM_BASE + 294*512)     # beamforming end
        is_bf = pm_start >= bf_pm_start and pm_end <= bf_pm_end
        tag = " [BEAMFORMING DATA]" if is_bf else ""

        entry_tag = ""
        if pm_start <= 0x8C000 <= pm_end:
            entry_tag = " [CONTAINS ENTRY POINT 0x8C000]"

        print(f"  PM 0x{pm_start:05X}-0x{pm_end:05X} ({length:6d} words, "
              f"DM 0x{dm_start:06X}-0x{dm_end:06X}){tag}{entry_tag}", file=sys.stderr)

    print(f"\n  Total non-empty: {total_nonzero} PM words "
          f"({total_nonzero*6/1024:.1f} KB)", file=sys.stderr)

    if args.stats:
        return

    # Determine what to disassemble
    if args.region:
        parts = args.region.split(":")
        r_start = int(parts[0], 16)
        r_end = int(parts[1], 16) if len(parts) > 1 else r_start + 256
        disasm_regions = [(r_start, r_end, r_end - r_start + 1)]
    elif args.code_only:
        # Exclude known data regions (beamforming)
        bf_pm_start = dm_to_pm(DM_BASE + 195*512)
        bf_pm_end = dm_to_pm(DM_BASE + 294*512)
        disasm_regions = [(s, e, l) for s, e, l in regions
                         if not (s >= bf_pm_start and e <= bf_pm_end)]
    else:
        disasm_regions = regions

    # Label generation pass
    all_labels = {}
    if args.labels:
        print("Generating labels (first pass)...", file=sys.stderr)
        for pm_start, pm_end, length in disasm_regions:
            start_idx = pm_start - PM_BASE
            end_idx = pm_end - PM_BASE
            region_words = words[start_idx:end_idx+1]
            targets = sharc_disasm.collect_targets(region_words, pm_start)
            labels = sharc_disasm.generate_labels(targets, pm_start, length)
            all_labels.update(labels)
        print(f"  {len(all_labels)} labels generated", file=sys.stderr)

    # Output
    out = open(args.output, "w", encoding="utf-8") if args.output else sys.stdout

    out.write("; ADSP-21489 SHARC DSP Firmware — Complete Disassembly\n")
    out.write(f"; Source: {os.path.basename(args.dump)} ({num_pm} PM48 words)\n")
    out.write("; Packing: 6-byte little-endian (Method C, confirmed)\n")
    out.write("; PM base: 0x80000\n")
    out.write(f"; Regions: {len(disasm_regions)} code/data blocks\n")
    out.write(f"; Labels: {len(all_labels)}\n")
    out.write(";\n")
    out.write("; Memory mapping:\n")
    out.write(";   DM address D → byte offset (D - 0x80000) × 4\n")
    out.write(";   PM address P → byte offset (P - 0x80000) × 6\n")
    out.write(";   DM 0x92000 = PM 0x8C000 (FINAL block entry point)\n")
    out.write(";\n")
    out.write("; Key regions:\n")
    out.write(";   PM 0x8C000: Application entry point (boot kernel self-replaces)\n")
    bf_s = dm_to_pm(DM_BASE + 195*512)
    bf_e = dm_to_pm(DM_BASE + 294*512)
    out.write(f";   PM 0x{bf_s:05X}-0x{bf_e:05X}: Beamforming gain matrix (data)\n")
    out.write(";\n\n")

    total_instructions = 0
    total_nops = 0
    total_data_suspicious = 0

    for region_idx, (pm_start, pm_end, length) in enumerate(disasm_regions):
        dm_start = pm_to_dm(pm_start)
        dm_end = pm_to_dm(pm_end)

        # Check if beamforming
        is_bf = pm_start >= bf_s and pm_end <= bf_e

        out.write(f"; {'='*68}\n")
        out.write(f"; Region {region_idx}: PM 0x{pm_start:05X}-0x{pm_end:05X} "
                  f"({length} words)\n")
        out.write(f";   DM 0x{dm_start:06X}-0x{dm_end:06X}")
        if is_bf:
            out.write("  [BEAMFORMING DATA — not code]")
        if pm_start <= 0x8C000 <= pm_end:
            out.write("  [ENTRY POINT]")
        out.write(f"\n; {'='*68}\n\n")

        if is_bf and args.code_only:
            out.write(";   (skipped — beamforming data region)\n\n")
            continue

        # Disassemble
        lines = disassemble_region(words, pm_start, pm_end, all_labels)

        for addr, w, mnemonic, label in lines:
            if w == 0:
                total_nops += 1
                continue  # Skip NOPs for readability

            total_instructions += 1

            # Prefix with label
            if label:
                out.write(f"\n{label}:\n")

            # Check for target labels in the mnemonic (replace addresses)
            display_mnemonic = mnemonic
            # Crude: find hex addresses in mnemonic and replace with labels
            for target_addr, target_label in all_labels.items():
                hex_str = f"0x{target_addr:05X}"
                hex_str2 = f"0x0{target_addr:05X}"
                if hex_str in display_mnemonic:
                    display_mnemonic = display_mnemonic.replace(hex_str, target_label)
                elif hex_str2 in display_mnemonic:
                    display_mnemonic = display_mnemonic.replace(hex_str2, target_label)

            dm_addr = pm_to_dm(addr)
            out.write(f"  {addr:05X}  {w:012X}  {display_mnemonic:50s} ; DM 0x{dm_addr:06X}\n")

    out.write(f"\n; {'='*68}\n")
    out.write(f"; Summary:\n")
    out.write(f";   Total non-NOP instructions: {total_instructions}\n")
    out.write(f";   NOPs skipped: {total_nops}\n")
    out.write(f";   Labels: {len(all_labels)}\n")
    out.write(f"; {'='*68}\n")

    if args.output:
        out.close()
        print(f"\nWrote disassembly to {args.output}", file=sys.stderr)

    print(f"\nDone: {total_instructions} instructions, {total_nops} NOPs, "
          f"{len(all_labels)} labels", file=sys.stderr)


if __name__ == "__main__":
    main()
