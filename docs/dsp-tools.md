# DSP Development Guide

Tools and workflow for analyzing, disassembling, and writing custom firmware
for the ADSP-21489 SHARC DSP on the Harman Kardon Invoke amplifier board.

> **Status**: The DSP has been fully reverse-engineered at the boot/communication
> level. Custom firmware upload is working. Audio processing internals
> (beamforming coefficients, filter topology, SPORT/TDM routing) are partially
> mapped and still under active research.

## Background

The Invoke's amplifier board contains an Analog Devices ADSP-21489 SHARC DSP
running at 450 MHz. It handles microphone beamforming, noise suppression, wake
word detection, and audio routing. The DSP has no persistent storage — its
firmware is uploaded over SPI from the ARM SoC on every cold boot.

This means:
- **The DSP cannot be bricked.** Power cycle and it's back to factory.
- **Custom firmware is safe to experiment with.** Upload fails just mean the
  stock firmware loads next boot.
- **All SRAM is volatile.** Nothing you write survives a reboot unless you
  modify the upload process.

For full hardware details, memory maps, and SPI protocol documentation, see
[dsp-reference.md](dsp-reference.md).

## Tools Overview

All tools are in `scripts/` and are standalone Python 3 scripts with no
external dependencies.

| Tool | Purpose |
|------|---------|
| `sharc_disasm.py` | Disassemble SHARC 48-bit instructions → assembly text |
| `sharc_asm.py` | Assemble SHARC assembly text → binary (inverse of above) |
| `ldr_parse.py` | Parse .ldr firmware files — block types, addresses, sizes |
| `ldr_build.py` | Build/patch .ldr firmware files from components |
| `ldr_disasm_pm.py` | Extract and disassemble all program memory from .ldr |
| `disasm_firmware.py` | Disassemble full SRAM dump (dsp_dump.bin) as PM48 words |
| `disasm_ldr_full.py` | Parse .ldr, reconstruct DM image, classify + disassemble all regions |
| `analyze_dsp_dump.py` | Analyze a 320-page SRAM memory dump from a running DSP |
| `dsp_dump_memory.py` | On-device tool: dump all 320 SRAM pages via SPI to file |
| `dsp_probe2.py` | On-device tool: focused DSP extraction with GPIO handshake |

## Quick Start

### Disassemble the stock firmware

```bash
# Parse the .ldr file to see its block structure
python scripts/ldr/ldr_parse.py vendor/firmware/squashfs-root/usr/share/dsp/dsp-img.ldr

# Disassemble the boot kernel (first 256 instructions)
python scripts/sharc/sharc_disasm.py vendor/firmware/squashfs-root/usr/share/dsp/dsp-img.ldr \
  --swap --base 0x8C000

# Extract and disassemble ALL program memory
python scripts/ldr/ldr_disasm_pm.py vendor/firmware/squashfs-root/usr/share/dsp/dsp-img.ldr
```

### Understand the firmware structure

The .ldr file contains:

1. **Boot kernel** (1536 bytes, 256 instructions) — a block loader at
   address 0x8C000 that reads subsequent blocks from SPI
2. **Data blocks** — loaded into DM (32-bit) or PM (48-bit) memory regions
3. **Final block** — triggers DMA and jumps to the application entry point

```bash
# See all blocks with types and addresses
python scripts/ldr/ldr_parse.py vendor/firmware/squashfs-root/usr/share/dsp/dsp-img.ldr --all
```

Block types:

| Type | Name | Data |
|------|------|------|
| 0 | FINAL | 384 DM words, DMA'd to PM, jumps to entry |
| 1 | ZERO_DM32 | Zero-fills a DM region (no payload) |
| 3 | LOAD_DM32 | Loads 32-bit words into DM |
| 5 | LOAD_PM48 | Loads 48-bit words into PM (packed as 3 DM32 → 2 PM48) |

### Analyze a live SRAM dump

If you have access to the device, dump the DSP's SRAM through the web API or
the MCU driver, then analyze it:

```bash
python scripts/dsp/analyze_dsp_dump.py
```

This produces:
- Page activity map (which memory regions are in use)
- Beamforming gain matrix analysis (7-mic array coefficients)
- SPORT/TDM register pattern search
- String extraction (RTOS error messages, debug strings)
- PM code region identification

## Writing Custom DSP Firmware

### Toolchain

No commercial tools required. The assembler and disassembler in this repo are
sufficient for writing and verifying SHARC code.

```bash
# Write assembly
vim my_firmware.asm

# Assemble to binary
python scripts/sharc/sharc_asm.py my_firmware.asm -o my_firmware.bin --hex

# Verify by disassembling the output
python scripts/sharc/sharc_disasm.py my_firmware.bin --swap --base 0x8C000

# Compare: original assembly should match disassembly
```

### Assembly syntax

The assembler uses the same syntax the disassembler produces, so round-tripping
works. Labels, conditional jumps, register transfers, and raw hex directives
are supported:

```asm
; Example: simple loop
_start:
        R0 = 0x0;
        R1 = 0x100;
_loop:
        DM(I0, M0) = R0;
        R0 = R0 + 1;
        IF NE JUMP _loop;
        RTS;
```

Supported instruction types:

| Type | Example |
|------|---------|
| NOP / IDLE | `NOP;` `IDLE;` |
| Register immediate | `R0 = 0x1234;` |
| Register transfer | `R0 = R1;` |
| DM/PM indexed | `DM(I0, M0) = R0;` `R1 = PM(I8, M8);` |
| DM/PM direct | `DM(0x1234) = R0;` |
| JUMP / CALL | `JUMP _label;` `CALL _label (DB);` |
| RTS / RTI | `RTS;` `RTI;` |
| DO UNTIL | `LCNTR = R0, DO _end UNTIL LCE;` |
| Bit operations | `BIT SET MODE1 0x1000;` |
| Raw hex | `.dw 0x000000000000` |

### Uploading to the device

Encore's DSP subsystem uploads firmware via SPI at boot. It searches these
paths in order: `/usr/share/dsp/dsp-img.ldr`, `/media/usb/dsp-img.ldr`,
`/data/test/dsp-img.ldr`.

The stock firmware lives on the **read-only** SquashFS rootfs at
`/usr/share/dsp/dsp-img.ldr` — you cannot overwrite it at runtime. To test
custom firmware, place it at an alternate path:

```bash
# Upload custom firmware to writable storage (device must be running)
cat my_firmware.ldr | ssh root@<DEVICE_IP> \
  'mkdir -p /data/test && cat > /data/test/dsp-img.ldr && sync'

# Reboot to trigger DSP firmware upload
ssh root@<DEVICE_IP> reboot
```

To revert: delete `/data/test/dsp-img.ldr` and reboot — Encore falls back to
the stock firmware on the read-only rootfs.

**Safety**: The DSP has no persistent storage. If your firmware doesn't work,
remove the custom .ldr and reboot — the stock firmware loads automatically.
You cannot permanently damage the DSP through firmware upload.

## Byte Ordering

The SHARC .ldr format has non-obvious byte ordering that's important to
understand:

```
SHARC instruction: 48 bits (6 bytes)

In .ldr file (disk):  bytes are REVERSED per 6-byte group
  Instruction 0xAABBCCDDEEFF → stored as FF EE DD CC BB AA

Boot kernel additionally: each byte is BIT-REVERSED (for SPI MSB-first)
  Byte 0xFF → 0xFF (palindrome)
  Byte 0x01 → 0x80
```

The tools handle this automatically:
- `--swap` reverses 6-byte groups (needed for all .ldr files)
- `--raw` un-reverses bits within each byte (needed for extracted boot kernel)

## Open Research Areas

The following aspects of the DSP firmware are not yet fully understood.
Contributions and findings are welcome.

- **SRU (Signal Routing Unit) pin mappings** — which SPORT channels connect
  to which physical I2S/TDM lines. Requires an IOP register dump (addresses
  below 0x80000, not captured in the current SRAM dump).
- **TDM slot assignments** — how the 7 microphones and stereo output map to
  SPORT TDM slots.
- **Beamforming algorithm** — the gain matrix structure is identified (200 KB
  region, two weight values) but the spatial processing topology is unknown.
- **Filter coefficient interpretation** — thousands of biquad coefficients
  are present but their mapping to frequency bands and processing stages is
  not documented.
- **RTOS task structure** — RTOS strings are present in SRAM, suggesting a
  multi-task firmware. Task boundaries and scheduling are unknown.
- **Wake word engine** — VAD (Voice Activity Detection) events are
  documented, but the acoustic model and feature extraction pipeline are not.

## File Reference

| File | Description |
|------|-------------|
| `scripts/sharc/sharc_disasm.py` | Disassembler — all 25 instruction types |
| `scripts/sharc/sharc_asm.py` | Assembler — two-pass, label resolution |
| `scripts/ldr/ldr_parse.py` | .ldr block parser — format documentation |
| `scripts/ldr/ldr_build.py` | .ldr builder/patcher — round-trip rebuild |
| `scripts/ldr/ldr_disasm_pm.py` | PM extractor — auto-detects packing |
| `scripts/dsp/disasm_firmware.py` | SRAM dump disassembler (PM48 from dsp_dump.bin) |
| `scripts/dsp/disasm_ldr_full.py` | Complete .ldr disassembler (block parse + DM reconstruct) |
| `scripts/dsp/analyze_dsp_dump.py` | SRAM analyzer — forensic analysis |
| `scripts/dsp/dsp_dump_memory.py` | On-device: dump 320 SRAM pages via SPI |
| `scripts/dsp/dsp_probe2.py` | On-device: focused DSP probe with GPIO handshake |
| `docs/dsp-reference.md` | Full technical reference |
| `vendor/dsp_boot_kernel_disasm.txt` | Complete boot kernel disassembly (256 instructions) |
| `vendor/dsp_firmware_disasm.txt` | SRAM dump disassembly (59,850 instructions) |
| `vendor/dsp_firmware_ldr_disasm.txt` | .ldr firmware disassembly (25,363 instructions) |

## See Also

- [dsp-reference.md](dsp-reference.md) — SPI protocol, commands, memory map,
  boot kernel analysis, firmware format specification
- [hardware.md](hardware.md) — SoC overview, I2C bus, audio signal path
- ADSP-21489 Hardware Reference Manual (Analog Devices, public)
- ADSP-214xx SHARC Processor Programming Reference Rev 2.4 (Analog Devices, public)
