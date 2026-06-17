# ADSP-21489 DSP Reference

Complete technical reference for the Analog Devices ADSP-21489 SHARC DSP on the
Harman Kardon Invoke amplifier board. Derived from Ghidra decompilation of the stock
`dsp-client` binary, firmware binary analysis, on-device testing, and the
official ADI hardware reference manual.

## Chip Identification

| Property | Value |
|----------|-------|
| Chip | **Analog Devices ADSP-21489** (SHARC, 4th generation) |
| Part marking | AD91210Z (Analog Devices alternate marking) |
| Location | Amplifier board (separate PCB from main SoC board) |
| Core | 450 MHz SIMD SHARC, 48-bit instruction words |
| SRAM | 5 Mbit on-chip (4 blocks, ~640 KB total) |
| SPORT | 8 serial ports, each supports up to 128 TDM channels |
| SPI | Slave interface for firmware upload + runtime messaging |
| Accelerators | Hardware FIR, IIR, FFT engines |
| Boot | SPI slave boot — firmware loaded from ARM SoC every cold boot |
| Persistent storage | **None** — all SRAM is volatile, cannot be bricked |

### SRAM Memory Map

| Block | Address Range | Size |
|-------|--------------|------|
| Block 0 | 0x0008C000 - 0x0009DFFF | 72 KB |
| Block 1 | 0x000AC000 - 0x000BDFFF | 72 KB |
| Block 2 | 0x000C5000 - 0x000DFFFF | 108 KB |
| Block 3 | 0x000E5000 - 0x000FFFFF | 108 KB |

Total addressable via memory dump: 320 pages (0x0000..0x013F), ~2 KB per page.

**Memory dump mapping**: Each page maps to DM address `0x80000 + page × 512`.
Pages 0-319 cover DM 0x80000-0xA7FFF (SRAM only). The dump does **NOT** contain
IOP registers (SPORT, SPI, PLL, DMA, SRU at DM 0x00000-0x3FFFF) — those are below
the dump range and would require a separate read mechanism to capture.

**DM ↔ PM address mapping**: DM and PM ports share the same physical SRAM through
different-width buses (32-bit DM vs 48-bit PM):

| Address Space | Word Width | Byte Offset Formula |
|---------------|-----------|---------------------|
| DM address D | 32 bits | `(D - 0x80000) × 4` |
| PM address P | 48 bits | `(P - 0x80000) × 6` |

Key equivalences: DM 0x92000 = PM 0x8C000 (both at byte offset 0x48000).
3 DM words (12 bytes) occupy the same physical memory as 2 PM words (12 bytes).

### Official Documentation

See [Appendix: Sources](#appendix-sources) for all datasheets, reference manuals,
and external references.

## What the DSP Firmware Does

Encore uploads the same `dsp-img.ldr` firmware blob the stock system used (160,484 bytes),
so this section describes what the DSP is doing on a running Encore device. Everything
below happens entirely on the SHARC at 450 MHz, with zero ARM CPU involvement:

1. **7-microphone beamforming** — receives TDM audio from all 7 MEMS mics via
   SPORT inputs, computes **binary-quantized delay-and-sum** beamforming to produce
   two output streams:
   - Left channel: far-field recognition stream (optimized for wake word / voice commands)
   - Right channel: near-field call stream (optimized for telephony)

   The beamforming gain matrix (200 KB, DM 0x97E80-0x9D2AD) uses only two values:
   100 (full gain) and 75 (~-2.5 dB reduced gain). This is a simple, fixed-geometry
   design — not adaptive MVDR. Weights are precomputed for the Invoke's circular
   mic array and loaded statically from the firmware binary.

2. **Wake word detection** — runs spectral template matching on the beamformed
   audio using biquad filters and 9 parallel 128-point FFTs. Fires `TRIGGER_FOUND`
   event when "Hey Cortana" is detected. The filter coefficients and spectral
   templates are embedded in the firmware binary. This is NOT a deep neural
   network — it's a classical DSP approach (filterbank + FFT + template correlation).

3. **Voice activity detection (VAD)** — fires `EXPECT_SPEECH` events when voice
   activity is detected, `CANCEL_TRIGGER` when it stops.

4. **Audio payload streaming** — after wake word trigger, manages the audio
   capture pipeline: `PAYLOAD_BEGIN` → streaming → `PAYLOAD_END` / `PAYLOAD_TIMEOUT`.

5. **Volume control** — applies digital gain to the output stream.

6. **Mic mute** — hardware-level mic mute before audio reaches the SoC.

7. **SPI slave protocol** — bidirectional communication with the ARM SoC for
   commands and events.

## SPI Communication

### Physical Interface

| Property | Value |
|----------|-------|
| Device node | `/dev/spidev0.0` |
| Mode | 3 (CPOL=1, CPHA=1) |
| Word size | 8 bits |
| Upload speed | 1,000,000 Hz (firmware upload) |
| Message speed | 58,824 Hz (runtime messaging) |

### GPIO Flow Control

The DSP uses 4 GPIO pins on the ARM SoC for SPI message flow control. These
must be configured via `/sys/class/gpio/` sysfs before bidirectional
communication works.

| GPIO | Direction | Active Level | Purpose |
|------|-----------|-------------|---------|
| 4 | Output | HIGH idle | CS/flow control — toggle LOW→HIGH before reading |
| 5 | Output | (upload only) | Temporarily exported during firmware upload CS |
| 12 | Input | HIGH = data | DSP has data to send |
| 13 | Input | (vendor: HIGH = ready) | Stock pin direction is input; the vendor "ARM ready" output use below is not used by Encore (it pegged the CPU) |
| 15 | Input | **LOW = ready** | DSP ready to receive (**active-low**) |

The send/receive steps below describe the vendor `dsp-client` GPIO handshake,
which drives GPIO 13 as an "ARM ready" output. Encore's active DSP path does not
use this handshake: it keeps the stock pin directions (GPIO 13 as an input) and
uses timing-based flow control, because configuring GPIO 13 as an output pegs the
CPU. The steps are kept as a reference to the vendor protocol.

**Send protocol** (ARM → DSP):
1. Set GPIO 13 direction to OUTPUT, value LOW
2. Poll GPIO 15 until **LOW** (DSP ready — **active-low**)
3. Set GPIO 13 HIGH (ARM ready)
4. Brief delay (~1μs)
5. SPI write message via `SPI_IOC_MESSAGE` ioctl (full-duplex)
6. Brief delay (~1μs)
7. Set GPIO 13 LOW (ARM done)
8. Poll GPIO 12 for response (if command expects one)

**Receive protocol** (DSP → ARM, unsolicited events):
1. Poll GPIO 12 — if LOW, no data pending; if **HIGH**, data ready
2. Toggle GPIO 4 **LOW→HIGH** (CS pulse)
3. Set GPIO 13 HIGH (ARM ready)
4. Brief delay (~1μs)
5. Full-duplex SPI transfer: send 2048 zeros, receive 2048 bytes response
6. Brief delay (~1μs)
7. Set GPIO 13 LOW (ARM done)

**Poll interval**: 200ms between event checks. Encore uses the same interval the stock
client did.

**Message retry protocol**: 3 retries per message, 3 DSP response waits,
6 DSP-ready timeouts. If all retries exhaust, re-uploads firmware.

### Message Frame Format (ARM → DSP)

All commands use a 5-byte header with 8-byte alignment:

```
Offset  Field         Size   Description
0       type_hi       1      Message category, high byte
1       type_lo       1      Message category, low byte
2       len_hi        1      Payload length, high byte
3       len_lo        1      Payload length, low byte
4       checksum      1      Sum of bytes 0-3 + all payload bytes (wrapping u8)
5..     payload       N      Command-specific data
N+5..   padding       0-7    Zero-fill to 8-byte boundary
```

### Event Frame Format (DSP → ARM)

Events received in the 2048-byte SPI read buffer:

```
Offset  Field         Size   Description
0       category_hi   1      Event category, high byte
1       category_lo   1      Event category, low byte
2       event_code    1      Event type within category
3..     event_data    N      Event-specific payload
```

All-zero buffer = no event.

## Commands (ARM → DSP)

All commands use `msg_type = 0x0000` except mic tests which use `0x0002`.

### Category 0x0000 — Standard Commands

| Code | Payload | Description |
|------|---------|-------------|
| 0x04 | `[level]` | **Set volume** (1-100) |
| 0x08 | (none) | **Query firmware version** |
| 0x09 | `[0/1]` | **Set mic mute** (0=unmuted, 1=muted) |
| 0x0B | `[state]` | **Set listening state** (triggers beamforming mode change) |
| 0x0C | `[page_hi, page_lo]` | **Dump memory page** (0x0000-0x013F) |

### Category 0x0002 — Diagnostic Commands

| Code | Payload | Description |
|------|---------|-------------|
| (mic_test_single) | `[test_data, test_data]` | Single microphone test |
| (mic_test_pair) | `[test_data, test_data]` | Microphone pair test |
| (mic_test_normal) | `[test_data]` | Normal mode mic test |
| (bypass_mode) | `[data, data]` | DSP bypass mode (raw audio passthrough) |

## Events (DSP → ARM)

### Category 0 — Audio/System Events

| Code | Name | Data | Description |
|------|------|------|-------------|
| 0x04 | `NEW_DAC_GAIN` | `[gain_level]` | DSP adjusted DAC gain |
| 0x05 | `EXPECT_SPEECH` | — | Voice activity detected (VAD) |
| 0x06 | `CANCEL_TRIGGER` | — | Voice activity ended |
| 0x07 | `SW_UPGRADE` | — | Software upgrade notification |
| 0x08 | `DSP_VERSION` | `[version_bytes...]` | Firmware version response |
| 0x09 | `MIC_MUTE` | `[0/1]` | Mic mute state echo |
| 0x0B | `CORTANA_SKYPE` | — | Skype call state |
| 0x0C | `MEM_DUMP` | `[page_hi, page_lo, data...]` | Memory dump page response |
| 0xFF | `ERR` | — | Error condition |

### Category 1 — Wake Word / Trigger Events

| Code | Name | Description |
|------|------|-------------|
| 0x00 | `TRIGGER_FOUND` | **Wake word detected ("Hey Cortana")** |
| 0x01 | `PAYLOAD_BEGIN` | Audio payload streaming started |
| 0x02 | `PAYLOAD_END` | Audio payload streaming complete |
| 0x03 | `PAYLOAD_TIMEOUT` | Payload capture timed out (silence) |
| 0x04 | `DSP_BOOTUP` | DSP firmware boot complete |
| 0xFF | `WRITE_ERR` | SPI write error |

### Category 2 — Mic Test Events

| Code | Name | Description |
|------|------|-------------|
| 0x00 | `MIC_TEST_SINGLE` | Single mic test result |
| 0x01 | `MIC_TEST_PAIR` | Mic pair test result |
| 0x02 | `MIC_NORMAL` | Normal mode test result |
| 0x03 | `HW_PERFORM_TEST` | Hardware performance test |
| 0xFF | `TEST_ERR` | Mic test error |

## Memory Dump Protocol

The ARM can read all DSP SRAM by requesting memory pages one at a time.

**Request:**
```
send_message(0x0000, [0x0C, page_hi, page_lo])
```

**Response** (via `poll_event()`):
```
Category: 0x0000
Code:     0x0C
Data:     [page_hi, page_lo, ...memory_data...]
```

| Parameter | Value |
|-----------|-------|
| Page range | 0x0000 to 0x013F (320 pages) |
| Data per page | ~2 KB |
| Total dump size | ~640 KB (= 5 Mbit SRAM) |
| Time per page | ~50-100 ms (SPI transfer + DSP processing) |
| Full dump time | ~30-60 seconds |

The stock `dsp-client` saves dumps to `/tmp/dsp_memory.dump`. Encore exposes
memory dumps via WebSocket: send `ClientMsg::DspMemoryDump { start_page, num_pages }`.

A standalone dump script is also available at `scripts/dsp/dsp_dump_memory.py` — it
opens `/dev/spidev0.0` directly and dumps all 320 pages to `/lsync/dsp_dump.bin`.
Safe to run alongside Encore (no SPI contention as long as you don't trigger DSP
commands from the web UI during the dump).

### Live SRAM Dump Results (2026-02-19)

Full 320-page dump captured from a running device with stock DSP firmware loaded.
Total: 655,360 bytes, 215,668 non-zero bytes (32.9% utilization), ~486 ms/page.

The dump is saved at `vendor/dsp_dump.bin` for offline analysis.

#### SRAM Content Map

| Pages | Offset (hex) | Size | Content | Non-zero |
|-------|-------------|------|---------|----------|
| 0-16 | 0x0000-0x87FF | 34 KB | **Runtime code + RTOS strings** | 55.3% |
| 17-31 | 0x8800-0xFFFF | 30 KB | Empty (zero-filled) | 0% |
| 32-39 | 0x10000-0x13FFF | 16 KB | Per-channel processing state (0x62 filler) | ~20% |
| 40-53 | 0x14000-0x1BFFF | 28 KB | Empty | 0% |
| 54-61 | 0x1C000-0x1FFFF | 16 KB | Per-channel processing state (0x62 filler) | ~20% |
| 62-75 | 0x20000-0x27FFF | 28 KB | Empty | 0% |
| 76-83 | 0x28000-0x2BFFF | 16 KB | Per-channel processing state (0x62 filler) | ~20% |
| 84-98 | 0x2C000-0x31FFF | 30 KB | Empty | 0% |
| 99-105 | 0x32000-0x35FFF | 14 KB | Per-channel processing state (0x62 filler) | ~20% |
| 106-116 | 0x36000-0x3AFFF | 22 KB | Empty | 0% |
| 117-127 | 0x3B000-0x3FFFF | 22 KB | Filter coefficients / state (0xF5 filler) | ~25% |
| 128-138 | 0x40000-0x45FFF | 24 KB | Empty | 0% |
| 139-149 | 0x46000-0x4AFFF | 22 KB | Filter coefficients / state (0xF5 filler) | ~25% |
| 150-160 | 0x4B000-0x503FF | 22 KB | Empty | 0% |
| 161-171 | 0x50800-0x55FFF | 22 KB | Filter coefficients / state (0xF5 filler) | ~25% |
| 172-183 | 0x56000-0x5BFFF | 24 KB | Empty | 0% |
| 184-194 | 0x5C000-0x613FF | 22 KB | Active processing data (mixed) | ~30% |
| 195-293 | 0x61800-0x93FFF | **200 KB** | **Beamforming gain matrix** | 100% |
| 294 | 0x94000-0x947FF | 2 KB | Transition (sparse data) | ~5% |
| 295-319 | 0x94800-0x9FFFF | 50 KB | **SHARC instruction code (PM)** | ~60% |

#### Page 0: RTOS Error Strings

The first page contains ASCII strings from the DSP's real-time operating system:

```
"C no timers available"
"bad allocation"
```

These are exception handler messages compiled into the DSP firmware — they confirm
the firmware uses a C++ runtime with timer management and dynamic memory allocation
(typical of Libre Wireless DSP applications).

#### Pages 32-105: Per-Channel Processing Banks

Four distinct regions with identical structure, each 8 pages wide, separated by
empty gaps. These correspond to the 7-microphone processing channels:

- **0x62 filler byte**: Marks uninitialized or padding areas within each bank
- **Active data**: Audio state variables, delay line pointers, intermediate
  computation results
- The 4-bank structure suggests: mic input banks (channels 1-4), mic input banks
  (channels 5-7), output banks (left/right), and a reference/control bank

#### Pages 117-171: Filter Coefficient Storage

Three identical-size regions filled with coefficient data:

- **0xF5 filler byte**: Marks boundaries between coefficient blocks
- Contains the 2,278 biquad filter coefficients identified in the `.ldr` analysis
- Organized in banks matching the per-channel processing regions above
- These are the Harman tuning curves (parametric EQ) applied per-channel before
  beamforming summation

#### Pages 195-293: Beamforming Gain Matrix (200 KB)

The largest single data structure in DSP memory. Contains ONLY two values:
- **0x64** (decimal 100) — full gain
- **0x4B** (decimal 75) — reduced gain (~-2.5 dB)

These alternate in a regular pattern visible when viewed in a hex editor with a
specific line width. This is the **frequency-dependent beamforming weight table**:

- **Purpose**: Per-frequency, per-microphone gain weights for the 7-mic delay-and-sum
  beamformer. Each frequency bin gets a different gain pattern across the 7 mics,
  steering the beam toward the strongest voice source.
- **Why only two values**: Binary quantization (full/reduced) rather than continuous
  weights — simpler than MVDR, computationally cheaper, sufficient for the
  speaker's fixed geometry.
- **200 KB for 7 mics**: This covers the full audio bandwidth at FFT bin resolution.
  With 128-point FFTs at 48 kHz, there are 64 frequency bins × 7 mics × multiple
  look directions = ~200 KB of precomputed steering vectors.
- **Not updatable at runtime**: These are static weights loaded from the firmware
  binary. The DSP does not adapt them during operation — the beamforming is fixed
  to the Invoke's physical mic geometry.

The gain matrix was also discovered visually: when viewed in Notepad or a hex
editor and the window is resized to the right width, the alternating 0x64/0x4B
values form a clear visual pattern of stripes, revealing the regular structure.

#### Pages 295-319: Program Memory (SHARC Code)

The final 25 pages contain 48-bit SHARC instructions with characteristic opcode
patterns:
- **0x3E, 0x3F**: Common ALU/MAC operations
- **0x7E, 0xFF**: Branch and control flow instructions
- **Dense instruction packing**: ~60% non-zero, consistent with compiled code

DM pages 295-319 map to DM 0x97600-0xA7FFF. Via the DM↔PM address mapping, this
corresponds to PM 0x8F955-0x9AAAA. Code quality validation confirms these decode
at 96-100% valid instruction rate using the 6-byte little-endian PM packing method.

## Firmware Upload

### Boot Process

The ADSP-21489 has **no onboard flash**. Firmware lives in volatile SRAM and
must be uploaded via SPI every cold boot. The hardware boot ROM (mask-programmed,
immutable) handles SPI slave boot automatically.

### Upload Sequence

1. **Reset DSP** via IO Expander (I2C 0x20, bit 0): HIGH → LOW → 20ms → HIGH
2. **Configure SoC audio registers** (Marvell BG2CDP AVIO):
   - `0xF7EA8008 = 0x0118D249` (audio clock — enables DSP clocking)
   - `0xF7E80404 = 0x00000F28` (AVIO pin mux — SPI upload mode)
   - `0xF7E80400 = 0x00000A08` (AVIO config — pin muxing)
   - (Stock `dsp-client` exported GPIO 5 as upload CS; Encore uses the kernel SPI driver's built-in CS)
3. **Bit-reverse** each firmware byte (MSB↔LSB swap via lookup table)
4. **Upload** over SPI at 1 MHz in 4-byte chunks
5. **Pause** 10ms every 1536 bytes (block boundary)
6. **Restore** AVIO registers for normal operation
7. **Switch** SPI to 58,824 Hz messaging speed
8. **Init GPIO** pins 4, 12, 13, 15 for bidirectional messaging

### Firmware File Format

| Property | Value |
|----------|-------|
| Path | `/usr/share/dsp/dsp-img.ldr` |
| Size | 160,484 bytes |
| Format | Custom Libre Wireless `.ldr` (NOT standard ADI format) |
| Boot kernel | First 1536 bytes |
| Bit reversal | Applied before SPI transmission (not in file) |

The `.ldr` format does **not** match the standard Analog Devices SHARC loader
format (which uses 16-byte block headers with 0xAD/0xAC/0xAB signature bytes).
Libre Wireless used a custom boot kernel. Standard ADI tools (`elfloader.exe`)
will not produce compatible files without understanding this custom format.

### Firmware Memory Layout (from .ldr block analysis)

The `.ldr` payload contains 392 blocks that load firmware across a wide address range,
far exceeding the 640 KB SRAM dump. Block destinations (DM addresses):

| Region | DM Address Range | Words | PM Equivalent | Content |
|--------|-----------------|-------|---------------|---------|
| FINAL (IVT) | 0x92000-0x9217F | 384 | PM 0x8C000 | Interrupt vector table + init + idle loop |
| Code + data | 0x98600-0xA4C00 | ~50K | PM 0x90400-0x96E00 | Beamforming tables + processing code |
| ISR handlers | 0xB2000-0xBC7E5 | ~44K | — | Interrupt service routines, function pointers |
| Code block | 0xC0000-0xC7FD9 | ~32K | — | Audio processing code |
| Code block | 0xE0000-0xE703D | ~29K | — | Signal processing code |
| Main firmware | 0x124354-0x12FAE6 | ~47K | — | Core firmware (init, SPORT config, SPI protocol) |
| Small data | 0x660000-0x66012D | 302 | — | Constants or lookup table |

**Total**: ~202K DM words loaded, only ~85K within the 640 KB dump range
(DM 0x80000-0xA7FFF). The remaining ~117K words load to addresses beyond the dump.

**IOP register references** found in firmware code (19 unique addresses at 0x40000+):
PLL, SPORT, SPI, DMA, SRU, and clock configuration.

### System Register References Found in Firmware

| Register | Address | Refs | Purpose |
|----------|---------|------|---------|
| SYSCTL | 0x00040000 | 9 | System control (master clock, memory arbitration) |
| REVPID | 0x00040033 | 1 | Silicon revision check |
| EPCTL | 0x00040093 | 1 | External port control |
| DMA_CFG | 0x0004013E | 1 | DMA configuration |
| SPI2_TDBR | 0x00040613 | 1 | SPI2 transmit buffer (comms back to ARM) |
| SPORT_CLK | 0x00040F6B | 1 | Serial port clock configuration |
| PMCTL0 | 0x00040F80 | 1 | Power management control 0 |
| PMCTL1 | 0x00040F81 | 2 | Power management control 1 |
| PMCTL_PLL | 0x00040F82 | 3 | PLL multiplier/configuration |
| PLLM | 0x00040F84 | 2 | PLL multiplier value |
| CLKCTL | 0x00040F8F | 3 | Clock selection |
| MISCR | 0x00040F94 | 2 | Miscellaneous control register |
| SPORT_MCTL | 0x00040F9E | 2 | SPORT multi-channel control |
| SPORT_CTL_A | 0x00040FA0 | 2 | SPORT control channel A (I2S stream 1) |
| SPORT_CTL_B | 0x00040FA4 | 3 | SPORT control channel B (I2S stream 2) |
| SPORT_CTL_C | 0x00040FA8 | 2 | SPORT control channel C (I2S stream 3) |
| IDP_DMA0 | 0x00049808 | 1 | Internal DMA Processor channel 0 |
| IDP_DMA1 | 0x00049A08 | 1 | Internal DMA Processor channel 1 |
| UART_CTL | 0x00041405 | 1 | UART control (debug?) |

**Key findings**: 3 SPORT channels (A, B, C) are configured — likely mic array input,
codec I2S to/from SoC, and possibly a third stream. 2 IDP DMA channels for zero-copy
audio streaming between SPORTs and internal SRAM.

### ADSP-21489 Register Map Reference

Key IOP register groups relevant to understanding and reproducing the stock firmware
configuration. All addresses are in the IOP memory space (0x00000000-0x0003FFFF).
Note that the memory dump command maps its pages to SRAM (DM 0x80000 and up), so these
IOP registers are **not** reachable through it — see [Remaining Unknowns](#remaining-unknowns).

#### SPORT Registers (Serial Ports — I2S/TDM Audio)

| Register | Address | Description |
|----------|---------|-------------|
| SPORT0_CTL_A | 0x00040F9E | SPORT 0 control channel A |
| SPORT0_CTL_B | 0x00040FA0 | SPORT 0 control channel B |
| SPORT1_CTL_A | 0x00040FA4 | SPORT 1 control channel A |
| SPORT1_CTL_B | 0x00040FA8 | SPORT 1 control channel B |
| SPORTx_MCTL | 0x000400A0+ | Multi-channel select (TDM slot enables) |
| SPORTx_CS0 | 0x000400A4+ | Channel select 0 (which TDM slots are active) |

**Key SPORT control bits**: SPEN (enable), SDEN (DMA enable), SLEN (serial word
length, typically 31 for 32-bit), SPEN_A/B (individual channel enable), DTYPE
(data type: right-justify, sign-extend, μ-law, A-law), MCE (multi-channel enable),
MFD (multi-channel frame delay), NCH (number of channels − 1).

#### SPI Registers

| Register | Address | Description |
|----------|---------|-------------|
| SPICTL | 0x00040600 | SPI control (SPIEN, MSTR, CPOL, CPHA, TIMOD) |
| SPIFLG | 0x00040601 | SPI flag (chip select control) |
| SPISTAT | 0x00040602 | SPI status (RXS, TXS, OVR, MODF) |
| TXSPI | 0x00040603 | SPI transmit data register |
| RXSPI | 0x00040604 | SPI receive data register |
| SPI2_TDBR | 0x00040613 | SPI2 transmit data buffer (used for ARM comms) |

The DSP operates as SPI slave. The ARM SoC is the SPI master.

#### PLL / Clock Configuration

| Register | Address | Description |
|----------|---------|-------------|
| PMCTL0 | 0x00040F80 | Power management control 0 (core clock select) |
| PMCTL1 | 0x00040F81 | Power management control 1 (peripheral enables) |
| PMCTL_PLL | 0x00040F82 | PLL configuration (multiply, divide, bypass) |
| PLLM | 0x00040F84 | PLL multiplier value |
| CLKCTL | 0x00040F8F | Clock output control and selection |

**PLL formula**: `f_core = f_clkin × PLLM ÷ PLLD`. Typical: 25 MHz × 18 ÷ 1 = 450 MHz.

#### DMA Configuration

| Register | Address | Description |
|----------|---------|-------------|
| IDP_DMA0_Ix | 0x00049808 | IDP DMA channel 0 index register |
| IDP_DMA1_Ix | 0x00049A08 | IDP DMA channel 1 index register |
| EPx_DMA_Ix | various | External port DMA index registers |

**DMA uses Transfer Control Blocks (TCBs)**: 8-word descriptors in SRAM that form
linked chains. Each TCB contains: next TCB pointer, start address, count, modify,
and channel config. The DMA engine auto-follows the chain for continuous audio
streaming without CPU intervention.

#### SRU (Signal Routing Unit)

The ADSP-21489's most unique feature — a crossbar switch that routes any signal
source to any destination. All audio pin assignments are made through SRU registers,
NOT hardwired.

| Register Group | Address Range | Description |
|----------------|--------------|-------------|
| SRU_CLK | 0x00040C00-0x00040C0F | Clock signal routing |
| SRU_DAT | 0x00040C10-0x00040C1F | Data signal routing |
| SRU_FS | 0x00040C20-0x00040C2F | Frame sync routing |
| SRU_PIN | 0x00040C30-0x00040C3F | Pin buffer enable/direction |
| DAI_PIN | 0x00040C40+ | DAI pin assignments |

**Implication**: The mic-to-SPORT mapping and I2S output pin assignments are entirely
software-defined through SRU. To reproduce the stock audio configuration for custom
firmware, the SRU registers must be read from a running system and replicated exactly.

### Audio Coefficients Found in Firmware

| Type | Count | Details |
|------|-------|---------|
| Volume dB table | 31 entries | -100 to -1 dB, maps MCU ring levels 0-30 |
| Gain interpolation | 17 steps | Unity (0 dB) to -6 dB in fine increments |
| 128-point FFT twiddles | 9 instances | 9 parallel FFTs (7 mics + 2 reference?) |
| Biquad filter sets | ~2,278 | Harman tuning parametric EQ |
| Delay line buffer | 257 samples | Echo cancellation or reverb |
| Notable constants | Pi, 1/sqrt(2), 1000.0, 8000.0 | Trig, Butterworth Q, frequency refs |

Estimated breakdown: ~54 KB code + ~102 KB coefficients/data.

### Notable Patterns in the Firmware Binary

| Offset | Content | Significance |
|--------|---------|-------------- |
| 0-1535 | Boot kernel | 256 × 48-bit instructions, many zero bytes |
| 3136 | Exponential gain table | 2^(-n/16) for n=0..16, likely volume curve |
| 8932 | Integer sequence [1,2,3,4,5,6,7] | Possibly mic index array |
| 89196 | Pi (3.14159...) | Trigonometric computations (beamforming) |
| Various | 1/sqrt(2) constants | Normalization factors |

12,888 unique finite float32 values were found across the entire binary. No
obvious 7-element beamforming weight table was identified — the algorithm likely
computes weights dynamically from phase delay calculations.

## Custom DSP Firmware

### Safety

**The DSP cannot be bricked.** It has no persistent storage — firmware is
uploaded from the ARM filesystem every boot. If custom firmware doesn't work:
reboot, stock firmware loads, everything recovers.

The amp mute is controlled by the IO Expander on the ARM side (I2C 0x20),
completely independent of the DSP. Mute before uploading experimental firmware
to protect speakers from garbage audio.

### Development Toolchain

| Tool | Purpose | Availability |
|------|---------|-------------|
| CrossCore Embedded Studio (CCES) | Official ADI IDE + compiler | Proprietary, [free evaluation](https://www.analog.com/en/resources/evaluation-hardware-and-software/software/adswt-cces.html) |
| SHARC C/C++ compiler | Compiles to 48-bit SHARC instructions | Part of CCES |
| `elfloader.exe` | Converts ELF to `.ldr` boot format | Part of CCES |
| Hardware reference manual | Register-level documentation | Free PDF from ADI |
| Programming reference | Instruction set documentation | Free PDF from ADI |

There is **no open-source SHARC compiler**. GCC and LLVM do not support the
SHARC ISA. CCES is the only option.

**CCES version warning**: CCES 3.0.3 (latest as of 2026) **cannot disassemble
classic SHARC** (ADSP-214xx). Its `elfdump.exe` recognizes "ADSP-21489" as a
processor name but the instruction decoder only supports SHARC+ (ADSP-215xx/SC5xx,
which use variable-length 16/32/48-bit VISA encoding). All classic SHARC 48-bit
fixed-width instructions fail with "fail to disassemble." The last ADI toolchain
to support ADSP-214xx disassembly was **VisualDSP++ 5.1.2** (discontinued, hard
to obtain).

**No open-source SHARC disassembler exists.** The closest is the **Wavedrum**
project (Haskell, targets ADSP-21375), which has partial ISA coverage for the same
instruction family but is incomplete. No Ghidra SLEIGH module, IDA Pro plugin,
radare2 plugin, or Binary Ninja plugin exists for the SHARC ISA.

#### Community / Open-Source Tools

| Tool | Purpose | Availability |
|------|---------|-------------|
| `sharc_disasm.py` | **Custom SHARC 48-bit disassembler** — decodes all 25 instruction types | This repo: `scripts/sharc/sharc_disasm.py` |
| `sharc_asm.py` | **Custom SHARC 48-bit assembler** — inverse of disassembler, produces .ldr byte format | This repo: `scripts/sharc/sharc_asm.py` |
| `disasm_firmware.py` | **SRAM dump disassembler** — reads dsp_dump.bin, produces full PM48 disassembly | This repo: `scripts/dsp/disasm_firmware.py` |
| `disasm_ldr_full.py` | **Complete .ldr disassembler** — parses blocks, reconstructs DM image, classifies code/data | This repo: `scripts/dsp/disasm_ldr_full.py` |
| `analyze_dsp_dump.py` | **SRAM analysis** — page map, beamforming matrix, strings, PM code regions | This repo: `scripts/dsp/analyze_dsp_dump.py` |
| `ldr_parse.py` | **.ldr block parser** — extracts (address, count, type) for all 392 blocks | This repo: `scripts/ldr/ldr_parse.py` |
| **remora** (GitHub) | Open-source ADSP-21489 audio project | MIT license, complete SRU routing + SPORT I2S + SPI slave + PLL init + DMA chaining examples |
| `adsp-ldr` (Python) | Parse/create SHARC `.ldr` boot files | Open source, handles standard ADI format (not Libre's custom format) |

**`sharc_disasm.py`** was written specifically for this project because no existing
tool can decode classic SHARC instructions. It covers all 25 instruction types
(4 groups: Compute+Move, Program Flow, Immediate Move, Miscellaneous), ALU/Multiplier/Shifter
opcodes, universal register names, system registers, and condition codes. Built from
the ADSP-21160 Instruction Set Reference Rev 2.1 (the ISA is shared across the
21160/2136x/214xx SHARC family).

Usage for this project:
```bash
python scripts/sharc/sharc_disasm.py vendor/dsp_boot_kernel.bin --raw --swap --base 0x8C000
```

Flags:
- `--raw`: input bytes are original .ldr format (not yet bit-reversed)
- `--swap`: reverse each 6-byte instruction word (LSB-first → MSB-first)
- `--base 0xNNNNN`: set disassembly base address (default 0x90000)
- `-o FILE`: write output to file instead of stdout

**`sharc_asm.py`** is the inverse of the disassembler — it reads `.asm` assembly source
and produces raw binary in `.ldr` byte ordering. Two-pass assembly with label resolution.
Verified byte-identical against the original boot kernel (256 instructions, 1536 bytes).

Usage:
```bash
python scripts/sharc/sharc_asm.py my_firmware.asm -o output.bin
python scripts/sharc/sharc_asm.py my_firmware.asm -o output.bin --hex  # hex listing to stdout
```

Flags:
- `-o FILE`: output binary file (required)
- `--hex`: print hex listing (address, word, instruction) to stdout
- `--base 0xNNNNN`: override base address (default: from `.base` directive or 0x8C000)

Assembly source format:
```asm
.base 0x8C000                       ; set base address
_label:     NOP                     ; label + instruction
            CALL _label             ; reference to label
            .dw 0x013E00021CB0      ; raw 48-bit hex (for undecoded instructions)
            IF EQ JUMP 0x08C092     ; conditional + absolute address
```

Supported instruction types: NOP, IDLE, Type 3 (DM/PM indexed), Type 5a (ureg=ureg),
Type 8a/8b (CALL/JUMP), Type 11a/b (RTS/RTI), Type 12a/b (DO UNTIL LCE), Type 13
(DO UNTIL cond), Type 14 (ureg↔DM/PM direct), Type 17 (ureg=imm), Type 18 (BIT
SET/CLR/TST), Type 20 (FLUSH CACHE). The `.dw` directive handles any instruction not
covered by the parser.

The complete boot kernel disassembly is at `vendor/dsp_boot_kernel_disasm.txt` — all
256 instructions with addresses, hex words, and decoded assembly.

**remora** is the most valuable reference for custom firmware — it's a working
ADSP-21489 audio project with complete source for all the subsystems we need:
SRU pin routing, SPORT configuration for I2S, SPI slave communication, PLL setup,
and DMA chain management. While it targets a different board, the ADSP-21489
peripheral code is directly applicable.

### What Custom Firmware Needs

A minimal "hello world" DSP firmware must:

1. **Configure SPORTs** to receive TDM audio from 7 microphones (SPORT pin
   assignments and TDM slot mapping are currently unknown — can be determined
   by dumping DSP memory while stock firmware runs and reading SPORT config
   registers)
2. **Configure SPORT output** for I2S to the WM8904 codec on the SoC
3. **Configure SPI slave** for command/event protocol with ARM
4. **Implement the SPI message protocol** (or a subset) so Encore can communicate

Advanced features to add incrementally:
- Delay-and-sum beamforming (well-documented algorithm, hardware FFT helps)
- Custom wake word detection (train with openWakeWord, port inference to SHARC)
- Volume/mute control
- Full stock protocol compatibility

### Remaining Unknowns

The full SRAM dump (`vendor/dsp_dump.bin`) captures DM 0x80000-0xA7FFF, but IOP
registers (DM 0x00000-0x3FFFF) are **outside** this range. The `mem_dump` command
pages map to SRAM, not IOP space. Extracting the actual hardware configuration
requires either:
- A modified dump targeting IOP addresses directly (would need custom DSP code or
  a patched `mem_dump` handler)
- Reading IOP registers from the running firmware's init code in the disassembly

| Unknown | Register(s) | Status |
|---------|-------------|--------|
| SPORT → mic pin mapping | SRU_DAT, SRU_CLK, SRU_FS, DAI_PIN | **Requires IOP dump** (not in SRAM pages) |
| TDM slot assignments | SPORTx_MCTL, SPORTx_CS0 | **Requires IOP dump** |
| I2S output configuration | SPORT TX control registers | **Requires IOP dump** |
| SPI slave configuration | SPICTL, SPIFLG | **Requires IOP dump** |
| PLL multiplier / core clock | PMCTL_PLL, PLLM, CLKCTL | **Requires IOP dump** |
| DMA chain layout | IDP_DMA TCB pointers | **Partially visible** in SRAM, full chain in IOP |
| Custom .ldr boot format | Libre Wireless proprietary | **DECODED** — see Boot Kernel Disassembly section below |

The firmware init code at DM 0x124354+ likely configures all IOP registers. The
complete `.ldr` disassembly (`vendor/dsp_firmware_ldr_disasm.txt`) contains this
code and can be analyzed to extract the register values programmatically.

### Boot Kernel Disassembly (DECODED)

The boot kernel (first 1536 bytes / 256 × 48-bit instructions of `dsp-img.ldr`) has been
fully disassembled using a custom Python tool (`scripts/sharc/sharc_disasm.py`). Full listing
in `vendor/dsp_boot_kernel_disasm.txt`.

**Byte ordering**: .ldr stores each 48-bit instruction as 6 bytes in **reverse order** (LSB
first). Each byte is then bit-reversed for SPI transmission. Base address is **0x0008C000**
(Block 0 SRAM start), not 0x90000.

**Boot kernel structure**:

| Address | Function |
|---------|----------|
| 0x8C000-0x8C004 | NOP (interrupt vector table, unused at boot) |
| 0x8C005 | `CALL 0x8C0A7` — jump to SPI init |
| 0x8C006-0x8C011 | System init: disable interrupts, clear circular buffer lengths, M6=1, M14=1 |
| 0x8C012-0x8C014 | Read boot block header from SPI status area (DM 0x30024) |
| 0x8C015-0x8C02E | **Block type dispatch** — reads header, decodes type, jumps to handler |
| 0x8C02F | `JUMP (PC,0)` — infinite loop (hang if unknown type) |
| 0x8C030 | RTI handler (SPI data-ready interrupt) |
| 0x8C031-0x8C03C | **SPI poll routine** — write DMA config to IOP regs, poll bit 15, return data |
| 0x8C03D-0x8C04E | **Handler: Zero-fill DM** (32-bit, writes R0=0 in loop) |
| 0x8C04F-0x8C05D | **Handler: Zero-fill DM** (48-bit via PX register) |
| 0x8C05E-0x8C06E | **Handler: Load DM from SPI** (32-bit word pairs via R6/R7) |
| 0x8C06F-0x8C075 | **Handler: Load DM from SPI** (8-bit → 32-bit packing via R8) |
| 0x8C076-0x8C086 | **Handler: Load PM from SPI** (48-bit via PX, paired reads) |
| 0x8C087-0x8C08F | **Handler: Load PM from SPI** (48-bit via PX1+PX2 assembly) |
| 0x8C090-0x8C091 | Redirect trampolines to other handlers |
| 0x8C092-0x8C0A4 | **Final block**: SPI DMA setup to PM 0x92000, size 0x180, FLUSH CACHE, jump to entry |
| 0x8C0A5 | IDLE (wait for interrupt after setup) |
| 0x8C0A7-0x8C0B7 | **SPI init**: configure clock/timing via DM 0x2000, 5000-cycle delay |
| 0x8C0B8-0x8C0FF | NOP padding (72 words) |

**Key memory-mapped I/O addresses used by boot kernel**:

| Address | Register | Purpose |
|---------|----------|---------|
| DM 0x00001080 | SPI DMA source address | Where to read from |
| DM 0x00001081 | SPI DMA increment | Word increment (M14=1) |
| DM 0x00001082 | SPI DMA word count | Number of words to transfer |
| DM 0x00001084 | SPI DMA status | Bit 15 = transfer complete |
| DM 0x00002000 | SPI control | Clock configuration, enable |
| DM 0x00030024 | SPI status/data | Boot block metadata |
| DM 0x00092003 | Boot header: address | Target memory address for block |
| DM 0x00092004 | Boot header: word count | Number of words in block |
| DM 0x00092005 | Boot header: type | Block type selector (0-10) |
| DM 0x00124006 | SPI RX buffer low | Received 32-bit data (low word) |
| DM 0x00124007 | SPI RX buffer high | Received 32-bit data (high word) |

**.ldr payload format** (decoded from dispatch table):

The payload after the boot kernel consists of sequential blocks. Each block has a 3-word
header (address, word_count, type) followed by `word_count` data words. Block types:

| Type | Handler | Description |
|------|---------|-------------|
| 0 | 0x8C092 | **Final block** — set up DMA to load remaining data, jump to firmware entry |
| 1 | 0x8C03D | Zero-fill DM (32-bit words) |
| 2 | 0x8C04F | Zero-fill DM (48-bit words via PX) |
| 3 | 0x8C05E | Load DM from SPI (32-bit, paired reads: low+high words) |
| 4 | 0x8C06F | Load DM from SPI (8-bit packed into 32-bit) |
| 5 | 0x8C076 | Load PM from SPI (48-bit via PX, paired reads) |
| 6 | 0x8C087 | Load PM from SPI (48-bit via PX1+PX2 register assembly) |
| 7 | 0x8C090 | Same as type 1 (zero-fill DM 32-bit) |
| 8 | 0x8C091 | Same as type 4 (load DM 8-bit packed) |
| 9 | 0x8C090 | Same as type 1 (zero-fill DM 32-bit) |
| 10 | 0x8C091 | Same as type 4 (load DM 8-bit packed) |

**Implications for custom firmware**:
- The boot kernel is a standard block loader — we can create custom .ldr files by
  encoding our firmware as a sequence of (address, count, type) blocks
- Block type 0 triggers the final DMA transfer and jumps to the entry point
- The FINAL block DMAs 384 DM32 words to DM 0x92000 (= PM 0x8C000), which
  **overwrites the boot kernel** with the application's interrupt vector table
- After DMA + FLUSH CACHE, `JUMP 0x8C003` transfers to the freshly-loaded IVT
  (the IVT has NOPs at vectors 0-2, with the real entry at vector 3)
- PX register (48-bit bus exchange) is used for 48-bit PM transfers, while R6/R7
  pairs handle 32-bit DM transfers
- The assembler (`scripts/sharc/sharc_asm.py`) and block parser (`scripts/ldr/ldr_parse.py`)
  together provide a complete toolchain for creating custom `.ldr` files

### Complete Firmware Disassembly (2026-02-19)

Two comprehensive disassemblies are available, produced by custom tools:

| File | Source | Instructions | Labels | Purpose |
|------|--------|-------------|--------|---------|
| `vendor/dsp_firmware_disasm.txt` | SRAM dump (640 KB) | 59,850 | 4,547 | Runtime state — shows what's actually in memory |
| `vendor/dsp_firmware_ldr_disasm.txt` | `.ldr` file (160 KB) | 25,363 | 2,290 | Original firmware — authoritative, pre-runtime |
| `vendor/dsp_boot_kernel_disasm.txt` | `.ldr` first 1536 B | 184 (non-NOP) | 41 | Boot kernel block loader |

**Key distinction**: The SRAM dump shows the runtime state after the firmware has
been running (including modified buffers, counters, and overwritten code). The `.ldr`
disassembly shows the original firmware as loaded. The FINAL block in the `.ldr`
overwrites PM 0x8C000 (originally the boot kernel) with the application's interrupt
vector table.

**Code quality**: `.ldr` code regions decode at 96-100% valid instruction rate,
confirming the PM packing method (6-byte little-endian) is correct. Data regions
decode at 0% (as expected).

#### Firmware Architecture

The application firmware follows a classic DSP interrupt-driven pattern:

1. **Interrupt Vector Table** (PM 0x8C000-0x8C0AB): 40 ISR entries, one per
   interrupt source. Most are `RTI` (return-from-interrupt, unused). Active entries
   jump to handlers via `CALL` or `JUMP`.

2. **Init loop** (PM 0x8C0AC-0x8C0BF): Reads a function pointer table from
   DM[0xB2000] and calls each init routine. The table contains entries pointing to
   0x124000+ (main firmware region) for SPORT, SPI, PLL, DMA, and SRU initialization.

3. **IDLE loop** (PM 0x8C0C0): After initialization, the DSP enters `IDLE` and
   processes all audio entirely through ISR handlers triggered by SPORT DMA
   completion interrupts. Zero main-loop CPU usage.

4. **ISR handlers**: Located at DM 0xB2000+ (alternating zero-fill + data blocks
   in the `.ldr`). These are runtime-populated vtable slots; the actual handler
   code lives in the 0x124000+ region.

#### Analysis Tools

| Script | Purpose |
|--------|---------|
| `scripts/dsp/disasm_firmware.py` | Disassemble SRAM dump as PM48 words (full memory image) |
| `scripts/dsp/disasm_ldr_full.py` | Parse `.ldr`, reconstruct DM image, disassemble all code regions |
| `scripts/dsp/analyze_dsp_dump.py` | SRAM dump analysis: page map, beamforming matrix, string extraction |
| `scripts/sharc/sharc_disasm.py` | Core SHARC 48-bit instruction decoder (used by all above) |
| `scripts/sharc/sharc_asm.py` | SHARC assembler (inverse of disassembler, .ldr byte output) |
| `scripts/ldr/ldr_parse.py` | `.ldr` block parser (header, type, address, count for each block) |

### Development Workflow

1. Write firmware in C using CCES on Windows/Linux
2. Compile to ELF, convert to `.ldr` with `elfloader.exe`
3. Copy `.ldr` to device (SSH, web UI upload, or USB)
4. Encore uploads it at boot (or hot-reload via web API — reset + re-upload takes ~2s)
5. If it doesn't work → reboot → stock firmware loads automatically
6. Iterate

### Alternative Approaches (No Custom DSP Firmware)

| Approach | Effort | Result |
|----------|--------|--------|
| **Use stock "Hey Cortana" trigger** | Zero | Wake word works but is "Hey Cortana" |
| **ARM-side wake word** | Medium | Custom wake word via openWakeWord on Cortex-A7 (~5-10% CPU). DSP still does beamforming. |
| **Patch spectral templates in stock firmware** | High risk | Replace the 2,278 biquad filter coefficients and FFT templates for a custom keyword. Requires understanding the filterbank→FFT→template correlation pipeline. Not a neural network — classical DSP, but still complex. |
| **Full custom DSP firmware** | Major project | Complete control, any wake word, custom beamforming. Use remora as starting point. Weeks of work. |

The pragmatic path is **ARM-side wake word detection** using the DSP's beamformed
left-channel output (capture pipeline in `audio/capture.rs`). The DSP continues
to do what it's good at (beamforming 7 mics in hardware at 450 MHz), while the
ARM runs openWakeWord with a custom wake word on the beamformed audio.

The SRAM dump and firmware disassembly confirmed that:
- Beamforming uses binary-quantized delay-and-sum (2 gain values: 75/100), not adaptive MVDR
- Wake word detection uses spectral template matching, not a neural network
- No weight matrices or activation functions to patch
- Replacing "Hey Cortana" would require regenerating filter coefficients
- Much easier to just ignore the DSP's wake word and run our own on ARM

## Firmware Search Paths

Encore (like the stock client before it) checks these locations in order when uploading
DSP firmware, which is what makes custom-firmware testing safe and reversible:

1. `/media/usb/dsp-img.ldr` (USB override)
2. `/data/test/dsp-img.ldr` (test override, writable)
3. `/usr/share/dsp/dsp-img.ldr` (production, read-only rootfs)

## Stock Software Notes

For the record: the stock system split DSP work across two binaries talking over a WAMP
RPC bus (`autobahn-cpp`). `dsp-client` owned the SPI link (commands, events, firmware
upload, a 500-entry outgoing message queue) and `mcu-interface` owned DSP power via the
IO Expander. Encore replaces all of that with direct SPI and I2C access in
`encore/crates/encore-firmware/src/mcu/dsp.rs` and `io_expander.rs`. The full
decompilation notes, including per-function addresses, can be regenerated with Ghidra
from a stock rootfs if you ever need them.

## DSP Power Control

DSP power is managed via the **IO Expander (I2C 0x20)**, independent of the
SPI communication channel. This was handled by `mcu-interface` in the stock
system, not by `dsp-client`.

### IO Expander Register Map (Complete)

All bits are on **register 0x01** (output port) unless noted:

| Bit | Mask | Controls | Set (1) | Clear (0) |
|-----|------|----------|---------|-----------|
| 0 | 0x01 | DSP reset | Release reset | Hold in reset |
| 1 | 0x02 | AMP mute | **MUTED** | Unmuted |
| 2 | 0x04 | DAC mute | Unmuted | **MUTED** |
| 3 | 0x08 | DSP power 2 (reg 0x02) | Power off | **Power on** |
| 4 | 0x10 | DSP power 1 (reg 0x01) | Power off | **Power on** |

**DSP power bits are active-low**: clear the bit to enable power, set to disable.
This is the reverse of what you'd expect. Confirmed from `mcu-interface.c`
decompilation at address 0x000b3a7c (`com.harman.vui.powerdspcontrol`):

```c
// Power ON: clear both power bits
reg01 &= 0xF7;  // Clear bit 3 of register 0x01
reg02 &= 0xEF;  // Clear bit 4 of register 0x02

// Power OFF: set both power bits (inverse)
reg01 |= 0x08;  // Set bit 3 of register 0x01
reg02 |= 0x10;  // Set bit 4 of register 0x02
```

**Note**: Bits 3 and 4 span two IO Expander registers (0x01 and 0x02). The
stock `mcu-interface` binary does read-modify-write on both registers.

### Power-On Sequence

The ordering below matters: muting before power-up prevents speaker pops, and the DSP
must be powered and reset-released before firmware upload can start. The sequence was
recovered from the stock `mcu-interface` decompilation, and Encore's boot follows the
same order:

```
1. IO Expander init
   - Mute AMP (set bit 1)
   - Mute DAC (clear bit 2)
   - Set bit 0 (release DSP reset)

2. Power on DSP
   - Register 0x01: set bit 4 (0x10)
   - Register 0x02: set bit 3 (0x08)
   (Note: boot-time init SETS these bits, contrary to the active-low
    power-on seen in the runtime power-toggle handler. The init sequence
    likely enables power rails differently from the runtime toggle.)

3. DAC init
   - 10-register TAS5756M configuration via I2C 0x4C

4. Stabilization delay

5. DSP firmware upload over SPI, then normal service startup
```

## Kernel SPI Driver Stack

The ARM SoC communicates with the DSP through a standard Linux SPI subsystem.
There is **no kernel-level DSP driver** — all DSP communication is userspace.

### Driver Hierarchy

```
┌──────────────────────────────────────┐
│  Userspace (dsp-client / Encore)     │
│  - Opens /dev/spidev0.0             │
│  - GPIO polling (4, 5, 12, 13, 15)  │
│  - SPI ioctl configuration          │
│  - Firmware upload + messaging      │
└─────────────┬────────────────────────┘
              │ /dev/spidev0.0
┌─────────────▼────────────────────────┐
│  Kernel SPI Subsystem                │
│  - spi-dw.c (DesignWare core, 936L) │
│  - spi-dw-mmio.c (MMIO binding)     │
│  - Generic spidev character driver   │
└─────────────┬────────────────────────┘
              │ SPI bus (mode 3, 8-bit)
┌─────────────▼────────────────────────┐
│  ADSP-21489 SHARC DSP (450 MHz)     │
│  - SPI slave interface              │
│  - 5 Mbit on-chip SRAM (volatile)   │
│  - dsp-img.ldr (160 KB firmware)    │
└──────────────────────────────────────┘
```

### Device Tree (berlin2cdp-a0.dtsi)

```dts
spi1: spi@F7E81C00 {
    compatible = "snps,designware-spi";
    reg = <0xF7E81C00 0x100>;
    num-cs = <4>;
    clocks = <&cfgclk>;
    interrupt-parent = <&apb_ictl>;
    interrupts = <7>;
};
```

### SPI ioctl Constants

| ioctl | Hex | Purpose |
|-------|-----|---------|
| `SPI_IOC_WR_MODE` | 0x40016b01 | Set SPI mode (3 = CPOL=1, CPHA=1) |
| `SPI_IOC_RD_MODE` | 0x80016b01 | Read SPI mode |
| `SPI_IOC_WR_BITS_PER_WORD` | 0x40016b03 | Set word size (8) |
| `SPI_IOC_RD_BITS_PER_WORD` | 0x80016b03 | Read word size |
| `SPI_IOC_WR_MAX_SPEED_HZ` | 0x40046b04 | Set clock (1 MHz upload, 59 kHz runtime) |
| `SPI_IOC_RD_MAX_SPEED_HZ` | 0x80046b04 | Read max speed |
| `SPI_IOC_MESSAGE(N)` | 0x40206b00 | Full-duplex SPI transfer |

### SoC Audio Subsystem MMIO (Marvell BG2CDP)

| Region | Address | Size | Purpose |
|--------|---------|------|---------|
| AG DHUB | 0xF7D00000 | 128 KB | Audio General Data Hub (DMA engine) |
| I2S | 0xF7E70000 | 64 KB | I2S serial audio port |
| AVIO Global | 0xF7E20000 | 64 KB | Audio/video global control |
| AVIF | 0xF7980000 | 512 KB | Audio/video interface |
| SPI-0 | 0xF7E81C00 | 256 B | DesignWare SPI controller |
| ZSP | 0xF7C00000 | 512 KB | Marvell internal DSP core (NOT the ADSP-21489) |

**ZSP note**: The Marvell BG2CDP has its own internal "ZSP" DSP at 0xF7C00000.
This is a **separate chip** from the ADSP-21489 on the amplifier board. The ZSP
appears unused in the Invoke — all audio processing goes to the external SHARC.

## Encore Implementation

### Current Status

Encore's DSP driver reproduces the stock `dsp-client` initialization sequence. AVIO
registers are configured with read-modify-write (writing fixed constants destroys the
kernel's I2S routing), GPIO 5 is set up via AVIO and exported only for the upload
chip-select, and GPIOs 4 (output) and 12/13/15 (inputs) are exported for runtime flow
control. Earlier Encore versions configured GPIO 13 as an output, which contradicted the
stock setup and caused CPU pegging; matching the stock pin directions fixed it.

What works:

- Firmware upload over SPI, following the stock register and GPIO sequence
- Commands: volume, mic mute, version query, memory dump
- Event reception by polling (200 ms interval): the `DSP_BOOTUP` event gates the unmute
  sequence at boot, and version and memory-dump responses arrive the same way

The stock firmware's wake word events (`TRIGGER_FOUND` and friends) can arrive through the
same polling path, but Encore does not act on them: the voice pipeline captures the DSP's
beamformed output through ALSA (`audio/capture.rs`) and leaves wake word detection to the
ARM side.

### Files

| File | Purpose |
|------|---------|
| `encore/crates/encore-firmware/src/mcu/dsp.rs` | DSP driver: firmware upload, commands, memory dump (bounded SPI, no GPIO) |
| `encore/crates/encore-firmware/src/mcu/io_expander.rs` | IO Expander: amp/DAC mute, DSP reset |
| `encore/crates/encore-firmware/src/audio/subsystem.rs` | Audio subsystem: DSP init, volume/mute routing |
| `scripts/dsp/dsp_dump_memory.py` | Standalone Python script for full 320-page SRAM dump |

### Key Types

```rust
// DSP events received via GPIO-controlled SPI
pub enum DspEvent {
    DacGain(u8),
    ExpectSpeech,          // VAD — voice activity detected
    CancelTrigger,         // VAD — voice activity ended
    Version(Vec<u8>),
    MicMute(bool),
    MemoryDump { page: u16, data: Vec<u8> },
    TriggerFound,          // Wake word detected ("Hey Cortana")
    PayloadBegin,
    PayloadEnd,
    Bootup,
    Unknown { category: u16, code: u8, data: Vec<u8> },
}
```

### Web API

| ClientMsg | Description |
|-----------|-------------|
| `DspSpiSend { msg_type, data }` | Send raw SPI message, get response |
| `DspMemoryDump { start_page, num_pages }` | Dump DSP memory pages |
| `DspPollEvents` | Poll for pending DSP events |
| `SetDspVolume(level)` | Set DSP volume (1-100) |
| `SetMicMute(muted)` | Set hardware mic mute |

## Appendix: Sources

The `vendor/` paths below refer to a local working tree of extracted stock firmware and
generated analysis outputs. That tree is **not** part of this repository (the stock
firmware is not redistributed); everything in it can be regenerated from your own stock
image using the analysis tools listed further down.

All protocol details were reverse-engineered from:
- `vendor/ghidra_output/dsp-client.c` — Ghidra decompilation of stock ARM binary
- `vendor/ghidra_output/mcu-interface.c` — Ghidra decompilation of MCU interface
- `vendor/firmware/squashfs-root/usr/share/dsp/dsp-img.ldr` — firmware binary (160,484 bytes)
- `vendor/dsp_dump.bin` — Live SRAM dump from running device (655,360 bytes, 2026-02-19)
- Python fallback driver removed — Encore handles all DSP I/O natively
- ADSP-21160 Instruction Set Reference, Rev 2.1 (Analog Devices, available from analog.com) — basis for the disassembler's instruction decoding

### Disassembly Output
- `vendor/dsp_boot_kernel_disasm.txt` — Boot kernel: 256 PM words, SPI block loader
- `vendor/dsp_firmware_disasm.txt` — SRAM dump: 59,850 instructions, 4,547 labels (runtime state)
- `vendor/dsp_firmware_ldr_disasm.txt` — .ldr file: 25,363 instructions, 2,290 labels (original firmware)
- `vendor/dsp_annotated.txt` — Fully annotated disassembly: 107 functions, 1,261 labels, IVT map, call graph (6.6 MB)

### Analysis Tools
- `scripts/sharc/sharc_disasm.py` — Core SHARC 48-bit instruction decoder (all 25 instruction types)
- `scripts/sharc/sharc_asm.py` — SHARC assembler (inverse of disassembler, .ldr byte output)
- `scripts/dsp/disasm_firmware.py` — Full SRAM dump disassembler (PM48 words from dsp_dump.bin)
- `scripts/dsp/disasm_ldr_full.py` — Complete .ldr disassembler (parses blocks, reconstructs DM, classifies code/data)
- `scripts/dsp/analyze_dsp_dump.py` — SRAM analysis: page map, beamforming matrix, string extraction
- `scripts/ldr/ldr_parse.py` — .ldr block parser (header, type, address, count)
- `scripts/dsp/dsp_dump_memory.py` — Standalone dump tool for full 320-page SRAM capture

### External References

- [ADSP-21489 Datasheet](https://www.analog.com/media/en/technical-documentation/data-sheets/adsp-21483_21486_21487_21488_21489.pdf)
- [ADSP-214xx Hardware Reference (Rev 1.1)](https://www.analog.com/media/en/dsp-documentation/processor-manuals/ADSP-214xx_hwr_rev1.1.pdf)
- [SHARC Programming Reference (Rev 2.4)](https://www.analog.com/media/en/dsp-documentation/processor-manuals/adsp-2136x_2137x_214xx_pgr_rev2.4.pdf)
- **ADSP-21160 SHARC Instruction Set Reference, Rev 2.1** — Analog Devices; classic SHARC ISA encoding, the basis for `sharc_disasm.py`. Available from analog.com (not redistributed here).
- [CrossCore Embedded Studio (CCES)](https://www.analog.com/en/resources/evaluation-hardware-and-software/software/adswt-cces.html) — free evaluation, only SHARC compiler (**cannot disassemble classic SHARC, only SHARC+**)
- **VisualDSP++ 5.1.2** — last ADI toolchain supporting ADSP-214xx (discontinued, not freely available)
- **remora** (GitHub) — open-source ADSP-21489 audio project (SRU, SPORT, SPI, PLL, DMA)
- **Wavedrum** (GitHub) — partial Haskell SHARC disassembler targeting ADSP-21375 (same ISA family)
- **adsp-ldr** (Python) — SHARC `.ldr` file parser (standard ADI format)
- **Analog Devices EngineerZone** — SHARC/ADSP-214xx community forums
