#!/usr/bin/env python3
"""Disassemble cortana_mcu.bin (TI MSP430FR5739) with full annotations.

Requires: msp430-objdump (apt install binutils-msp430)
Input:  vendor/firmware/squashfs-root/usr/share/mcu/cortana_mcu.bin
Output: vendor/firmware/cortana_mcu_disasm_proper.txt

The binary has three regions:
  0xCC00-0xD1FF  Data tables (config, LED LUT, strings, command tables)
  0xD200-0xFF7F  Executable code
  0xFF80-0xFFFF  Interrupt Vector Table

msp430-objdump chokes on the data region (0xCC00-0xD1FF) because certain
byte patterns cause it to stop decoding.  We work around this by:
  1. Dumping 0xCC00-0xD1FF as annotated hex (with string extraction)
  2. Disassembling 0xD200-0xFF7F as code (works perfectly)
  3. Decoding 0xFF80-0xFFFF as the IVT
"""

import struct
import re
import subprocess
import os

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, '..', '..'))

BINFILE = os.path.join(REPO_ROOT, 'vendor/firmware/squashfs-root/usr/share/mcu/cortana_mcu.bin')
OUTFILE = os.path.join(REPO_ROOT, 'vendor/firmware/cortana_mcu_disasm_proper.txt')
TMPELF_STEP1 = '/tmp/cortana_mcu_step1.elf'
TMPELF_STEP2 = '/tmp/cortana_mcu_step2.elf'

BASE = 0xCC00
CODE_START = 0xD200  # objdump works cleanly from here
IVT_START = 0xFF80

# MSP430FR5739 peripheral register map
PERIPHERALS = {
    # Port 1
    0x0200: 'P1IN', 0x0201: 'P1OUT', 0x0202: 'P1DIR', 0x0203: 'P1REN',
    0x0204: 'P1SEL0', 0x0205: 'P1SEL1', 0x020A: 'P1IES', 0x020C: 'P1IE',
    0x020E: 'P1IFG',
    # Port 2
    0x0220: 'P2IN', 0x0221: 'P2OUT', 0x0222: 'P2DIR', 0x0223: 'P2REN',
    0x0224: 'P2SEL0', 0x0225: 'P2SEL1',
    # Port 3
    0x0240: 'P3IN', 0x0241: 'P3OUT', 0x0242: 'P3DIR', 0x0243: 'P3REN',
    0x0244: 'P3SEL0', 0x0245: 'P3SEL1',
    # Port 4
    0x0260: 'P4IN', 0x0261: 'P4OUT', 0x0262: 'P4DIR', 0x0263: 'P4REN',
    0x0264: 'P4SEL0', 0x0265: 'P4SEL1',
    # Port J
    0x0320: 'PJIN', 0x0322: 'PJOUT', 0x0324: 'PJDIR', 0x0326: 'PJREN',
    0x032A: 'PJSEL0', 0x032C: 'PJSEL1',
    # Timer0_A3
    0x0340: 'TA0CTL', 0x0342: 'TA0CCTL0', 0x0344: 'TA0CCTL1',
    0x0346: 'TA0CCTL2', 0x0350: 'TA0R', 0x0352: 'TA0CCR0',
    0x0354: 'TA0CCR1', 0x0356: 'TA0CCR2', 0x0360: 'TA0EX0', 0x036E: 'TA0IV',
    # Timer1_A3
    0x0380: 'TA1CTL', 0x0382: 'TA1CCTL0', 0x0384: 'TA1CCTL1',
    0x0386: 'TA1CCTL2', 0x0390: 'TA1R', 0x0392: 'TA1CCR0',
    0x0394: 'TA1CCR1', 0x0396: 'TA1CCR2', 0x03A0: 'TA1EX0', 0x03AE: 'TA1IV',
    # Timer0_B7
    0x03C0: 'TB0CTL', 0x03C2: 'TB0CCTL0', 0x03C4: 'TB0CCTL1',
    0x03C6: 'TB0CCTL2', 0x03D0: 'TB0R', 0x03D2: 'TB0CCR0',
    0x03D4: 'TB0CCR1', 0x03D6: 'TB0CCR2', 0x03E0: 'TB0EX0', 0x03EE: 'TB0IV',
    # WDT_A (base 0x01C0, WDTCTL at offset 0x0C)
    0x01CC: 'WDTCTL',
    # PMM
    0x0120: 'PMMCTL0', 0x0122: 'PMMCTL1', 0x0124: 'PMMCTL2',
    0x0130: 'PMMIFG', 0x0132: 'PM5CTL0',
    # CS (Clock System)
    0x0160: 'CSCTL0', 0x0162: 'CSCTL1', 0x0164: 'CSCTL2',
    0x0166: 'CSCTL3', 0x0168: 'CSCTL4', 0x016A: 'CSCTL5', 0x016C: 'CSCTL6',
    # FRAM Controller
    0x01A0: 'FRCTL0', 0x01A2: 'GCCTL0', 0x01A4: 'GCCTL1',
    # eUSCI_A0 (UART debug console)
    0x0500: 'UCA0CTLW0', 0x0502: 'UCA0CTLW1', 0x0506: 'UCA0BRW',
    0x0508: 'UCA0MCTLW', 0x050A: 'UCA0STATW', 0x050C: 'UCA0RXBUF',
    0x050E: 'UCA0TXBUF', 0x051A: 'UCA0IE', 0x051C: 'UCA0IFG', 0x051E: 'UCA0IV',
    # eUSCI_B0 (I2C slave, addr 0x36)
    0x0540: 'UCB0CTLW0', 0x0542: 'UCB0CTLW1', 0x0546: 'UCB0BRW',
    0x0548: 'UCB0STATW', 0x054A: 'UCB0TBCNT', 0x054C: 'UCB0RXBUF',
    0x054E: 'UCB0TXBUF', 0x0560: 'UCB0I2COA0', 0x0562: 'UCB0I2COA1',
    0x0564: 'UCB0I2COA2', 0x0566: 'UCB0I2COA3', 0x056A: 'UCB0I2CSA',
    0x056C: 'UCB0IE', 0x056E: 'UCB0IFG', 0x0570: 'UCB0IV',
    # ADC10_B
    0x0700: 'ADC10CTL0', 0x0702: 'ADC10CTL1', 0x0704: 'ADC10CTL2',
    0x070A: 'ADC10MCTL0', 0x0712: 'ADC10MEM0',
    # Comp_D
    0x08C0: 'CDCTL0', 0x08C2: 'CDCTL1', 0x08C4: 'CDCTL2', 0x08C6: 'CDCTL3',
    # Info Memory
    0x1800: 'TLV_START', 0x1840: 'TAG_DCO_30', 0x1880: 'INFO_C', 0x18C0: 'INFO_D',
}

# Interrupt Vector Table (0xFF80-0xFFFF)
IVT_LABELS = {
    0xFFC4: 'RTC',          0xFFC6: 'Port4',        0xFFC8: 'Port3',
    0xFFCA: 'Timer3B_CCR0', 0xFFCC: 'Timer3B_CCRx', 0xFFCE: 'Port2',
    0xFFD0: 'Timer2B_CCR0', 0xFFD2: 'Timer2B_CCRx', 0xFFD4: 'Port1',
    0xFFD6: 'Timer1A_CCR0', 0xFFD8: 'Timer1A_CCRx', 0xFFDA: 'DMA',
    0xFFDC: 'eUSCI_A1',     0xFFDE: 'Timer0A_CCR0', 0xFFE0: 'Timer0A_CCRx',
    0xFFE2: 'ADC',          0xFFE4: 'eUSCI_B0',     0xFFE6: 'eUSCI_A0',
    0xFFE8: 'WDT',          0xFFEA: 'Timer0B_CCR0', 0xFFEC: 'Timer0B_CCRx',
    0xFFEE: 'CompD',        0xFFF0: 'UserNMI',      0xFFF2: 'Rsvd_FFF2',
    0xFFF4: 'Rsvd_FFF4',    0xFFF6: 'Rsvd_FFF6',    0xFFF8: 'Rsvd_FFF8',
    0xFFFA: 'Rsvd_FFFA',    0xFFFC: 'SysNMI',       0xFFFE: 'Reset',
}


def extract_strings(data, base, start_off, end_off, min_len=4):
    """Find null-terminated ASCII strings in a byte range."""
    strings = []
    cur = b''
    cur_start = None
    for i in range(start_off, end_off):
        b = data[i]
        if 0x20 <= b < 0x7F or b in (0x0A, 0x0D, 0x09):
            if cur_start is None:
                cur_start = i
            cur += bytes([b])
        else:
            if len(cur) >= min_len and b == 0:
                strings.append((base + cur_start, cur.decode('ascii', errors='replace')))
            cur = b''
            cur_start = None
    return strings


def annotate_data_region(data, base, start, end):
    """Produce annotated hex dump for the data region."""
    lines = []
    off = start - base
    end_off = end - base

    # Find strings first for annotation
    strings = extract_strings(data, base, off, end_off, min_len=3)
    string_map = {}
    for addr, s in strings:
        string_map[addr] = s

    while off < end_off:
        addr = base + off
        # 16 bytes per line
        chunk = data[off:off+16]
        hex_parts = ' '.join(f'{b:02x}' for b in chunk)
        ascii_parts = ''.join(chr(b) if 0x20 <= b < 0x7F else '.' for b in chunk)

        # Check if this line has a string start
        annotation = ''
        for sa, ss in strings:
            if addr <= sa < addr + 16:
                annotation = f'  ; "{ss}"'
                break

        # Check for FRAM pointers (little-endian 16-bit values pointing into FRAM)
        ptrs = []
        for i in range(0, min(len(chunk), 16), 2):
            if i + 1 < len(chunk):
                w = struct.unpack('<H', chunk[i:i+2])[0]
                if CODE_START <= w < IVT_START:
                    ptrs.append(f'{w:#06x}')
        if ptrs and not annotation:
            annotation = f'  ; ptr: {", ".join(ptrs)}'

        lines.append(f'    {addr:04x}:  {hex_parts:<48s} {ascii_parts}{annotation}')
        off += 16

    return lines


def main():
    with open(BINFILE, 'rb') as f:
        data = f.read()

    SIZE = len(data)
    print(f'Binary: {SIZE} bytes ({SIZE:#x}), address {BASE:#06x}-{BASE + SIZE - 1:#06x}')

    # Step 1: Create ELF wrapper (two-step for section rename)
    print('Creating ELF wrapper...')
    subprocess.run([
        'msp430-objcopy', '-I', 'binary', '-O', 'elf32-msp430',
        '--change-section-address', f'.data={BASE:#x}',
        BINFILE, TMPELF_STEP1,
    ], check=True)
    subprocess.run([
        'msp430-objcopy',
        '--rename-section', '.data=.text,contents,alloc,load,readonly,code',
        TMPELF_STEP1, TMPELF_STEP2,
    ], check=True)

    # Step 2: Disassemble code region only (0xD200-0xFF7F)
    print('Disassembling code region...')
    result = subprocess.run([
        'msp430-objdump', '-D', '-m', 'msp430',
        f'--start-address={CODE_START:#x}',
        f'--stop-address={IVT_START:#x}',
        TMPELF_STEP2,
    ], capture_output=True, text=True)

    # Step 3: Parse instructions and extract targets
    instructions = []
    call_targets = set()
    branch_targets = set()

    for line in result.stdout.strip().split('\n'):
        m = re.match(r'\s+([0-9a-f]+):\s+(.+)', line)
        if m:
            addr = int(m.group(1), 16)
            rest = m.group(2)
            instructions.append((addr, rest, line))

            cm = re.search(r'call\s+#-?\d+\s+;\s*#0x([0-9a-f]+)', rest)
            if cm:
                call_targets.add(int(cm.group(1), 16))

            bm = re.search(r'br\s+#0x([0-9a-f]+)', rest)
            if bm:
                branch_targets.add(int(bm.group(1), 16))

    print(f'  {len(instructions)} instructions, {len(call_targets)} calls, {len(branch_targets)} branches')

    # Step 4: Build ISR map from IVT
    isr_targets = {}
    for vec_addr, name in IVT_LABELS.items():
        off = vec_addr - BASE
        target = struct.unpack('<H', data[off:off+2])[0]
        if target != 0 and target != 0xFFFF and BASE <= target < BASE + SIZE:
            isr_targets.setdefault(target, []).append(name)

    # Step 5: Build label map
    labels = {}
    for addr, names in isr_targets.items():
        if len(names) > 3:
            # Many vectors pointing to same address = default trap handler
            labels[addr] = 'ISR_default_trap'
        else:
            labels[addr] = 'ISR_' + '_'.join(names)

    for addr in sorted(call_targets):
        if addr not in labels and BASE <= addr < BASE + SIZE:
            labels[addr] = f'func_{addr:04x}'

    for addr in sorted(branch_targets):
        if addr not in labels and BASE <= addr < BASE + SIZE:
            labels[addr] = f'loc_{addr:04x}'

    # Build xref map
    xrefs = {}
    for addr, rest, _ in instructions:
        for pattern in [r'call\s+#-?\d+\s+;\s*#0x([0-9a-f]+)', r'br\s+#0x([0-9a-f]+)']:
            cm = re.search(pattern, rest)
            if cm:
                t = int(cm.group(1), 16)
                xrefs.setdefault(t, []).append(addr)

    print(f'  {len(labels)} labels ({len(isr_targets)} ISR, {len(call_targets)} func, {len(branch_targets)} loc)')

    # Step 6: Extract strings from data region
    data_strings = extract_strings(data, BASE, 0, CODE_START - BASE, min_len=3)
    print(f'  {len(data_strings)} strings in data region')

    # Step 7: Write annotated disassembly
    print(f'Writing {OUTFILE}...')
    with open(OUTFILE, 'w') as out:
        # ---- Header ----
        out.write('; ==========================================================================\n')
        out.write('; cortana_mcu.bin  --  TI MSP430FR5739 MCU Firmware\n')
        out.write('; Harman Kardon Invoke Smart Speaker\n')
        out.write('; ==========================================================================\n')
        out.write(f'; Binary size : {SIZE} bytes ({SIZE:#x})\n')
        out.write(f'; FRAM range  : {BASE:#06x} - {BASE + SIZE - 1:#06x}\n')
        out.write('; Tool        : msp430-objdump -D -m msp430\n')
        out.write(';\n')
        out.write('; Memory Map (MSP430FR5739):\n')
        out.write(';   0x0000-0x001F  Special Function Registers\n')
        out.write(';   0x0100-0x015F  Peripheral modules (8-bit)\n')
        out.write(';   0x0160-0x09FF  Peripheral modules (16-bit)\n')
        out.write(';   0x1800-0x18FF  Info Memory (TLV calibration data)\n')
        out.write(';   0x1C00-0x1FFF  SRAM (1 KB)\n')
        out.write(';   0x2000-0x23FF  SRAM (extended, 1 KB)\n')
        out.write(';   0xC200-0xCBFF  Data FRAM (2.5 KB, NOT in this binary)\n')
        out.write(';   0xCC00-0xD1FF  Code FRAM: data tables (this binary)\n')
        out.write(';   0xD200-0xFF7F  Code FRAM: executable code (this binary)\n')
        out.write(';   0xFF80-0xFFFF  Interrupt Vector Table\n')
        out.write(';\n')
        out.write('; Key Peripherals Used:\n')
        out.write(';   eUSCI_A0 (0x0500) -- UART debug console (TX/RX ring buffer)\n')
        out.write(';   eUSCI_B0 (0x0540) -- I2C slave at address 0x36 (SoC commands)\n')
        out.write(';   Timer0_A (0x0340) -- General purpose timer\n')
        out.write(';   Timer1_A (0x0380) -- LED animation frame timer\n')
        out.write(';   CS (0x0160)       -- Clock system configuration\n')
        out.write(';\n')

        # ---- IVT Summary ----
        out.write('; ==========================================================================\n')
        out.write('; INTERRUPT VECTOR TABLE SUMMARY\n')
        out.write('; ==========================================================================\n')
        for vec_addr in sorted(IVT_LABELS.keys()):
            name = IVT_LABELS[vec_addr]
            off = vec_addr - BASE
            target = struct.unpack('<H', data[off:off+2])[0]
            if target == 0xFFFF:
                out.write(f';   [{vec_addr:#06x}] {name:20s} = 0xFFFF (unused)\n')
            elif target == 0:
                out.write(f';   [{vec_addr:#06x}] {name:20s} = 0x0000 (null)\n')
            else:
                lbl = labels.get(target, '')
                if lbl:
                    lbl = f'  ({lbl})'
                out.write(f';   [{vec_addr:#06x}] {name:20s} = {target:#06x}{lbl}\n')
        out.write(';\n')

        # ---- Function Index ----
        out.write('; ==========================================================================\n')
        out.write('; FUNCTION INDEX\n')
        out.write('; ==========================================================================\n')
        for addr in sorted(labels.keys()):
            lbl = labels[addr]
            if lbl.startswith('ISR_') or lbl.startswith('func_'):
                refs = xrefs.get(addr, [])
                ref_str = ''
                if refs:
                    ref_str = f'  (from {", ".join(f"{r:#06x}" for r in sorted(refs)[:8])})'
                out.write(f';   {addr:#06x}  {lbl}{ref_str}\n')
        out.write(';\n')

        # ---- String Table ----
        out.write('; ==========================================================================\n')
        out.write('; STRINGS (in data region 0xCC00-0xD1FF)\n')
        out.write('; ==========================================================================\n')
        for addr, s in data_strings:
            out.write(f';   {addr:#06x}  "{s}"\n')
        out.write(';\n\n')

        # ---- Data Region ----
        out.write('; ==========================================================================\n')
        out.write('; DATA REGION (0xCC00-0xD1FF) -- hex dump\n')
        out.write('; ==========================================================================\n')
        out.write('; 0xCC00-0xCC8F: Configuration/calibration data\n')
        out.write('; 0xCC90-0xCCCF: Misc tables (LED indices, addresses)\n')
        out.write('; 0xCCD0-0xCE8F: LED brightness lookup table (gamma?)\n')
        out.write('; 0xCE90-0xCFFF: More lookup tables\n')
        out.write('; 0xD000-0xD0DF: ASCII strings (UART commands, debug)\n')
        out.write('; 0xD0E0-0xD1FF: Command dispatch tables, init data\n')
        out.write(';\n')
        for line in annotate_data_region(data, BASE, BASE, CODE_START):
            out.write(line + '\n')
        out.write('\n')

        # ---- Code Region ----
        out.write('; ==========================================================================\n')
        out.write('; CODE REGION (0xD200-0xFF7F)\n')
        out.write('; ==========================================================================\n\n')

        for idx, (addr, rest, line) in enumerate(instructions):
            # Label
            if addr in labels:
                lbl = labels[addr]
                out.write('\n')
                refs = xrefs.get(addr, [])
                if refs:
                    out.write(f'; xrefs: {", ".join(f"{r:#06x}" for r in sorted(refs))}\n')
                out.write(f'{lbl}:  ; {addr:#06x}\n')

            # Annotate peripheral accesses
            annotation = ''
            for pmatch in re.finditer(r'&0x([0-9a-f]{4})', rest):
                paddr = int(pmatch.group(1), 16)
                if paddr in PERIPHERALS:
                    annotation += f'  ; {PERIPHERALS[paddr]}'
            for imatch in re.finditer(r'#0x([0-9a-f]{4})', rest):
                ival = int(imatch.group(1), 16)
                if ival in PERIPHERALS and 'call' not in rest and 'br' not in rest:
                    if PERIPHERALS[ival] not in annotation:
                        annotation += f'  ; -> {PERIPHERALS[ival]}'

            # Annotate call/branch targets with label names
            for pattern in [r'#0x([0-9a-f]+)']:
                for tm in re.finditer(pattern, rest):
                    t = int(tm.group(1), 16)
                    if t in labels and ('call' in rest or 'br' in rest):
                        annotation += f'  ; -> {labels[t]}'

            out.write(f'{line.rstrip()}{annotation}\n')

        # ---- IVT Decoded ----
        out.write('\n; ==========================================================================\n')
        out.write('; INTERRUPT VECTOR TABLE (0xFF80-0xFFFF)\n')
        out.write('; ==========================================================================\n')
        ivt_off = IVT_START - BASE
        for addr in range(IVT_START, BASE + SIZE, 2):
            off = addr - BASE
            target = struct.unpack('<H', data[off:off+2])[0]
            name = IVT_LABELS.get(addr, '')
            if name:
                tgt_lbl = labels.get(target, '')
                if target == 0xFFFF:
                    out.write(f'    {addr:04x}:  {target:04x}        ; {name} (unused/erased)\n')
                else:
                    arrow = f' -> {tgt_lbl}' if tgt_lbl else ''
                    out.write(f'    {addr:04x}:  {target:04x}        ; {name}{arrow}\n')
            else:
                out.write(f'    {addr:04x}:  {target:04x}\n')

        out.write('\n; === END OF DISASSEMBLY ===\n')

    with open(OUTFILE) as f:
        total_lines = sum(1 for _ in f)
    print(f'Done! {total_lines} lines written to {OUTFILE}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
