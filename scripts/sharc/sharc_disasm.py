#!/usr/bin/env python3
"""ADSP-21489 SHARC 48-bit instruction disassembler.

Based on ADSP-21160 Instruction Set Reference Rev 2.1 and
SHARC Processor Programming Reference Rev 2.4 (ADSP-214xx).

Byte ordering:
  The .ldr file stores each 6-byte instruction with bytes in REVERSED order
  (LSB first). Use --swap to reverse each 6-byte group before decoding.

  The vendor/dsp_boot_kernel.bin file is additionally byte-level bit-reversed
  (for SPI transfer). Use --raw --swap to handle that format.

  For dsp-img.ldr (raw .ldr on disk):  --swap only
  For dsp_boot_kernel.bin (extracted):  --raw --swap

Usage:
  python sharc_disasm.py dsp-img.ldr --swap --base 0x8C000
  python sharc_disasm.py dsp_boot_kernel.bin --raw --swap --base 0x8C000
"""
import sys, struct

# ── Condition codes (5-bit) ──────────────────────────────────────────
COND = {
    0x00: "EQ", 0x01: "NE", 0x02: "GT", 0x03: "LT",
    0x04: "GE", 0x05: "LE", 0x06: "AC", 0x07: "NOT AC",
    0x08: "AV", 0x09: "NOT AV", 0x0A: "MV", 0x0B: "NOT MV",
    0x0C: "MS", 0x0D: "NOT MS", 0x0E: "SV", 0x0F: "NOT SV",
    0x10: "SZ", 0x11: "NOT SZ", 0x12: "TF", 0x13: "NOT TF",
    0x14: "FLAG0_IN", 0x15: "NOT FLAG0_IN",
    0x16: "FLAG1_IN", 0x17: "NOT FLAG1_IN",
    0x18: "FLAG2_IN", 0x19: "NOT FLAG2_IN",
    0x1A: "FLAG3_IN", 0x1B: "NOT FLAG3_IN",
    0x1C: "BM", 0x1D: "NOT BM",
    0x1E: "FOREVER", 0x1F: "TRUE",
}

# Termination conditions for DO UNTIL
TERM = dict(COND)
TERM[0x1E] = "LCE"       # loop counter expired
TERM[0x1F] = "FOREVER"   # never terminate

# ── Universal register codes (8-bit) ────────────────────────────────
def ureg_name(code):
    hi = (code >> 4) & 0xF
    lo = code & 0xF
    if hi == 0: return f"R{lo}"
    if hi == 1: return f"I{lo}"
    if hi == 2: return f"M{lo}"
    if hi == 3: return f"L{lo}"
    if hi == 4: return f"B{lo}"
    if hi == 5: return f"S{lo}"  # Y register file
    if hi == 6:
        names = ["FADDR","DADDR","MODE1","PC","PCSTK","PCSTKP",
                 "LADDR","CURLCNTR","LCNTR","EMUCLK","EMUCLK2",
                 "PX","PX1","PX2","TPERIOD","TCOUNT"]
        return names[lo] if lo < len(names) else f"UREG_6_{lo}"
    if hi == 7:
        names = ["USTAT1","USTAT2","?7_2","MMASK","MODE2","FLAGS",
                 "ASTATx","ASTATy","STKYx","STKYy","IRPTL","IMASK",
                 "IMASKP","LIRPTL","USTAT3","USTAT4"]
        return names[lo] if lo < len(names) else f"UREG_7_{lo}"
    return f"UREG_{code:02x}"

# ── System register codes (4-bit, for Type 18) ──────────────────────
SREG = {
    0: "USTAT1", 1: "USTAT2", 2: "?SR2", 3: "MMASK",
    4: "MODE2", 5: "FLAGS", 6: "ASTATx", 7: "ASTATy",
    8: "STKYx", 9: "STKYy", 10: "IRPTL", 11: "IMASK",
    12: "IMASKP", 13: "LIRPTL", 14: "USTAT3", 15: "USTAT4",
}

BOP = {0: "SET", 1: "CLR", 2: "TGL", 4: "TST", 5: "XOR"}

# ── Data registers ──────────────────────────────────────────────────
def dreg(n): return f"R{n & 0xF}"
def freg(n): return f"F{n & 0xF}"

# ── DAG registers ───────────────────────────────────────────────────
def dag1_i(n): return f"I{n & 7}"
def dag1_m(n): return f"M{n & 7}"
def dag2_i(n): return f"I{(n & 7) + 8}"
def dag2_m(n): return f"M{(n & 7) + 8}"

# ── ALU opcodes ─────────────────────────────────────────────────────
ALU_OPS = {
    0x01: ("Rn","Rx + Ry"), 0x02: ("Rn","Rx - Ry"),
    0x05: ("Rn","Rx + Ry + CI"), 0x06: ("Rn","Rx - Ry + CI - 1"),
    0x09: ("Rn","(Rx + Ry)/2"), 0x0A: (None,"COMP(Rx, Ry)"),
    0x0B: (None,"COMPU(Rx, Ry)"),
    0x21: ("Rn","PASS Rx"), 0x22: ("Rn","-Rx"),
    0x25: ("Rn","Rx + CI"), 0x26: ("Rn","Rx + CI - 1"),
    0x29: ("Rn","Rx + 1"), 0x2A: ("Rn","Rx - 1"),
    0x30: ("Rn","ABS Rx"),
    0x40: ("Rn","Rx AND Ry"), 0x41: ("Rn","Rx OR Ry"),
    0x42: ("Rn","Rx XOR Ry"), 0x43: ("Rn","NOT Rx"),
    0x61: ("Rn","MIN(Rx, Ry)"), 0x62: ("Rn","MAX(Rx, Ry)"),
    0x63: ("Rn","CLIP Rx BY Ry"),
    0x81: ("Fn","Fx + Fy"), 0x82: ("Fn","Fx - Fy"),
    0x89: ("Fn","(Fx + Fy)/2"), 0x8A: (None,"COMP(Fx, Fy)"),
    0x91: ("Fn","ABS(Fx + Fy)"), 0x92: ("Fn","ABS(Fx - Fy)"),
    0xA1: ("Fn","PASS Fx"), 0xA2: ("Fn","-Fx"),
    0xA5: ("Fn","RND Fx"), 0xAD: ("Rn","MANT Fx"),
    0xB0: ("Fn","ABS Fx"), 0xBD: ("Fn","SCALB Fx BY Ry"),
    0xC1: ("Rn","LOGB Fx"), 0xC4: ("Fn","RECIPS Fx"),
    0xC5: ("Fn","RSQRTS Fx"), 0xC9: ("Rn","FIX Fx"),
    0xCA: ("Fn","FLOAT Rx"), 0xCD: ("Rn","TRUNC Fx"),
    0xD9: ("Rn","FIX Fx BY Ry"), 0xDA: ("Fn","FLOAT Rx BY Ry"),
    0xDD: ("Rn","TRUNC Fx BY Ry"),
    0xE0: ("Fn","Fx COPYSIGN Fy"), 0xE1: ("Fn","MIN(Fx, Fy)"),
    0xE2: ("Fn","MAX(Fx, Fy)"), 0xE3: ("Fn","CLIP Fx BY Fy"),
}

SHIFT_OPS = {
    0x00: ("Rn","LSHIFT Rx BY Ry"), 0x04: ("Rn","ASHIFT Rx BY Ry"),
    0x08: ("Rn","ROT Rx BY Ry"),
    0x20: ("Rn","Rn OR LSHIFT Rx BY Ry"),
    0x24: ("Rn","Rn OR ASHIFT Rx BY Ry"),
    0x40: ("Rn","FEXT Rx BY Ry"), 0x44: ("Rn","FDEP Rx BY Ry"),
    0x48: ("Rn","FEXT Rx BY Ry (SE)"), 0x4C: ("Rn","FDEP Rx BY Ry (SE)"),
    0x64: ("Rn","Rn OR FDEP Rx BY Ry"),
    0x6C: ("Rn","Rn OR FDEP Rx BY Ry (SE)"),
    0x80: ("Rn","EXP Rx"), 0x84: ("Rn","EXP Rx (EX)"),
    0x88: ("Rn","LEFTZ Rx"), 0x8C: ("Rn","LEFTO Rx"),
    0x90: ("Rn","FPACK Fx"), 0x94: ("Fn","FUNPACK Rx"),
    0xC0: ("Rn","BSET Rx BY Ry"), 0xC4: ("Rn","BCLR Rx BY Ry"),
    0xC8: ("Rn","BTGL Rx BY Ry"), 0xCC: (None,"BTST Rx BY Ry"),
}

# ── Multiplier opcodes (8-bit) ─────────────────────────────────────
# Sign modes: SS=signed*signed, UU=unsigned*unsigned, SU/US=mixed
# Result types: SSI=integer, SSF=fractional, SSFI=fractional+integer
MUL_OPS = {
    # Fixed-point: Rn = Rx * Ry
    0x00: ("Rn", "Rx * Ry (SSI)"),   0x01: ("Rn", "Rx * Ry (SSF)"),
    0x02: ("Rn", "Rx * Ry (SSFI)"),
    0x40: ("Rn", "Rx * Ry (UUI)"),   0x41: ("Rn", "Rx * Ry (UUF)"),
    0x42: ("Rn", "Rx * Ry (UUFI)"),
    0x60: ("Rn", "Rx * Ry (SUI)"),   0x61: ("Rn", "Rx * Ry (SUF)"),
    0x80: ("Rn", "Rx * Ry (USI)"),   0x81: ("Rn", "Rx * Ry (USF)"),
    # MRF accumulate: MRF = MRF + Rx * Ry
    0x04: (None, "MRF = MRF + Rx * Ry (SSI)"),
    0x05: (None, "MRF = MRF + Rx * Ry (SSF)"),
    0x06: (None, "MRF = MRF + Rx * Ry (SSFI)"),
    0x44: (None, "MRF = MRF + Rx * Ry (UUI)"),
    0x45: (None, "MRF = MRF + Rx * Ry (UUF)"),
    0x64: (None, "MRF = MRF + Rx * Ry (SUI)"),
    0x84: (None, "MRF = MRF + Rx * Ry (USI)"),
    # MRF subtract: MRF = MRF - Rx * Ry
    0x08: (None, "MRF = MRF - Rx * Ry (SSI)"),
    0x09: (None, "MRF = MRF - Rx * Ry (SSF)"),
    0x48: (None, "MRF = MRF - Rx * Ry (UUI)"),
    0x68: (None, "MRF = MRF - Rx * Ry (SUI)"),
    # MRB accumulate/subtract (back accumulator)
    0x14: (None, "MRB = MRB + Rx * Ry (SSI)"),
    0x15: (None, "MRB = MRB + Rx * Ry (SSF)"),
    0x18: (None, "MRB = MRB - Rx * Ry (SSI)"),
    0x19: (None, "MRB = MRB - Rx * Ry (SSF)"),
    # Result transfers
    0x10: ("Rn", "SAT MRF"),  0x11: ("Rn", "RND MRF"),
    0x12: ("Rn", "MR0F"),     0x13: ("Rn", "MR1F"),
    0x20: ("Rn", "SAT MRB"),  0x21: ("Rn", "RND MRB"),
    0x22: ("Rn", "MR0B"),     0x23: ("Rn", "MR1B"),
    # MR clear
    0x0C: (None, "MRF = 0"),  0x1C: (None, "MRB = 0"),
    # Floating-point multiply
    0x30: ("Fn", "Fx * Fy"),
    0x34: (None, "MRF = MRF + Fx * Fy"),
    0x38: (None, "MRF = MRF - Fx * Fy"),
    0x70: ("Fn", "Fx * Fy"),  # alternate encoding
}

def decode_multiplier(opcode, rn, rx, ry):
    """Decode multiplier operation."""
    if opcode in MUL_OPS:
        dst, op = MUL_OPS[opcode]
        op = op.replace("Rx", dreg(rx)).replace("Ry", dreg(ry))
        op = op.replace("Fx", freg(rx)).replace("Fy", freg(ry))
        if dst:
            dst = dst.replace("Rn", dreg(rn)).replace("Fn", freg(rn))
            return f"{dst} = {op}"
        return op
    return f"MUL op={opcode:02x} rn={rn} rx={rx} ry={ry}"


def decode_compute(c):
    """Decode 23-bit compute field. Returns string or None if NOP."""
    if c == 0:
        return None  # no compute (NOP)

    # Check for multifunction (bit 22 = 0 for some multifunction patterns)
    cu = (c >> 21) & 0x3   # bits[22:21]
    opcode = (c >> 13) & 0xFF  # bits[20:13]
    rn = (c >> 9) & 0xF   # bits[12:9]
    rx = (c >> 5) & 0xF   # bits[8:5]
    ry = (c >> 1) & 0xF   # bits[4:1]

    if cu == 0b00:  # ALU
        if opcode in ALU_OPS:
            dst, op = ALU_OPS[opcode]
            op = op.replace("Rx", dreg(rx)).replace("Ry", dreg(ry))
            op = op.replace("Fx", freg(rx)).replace("Fy", freg(ry))
            if dst:
                dst = dst.replace("Rn", dreg(rn)).replace("Fn", freg(rn))
                return f"{dst} = {op}"
            return op
        return f"ALU op={opcode:02x} rn={rn} rx={rx} ry={ry}"

    elif cu == 0b01:  # Multiplier
        return decode_multiplier(opcode, rn, rx, ry)

    elif cu == 0b10:  # Shifter
        if opcode in SHIFT_OPS:
            dst, op = SHIFT_OPS[opcode]
            op = op.replace("Rx", dreg(rx)).replace("Ry", dreg(ry))
            op = op.replace("Fx", freg(rx)).replace("Fy", freg(ry))
            if dst:
                dst = dst.replace("Rn", dreg(rn)).replace("Fn", freg(rn))
                return f"{dst} = {op}"
            return op
        return f"SHIFT op={opcode:02x} rn={rn} rx={rx} ry={ry}"

    else:  # cu == 0b11, could be multifunction or MR transfer
        # Check for MR register transfer: bits[22:17] = 100000
        if (c >> 17) == 0b100000:
            t = (c >> 16) & 1
            ai = (c >> 12) & 0xF
            rk = (c >> 8) & 0xF
            mr_names = {0:"MR0F",1:"MR1F",2:"MR2F",4:"MR0B",5:"MR1B",6:"MR2B"}
            mr = mr_names.get(ai, f"MR?{ai}")
            if t:
                return f"{mr} = {dreg(rk)}"
            else:
                return f"{dreg(rk)} = {mr}"
        return f"COMPUTE {c:06x}"

    return f"COMPUTE {c:06x}"


def cond_prefix(c):
    """Return condition prefix string."""
    if c == 0x1F:  # TRUE / always
        return ""
    name = COND.get(c, f"?{c}")
    return f"IF {name} "


def bits(w, hi, lo):
    """Extract bits[hi:lo] from 48-bit word (inclusive)."""
    mask = (1 << (hi - lo + 1)) - 1
    return (w >> lo) & mask


def sign_extend(val, nbits):
    """Sign-extend an n-bit value."""
    if val & (1 << (nbits - 1)):
        val -= (1 << nbits)
    return val


def disasm(w, addr):
    """Disassemble one 48-bit instruction word. Returns string."""
    if w == 0:
        return "NOP;"

    grp = bits(w, 47, 45)

    # ── Group 001: Type 1 (Compute + DM + PM) ───────────────────────
    if grp == 0b001:
        dmd = bits(w, 44, 44)
        dmi = bits(w, 43, 41)
        dmm = bits(w, 40, 38)
        dm_dreg = bits(w, 37, 34)
        pmi = bits(w, 33, 31)
        pmm = bits(w, 30, 28)
        pmd = bits(w, 27, 27)
        pm_dreg = bits(w, 26, 23)
        comp = bits(w, 22, 0)

        parts = []
        c = decode_compute(comp)
        if c: parts.append(c)

        if dmd:
            parts.append(f"DM({dag1_i(dmi)},{dag1_m(dmm)}) = {dreg(dm_dreg)}")
        else:
            parts.append(f"{dreg(dm_dreg)} = DM({dag1_i(dmi)},{dag1_m(dmm)})")

        if pmd:
            parts.append(f"PM({dag2_i(pmi)},{dag2_m(pmm)}) = {dreg(pm_dreg)}")
        else:
            parts.append(f"{dreg(pm_dreg)} = PM({dag2_i(pmi)},{dag2_m(pmm)})")

        return " , ".join(parts) + ";"

    # ── Group 010: Type 3 (Compute + ureg↔DM|PM, reg modify) ────────
    if grp == 0b010:
        u = bits(w, 44, 44)
        i = bits(w, 43, 41)
        m = bits(w, 40, 38)
        cond = bits(w, 37, 33)
        g = bits(w, 32, 32)
        d = bits(w, 31, 31)
        l = bits(w, 30, 30)
        ureg = bits(w, 29, 23)
        comp = bits(w, 22, 0)

        # ureg is 7 bits here — need to map to 8-bit code
        # For Type 3, the UREG[7:1] = bits[29:23], UREG[0] comes from context
        # Actually, many docs say it's the full code in 7 bits... treating as [6:0]
        ureg_code = ureg  # Approximate
        ur = ureg_name(ureg_code)

        mem = "PM" if g else "DM"
        ireg = dag2_i(i) if g else dag1_i(i)
        mreg = dag2_m(m) if g else dag1_m(m)
        lw = " (LW)" if l else ""

        parts = []
        c = decode_compute(comp)
        if c: parts.append(c)

        cp = cond_prefix(cond)
        if d:
            parts.append(f"{cp}{mem}({ireg},{mreg}) = {ur}{lw}")
        else:
            parts.append(f"{cp}{ur} = {mem}({ireg},{mreg}){lw}")

        return " , ".join(parts) + ";"

    # ── Group 011: Type 4 / 5a / 5b ─────────────────────────────────
    if grp == 0b011:
        b44 = bits(w, 44, 44)
        if b44 == 0:  # Type 4
            i = bits(w, 43, 41)
            g = bits(w, 40, 40)
            d = bits(w, 39, 39)
            u = bits(w, 38, 38)
            cond = bits(w, 37, 33)
            data = bits(w, 32, 27)
            drg = bits(w, 26, 23)
            comp = bits(w, 22, 0)

            mem = "PM" if g else "DM"
            ireg = dag2_i(i) if g else dag1_i(i)
            data_s = sign_extend(data, 6)

            parts = []
            c = decode_compute(comp)
            if c: parts.append(c)

            cp = cond_prefix(cond)
            if d:
                parts.append(f"{cp}{mem}({ireg},{data_s}) = {dreg(drg)}")
            else:
                parts.append(f"{cp}{dreg(drg)} = {mem}({ireg},{data_s})")

            return " , ".join(parts) + ";"

        else:  # bit[44]=1
            b43 = bits(w, 43, 43)
            if b43 == 0:  # Type 5a (ureg=ureg)
                src = bits(w, 42, 36)
                cond = bits(w, 35, 31)
                # dst and other bits...
                dst = bits(w, 29, 23)
                comp = bits(w, 22, 0)
                cp = cond_prefix(cond)
                parts = []
                c = decode_compute(comp)
                if c: parts.append(c)
                parts.append(f"{cp}{ureg_name(dst)} = {ureg_name(src)}")
                return " , ".join(parts) + ";"

            else:  # Type 5b (X↔Y swap)
                comp = bits(w, 22, 0)
                return f"TYPE5b SWAP , COMPUTE={comp:06x};"

    # ── Group 100: Type 6a / 16 ─────────────────────────────────────
    if grp == 0b100:
        b44 = bits(w, 44, 44)
        if b44 == 0:  # Type 6a (Immediate Shift + DM|PM)
            i = bits(w, 43, 41)
            m = bits(w, 40, 38)
            cond = bits(w, 37, 33)
            g = bits(w, 32, 32)
            d = bits(w, 31, 31)
            drg = bits(w, 26, 23)
            shiftop = bits(w, 21, 14)
            data = bits(w, 13, 6)
            rn = bits(w, 5, 2)

            mem = "PM" if g else "DM"
            ireg = dag2_i(i) if g else dag1_i(i)
            mreg = dag2_m(m) if g else dag1_m(m)
            data_s = sign_extend(data, 8)

            shift_str = f"SHIFT[{shiftop:02x}] {dreg(rn)} BY {data_s}"
            cp = cond_prefix(cond)

            parts = [shift_str]
            if d:
                parts.append(f"{mem}({ireg},{mreg}) = {dreg(drg)}")
            else:
                parts.append(f"{dreg(drg)} = {mem}({ireg},{mreg})")
            return f"{cp}" + " , ".join(parts) + ";"

        else:  # Type 16 (Immediate data → DM|PM)
            i = bits(w, 43, 41)
            m = bits(w, 40, 38)
            g = bits(w, 37, 37)
            data = bits(w, 31, 0)

            mem = "PM" if g else "DM"
            ireg = dag2_i(i) if g else dag1_i(i)
            mreg = dag2_m(m) if g else dag1_m(m)

            return f"{mem}({ireg},{mreg}) = 0x{data:08X};"

    # ── Group 101: Type 15 (ureg↔DM|PM indirect + imm modify) ───────
    if grp == 0b101:
        g = bits(w, 44, 44)
        i = bits(w, 43, 41)
        d = bits(w, 40, 40)
        l = bits(w, 39, 39)
        ureg = bits(w, 38, 32)
        data = bits(w, 31, 0)

        mem = "PM" if g else "DM"
        ireg = dag2_i(i) if g else dag1_i(i)
        ur = ureg_name(ureg)
        lw = " (LW)" if l else ""
        data_s = sign_extend(data, 32)

        if d:
            return f"{mem}({ireg},0x{data:08X}) = {ur}{lw};"
        else:
            return f"{ur} = {mem}({ireg},0x{data:08X}){lw};"

    # ── Group 110: Type 10a (Indirect jump + DM + compute) ──────────
    if grp == 0b110:
        d = bits(w, 44, 44)
        dmi = bits(w, 43, 41)
        dmm = bits(w, 40, 38)
        cond = bits(w, 37, 33)
        pmi = bits(w, 32, 30)
        pmm = bits(w, 29, 27)
        drg = bits(w, 26, 23)
        comp = bits(w, 22, 0)

        cp = cond_prefix(cond)
        parts = [f"{cp}JUMP ({dag2_m(pmm)},{dag2_i(pmi)})"]
        c = decode_compute(comp)
        if c: parts.append(c)
        if d:
            parts.append(f"DM({dag1_i(dmi)},{dag1_m(dmm)}) = {dreg(drg)}")
        else:
            parts.append(f"{dreg(drg)} = DM({dag1_i(dmi)},{dag1_m(dmm)})")
        return " , ELSE ".join(parts[:1]) + " , ".join([""] + parts[1:]) + ";"

    # ── Group 111: Type 10b (PC-rel jump + DM + compute) ────────────
    if grp == 0b111:
        d = bits(w, 44, 44)
        dmi = bits(w, 43, 41)
        dmm = bits(w, 40, 38)
        cond = bits(w, 37, 33)
        reladdr = sign_extend(bits(w, 32, 27), 6)
        drg = bits(w, 26, 23)
        comp = bits(w, 22, 0)

        cp = cond_prefix(cond)
        target = addr + reladdr
        parts = [f"{cp}JUMP (PC,{reladdr})  /*0x{target:05X}*/"]
        c = decode_compute(comp)
        if c: parts.append(c)
        if d:
            parts.append(f"DM({dag1_i(dmi)},{dag1_m(dmm)}) = {dreg(drg)}")
        else:
            parts.append(f"{dreg(drg)} = DM({dag1_i(dmi)},{dag1_m(dmm)})")
        return " , ".join(parts) + ";"

    # ── Group 000: subop decode ──────────────────────────────────────
    if grp == 0b000:
        subop = bits(w, 44, 40)

        # NOP / IDLE
        if subop == 0b00000:
            if bits(w, 39, 39):
                return "IDLE;"
            return "NOP;"  # with possible garbage in lower bits

        # Type 2: Conditional compute
        if subop == 0b00001:
            cond = bits(w, 39, 35)
            comp = bits(w, 22, 0)
            cp = cond_prefix(cond)
            c = decode_compute(comp)
            return f"{cp}{c or 'NOP'};"

        # Type 6b: Immediate shift (no data access)
        if subop == 0b00010:
            cond = bits(w, 39, 35)
            shiftop = bits(w, 21, 14)
            data = sign_extend(bits(w, 13, 6), 8)
            rn = bits(w, 5, 2)
            rx = bits(w, 8, 5)  # approximate
            cp = cond_prefix(cond)
            return f"{cp}SHIFT[{shiftop:02x}] {dreg(rn)} = {dreg(rx)} BY {data};"

        # Type 7: Compute, modify
        if subop == 0b00100:
            g = bits(w, 39, 39)
            cond = bits(w, 38, 34)
            i = bits(w, 33, 31)
            m = bits(w, 30, 28)
            comp = bits(w, 22, 0)
            cp = cond_prefix(cond)
            ireg = dag2_i(i) if g else dag1_i(i)
            mreg = dag2_m(m) if g else dag1_m(m)
            parts = []
            c = decode_compute(comp)
            if c: parts.append(c)
            parts.append(f"MODIFY({ireg},{mreg})")
            return f"{cp}" + " , ".join(parts) + ";"

        # Type 8a: Direct jump/call (absolute)
        if subop == 0b00110:
            b = bits(w, 39, 39)
            a = bits(w, 38, 38)
            cond = bits(w, 37, 33)
            j = bits(w, 32, 32)
            ci = bits(w, 31, 31)
            address = bits(w, 23, 0)

            op = "CALL" if b else "JUMP"
            cp = cond_prefix(cond)
            flags = ""
            if j: flags += " (DB)"
            if a: flags += " (LA)"
            if ci: flags += " (CI)"
            return f"{cp}{op} 0x{address:06X}{flags};"

        # Type 8b: Direct jump/call (PC-relative)
        if subop == 0b00111:
            b = bits(w, 39, 39)
            a = bits(w, 38, 38)
            cond = bits(w, 37, 33)
            j = bits(w, 32, 32)
            ci = bits(w, 31, 31)
            reladdr = sign_extend(bits(w, 23, 0), 24)

            op = "CALL" if b else "JUMP"
            cp = cond_prefix(cond)
            target = addr + reladdr
            flags = ""
            if j: flags += " (DB)"
            if a: flags += " (LA)"
            if ci: flags += " (CI)"
            return f"{cp}{op} (PC,{reladdr})  /*0x{target:05X}*/{flags};"

        # Type 9a: Indirect jump/call + compute
        if subop == 0b01000:
            b = bits(w, 39, 39)
            a = bits(w, 38, 38)
            cond = bits(w, 37, 33)
            pmi = bits(w, 32, 30)
            pmm = bits(w, 29, 27)
            j = bits(w, 26, 26)
            e = bits(w, 25, 25)
            ci = bits(w, 24, 24)
            comp = bits(w, 22, 0)

            op = "CALL" if b else "JUMP"
            cp = cond_prefix(cond)
            flags = ""
            if j: flags += " (DB)"
            if a: flags += " (LA)"
            if ci: flags += " (CI)"

            parts = [f"{cp}{op} ({dag2_m(pmm)},{dag2_i(pmi)}){flags}"]
            c = decode_compute(comp)
            if c:
                if e:
                    parts.append(f"ELSE {c}")
                else:
                    parts.append(c)
            return " , ".join(parts) + ";"

        # Type 9b: PC-relative jump/call + compute
        if subop == 0b01001:
            b = bits(w, 39, 39)
            a = bits(w, 38, 38)
            cond = bits(w, 37, 33)
            reladdr = sign_extend(bits(w, 32, 27), 6)
            j = bits(w, 26, 26)
            e = bits(w, 25, 25)
            ci = bits(w, 24, 24)
            comp = bits(w, 22, 0)

            op = "CALL" if b else "JUMP"
            cp = cond_prefix(cond)
            target = addr + reladdr
            flags = ""
            if j: flags += " (DB)"
            if a: flags += " (LA)"
            if ci: flags += " (CI)"

            parts = [f"{cp}{op} (PC,{reladdr})  /*0x{target:05X}*/{flags}"]
            c = decode_compute(comp)
            if c:
                if e:
                    parts.append(f"ELSE {c}")
                else:
                    parts.append(c)
            return " , ".join(parts) + ";"

        # Type 11a: RTS + compute
        if subop == 0b01010:
            cond = bits(w, 39, 35)
            j = bits(w, 32, 32)
            e = bits(w, 31, 31)
            lr = bits(w, 30, 30)
            comp = bits(w, 22, 0)
            cp = cond_prefix(cond)
            flags = ""
            if j: flags += " (DB)"
            if lr: flags += " (LR)"
            parts = [f"{cp}RTS{flags}"]
            c = decode_compute(comp)
            if c:
                if e:
                    parts.append(f"ELSE {c}")
                else:
                    parts.append(c)
            return " , ".join(parts) + ";"

        # Type 11b: RTI + compute
        if subop == 0b01011:
            cond = bits(w, 39, 35)
            j = bits(w, 32, 32)
            e = bits(w, 31, 31)
            comp = bits(w, 22, 0)
            cp = cond_prefix(cond)
            flags = ""
            if j: flags += " (DB)"
            parts = [f"{cp}RTI{flags}"]
            c = decode_compute(comp)
            if c:
                if e:
                    parts.append(f"ELSE {c}")
                else:
                    parts.append(c)
            return " , ".join(parts) + ";"

        # Type 12a: Do Until Counter (immediate)
        if subop == 0b01100:
            data = bits(w, 39, 24)
            reladdr = bits(w, 23, 0)
            target = addr + reladdr
            return f"LCNTR = {data}, DO 0x{target:05X} UNTIL LCE;"

        # Type 12b: Do Until Counter (ureg)
        if subop == 0b01101:
            ureg = bits(w, 38, 32)
            reladdr = bits(w, 23, 0)
            target = addr + reladdr
            return f"LCNTR = {ureg_name(ureg)}, DO 0x{target:05X} UNTIL LCE;"

        # Type 13: Do Until (condition)
        if subop == 0b01110:
            term = bits(w, 39, 35)
            reladdr = bits(w, 23, 0)
            target = addr + reladdr
            t = TERM.get(term, f"?{term}")
            return f"DO 0x{target:05X} UNTIL {t};"

        # Type 17: Immediate data → ureg
        if subop == 0b01111:
            ureg = bits(w, 38, 32)
            data = bits(w, 31, 0)
            return f"{ureg_name(ureg)} = 0x{data:08X};"

        # Type 14: ureg ↔ DM|PM (direct addressing)
        if (subop >> 2) == 0b100:  # bits[44:42] = 100
            g = bits(w, 41, 41)
            d = bits(w, 40, 40)
            l = bits(w, 39, 39)  # Long Word
            ureg = bits(w, 38, 32)
            address = bits(w, 31, 0)

            mem = "PM" if g else "DM"
            ur = ureg_name(ureg)
            lw = " (LW)" if l else ""

            if d:
                return f"{mem}(0x{address:08X}) = {ur}{lw};"
            else:
                return f"{ur} = {mem}(0x{address:08X}){lw};"

        # Type 18: System register bit manipulation
        if subop == 0b10100:
            bop = bits(w, 39, 37)
            sreg = bits(w, 36, 33)
            data = bits(w, 31, 0)

            op = BOP.get(bop, f"BOP{bop}")
            sr = SREG.get(sreg, f"SREG{sreg}")
            return f"BIT {op} {sr} 0x{data:08X};"

        # Type 19: I register modify / bitrev
        if subop == 0b10110:
            br = bits(w, 39, 39)
            g = bits(w, 38, 38)
            i = bits(w, 37, 35)
            data = bits(w, 31, 0)
            ireg = dag2_i(i) if g else dag1_i(i)
            data_s = sign_extend(data, 32)
            if br:
                return f"BITREV ({ireg},0x{data:08X});"
            else:
                return f"MODIFY ({ireg},0x{data:08X});"

        # Type 20: Push/Pop/Flush
        if subop == 0b10111:
            lpu = bits(w, 39, 39)
            lpo = bits(w, 38, 38)
            spu = bits(w, 37, 37)
            spo = bits(w, 36, 36)
            ppu = bits(w, 35, 35)
            ppo = bits(w, 34, 34)
            fc = bits(w, 33, 33)
            ops = []
            if lpu: ops.append("PUSH LOOP")
            if lpo: ops.append("POP LOOP")
            if spu: ops.append("PUSH STS")
            if spo: ops.append("POP STS")
            if ppu: ops.append("PUSH PCSTK")
            if ppo: ops.append("POP PCSTK")
            if fc: ops.append("FLUSH CACHE")
            return ", ".join(ops) + ";" if ops else "TYPE20 (no ops);"

        # Type 25a/b: CJUMP
        if subop == 0b11000:
            address = bits(w, 23, 0)
            return f"CJUMP 0x{address:06X};"

        # Type 25c: RFRAME
        if subop == 0b11001:
            return "RFRAME;"

        return f"TYPE? subop={subop:05b} data=0x{w:012X};"

    return f"UNKNOWN grp={grp:03b} word=0x{w:012X};"


def reverse_bits(b):
    r = 0
    for i in range(8):
        r = (r << 1) | ((b >> i) & 1)
    return r


def extract_words(data, raw=False, swap=False):
    """Extract 48-bit instruction words from binary data."""
    if raw:
        data = bytes(reverse_bits(b) for b in data)
    words = []
    for i in range(len(data) // 6):
        word_bytes = data[i*6:(i+1)*6]
        if swap:
            word_bytes = bytes(reversed(word_bytes))
        w = 0
        for b in word_bytes:
            w = (w << 8) | b
        words.append(w)
    return words


def collect_targets(words, base):
    """First pass: collect all JUMP/CALL target addresses for label generation."""
    targets = set()
    for i, w in enumerate(words):
        if w == 0:
            continue
        addr = base + i
        grp = bits(w, 47, 45)

        if grp == 0b000:
            subop = bits(w, 44, 40)
            # Type 8a: Direct jump/call (absolute)
            if subop == 0b00110:
                targets.add(bits(w, 23, 0))
            # Type 8b: PC-relative jump/call
            elif subop == 0b00111:
                reladdr = sign_extend(bits(w, 23, 0), 24)
                targets.add(addr + reladdr)
            # Type 9b: PC-relative jump/call + compute
            elif subop == 0b01001:
                reladdr = sign_extend(bits(w, 32, 27), 6)
                targets.add(addr + reladdr)
            # Type 12a: DO UNTIL (immediate counter)
            elif subop == 0b01100:
                targets.add(addr + bits(w, 23, 0))
            # Type 12b: DO UNTIL (register counter)
            elif subop == 0b01101:
                targets.add(addr + bits(w, 23, 0))
            # Type 13: DO UNTIL (condition)
            elif subop == 0b01110:
                targets.add(addr + bits(w, 23, 0))
        # Group 110/111: indirect/relative jump + DM + compute
        elif grp == 0b111:
            reladdr = sign_extend(bits(w, 32, 27), 6)
            targets.add(addr + reladdr)

    return targets


def generate_labels(targets, base, num_words):
    """Generate label names for target addresses."""
    labels = {}
    idx = 0
    for addr in sorted(targets):
        if base <= addr < base + num_words:
            labels[addr] = f"L_{addr:05X}"
            idx += 1
        # Also add labels for addresses outside our range (external targets)
        elif addr not in labels:
            labels[addr] = f"L_{addr:05X}"
    return labels


def main():
    import argparse
    parser = argparse.ArgumentParser(
        description="SHARC 48-bit disassembler",
        epilog="Byte ordering: use --swap for .ldr files, --raw --swap for bit-reversed extracts")
    parser.add_argument("input", help="Binary input file")
    parser.add_argument("--raw", action="store_true",
                        help="Apply bit-reversal (for files that are byte-level bit-reversed)")
    parser.add_argument("--swap", action="store_true",
                        help="Reverse 6-byte word groups (needed for .ldr byte ordering)")
    parser.add_argument("--base", type=lambda x: int(x, 0), default=0x8C000,
                        help="Base address (default 0x8C000)")
    parser.add_argument("--labels", action="store_true",
                        help="Generate labels for JUMP/CALL targets")
    parser.add_argument("-o", "--output", help="Output file (default stdout)")
    args = parser.parse_args()

    with open(args.input, "rb") as f:
        data = f.read()

    words = extract_words(data, raw=args.raw, swap=args.swap)
    base = args.base

    # Label generation (first pass)
    labels = {}
    if args.labels:
        targets = collect_targets(words, base)
        labels = generate_labels(targets, base, len(words))

    lines = []
    valid = 0
    fail = 0

    for i, w in enumerate(words):
        addr = base + i
        hexstr = f"{w:012X}"

        # Add label line if this address is a target
        if addr in labels and args.labels:
            lines.append(f"\n{labels[addr]}:")

        try:
            asm = disasm(w, addr)
            # Substitute absolute addresses with labels
            if args.labels:
                for target_addr, label in labels.items():
                    hex_addr = f"0x{target_addr:06X}"
                    short_hex = f"0x{target_addr:05X}"
                    if hex_addr in asm:
                        asm = asm.replace(hex_addr, label)
                    elif short_hex in asm:
                        asm = asm.replace(short_hex, label)
            if "UNKNOWN" in asm or "TYPE?" in asm:
                fail += 1
            else:
                valid += 1
        except Exception as e:
            asm = f"ERROR: {e}"
            fail += 1

        lines.append(f"{addr:05X}  {hexstr}  {asm}")

    mode = []
    if args.raw: mode.append("raw+bitrev")
    if args.swap: mode.append("swap")
    mode_str = " + ".join(mode) if mode else "none"

    header = f"; SHARC Disassembly\n"
    header += f"; Source: {args.input}\n"
    header += f"; Mode: {mode_str}\n"
    header += f"; Instructions: {len(words)} ({valid} decoded, {fail} failed)\n"
    header += f"; Base address: 0x{base:05X}\n"
    if labels:
        header += f"; Labels: {len(labels)}\n"
    header += ";\n"

    output = header + "\n".join(lines) + "\n"

    if args.output:
        with open(args.output, "w") as f:
            f.write(output)
        print(f"Wrote {args.output} ({valid} decoded, {fail} failed"
              f"{f', {len(labels)} labels' if labels else ''})")
    else:
        print(output)


if __name__ == "__main__":
    main()
