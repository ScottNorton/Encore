#!/usr/bin/env python3
"""
ADSP-21489 SHARC DSP SRAM dump analysis for Harman Kardon Invoke.
Analyzes vendor/dsp_dump.bin (655,360 bytes = 320 pages x 2048 bytes).
"""
import struct
import sys
import os
import re
import collections

# Force UTF-8 output on Windows
if sys.platform == "win32":
    import io
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")

DUMP_PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                          "vendor", "dsp_dump.bin")
PAGE_SIZE = 2048       # bytes per page
NUM_PAGES = 320        # total pages
DM_BASE   = 0x80000   # ADSP-21489 Block 0 DM SRAM base address
# Each DM word = 32 bits = 4 bytes; 2048 bytes / 4 = 512 DM words per page
DM_WORDS_PER_PAGE = PAGE_SIZE // 4   # = 512


def load_dump(path):
    with open(path, "rb") as f:
        data = f.read()
    assert len(data) == NUM_PAGES * PAGE_SIZE, f"Expected {NUM_PAGES*PAGE_SIZE}, got {len(data)}"
    return data


def get_page(data, page_idx):
    start = page_idx * PAGE_SIZE
    return data[start: start + PAGE_SIZE]


# ─────────────────────────────────────────────────────────────────────────────
# 1. PAGE ACTIVITY MAP
# ─────────────────────────────────────────────────────────────────────────────

def analyze_pages(data):
    print("=" * 72)
    print("1. PAGE ACTIVITY MAP")
    print("=" * 72)

    page_info = []
    for p in range(NUM_PAGES):
        page = get_page(data, p)
        nonzero = sum(1 for b in page if b != 0)
        pct = nonzero / PAGE_SIZE * 100
        first_nz = next((i for i, b in enumerate(page) if b != 0), None)
        if pct == 0:
            cls = "empty"
        elif pct < 10:
            cls = "sparse"
        elif pct < 90:
            cls = "dense"
        else:
            cls = "full"
        dm_addr = DM_BASE + p * DM_WORDS_PER_PAGE
        page_info.append((p, pct, first_nz, cls, dm_addr))

    # Print compact summary: show page ranges by classification
    print(f"\n{'Page':>5} {'DM Addr':>10} {'%NonZero':>10} {'First NZ':>10} {'Class':>8}")
    print("-" * 52)
    for p, pct, first_nz, cls, dm_addr in page_info:
        if cls != "empty":
            fnz_str = f"0x{first_nz:04X}" if first_nz is not None else "  none"
            print(f"  {p:3d}   0x{dm_addr:06X}  {pct:8.1f}%  {fnz_str:>10}  {cls:>8}")

    # Range summary
    print()
    print("Range summary (non-empty runs):")
    in_run = False
    run_start = None
    prev_cls = None
    for p, pct, _, cls, dm_addr in page_info:
        if cls != "empty":
            if not in_run:
                run_start = p
                in_run = True
        else:
            if in_run:
                run_end = p - 1
                start_addr = DM_BASE + run_start * DM_WORDS_PER_PAGE
                end_addr   = DM_BASE + run_end  * DM_WORDS_PER_PAGE + DM_WORDS_PER_PAGE - 1
                print(f"  Pages {run_start:3d}-{run_end:3d}  DM 0x{start_addr:06X}-0x{end_addr:06X}  ({run_end-run_start+1} pages)")
                in_run = False
    if in_run:
        run_end = NUM_PAGES - 1
        start_addr = DM_BASE + run_start * DM_WORDS_PER_PAGE
        end_addr   = DM_BASE + run_end   * DM_WORDS_PER_PAGE + DM_WORDS_PER_PAGE - 1
        print(f"  Pages {run_start:3d}-{run_end:3d}  DM 0x{start_addr:06X}-0x{end_addr:06X}  ({run_end-run_start+1} pages)")

    return page_info


# ─────────────────────────────────────────────────────────────────────────────
# 2. ADDRESS MAPPING
# ─────────────────────────────────────────────────────────────────────────────

def analyze_address_mapping(data):
    print()
    print("=" * 72)
    print("2. ADDRESS MAPPING")
    print("=" * 72)

    # Verify hypothesis: page N → DM address (0x80000 + N * 512)
    # ADSP-21489 memory map (DM 32-bit words):
    #   Block 0 DM:  0x80000 - 0x9FFFF (128K words = 512KB)
    #   Block 1 DM:  0xA0000 - 0xBFFFF (128K words = 512KB)
    #   Block 2 DM:  0xC0000 - 0xDFFFF (128K words = 512KB)
    #   ...
    # 320 pages × 512 words = 163840 words → covers DM 0x80000-0xA7FFF
    total_words = NUM_PAGES * DM_WORDS_PER_PAGE
    end_addr = DM_BASE + total_words - 1
    print(f"\n  Dump mapping assumption:")
    print(f"    Page 0   -> DM 0x{DM_BASE:06X}")
    print(f"    Page 319 -> DM 0x{DM_BASE + 319*DM_WORDS_PER_PAGE:06X}")
    print(f"    Total words: {total_words}  (0x{total_words:X})")
    print(f"    Last DM address: 0x{end_addr:06X}")
    print()

    # PM 0x92000 target from ldr analysis (384 DM32 words loaded via DMA)
    pm_target = 0x92000
    page_for_pm = (pm_target - DM_BASE) // DM_WORDS_PER_PAGE
    offset_in_page = ((pm_target - DM_BASE) % DM_WORDS_PER_PAGE) * 4
    print(f"  PM 0x92000 target (384 DM32 words from ldr):")
    print(f"    (0x{pm_target:06X} - 0x{DM_BASE:06X}) / {DM_WORDS_PER_PAGE} = page {page_for_pm}")
    print(f"    Byte offset in page: 0x{offset_in_page:X}")
    print()

    # Check what's at that page
    if page_for_pm < NUM_PAGES:
        page = get_page(data, page_for_pm)
        nonzero = sum(1 for b in page if b != 0)
        pct = nonzero / PAGE_SIZE * 100
        print(f"    Page {page_for_pm} contents: {pct:.1f}% non-zero")
        # Show first 64 bytes as hex
        print(f"    First 64 bytes (raw): {page[:64].hex()}")
        # Show as 32-bit words
        words = struct.unpack_from(f"<{PAGE_SIZE//4}I", page)
        print(f"    First 16 DM32 words:")
        for i in range(16):
            print(f"      [DM 0x{pm_target+i:06X}] = 0x{words[i]:08X}")

    # Also check Block 1 boundary
    block1_start = 0xA0000
    if block1_start >= DM_BASE:
        block1_page = (block1_start - DM_BASE) // DM_WORDS_PER_PAGE
        print(f"\n  Block 1 DM start (0x{block1_start:06X}) → page {block1_page}")
        if block1_page < NUM_PAGES:
            page = get_page(data, block1_page)
            nz = sum(1 for b in page if b != 0)
            print(f"    Page {block1_page}: {nz/PAGE_SIZE*100:.1f}% non-zero")


# ─────────────────────────────────────────────────────────────────────────────
# 3. SPORT/TDM REGISTER EXTRACTION
# ─────────────────────────────────────────────────────────────────────────────

# ADSP-21489 IOP registers are in DM space at addresses starting from 0x0 in IOP
# but they appear in the normal DM map. The actual layout:
#   The "IOP registers" for ADSP-21489 are accessible via DM addresses in the
#   range 0x0000 to some upper limit — BUT in the DSP, IOP registers appear
#   at DM addresses in a separate space, not at 0x80000+.
#   The dump starts at DM 0x80000 (SRAM), so IOP regs are NOT in this dump
#   unless there's a different offset.
#   However, from ldr analysis: SPI IOP regs are at DM 0x1080-0x1084,
#   DM 0x2000, DM 0x30024, etc. — all below 0x80000, so NOT in this dump.
#
#   But SPORT registers might be in the data pages if they were mirrored.
#   Let's still look at the first few pages for interesting values.

# ADSP-21489 SPORT registers (DM addresses, from hardware reference):
SPORT_REGS = {
    # SPORT0
    0x0000_1C00: "SPORT0_MCTL",      # multichannel control
    0x0000_1C01: "SPORT0_CS0",        # channel select 0 (slots 0-31)
    0x0000_1C02: "SPORT0_CS1",        # channel select 1 (slots 32-63)
    0x0000_1C03: "SPORT0_CS2",        # channel select 2 (slots 64-95)
    0x0000_1C04: "SPORT0_CS3",        # channel select 3 (slots 96-127)
    0x0000_1C05: "SPORT0_TX_MCTL",    # TX multichannel control
    0x0000_1C06: "SPORT0_TX_CS0",     # TX channel select 0
    0x0000_1A00: "SPORT0_CTL",        # transmit control
    0x0000_1A01: "SPORT0_DIV",        # clock/frame divisors
    # SPORT1
    0x0000_1D00: "SPORT1_MCTL",
    0x0000_1D01: "SPORT1_CS0",
    0x0000_1D02: "SPORT1_CS1",
    0x0000_1B00: "SPORT1_CTL",
    # SPORT2
    0x0000_1E00: "SPORT2_MCTL",
    0x0000_1E01: "SPORT2_CS0",
    0x0000_1B00: "SPORT2_CTL",
    # SPORT3
    0x0000_1F00: "SPORT3_MCTL",
    0x0000_1F01: "SPORT3_CS0",
}

def analyze_sport_registers(data):
    print()
    print("=" * 72)
    print("3. SPORT/TDM REGISTER EXTRACTION")
    print("=" * 72)
    print()
    print("  Note: IOP registers are at DM addresses < 0x80000 (below dump start).")
    print("  The dump covers DM 0x80000-0xA7FFF (SRAM only).")
    print("  Looking for SPORT config data that may be stored in SRAM (coefficients,")
    print("  config tables, or register shadow values in page 0-31).")
    print()

    # Look at first 32 pages for any non-zero structured data
    print("  First 32 pages (DM 0x80000-0x83FFF) — looking for IOP-like values:")
    for pg in range(32):
        page = get_page(data, pg)
        nz = sum(1 for b in page if b != 0)
        if nz > 0:
            words = struct.unpack_from(f"<{PAGE_SIZE//4}I", page)
            # Find non-zero words
            nz_words = [(i, w) for i, w in enumerate(words) if w != 0]
            dm_base_pg = DM_BASE + pg * DM_WORDS_PER_PAGE
            print(f"\n  Page {pg:3d} (DM 0x{dm_base_pg:06X}): {nz} non-zero bytes, {len(nz_words)} non-zero words")
            for idx, w in nz_words[:32]:
                dm_addr = dm_base_pg + idx
                print(f"    DM[0x{dm_addr:06X}] = 0x{w:08X}  ({w})")
            if len(nz_words) > 32:
                print(f"    ... and {len(nz_words)-32} more non-zero words")

    # Now search for TDM-like patterns: multichannel enable words
    # A 7-mic TDM setup would use 8 slots (power of 2), so channel select
    # would be 0b01111111 = 0x7F or 0xFF for 8 channels
    print()
    print("  Searching for potential SPORT multichannel config values (0x7F, 0xFF, 0x3FF):")
    sport_candidates = []
    for pg in range(NUM_PAGES):
        page = get_page(data, pg)
        words = struct.unpack_from(f"<{PAGE_SIZE//4}I", page)
        for idx, w in enumerate(words):
            if w in (0x7F, 0xFF, 0x1FF, 0x3FF, 0x7F7F, 0xFFFF, 0x00000007,
                     0x00000008, 0x00000007F, 0xFF00FF, 0x0000007F):
                dm_addr = DM_BASE + pg * DM_WORDS_PER_PAGE + idx
                sport_candidates.append((dm_addr, w, pg, idx))
    for dm_addr, w, pg, idx in sport_candidates[:30]:
        print(f"    DM[0x{dm_addr:06X}] (pg {pg:3d} off {idx:3d}) = 0x{w:08X}")
    if len(sport_candidates) > 30:
        print(f"    ... and {len(sport_candidates)-30} more matches")


# ─────────────────────────────────────────────────────────────────────────────
# 4. BEAMFORMING GAIN MATRIX ANALYSIS
# ─────────────────────────────────────────────────────────────────────────────

def analyze_beamforming(data):
    print()
    print("=" * 72)
    print("4. BEAMFORMING GAIN MATRIX ANALYSIS (pages 195-293)")
    print("=" * 72)

    bf_start_page = 195
    bf_end_page   = 293   # inclusive
    bf_data = data[bf_start_page * PAGE_SIZE : (bf_end_page + 1) * PAGE_SIZE]
    bf_len  = len(bf_data)
    print(f"\n  Region: pages {bf_start_page}-{bf_end_page}, {bf_len} bytes ({bf_len//1024} KB)")
    print(f"  DM addresses: 0x{DM_BASE + bf_start_page*DM_WORDS_PER_PAGE:06X} - "
          f"0x{DM_BASE + bf_end_page*DM_WORDS_PER_PAGE + DM_WORDS_PER_PAGE - 1:06X}")

    # Count non-zero bytes
    nz_bytes = sum(1 for b in bf_data if b != 0)
    print(f"\n  Non-zero bytes: {nz_bytes} / {bf_len} ({nz_bytes/bf_len*100:.1f}%)")

    # -- Analyze as 32-bit little-endian words --
    num_words = bf_len // 4
    words_32 = struct.unpack_from(f"<{num_words}I", bf_data)
    nz_words = [w for w in words_32 if w != 0]
    print(f"\n  As 32-bit LE words: {len(nz_words)} non-zero out of {num_words}")

    # Value frequency distribution
    counter = collections.Counter(words_32)
    most_common = counter.most_common(30)
    print("\n  Top 30 most frequent 32-bit values:")
    print(f"    {'Value':>12}  {'Count':>8}  {'Pct':>7}  As float  As signed")
    for val, cnt in most_common:
        pct = cnt / num_words * 100
        try:
            as_f = struct.unpack("<f", struct.pack("<I", val))[0]
            f_str = f"{as_f:10.4g}"
        except:
            f_str = "  ------"
        signed = struct.unpack("<i", struct.pack("<I", val))[0]
        print(f"    0x{val:08X}  {cnt:8d}  {pct:6.2f}%  {f_str}  {signed}")

    # Check for common fixed-point patterns
    print()
    # Look at individual bytes
    byte_counter = collections.Counter(bf_data)
    print("  Top 16 most frequent bytes:")
    for byte_val, cnt in byte_counter.most_common(16):
        pct = cnt / bf_len * 100
        print(f"    0x{byte_val:02X} ({byte_val:3d}): {cnt:7d} times ({pct:.2f}%)")

    # -- Try to determine float vs fixed-point --
    # ADSP-21489 native format is 32-bit IEEE float for most DSP ops
    # But also supports 1.31 fixed-point (Qx format)
    # Check if values look like normalized floats (exponent 0x3F-0x42 → magnitude ~0.5-8)
    valid_floats = 0
    float_vals = []
    for w in nz_words:
        try:
            f = struct.unpack("<f", struct.pack("<I", w))[0]
            if 1e-6 < abs(f) < 1e6 and not (f != f):  # finite, reasonable range
                valid_floats += 1
                float_vals.append(f)
        except:
            pass
    print(f"\n  Non-zero words interpretable as reasonable IEEE float: {valid_floats} / {len(nz_words)}"
          f" ({valid_floats/max(len(nz_words),1)*100:.1f}%)")
    if float_vals:
        float_vals.sort()
        print(f"  Float range: {float_vals[0]:.6g} to {float_vals[-1]:.6g}")
        print(f"  Float median: {float_vals[len(float_vals)//2]:.6g}")

    # -- Check for groups-of-7 pattern (7 microphones) --
    print()
    print("  Looking for repeating patterns (groups of 7 for 7-mic array):")
    # Take consecutive non-zero words and check if every 7th is similar
    # Try 32-bit words
    sample_words = [w for w in words_32[:2048] if w != 0]
    if len(sample_words) >= 14:
        # Check period 7
        diffs_7 = []
        for i in range(0, min(70, len(sample_words)-7), 7):
            chunk = sample_words[i:i+7]
            diffs_7.append(max(chunk) - min(chunk))
        print(f"    Period-7 intra-group range (first 10 groups): "
              f"{[hex(d) for d in diffs_7]}")

    # -- Check for matrix structure: try different groupings --
    print()
    print("  Stride analysis — looking for repeating value blocks:")
    # Check if data repeats every N words
    for stride in [7, 8, 12, 16, 32, 64]:
        # Compare first block vs second block
        block_bytes = stride * 4
        if bf_len >= block_bytes * 4:
            b1 = bf_data[:block_bytes]
            b2 = bf_data[block_bytes:block_bytes*2]
            b3 = bf_data[block_bytes*2:block_bytes*3]
            matches_12 = sum(a == b for a, b in zip(b1, b2))
            matches_13 = sum(a == b for a, b in zip(b1, b3))
            print(f"    Stride {stride:3d} words ({stride*4} bytes): "
                  f"block0 vs block1 match: {matches_12/block_bytes*100:.1f}%, "
                  f"block0 vs block2: {matches_13/block_bytes*100:.1f}%")

    # -- Look at beginning of beamforming region in detail --
    print()
    print("  First 128 bytes of beamforming region (raw hex):")
    print("  " + bf_data[:128].hex())
    print()
    print("  First 32 words as float32:")
    first_floats = struct.unpack_from("<32f", bf_data)
    for i, f in enumerate(first_floats):
        print(f"    [{i:3d}] 0x{struct.unpack('<I', struct.pack('<f', f))[0]:08X} = {f:12.6g}")

    # -- Try to find the matrix dimensions --
    # If it's [N_beams × N_mics × N_freqs]:
    # 200KB = 51200 words. Common sizes: 8 beams × 7 mics × 913 freq = 51128 ≈ 51200
    print()
    total_words_bf = bf_len // 4
    print(f"  Total 32-bit words in region: {total_words_bf}")
    print("  Possible matrix factorizations [beams × mics × freqs]:")
    for n_mics in [7, 8]:
        for n_beams in [1, 8, 12, 16, 24, 36, 72]:
            remaining = total_words_bf / (n_beams * n_mics)
            if remaining == int(remaining) and 16 <= remaining <= 2048:
                print(f"    {n_beams} beams × {n_mics} mics × {int(remaining)} freq bins = {total_words_bf} words")
    # Also try complex pairs (real+imag):
    print("  Possible matrix factorizations with complex (×2 words per entry):")
    for n_mics in [7, 8]:
        for n_beams in [1, 8, 12, 16, 24, 36, 72]:
            remaining = total_words_bf / (n_beams * n_mics * 2)
            if remaining == int(remaining) and 16 <= remaining <= 2048:
                print(f"    {n_beams} beams × {n_mics} mics × {int(remaining)} freq bins (complex) = {total_words_bf} words")


# ─────────────────────────────────────────────────────────────────────────────
# 5. STRING EXTRACTION
# ─────────────────────────────────────────────────────────────────────────────

def extract_strings(data, min_len=4):
    print()
    print("=" * 72)
    print("5. STRING EXTRACTION (all ASCII strings ≥ 4 chars)")
    print("=" * 72)
    print()

    strings_found = []
    # Scan for printable ASCII runs
    printable = set(range(0x20, 0x7F)) | {0x09, 0x0A, 0x0D}
    i = 0
    while i < len(data):
        # Find start of printable run
        if data[i] in printable:
            j = i
            while j < len(data) and data[j] in printable:
                j += 1
            s = data[i:j]
            # Filter out pure whitespace and require actual characters
            try:
                s_str = s.decode("ascii")
                stripped = s_str.strip()
                if len(stripped) >= min_len:
                    page = i // PAGE_SIZE
                    offset = i % PAGE_SIZE
                    dm_addr = DM_BASE + (i // 4)
                    strings_found.append((i, page, offset, dm_addr, stripped))
            except:
                pass
            i = j
        else:
            i += 1

    print(f"  Found {len(strings_found)} strings\n")
    for abs_off, page, off, dm_addr, s in strings_found:
        # Truncate very long strings
        display = s if len(s) <= 80 else s[:77] + "..."
        print(f"  [pg {page:3d} off 0x{off:04X} DM 0x{dm_addr:06X}] {repr(display)}")


# ─────────────────────────────────────────────────────────────────────────────
# 6. PM CODE REGION ANALYSIS
# ─────────────────────────────────────────────────────────────────────────────

# Try to import disassembler
try:
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__))))
    import sharc_disasm
    HAS_DISASM = True
except ImportError:
    HAS_DISASM = False

def try_disasm_word(raw_6bytes, method_name):
    """Attempt to disassemble a 6-byte PM word."""
    if not HAS_DISASM:
        return None
    # Build a 48-bit integer from the 6 bytes (little-endian)
    val = int.from_bytes(raw_6bytes, "little")
    # Try to decode — the disassembler expects the word in canonical form
    try:
        result = sharc_disasm.decode_instruction(val)
        return result
    except:
        return None

def check_valid_sharc_instruction(word_48bit):
    """
    Heuristic check: does this 48-bit value look like a plausible SHARC instruction?
    Top 4 bits (bits 47-44) are the type field.
    Known type ranges: 0x0-0xF (but some are invalid).
    """
    type_field = (word_48bit >> 44) & 0xF
    # Types 0,1,2,3,4,5,6,7,8,9,A,B are defined for ADSP-214xx
    # Type 0xF is CALL/JUMP with 24-bit target
    # Types 0xC, 0xD, 0xE are less common
    return type_field <= 0xF

def analyze_pm_code(data):
    print()
    print("=" * 72)
    print("6. PM CODE REGION ANALYSIS")
    print("=" * 72)

    # From address mapping: PM 0x92000 → page (0x92000 - 0x80000) / 512 = 144
    pm_page = (0x92000 - DM_BASE) // DM_WORDS_PER_PAGE
    pm_byte_offset = ((0x92000 - DM_BASE) % DM_WORDS_PER_PAGE) * 4
    pm_abs_offset = pm_page * PAGE_SIZE + pm_byte_offset

    print(f"\n  Target: PM 0x92000 → page {pm_page}, byte offset 0x{pm_byte_offset:X}")
    print(f"  Absolute byte offset in dump: 0x{pm_abs_offset:X}")

    if pm_page >= NUM_PAGES:
        print("  ERROR: Page out of range!")
        return

    # Extract 384 DM32 words = 1536 bytes (from ldr analysis)
    n_dm_words = 384
    region = data[pm_abs_offset : pm_abs_offset + n_dm_words * 4]
    nz = sum(1 for b in region if b != 0)
    print(f"\n  Region ({n_dm_words} DM32 words = {n_dm_words*4} bytes): {nz/len(region)*100:.1f}% non-zero")
    print(f"  First 48 bytes (hex): {region[:48].hex()}")
    print()

    # The ldr analysis says 384 DM32 words are loaded to PM via DMA.
    # DM→PM transfer packs DM words into PM 48-bit words.
    # SHARC DM→PM DMA packs: 3 DM32 words → 2 PM48 words
    # So 384 DM32 → 256 PM48 instructions

    print("  Trying 4 DM32→PM48 packing methods on first 96 DM32 words (→ 64 PM48):")
    print()

    dm_words = struct.unpack_from(f"<{n_dm_words}I", region)

    def try_method(name, fn, num_pm=64):
        """Try a packing method, report how many look valid."""
        print(f"  --- Method {name} ---")
        pm_words = fn(dm_words[:num_pm * 3 // 2 * 2])  # ensure enough input
        valid = 0
        for i, pm in enumerate(pm_words[:num_pm]):
            is_v = check_valid_sharc_instruction(pm)
            if is_v:
                valid += 1
        print(f"  Valid-looking instructions: {valid} / {min(num_pm, len(pm_words))}")
        print(f"  First 8 PM words (48-bit hex):")
        for i, pm in enumerate(pm_words[:8]):
            type_f = (pm >> 44) & 0xF
            print(f"    PM[{i:2d}] = 0x{pm:012X}  type={type_f}")
        return pm_words

    # Method A: pm0 = w0 | ((w1 & 0xFFFF) << 32), pm1 = (w1 >> 16) | (w2 << 16)
    def method_a(dm):
        result = []
        i = 0
        while i + 2 < len(dm):
            w0, w1, w2 = dm[i], dm[i+1], dm[i+2]
            pm0 = (w0 & 0xFFFFFFFF) | (((w1 & 0xFFFF) & 0xFFFFFFFF) << 32)
            pm1 = ((w1 >> 16) & 0xFFFF) | ((w2 & 0xFFFFFFFF) << 16)
            result.append(pm0 & 0xFFFFFFFFFFFF)
            result.append(pm1 & 0xFFFFFFFFFFFF)
            i += 3
        return result

    # Method B: pm0 = (w0 << 16) | ((w1 >> 16) & 0xFFFF), pm1 = ((w1 & 0xFFFF) << 32) | w2
    def method_b(dm):
        result = []
        i = 0
        while i + 2 < len(dm):
            w0, w1, w2 = dm[i], dm[i+1], dm[i+2]
            pm0 = (((w0 & 0xFFFFFFFF) << 16) | ((w1 >> 16) & 0xFFFF)) & 0xFFFFFFFFFFFF
            pm1 = (((w1 & 0xFFFF) << 32) | (w2 & 0xFFFFFFFF)) & 0xFFFFFFFFFFFF
            result.append(pm0)
            result.append(pm1)
            i += 3
        return result

    # Method C: 12 bytes as 2 × 6-byte LE PM words
    def method_c(dm):
        result = []
        raw = b"".join(struct.pack("<I", w) for w in dm)
        i = 0
        while i + 5 < len(raw):
            chunk = raw[i:i+6]
            pm = int.from_bytes(chunk, "little")
            result.append(pm & 0xFFFFFFFFFFFF)
            i += 6
        return result

    # Method D: 12 bytes as 2 × 6-byte BE PM words
    def method_d(dm):
        result = []
        raw = b"".join(struct.pack("<I", w) for w in dm)
        i = 0
        while i + 5 < len(raw):
            chunk = raw[i:i+6]
            pm = int.from_bytes(chunk, "big")
            result.append(pm & 0xFFFFFFFFFFFF)
            i += 6
        return result

    pm_a = try_method("A  [pm0=w0|(w1&0xFFFF)<<32, pm1=(w1>>16)|(w2<<16)]", method_a)
    print()
    pm_b = try_method("B  [pm0=(w0<<16)|(w1>>16), pm1=(w1&0xFFFF)<<32|w2]", method_b)
    print()
    pm_c = try_method("C  [12 bytes → 2×6-byte LE]", method_c)
    print()
    pm_d = try_method("D  [12 bytes → 2×6-byte BE]", method_d)

    # If disassembler is available, try to decode with best method
    if HAS_DISASM:
        print()
        print("  Attempting disassembly with sharc_disasm (all 4 methods):")
        for name, pm_words in [("A", pm_a), ("B", pm_b), ("C", pm_c), ("D", pm_d)]:
            print(f"\n  Method {name} — first 16 instructions:")
            for i, pm in enumerate(pm_words[:16]):
                raw_bytes = pm.to_bytes(6, "little")
                result = None
                try:
                    result = sharc_disasm.decode_instruction(pm)
                except Exception as e:
                    result = f"<error: {e}>"
                print(f"    [{i:2d}] 0x{pm:012X}  {result}")

    # Also: look at the broader page 144 region
    print()
    print(f"  Full page {pm_page} analysis:")
    page_144 = get_page(data, pm_page)
    nz_pg = sum(1 for b in page_144 if b != 0)
    print(f"  Page {pm_page}: {nz_pg/PAGE_SIZE*100:.1f}% non-zero ({nz_pg} bytes)")
    # Surrounding pages
    for p in range(max(0, pm_page-2), min(NUM_PAGES, pm_page+10)):
        pg = get_page(data, p)
        nz2 = sum(1 for b in pg if b != 0)
        dm_a = DM_BASE + p * DM_WORDS_PER_PAGE
        marker = " ← PM 0x92000" if p == pm_page else ""
        print(f"    Page {p:3d} (DM 0x{dm_a:06X}): {nz2/PAGE_SIZE*100:6.1f}% non-zero{marker}")


# ─────────────────────────────────────────────────────────────────────────────
# BONUS: Additional structural analysis
# ─────────────────────────────────────────────────────────────────────────────

def analyze_overall_structure(data):
    print()
    print("=" * 72)
    print("BONUS: OVERALL STRUCTURE & NOTABLE REGIONS")
    print("=" * 72)
    print()

    # Find largest contiguous non-zero regions
    page_nz = []
    for p in range(NUM_PAGES):
        page = get_page(data, p)
        nz = sum(1 for b in page if b != 0)
        page_nz.append(nz)

    # Find transitions
    print("  Data density per page group (8 pages per line):")
    for row in range(0, NUM_PAGES, 8):
        densities = []
        for p in range(row, min(NUM_PAGES, row+8)):
            pct = page_nz[p] / PAGE_SIZE * 100
            if pct == 0:
                densities.append("  --")
            elif pct < 25:
                densities.append(f"{pct:4.0f}")
            elif pct < 75:
                densities.append(f"{pct:4.0f}")
            else:
                densities.append(f"{pct:4.0f}")
        row_end = min(NUM_PAGES, row+8) - 1
        dm_s = DM_BASE + row * DM_WORDS_PER_PAGE
        print(f"  pg {row:3d}-{row_end:3d} (DM 0x{dm_s:06X}): [{'|'.join(densities):}]")

    # Find any magic numbers / signatures
    print()
    print("  Searching for known SHARC signatures and magic numbers:")
    # SHARC jump-to-address pattern: opcode type 0xF (direct jump), often 0xFCxxxxxx at top
    # Also look for NOP: 0x000000000000 or 0xFFFFFFFFFFFF
    jump_count = 0
    nop_count  = 0
    for p in range(NUM_PAGES):
        page = get_page(data, p)
        # Check as 6-byte LE words
        for i in range(0, PAGE_SIZE - 5, 6):
            chunk = page[i:i+6]
            val = int.from_bytes(chunk, "little")
            top4 = (val >> 44) & 0xF
            if top4 == 0xF:
                jump_count += 1
            if val == 0 or val == 0xFFFFFFFFFFFF:
                nop_count += 1
    print(f"    Type-0xF (JUMP/CALL) pattern count (6-byte LE): {jump_count}")
    print(f"    All-zero or all-FF 6-byte groups: {nop_count}")

    # Report unique 32-bit words
    print()
    all_words = struct.unpack_from(f"<{len(data)//4}I", data)
    unique_words = len(set(all_words))
    print(f"  Total 32-bit words: {len(all_words)}")
    print(f"  Unique 32-bit values: {unique_words}")
    zero_words = sum(1 for w in all_words if w == 0)
    print(f"  Zero words: {zero_words} ({zero_words/len(all_words)*100:.1f}%)")

    # Histogram of byte values in non-empty pages
    print()
    print("  Overall byte value distribution (top 20):")
    byte_counter = collections.Counter()
    for p in range(NUM_PAGES):
        if page_nz[p] > 0:
            byte_counter.update(get_page(data, p))
    for bv, cnt in byte_counter.most_common(20):
        pct = cnt / (NUM_PAGES * PAGE_SIZE) * 100
        print(f"    0x{bv:02X} ({bv:3d}): {cnt:8d}  ({pct:.3f}%)")


# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

def main():
    print(f"Loading {DUMP_PATH}...")
    data = load_dump(DUMP_PATH)
    print(f"Loaded {len(data)} bytes ({len(data)//1024} KB), {NUM_PAGES} pages × {PAGE_SIZE} bytes")
    print(f"DM address mapping: page 0 = DM 0x{DM_BASE:06X}, page 319 = DM 0x{DM_BASE + 319*DM_WORDS_PER_PAGE:06X}")
    print(f"Disassembler available: {HAS_DISASM}")

    analyze_pages(data)
    analyze_address_mapping(data)
    analyze_sport_registers(data)
    analyze_beamforming(data)
    extract_strings(data)
    analyze_pm_code(data)
    analyze_overall_structure(data)

    print()
    print("=" * 72)
    print("ANALYSIS COMPLETE")
    print("=" * 72)


if __name__ == "__main__":
    main()
