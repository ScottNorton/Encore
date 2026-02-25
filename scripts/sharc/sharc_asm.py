#!/usr/bin/env python3
"""ADSP-21489 SHARC 48-bit instruction assembler.

Inverse of sharc_disasm.py — reads .asm assembly source, produces raw binary
in .ldr byte ordering (reversed 6-byte groups, bit-reversed bytes).

Two-pass assembly:
  Pass 1: Scan for labels, build symbol table (label → address)
  Pass 2: Encode instructions, resolve label references

Usage:
  python sharc_asm.py <input.asm> -o <output.bin>
  python sharc_asm.py <input.asm> -o <output.bin> --hex  # hex dump to stdout
"""
import sys, re, argparse

# ── Condition codes (name → 5-bit code) ──────────────────────────────
COND_REV = {
    "EQ": 0x00, "NE": 0x01, "GT": 0x02, "LT": 0x03,
    "GE": 0x04, "LE": 0x05, "AC": 0x06, "NOT AC": 0x07,
    "AV": 0x08, "NOT AV": 0x09, "MV": 0x0A, "NOT MV": 0x0B,
    "MS": 0x0C, "NOT MS": 0x0D, "SV": 0x0E, "NOT SV": 0x0F,
    "SZ": 0x10, "NOT SZ": 0x11, "TF": 0x12, "NOT TF": 0x13,
    "FLAG0_IN": 0x14, "NOT FLAG0_IN": 0x15,
    "FLAG1_IN": 0x16, "NOT FLAG1_IN": 0x17,
    "FLAG2_IN": 0x18, "NOT FLAG2_IN": 0x19,
    "FLAG3_IN": 0x1A, "NOT FLAG3_IN": 0x1B,
    "BM": 0x1C, "NOT BM": 0x1D,
    "FOREVER": 0x1E, "TRUE": 0x1F,
}

# Termination conditions for DO UNTIL
TERM_REV = dict(COND_REV)
TERM_REV["LCE"] = 0x1E  # loop counter expired (overrides FOREVER)

# ── Universal register codes (name → 7-bit code) ─────────────────────
def ureg_code(name):
    """Register name → 7-bit code."""
    name = name.strip()
    # Rn
    m = re.match(r'^R(\d+)$', name)
    if m: return int(m.group(1))
    # In
    m = re.match(r'^I(\d+)$', name)
    if m: return 0x10 | int(m.group(1))
    # Mn
    m = re.match(r'^M(\d+)$', name)
    if m: return 0x20 | int(m.group(1))
    # Ln
    m = re.match(r'^L(\d+)$', name)
    if m: return 0x30 | int(m.group(1))
    # Bn
    m = re.match(r'^B(\d+)$', name)
    if m: return 0x40 | int(m.group(1))
    # Sn (Y register file)
    m = re.match(r'^S(\d+)$', name)
    if m: return 0x50 | int(m.group(1))
    # Group 6 system regs
    grp6 = {
        "FADDR": 0x60, "DADDR": 0x61, "MODE1": 0x62, "PC": 0x63,
        "PCSTK": 0x64, "PCSTKP": 0x65, "LADDR": 0x66, "CURLCNTR": 0x67,
        "LCNTR": 0x68, "EMUCLK": 0x69, "EMUCLK2": 0x6A,
        "PX": 0x6B, "PX1": 0x6C, "PX2": 0x6D, "TPERIOD": 0x6E, "TCOUNT": 0x6F,
    }
    if name in grp6: return grp6[name]
    # Group 7 system regs
    grp7 = {
        "USTAT1": 0x70, "USTAT2": 0x71, "MMASK": 0x73,
        "MODE2": 0x74, "FLAGS": 0x75, "ASTATx": 0x76, "ASTATy": 0x77,
        "STKYx": 0x78, "STKYy": 0x79, "IRPTL": 0x7A, "IMASK": 0x7B,
        "IMASKP": 0x7C, "LIRPTL": 0x7D, "USTAT3": 0x7E, "USTAT4": 0x7F,
    }
    if name in grp7: return grp7[name]
    raise ValueError(f"Unknown register: {name}")

# ── System register codes (name → 4-bit code, for Type 18) ───────────
SREG_REV = {
    "USTAT1": 0, "USTAT2": 1, "?SR2": 2, "SR2": 2, "MMASK": 3,
    "MODE2": 4, "FLAGS": 5, "ASTATx": 6, "ASTATy": 7,
    "STKYx": 8, "STKYy": 9, "IRPTL": 10, "IMASK": 11,
    "IMASKP": 12, "LIRPTL": 13, "USTAT3": 14, "USTAT4": 15,
}

# ── Bit operation codes (name → 3-bit code) ──────────────────────────
BOP_REV = {"SET": 0, "CLR": 1, "TGL": 2, "TST": 4, "XOR": 5}


def reverse_bits(b):
    """Reverse the 8 bits of a byte."""
    r = 0
    for i in range(8):
        r = (r << 1) | ((b >> i) & 1)
    return r


def encode_word(w):
    """48-bit word → 6 bytes in .ldr format (reversed group, no bit-reversal).

    The .ldr file on disk stores bytes in swapped (LSB-first) order but NOT
    bit-reversed.  Bit-reversal is handled by the SPI hardware during transfer.
    """
    # Big-endian split: byte[0]=bits[47:40] .. byte[5]=bits[7:0]
    raw = [(w >> (40 - i*8)) & 0xFF for i in range(6)]
    # Reverse 6-byte group (LSB first)
    raw = raw[::-1]
    return bytes(raw)


def parse_hex(s):
    """Parse hex string like '0x1234' or '0x00001234' to int."""
    s = s.strip()
    if s.startswith('0x') or s.startswith('0X'):
        return int(s, 16)
    return int(s, 0)


def parse_int(s):
    """Parse integer, supporting 0x hex, decimal, and negative."""
    s = s.strip()
    if s.startswith('-'):
        return -parse_int(s[1:])
    if s.startswith('0x') or s.startswith('0X'):
        return int(s, 16)
    return int(s)


def mask(val, nbits):
    """Mask value to nbits (handles negative / sign-extended values)."""
    return val & ((1 << nbits) - 1)


class Assembler:
    def __init__(self, base_addr=0x8C000):
        self.base = base_addr
        self.symbols = {}  # label → address
        self.lines = []    # parsed (line_num, label, instruction_text)
        self.words = []    # assembled 48-bit words
        self.errors = []

    def error(self, line_num, msg):
        self.errors.append(f"Line {line_num}: {msg}")

    def parse_source(self, text):
        """Parse assembly source into (line_num, label, instruction) tuples."""
        for line_num, raw in enumerate(text.split('\n'), 1):
            # Strip comment (semicolon to EOL, but not inside .dw hex values)
            line = raw.split(';')[0].strip()
            if not line:
                continue

            label = None
            # Extract label: word followed by colon at start of line
            m = re.match(r'^(\w+):\s*(.*)', line)
            if m:
                label = m.group(1)
                line = m.group(2).strip()

            if label or line:
                self.lines.append((line_num, label, line))

    def is_directive(self, instr):
        """Check if instruction is a non-emitting directive."""
        return instr.lower().startswith('.base')

    def pass1(self):
        """Build symbol table: label → address."""
        addr = self.base
        for line_num, label, instr in self.lines:
            if label:
                if label in self.symbols:
                    self.error(line_num, f"Duplicate label: {label}")
                self.symbols[label] = addr
            # Each non-empty instruction line produces exactly one word
            # (but directives like .base don't emit)
            if instr and not self.is_directive(instr):
                addr += 1

    def resolve(self, name_or_hex, line_num):
        """Resolve a label or hex address to an integer address."""
        if name_or_hex in self.symbols:
            return self.symbols[name_or_hex]
        try:
            return parse_hex(name_or_hex)
        except ValueError:
            self.error(line_num, f"Unresolved symbol: {name_or_hex}")
            return 0

    def parse_cond(self, text):
        """Parse condition from 'IF cond' prefix, return (cond_code, remaining_text).
        If no IF prefix, returns (0x1F=TRUE, text)."""
        text = text.strip()
        m = re.match(r'^IF\s+(NOT\s+\w+|\w+)\s+', text, re.IGNORECASE)
        if m:
            cond_str = m.group(1).strip().upper()
            # Normalize whitespace in "NOT XX"
            cond_str = re.sub(r'\s+', ' ', cond_str)
            if cond_str in COND_REV:
                return COND_REV[cond_str], text[m.end():].strip()
            # Could be "IF FOREVER" used as a condition
            if cond_str == "FOREVER":
                return COND_REV["FOREVER"], text[m.end():].strip()
        return 0x1F, text  # TRUE (unconditional)

    def encode_instruction(self, text, addr, line_num):
        """Encode one assembly instruction → 48-bit word."""
        text = text.strip()
        if not text:
            return 0  # NOP

        # ── .dw raw hex directive ─────────────────────────────────────
        m = re.match(r'^\.dw\s+(0x[0-9A-Fa-f]+)', text, re.IGNORECASE)
        if m:
            return parse_hex(m.group(1))

        # ── .base directive (handled in pass1 meta, skip here) ────────
        if text.lower().startswith('.base'):
            return None  # not an instruction

        # ── NOP ───────────────────────────────────────────────────────
        if text.upper() == 'NOP':
            return 0x000000000000

        # ── IDLE ──────────────────────────────────────────────────────
        if text.upper() == 'IDLE':
            return 0x008000000000

        # ── Type 17: Ureg = 0xNNNNNNNN (immediate data → ureg) ───────
        m = re.match(r'^(\w+)\s*=\s*(0x[0-9A-Fa-f]+|\d+)\s*$', text)
        if m:
            rname = m.group(1)
            val = parse_int(m.group(2))
            try:
                ur = ureg_code(rname)
            except ValueError:
                self.error(line_num, f"Unknown register in immediate assign: {rname}")
                return 0
            # Type 17: grp=000, subop=01111, ureg@[38:32], data@[31:0]
            w = (0b000 << 45) | (0b01111 << 40) | (ur << 32) | mask(val, 32)
            return w

        # ── Type 14: Ureg = DM(addr) / DM(addr) = Ureg ──────────────
        # Also handles PM variant
        m = re.match(r'^(\w+)\s*=\s*(DM|PM)\((0x[0-9A-Fa-f]+)\)\s*$', text)
        if m:
            rname = m.group(1)
            mem = m.group(2)
            address = parse_hex(m.group(3))
            ur = ureg_code(rname)
            g = 1 if mem == "PM" else 0
            d = 0  # load: ureg = mem(addr)
            # grp=000, subop=100xx → bits[44:42]=100, g@41, d@40
            w = (0b000 << 45) | (0b100 << 42) | (g << 41) | (d << 40) | (ur << 32) | mask(address, 32)
            return w

        m = re.match(r'^(DM|PM)\((0x[0-9A-Fa-f]+)\)\s*=\s*(\w+)(\s*\(LW\))?\s*$', text)
        if m:
            mem = m.group(1)
            address = parse_hex(m.group(2))
            rname = m.group(3)
            lw = 1 if m.group(4) else 0
            ur = ureg_code(rname)
            g = 1 if mem == "PM" else 0
            d = 1  # store: mem(addr) = ureg
            w = (0b000 << 45) | (0b100 << 42) | (g << 41) | (d << 40) | (lw << 39) | (ur << 32) | mask(address, 32)
            return w

        # ── Type 14: Ureg = PM(addr) (LW) variant ────────────────────
        m = re.match(r'^(\w+)\s*=\s*(DM|PM)\((0x[0-9A-Fa-f]+)\)\s*(\(LW\))?\s*$', text)
        if m:
            rname = m.group(1)
            mem = m.group(2)
            address = parse_hex(m.group(3))
            lw = 1 if m.group(4) else 0
            ur = ureg_code(rname)
            g = 1 if mem == "PM" else 0
            d = 0
            w = (0b000 << 45) | (0b100 << 42) | (g << 41) | (d << 40) | (lw << 39) | (ur << 32) | mask(address, 32)
            return w

        # ── Type 18: BIT SET/CLR/TGL/TST sreg mask ───────────────────
        m = re.match(r'^BIT\s+(SET|CLR|TGL|TST|XOR)\s+([\?\w]+)\s+(0x[0-9A-Fa-f]+)\s*$', text, re.IGNORECASE)
        if m:
            bop = BOP_REV[m.group(1).upper()]
            sreg_name = m.group(2)
            data = parse_hex(m.group(3))
            if sreg_name not in SREG_REV:
                self.error(line_num, f"Unknown system register: {sreg_name}")
                return 0
            sreg = SREG_REV[sreg_name]
            # grp=000, subop=10100, bop@[39:37], sreg@[36:33], data@[31:0]
            w = (0b000 << 45) | (0b10100 << 40) | (bop << 37) | (sreg << 33) | mask(data, 32)
            return w

        # ── Type 8a: [IF cond] CALL/JUMP addr ────────────────────────
        cond, rest = self.parse_cond(text)
        m = re.match(r'^(CALL|JUMP)\s+(0x[0-9A-Fa-f]+|\w+)\s*$', rest, re.IGNORECASE)
        if m:
            op = m.group(1).upper()
            target = self.resolve(m.group(2), line_num)
            b = 1 if op == "CALL" else 0
            # grp=000, subop=00110, b@39, a=0@38, cond@[37:33], j=0@32, ci=0@31, addr@[23:0]
            w = (0b000 << 45) | (0b00110 << 40) | (b << 39) | (cond << 33) | mask(target, 24)
            return w

        # ── Type 8a with (DB) flag: [IF cond] CALL/JUMP addr (DB) ────
        m = re.match(r'^(CALL|JUMP)\s+(0x[0-9A-Fa-f]+|\w+)\s*\(DB\)\s*$', rest, re.IGNORECASE)
        if m:
            op = m.group(1).upper()
            target = self.resolve(m.group(2), line_num)
            b = 1 if op == "CALL" else 0
            # j=1 for (DB)
            w = (0b000 << 45) | (0b00110 << 40) | (b << 39) | (cond << 33) | (1 << 32) | mask(target, 24)
            return w

        # ── Type 8b: [IF cond] JUMP (PC,rel) ─────────────────────────
        m = re.match(r'^JUMP\s+\(PC\s*,\s*(-?\d+)\)', rest, re.IGNORECASE)
        if m:
            reladdr = int(m.group(1))
            b = 0  # JUMP, not CALL
            w = (0b000 << 45) | (0b00111 << 40) | (b << 39) | (cond << 33) | mask(reladdr, 24)
            return w

        # ── Type 11a: [IF cond] RTS ──────────────────────────────────
        m = re.match(r'^RTS\s*$', rest, re.IGNORECASE)
        if m:
            # grp=000, subop=01010, cond@[39:35], bits[34:33]=11 (reserved, always set)
            w = (0b000 << 45) | (0b01010 << 40) | (cond << 35) | (0b11 << 33)
            return w

        # ── Type 11b: [IF cond] RTI ──────────────────────────────────
        m = re.match(r'^RTI\s*$', rest, re.IGNORECASE)
        if m:
            # grp=000, subop=01011, cond@[39:35], bits[34:33]=11 (reserved, always set)
            w = (0b000 << 45) | (0b01011 << 40) | (cond << 35) | (0b11 << 33)
            return w

        # ── Type 12a: LCNTR = imm, DO addr/label UNTIL LCE ──────────
        m = re.match(r'^LCNTR\s*=\s*(\d+)\s*,\s*DO\s+(0x[0-9A-Fa-f]+|\w+)\s+UNTIL\s+LCE\s*$', text, re.IGNORECASE)
        if m:
            count = int(m.group(1))
            target = self.resolve(m.group(2), line_num)
            reladdr = target - addr
            # grp=000, subop=01100, data@[39:24]=count, reladdr@[23:0]
            w = (0b000 << 45) | (0b01100 << 40) | (mask(count, 16) << 24) | mask(reladdr, 24)
            return w

        # ── Type 12b: LCNTR = Ureg, DO addr/label UNTIL LCE ─────────
        m = re.match(r'^LCNTR\s*=\s*(\w+)\s*,\s*DO\s+(0x[0-9A-Fa-f]+|\w+)\s+UNTIL\s+LCE\s*$', text, re.IGNORECASE)
        if m:
            rname = m.group(1)
            target = self.resolve(m.group(2), line_num)
            reladdr = target - addr
            try:
                ur = ureg_code(rname)
            except ValueError:
                # Might be a decimal number — but we already tried that above
                self.error(line_num, f"Unknown register for LCNTR: {rname}")
                return 0
            # grp=000, subop=01101, ureg@[38:32], reladdr@[23:0]
            w = (0b000 << 45) | (0b01101 << 40) | (ur << 32) | mask(reladdr, 24)
            return w

        # ── Type 13: DO addr/label UNTIL cond ────────────────────────
        m = re.match(r'^DO\s+(0x[0-9A-Fa-f]+|\w+)\s+UNTIL\s+(\w+(?:\s+\w+)?)\s*$', text, re.IGNORECASE)
        if m:
            target = self.resolve(m.group(1), line_num)
            term_name = m.group(2).strip().upper()
            reladdr = target - addr
            if term_name not in TERM_REV:
                self.error(line_num, f"Unknown termination condition: {term_name}")
                return 0
            term = TERM_REV[term_name]
            # grp=000, subop=01110, term@[39:35], reladdr@[23:0]
            w = (0b000 << 45) | (0b01110 << 40) | (term << 35) | mask(reladdr, 24)
            return w

        # ── Type 20: FLUSH CACHE / PUSH/POP ──────────────────────────
        m = re.match(r'^FLUSH\s+CACHE\s*$', text, re.IGNORECASE)
        if m:
            # grp=000, subop=10111, fc bit@33
            w = (0b000 << 45) | (0b10111 << 40) | (1 << 33)
            return w

        # ── Type 5a: [IF cond] Ureg = Ureg (with group 011) ──────────
        # Pattern: IF cond Ureg = Ureg  OR  Ureg = Ureg
        cond2, rest2 = self.parse_cond(text)
        m = re.match(r'^(\w+)\s*=\s*(\w+)\s*$', rest2)
        if m:
            dst_name = m.group(1)
            src_name = m.group(2)
            # Check if both are valid uregs
            try:
                dst = ureg_code(dst_name)
                src = ureg_code(src_name)
            except ValueError:
                pass
            else:
                # Type 5a: grp=011, bit44=1, bit43=0, src@[42:36], cond@[35:31], bit30=0, dst@[29:23], compute@[22:0]=0
                w = (0b011 << 45) | (1 << 44) | (0 << 43) | (src << 36) | (cond2 << 31) | (dst << 23)
                return w

        # ── Type 3: DM(Ii,Mm) = Ureg (with compute, group 010) ───────
        # Pattern: [IF cond] DM(Ii,Mm) = Ureg
        cond3, rest3 = self.parse_cond(text)
        m = re.match(r'^(DM|PM)\(I(\d+)\s*,\s*M(\d+)\)\s*=\s*(\w+)(\s*\(LW\))?\s*$', rest3, re.IGNORECASE)
        if m:
            mem = m.group(1).upper()
            i_num = int(m.group(2))
            m_num = int(m.group(3))
            rname = m.group(4)
            lw = 1 if m.group(5) else 0
            ur = ureg_code(rname)
            g = 1 if mem == "PM" else 0
            d = 1  # store
            # For DM: i=bits[2:0] of I register, m=bits[2:0] of M register
            if g == 0:
                i_field = i_num & 7
                m_field = m_num & 7
            else:
                i_field = (i_num - 8) & 7
                m_field = (m_num - 8) & 7
            # u bit: always 1 for universal register access (Type 3)
            u = 1
            # grp=010, u@44, i@[43:41], m@[40:38], cond@[37:33], g@32, d@31, l@30, ureg@[29:23], compute@[22:0]=0
            w = (0b010 << 45) | (u << 44) | (i_field << 41) | (m_field << 38) | (cond3 << 33) | (g << 32) | (d << 31) | (lw << 30) | (ur << 23)
            return w

        # ── Type 3: Ureg = DM(Ii,Mm) ─────────────────────────────────
        m = re.match(r'^(\w+)\s*=\s*(DM|PM)\(I(\d+)\s*,\s*M(\d+)\)(\s*\(LW\))?\s*$', rest3, re.IGNORECASE)
        if m:
            rname = m.group(1)
            mem = m.group(2).upper()
            i_num = int(m.group(3))
            m_num = int(m.group(4))
            lw = 1 if m.group(5) else 0
            ur = ureg_code(rname)
            g = 1 if mem == "PM" else 0
            d = 0  # load
            if g == 0:
                i_field = i_num & 7
                m_field = m_num & 7
            else:
                i_field = (i_num - 8) & 7
                m_field = (m_num - 8) & 7
            u = 1
            w = (0b010 << 45) | (u << 44) | (i_field << 41) | (m_field << 38) | (cond3 << 33) | (g << 32) | (d << 31) | (lw << 30) | (ur << 23)
            return w

        # ── Type 6b: [IF cond] SHIFT[op] Rn = Rx BY imm ─────────────
        m = re.match(r'^SHIFT\[([0-9A-Fa-f]+)\]\s+R(\d+)\s*=\s*R(\d+)\s+BY\s+(-?\d+)\s*$', rest, re.IGNORECASE)
        if m:
            shiftop = int(m.group(1), 16)
            rn = int(m.group(2))
            rx = int(m.group(3))
            imm = int(m.group(4))
            # grp=000, subop=00010, cond@[39:35], shiftop@[21:14], imm@[13:6], rn@[5:2], rx@[8:5]
            w = (0b000 << 45) | (0b00010 << 40) | (cond << 35) | (shiftop << 14) | (mask(imm, 8) << 6) | (rn << 2) | ((rx & 0xF) << 5)
            return w

        # If nothing matched
        self.error(line_num, f"Cannot encode: {text}")
        return 0

    def pass2(self):
        """Encode all instructions, resolving labels."""
        addr = self.base
        for line_num, label, instr in self.lines:
            if not instr:
                continue
            # Skip .base directive
            if instr.lower().startswith('.base'):
                continue
            w = self.encode_instruction(instr, addr, line_num)
            if w is None:
                continue  # directive, not an instruction
            self.words.append((addr, w, line_num, instr))
            addr += 1

    def assemble(self, text):
        """Full two-pass assembly."""
        # Handle .base directive before pass1
        for line in text.split('\n'):
            stripped = line.split(';')[0].strip()
            m = re.match(r'^\.base\s+(0x[0-9A-Fa-f]+|\d+)', stripped, re.IGNORECASE)
            if m:
                self.base = parse_int(m.group(1))

        self.parse_source(text)
        self.pass1()
        self.pass2()
        return len(self.errors) == 0

    def get_binary(self):
        """Return assembled binary in .ldr byte format."""
        output = bytearray()
        for addr, w, line_num, instr in self.words:
            output.extend(encode_word(w))
        return bytes(output)

    def get_hex_listing(self):
        """Return hex listing for debugging."""
        lines = []
        for addr, w, line_num, instr in self.words:
            lines.append(f"{addr:05X}  {w:012X}  {instr}")
        return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description="SHARC 48-bit assembler")
    parser.add_argument("input", help="Assembly source file (.asm)")
    parser.add_argument("-o", "--output", required=True, help="Output binary file")
    parser.add_argument("--hex", action="store_true", help="Print hex listing to stdout")
    parser.add_argument("--base", type=lambda x: int(x, 0), default=None,
                        help="Override base address (default: from .base directive or 0x8C000)")
    args = parser.parse_args()

    with open(args.input, "r") as f:
        source = f.read()

    asm = Assembler()
    if args.base is not None:
        asm.base = args.base

    if not asm.assemble(source):
        for e in asm.errors:
            print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(1)

    binary = asm.get_binary()

    with open(args.output, "wb") as f:
        f.write(binary)

    if args.hex:
        print(asm.get_hex_listing())

    print(f"Assembled {len(asm.words)} instructions ({len(binary)} bytes) -> {args.output}")
    if asm.errors:
        print(f"Warnings: {len(asm.errors)}")
        for e in asm.errors:
            print(f"  {e}", file=sys.stderr)


if __name__ == "__main__":
    main()
