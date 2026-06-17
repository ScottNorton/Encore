# Encore Porting Status — Vendor Cross-Reference

Comprehensive audit of every binary, script, firmware blob, kernel module, and
hardware interface in the stock Harman Kardon Invoke firmware, cross-referenced
against Encore's Rust implementation status.

**Vendor source**: `vendor/firmware/squashfs-root/` (stock rootfs), `vendor/kernel/` (full kernel source)
**Encore source**: `encore/crates/` (Rust workspace), `rootfs/` (overlay)

> **Note (snapshot):** This audit is a point-in-time cross-reference (see the date at the
> bottom). Some items listed below as not-yet-ported have since landed in Encore — most
> notably native microphone capture (`audio/capture.rs`, replacing `arecord`) and DSP event
> handling (bootup, version, mic mute) in `mcu/dsp.rs`. When a row disagrees with the
> source tree, trust the source tree.

---

## Legend

| Symbol | Meaning |
|--------|---------|
| :white_check_mark: | Fully ported to Rust in Encore |
| :large_orange_diamond: | Partially ported / shell script shim still needed |
| :red_circle: | Not ported — stock binary or script still required |
| :no_entry: | Removed — Harman/Microsoft service, not needed |
| :black_circle: | Kernel/bootloader — cannot be changed (OTP-encrypted) |

---

## 1. Boot Chain

The stock boot sequence is: U-Boot → kernel → `init` (Android-style) → `init.rc` → `mount_partition.sh` → Podium (service manager) → all daemons.

Encore hooks in at `mount_partition.sh`, replacing Podium and all downstream daemons.

| Component | Stock Path | Status | Encore Equivalent | Notes |
|-----------|-----------|--------|-----------------|-------|
| Bootloader | NAND partition 0 | :black_circle: | — | OTP-encrypted, immutable |
| Kernel (3.8.13) | NAND partition 2 | :black_circle: | — | OTP-encrypted, immutable |
| `init` binary | `/init` | :black_circle: | — | Android init, executes init.rc |
| `init.rc` | `/init.rc` | :red_circle: | — | Unchanged stock; calls mount_partition.sh |
| `mount_partition.sh` | `/sbin/mount_partition.sh` | :large_orange_diamond: | — | Shell script mounts partitions + launches Encore via supervisor |
| `wpa_supplicant_setup.sh` | `/sbin/wpa_supplicant_setup.sh` | :red_circle: | — | Stock script loads WiFi kernel modules, sets MAC |
| `start_ap.sh` | `/sbin/start_ap.sh` | :red_circle: | — | Shell: hostapd + dnsmasq on p2p0 |
| `auto_wifi_firewall.sh` | `/sbin/auto_wifi_firewall.sh` | :red_circle: | — | Shell: wpa_cli connect + iptables |
| `S01firewall` | `/etc/init.d/S01firewall` | :red_circle: | — | Shell: iptables rules (LAN-only inbound) |
| Boot-time I2C mute | `S00audiomute` | :white_check_mark: | `tools/i2c_mute.c` | Static C binary (9.5 KB), replaces Python3 snippet |
| `boot_complete.sh` | `/sbin/boot_complete.sh` | :no_entry: | — | Writes marker file; not needed |
| `unmute_audio.sh` | `/sbin/unmute_audio.sh` | :no_entry: | — | amixer commands; disabled in stock init.rc |
| `copy_firmware.sh` | `/sbin/copy_firmware.sh` | :no_entry: | — | OTA recovery; disabled |
| `watchdog_setup.sh` | `/sbin/watchdog_setup.sh` | :no_entry: | — | Process monitor config; Podium handles this |

### Boot chain porting gaps

1. **WiFi module loading** — `wpa_supplicant_setup.sh` loads `mlan.ko` + `sd8xxx.ko`. Still required as shell (kernel module loading can't be done from userspace Rust without `init_module` syscall, which isn't worth the complexity).
2. **AP mode** — `start_ap.sh` configures hostapd + dnsmasq. Could be ported to Rust but low priority since it works.
3. **Firewall** — `iptables` binary is missing on device. Encore's `network/firewall.rs` exists but can't apply rules without it.

---

## 2. Podium Services (Stock Service Manager)

Podium (`/usr/bin/run-podium.sh`) manages 14 services. Encore replaces all of them
via its `SubsystemManager`.

| Stock Service | Binary | Size | Status | Encore Subsystem |
|--------------|--------|------|--------|----------------|
| `mcu-interface` | `/usr/bin/mcu-interface` | 1.15 MB | :white_check_mark: | `mcu/` (I2C, IO Expander, DAC, DSP) |
| `dsp-client` | `/usr/bin/dsp-client` | 700 KB | :white_check_mark: | `mcu/dsp.rs` (SPI firmware upload) |
| `audio-ui` | `/usr/bin/audio-ui` | 1.4 MB | :white_check_mark: | `audio/subsystem.rs` (mixer, PCM, ALSA ctl) |
| `connection-manager` | `/usr/bin/connection-manager` | 144 KB | :white_check_mark: | `network/` (wpa_supplicant, state) |
| `spotify` | `/usr/bin/spotify` | 368 KB | :white_check_mark: | `spotify.rs` (librespot in-process) |
| `music-source-manager` | `/usr/bin/music-source-manager` | ? | :no_entry: | — (media routing; not needed with single mixer) |
| `bluetooth` | `/usr/bin/bluetooth` | 728 KB | :white_check_mark: | `bluetooth/` (raw kernel sockets — HCI mgmt + L2CAP, aptX HD + aptX + SBC) |
| `cortana-harness` | `/usr/bin/cortana-harness` | 99 KB | :no_entry: | Wyoming replaces Cortana |
| `cortana` | `/usr/bin/cortana` | 1.6 MB | :no_entry: | — (Microsoft voice; removed) |
| `engine` | `/usr/bin/engine` | 1.9 MB | :no_entry: | — (speech engine; removed) |
| `visual-ui` | `/usr/bin/visual-ui` | 651 KB | :no_entry: | — (LED visual feedback; Encore has `led.rs`) |
| `factory-test` | `/usr/bin/factory-test` | 1.1 MB | :no_entry: | — (factory testing; not needed) |
| `crash-uploader-HK.sh` | `/usr/bin/crash-uploader-HK.sh` | script | :no_entry: | — (Harman telemetry; removed) |
| `device_auto_recovery.sh` | script | script | :no_entry: | — (watchdog.rs handles recovery) |

---

## 3. Hardware Interfaces (I2C Bus 0)

All discovered via vendor source + device probing. Vendor kernel provides
DesignWare I2C driver (`i2c-designware-platdrv.c`).

| Address | IC | Stock Driver | Status | Encore Module | RE Completeness |
|---------|-------|-------------|--------|-------------|-----------------|
| `0x20` | IO Expander (PCA9538) | `mcu-interface` | :white_check_mark: | `mcu/io_expander.rs` | 100% — mute/unmute, DSP reset |
| `0x36` | MCU (TI MSP430FR5739) | `mcu-interface` | :white_check_mark: | `mcu/mod.rs` | ~90% — commands, events, LED, volume |
| `0x4C` | DAC (TI TAS5756M) | `audio-ui` / `mcu-interface` | :white_check_mark: | `mcu/dac.rs` | ~70% — init + volume; full register map unknown |
| `0x10` | **Not a device** | — | :no_entry: | — | Bitmask 0x10 on IO Expander 0x20, not separate IC |
| `0x64` | Unknown | — | :no_entry: | — | Zero references in any stock binary; unused or absent |
| `0x48` | Temp sensor (LM75?) | — | :no_entry: | — | Not populated on PCB |

### MCU Protocol (0x36) — Detailed Status

| Feature | Status | Notes |
|---------|--------|-------|
| Version query (`0x01`) | :white_check_mark: | |
| LED ring write (39 bytes) | :white_check_mark: | 13 LEDs × 3 RGB |
| LED acknowledge (`0x23`) | :white_check_mark: | |
| Heartbeat (`0x24`) | :white_check_mark: | |
| Device color (`0x25`) | :white_check_mark: | |
| Status query (`0x26`) | :white_check_mark: | |
| Volume set (`0x03`) | :white_check_mark: | With sequence toggle |
| Touch press events | :white_check_mark: | Short + long |
| Volume ring events | :white_check_mark: | Up/down with step count |
| Bluetooth button events | :white_check_mark: | Short + long |
| Mic mute button events | :large_orange_diamond: | Event code inferred, not confirmed on device |
| Reset button events | :large_orange_diamond: | Event code inferred |
| MCU firmware update | :large_orange_diamond: | Protocol decoded: cmd 0x12 (128-byte chunks) + cmd 0x11 (CRC16 verify) |

### Unknown I2C Devices — RESOLVED (Ghidra)

Ghidra decompilation of `mcu-interface` (2,624 functions) revealed:
- **0x10**: **Not a separate I2C device.** The value 0x10 is a bitmask (bit 4) used in register operations on the IO Expander at 0x20. The I2C bus scan artifact was likely the IO Expander responding to a broadcast.
- **0x64**: **Zero references** in the entire `mcu-interface` binary. Not initialized, not read, not written. Either absent on the PCB or controlled by a subsystem we don't interact with (factory provisioning?).

---

## 4. DSP (ADSP-21489 SHARC via SPI-0)

| Feature | Status | Encore Module | Notes |
|---------|--------|-------------|-------|
| SoC register programming | :white_check_mark: | `mcu/dsp.rs` | `/dev/mem` mmap: AVIO_404, AVIO_400, AUDIO_CLK |
| Firmware upload (SPI) | :white_check_mark: | `mcu/dsp.rs` | 160 KB, bit-reversed, 1 MHz, 4-byte chunks |
| IO Expander DSP reset | :white_check_mark: | `mcu/io_expander.rs` | Toggle bit 0 |
| Volume command | :white_check_mark: | `mcu/dsp.rs` | Message type 0x0000, data `[0x04, level]` |
| Mic mute command | :red_circle: | — | Message type 0x0000, data `[0x09, 0/1]` — decoded via Ghidra |
| Get version command | :red_circle: | — | Message type 0x0000, data `[0x08]` — response: 4 bytes (V.X.X.X) |
| HW test command | :no_entry: | — | Message type 0x0002, data `[0x03, param]` — factory only |
| Memory dump command | :no_entry: | — | Message type 0x0000, data `[0x0c, addr_lo, addr_hi]` — debug only |
| DSP firmware internals | :red_circle: | — | ADSP-21489 SHARC ISA; beamforming/AEC/EQ baked into firmware |

### DSP Firmware (`dsp-img.ldr`)
- **Size**: 160,768 bytes (157 KB)
- **Format**: Custom Libre Wireless `.ldr` loader image, bit-reversed per byte
- **Upload**: SPI-0 at 1 MHz, 4-byte chunks with pause every 1536 bytes
- **Location**: `/usr/share/dsp/dsp-img.ldr` (also checked `/media/usb/`, `/data/test/`)

### DSP Command Set — FULLY DECODED (Ghidra)

Ghidra decompilation of `dsp-client` (2,025 functions) revealed the **complete** userspace command set:

| Opcode | Command | Payload | Notes |
|--------|---------|---------|-------|
| `0x04` | Set volume | `[0x04, level]` | Already implemented in Encore |
| `0x09` | Mic mute | `[0x09, 0/1]` | Useful for privacy button |
| `0x08` | Get version | `[0x08]` | Returns 4-byte version quad |
| `0x03` | HW perform test | `[0x03, param]` | Factory test only |
| `0x0c` | Memory dump | `[0x0c, addr_lo, addr_hi]` | Debug diagnostic |
| `0x01` | Trigger/payload | `[0x01]` | Firmware upload marker |

**Key insight**: There are NO EQ, beamforming, echo cancellation, or audio routing commands.
All DSP audio processing is **hardcoded in the DSP firmware** (`dsp-img.ldr`). The userspace
only controls volume and mic mute. This means Encore is NOT missing any DSP functionality.

### DSP Events (FROM DSP → Host)

| Event Type | Code | Name | Action |
|-----------|------|------|--------|
| 0x0001 | 0x04 | DSP_BOOTUP | DSP ready; triggers DAC/AMP unmute |
| 0x0000 | 0x08 | DSP_VERSION | 4-byte version response |
| 0x0000 | 0x04 | NEW_DAC_GAIN | DAC gain update notification |
| 0x0000 | 0x05 | EXPECT_SPEECH | Speech input detected |
| 0x0000 | 0x06 | CANCEL_TRIGGER | Cancel wake word trigger |
| 0x0000 | 0x09 | MIC_MUTE | Mic muted confirmation |
| 0x0001 | 0x00 | TRIGGER_FOUND | Wake word detected |
| 0x0001 | 0x01 | PAYLOAD_BEGIN | Audio payload start |
| 0x0001 | 0x02 | PAYLOAD_END | Audio payload complete |
| 0x0001 | 0x03 | PAYLOAD_TIMEOUT | Audio payload timed out |
| 0x0002 | 0x00-03 | MIC_TEST_* | Mic hardware test results |
| 0x00xx | 0xff | ERROR | Generic/write/test error |

### DSP SPI Protocol Details

- **5-byte header**: `[type_hi, type_lo, len_hi, len_lo, checksum]`
- **Checksum**: Simple wrapping sum of all header + payload bytes
- **Padding**: Messages padded to 8-byte alignment (matches Encore's `build_spi_message`)
- **GPIO**: Upload uses GPIOs 4, 5, 12, 13, 15 for reset/chipselect sequence
- **Interrupt**: GPIO 3 signals DSP has response ready
- **Retry**: 3 attempts with 100ms/100ms/500ms backoff
- **Queue**: 500-entry circular buffer for pending messages

---

## 5. Audio Pipeline

| Component | Stock | Status | Encore Module | Notes |
|-----------|-------|--------|-------------|-------|
| ALSA PCM playback (card 1, device 0) | ALSA (libasound/tinyalsa) | :white_check_mark: | `audio/pcm.rs` | Direct ioctl, S32_LE 48kHz 2ch |
| ALSA mixer controls (card 1) | `amixer` binary | :white_check_mark: | `audio/alsa_ctl.rs` | 44 WM8904 controls; dump + set |
| WM8904 codec init | Kernel driver (wm8904.c) | :white_check_mark: | `audio/subsystem.rs` | Master=90, HP=50, OSR=on |
| Software mixer | GStreamer / audio-ui | :white_check_mark: | `audio/mixer.rs` | Ring buffer SPSC, lock-free |
| 44.1→48kHz resampler | GStreamer | :white_check_mark: | `spotify.rs` | Linear interpolation |
| Microphone capture | `arecord` binary | :red_circle: | — | Wyoming uses `arecord -D mic` |
| Microphone array / beamforming | DSP firmware | :red_circle: | — | 7-mic array, DSP handles |
| AEC (echo cancellation) | DSP firmware | :red_circle: | — | DSP firmware handles |

### Audio Machine Driver (vendor kernel)

From `vendor/kernel/sound/soc/berlin/marvell-wm8904.c`:
- WM8904 codec attached via I2S to Berlin SoC
- dHUB (DMA engine) handles audio transfers
- Machine driver configures WM8904 as I2S master (Bclk + LrClk)
- DAPM route: Speaker output from HP_L/HP_R
- IRQ-driven PCM via `berlin_devices_aout_isr()`

**Key insight**: The kernel handles codec↔SoC I2S routing. Encore only needs ALSA PCM ioctls for playback, which is already working. No need to touch the machine driver.

---

## 6. Kernel Modules

| Module | Source Available | Status | Notes |
|--------|----------------|--------|-------|
| `mlan.ko` (404 KB) | :white_check_mark: `vendor/kernel/arch/arm/mach-berlin/modules/wlan_sd8887/` | :red_circle: Stock | WiFi MAC layer — works, no changes needed |
| `sd8xxx.ko` (565 KB) | :white_check_mark: Same dir | :red_circle: Stock | WiFi SDIO transport — works |
| `bt8xxx.ko` (108 KB) | :white_check_mark: `vendor/kernel/arch/arm/mach-berlin/modules/bt_sd8887/` | :white_check_mark: Working | BT SDIO driver — loaded by Encore, managed via raw kernel sockets |
| `btmrvl.ko` (11 KB) | :white_check_mark: `vendor/kernel/drivers/bluetooth/` | :red_circle: Stock | HCI bridge for bt8xxx |
| `ehci-platform.ko` (6 KB) | :white_check_mark: Mainline | :red_circle: Stock | USB host controller |
| `touchfilekmod.ko` (3 KB) | Unknown | :red_circle: Stock | Unknown purpose |
| `wm8904.c` (codec) | :white_check_mark: Mainline + Berlin | :black_circle: Built-in | WM8904 ALSA codec driver |
| `marvell-wm8904.c` (machine) | :white_check_mark: `vendor/kernel/sound/soc/berlin/` | :black_circle: Built-in | SoC↔codec tie-in |
| DesignWare I2C | :white_check_mark: Mainline | :black_circle: Built-in | I2C bus driver |
| DesignWare SPI | :white_check_mark: Mainline | :black_circle: Built-in | SPI bus driver |
| DesignWare WDT | :white_check_mark: Mainline | :black_circle: Built-in | Watchdog timer |

### Bluetooth Driver Analysis

**Source**: `vendor/kernel/arch/arm/mach-berlin/modules/bt_sd8887/`

**Status: WORKING** — Encore manages Bluetooth via raw kernel sockets (HCI management + L2CAP + SDP + AVDTP). No BlueZ daemon or D-Bus required.

**Working sequence** (device-tested):
1. `insmod bt8xxx.ko bt_mac=XX:XX:XX:XX:XX:XX` → hci0 appears
2. `hciconfig hci0 up`
3. Encore manages BT via raw kernel sockets (HCI management + L2CAP)
4. Phone paired + connected, A2DP Audio Sink confirmed

**Encore manages the full lifecycle** in `bluetooth/mod.rs`:
- Module load → HCI config → raw kernel socket management
- Three codecs registered: aptX HD (highest priority), aptX, SBC (fallback)
- No BlueZ userspace stack required — direct kernel socket API

---

## 7. Firmware Blobs (Binary, No Source)

| Blob | Size | Location | Status | Notes |
|------|------|----------|--------|-------|
| `dsp-img.ldr` | 157 KB | `/usr/share/dsp/` | :red_circle: Required | ADSP-21489 SHARC DSP firmware; upload protocol RE'd |
| `sd8887_wlan_a2_p78.bin` | 365 KB | `/lib/firmware/mrvl/` | :red_circle: Required | WiFi radio firmware |
| `sd8887_bt_a2.bin` | 253 KB | `/lib/firmware/mrvl/` | :red_circle: Required | BT radio firmware (primary) |
| `sd8887_bt_a2_new.bin` | 178 KB | `/lib/firmware/mrvl/` | :red_circle: Available | BT firmware (newer variant) |
| `txpwrlimit_cfg_8887.bin` | 8.9 KB | `/lib/firmware/mrvl/` | :red_circle: Required | WiFi TX power limits |
| `cortana_mcu.bin` | 13 KB | `/usr/share/mcu/` | :red_circle: Unknown | MSP430FR5739 MCU firmware — update protocol decoded |
| `WlanCalData_ext-LS9*.conf` | 1.7 KB ea | `/lib/firmware/mrvl/` | :red_circle: Required | WiFi calibration data |
| LED animations (×35) | ~144 KB | `/usr/share/lights/` | :no_entry: Replaceable | Binary animation files; format RE'd |

---

## 8. External Binaries Still Required

These are stock binaries that Encore (or the boot chain) still depends on.

| Binary | Purpose | Called By | Replaceable with Rust? |
|--------|---------|-----------|----------------------|
| `wpa_supplicant` | WiFi 802.11 association | Boot script + Encore network | No — kernel requires it |
| `wpa_cli` | WiFi management commands | Boot scripts, Encore `wpa.rs` | No — talks to wpa_supplicant |
| `hostapd` | WiFi AP mode | `start_ap.sh` | No — kernel requires it |
| `dnsmasq` | DNS/DHCP for AP mode | `start_ap.sh` | Not worth it |
| `arecord` | Mic capture (16kHz S16_LE) | Encore `wyoming/` | :white_check_mark: Yes — direct PCM ioctls like `pcm.rs` |
| `ifconfig` | Get interface IP | Encore `network/mod.rs` | :white_check_mark: Yes — parse `/proc/net/fib_trie` or netlink |
| `iptables` | Firewall rules | `S01firewall` | :large_orange_diamond: Missing on device; needs cross-compile or netlink |
| `insmod` | Load kernel modules | Boot scripts | No — need `init_module` syscall |
| `i2c_mute` | Boot-time I2C mute | `S00audiomute` | :white_check_mark: Static C binary (9.5 KB), runs before Encore starts |
| `amixer` | ALSA mixer control | Removed | :white_check_mark: Already replaced by `alsa_ctl.rs` |
| `aplay` | Audio playback | Removed | :white_check_mark: Already replaced by `pcm.rs` |

---

## 9. Python Services (Replaced by Encore)

| Python Script | Size | Status | Encore Equivalent |
|--------------|------|--------|-----------------|
| `mcu_driver.py` | ~2000 lines | :white_check_mark: | `mcu/`, `led.rs`, `audio/subsystem.rs` |
| `hk-webserver.py` | ~1600 lines | :white_check_mark: | `web/` (Axum + WASM dashboard) |
| `ha_bridge.py` | ~1400 lines | :white_check_mark: | `homeassistant.rs` |
| `wyoming_satellite.py` | ~1300 lines | :white_check_mark: | `wyoming/` |
| `bt_audio.py` | ~800 lines | :white_check_mark: | `bluetooth/` (raw kernel sockets — HCI mgmt + L2CAP, aptX HD + aptX + SBC) |
| `start_librespot.sh` | ~100 lines | :white_check_mark: | `spotify.rs` (librespot in-process) |
| `spotify_event.sh` | ~20 lines | :white_check_mark: | `spotify.rs` (event handling in-process) |

---

## 10. Network Stack

| Component | Stock | Status | Encore Module | Notes |
|-----------|-------|--------|-------------|-------|
| WiFi client (wpa_supplicant) | Stock binary | :white_check_mark: | `network/wpa.rs` | Unix socket IPC |
| WiFi state monitoring | `connection-manager` | :white_check_mark: | `network/mod.rs` | Parse `/proc/net/wireless` |
| WiFi AP mode | `hostapd` + `start_ap.sh` | :red_circle: Shell | Encore relies on boot script |
| DNS/DHCP (AP mode) | `dnsmasq` | :red_circle: Shell | Boot script handles |
| Firewall | `iptables` | :red_circle: Missing | `network/firewall.rs` exists but can't run |
| mDNS (Spotify discovery) | `mdnsd` (stock) + librespot | :white_check_mark: | librespot handles mDNS |
| DNS resolution fix | `auto_wifi_firewall.sh` | :red_circle: Shell | Writes gateway IP to `/etc/resolv.conf` |

### USB RNDIS network gadget — :white_check_mark: Working (verified on hardware 2026-06-17)

Encore adds a USB network gadget the stock firmware does not configure for end users. Plug a USB cable from the speaker's USB Mini-B port into a computer and the speaker appears as a USB network adapter. On Windows it binds the built-in RNDIS driver automatically (via Microsoft OS descriptors), with no driver install. This does not depend on WiFi and comes up automatically at boot, so it also works as a recovery channel when WiFi or the access point is unavailable.

| Property | Value |
|----------|-------|
| Speaker USB address | `10.55.55.1` (runs a small DHCP server on the link, pool `10.55.55.10-50`) |
| Gateway / DNS advertised | None (deliberate, so plugging in never disturbs the computer's existing internet, e.g. a phone tether) |
| Services over USB | Web dashboard at `http://10.55.55.1/` (plain HTTP, no certificate prompt on this link), SSH at `root@10.55.55.1`, the same HTTP/WebSocket API the desktop/mobile apps use |
| Controller | Marvell `mv_udc` USB device controller |
| Kernel modules | Patched `g_ether`/RNDIS, built with the period Linaro 4.9.4 cross-compiler (a modern gcc builds modules that load but then fault the 3.8.13 kernel) |
| Module location | Baked into the rootfs at `/usr/lib/usbgadget/` |
| Bring-up | `rootfs/sbin/usb_gadget.sh`, self-healed by `usb_gadget_monitor.sh` |
| Kernel patches | Tracked as diffs in `scripts/device/usb-gadget-patches/`, rebuilt with `scripts/device/build_usb_gadget_modules.sh` |

**SSH note**: the speaker runs Dropbear (an older SSH server). Add `-o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa`. User `root`, password `ridiculous` (same on every device; change with `passwd` on an untrusted network).

**Known limitation (open)**: the `mv_udc` controller stalls multi-packet bulk transfers, so the gadget MTU is fixed at 400 bytes. This keeps every frame a single USB packet, so the stall never triggers. It caps throughput (measured around 7 MB/s, which is fine for the dashboard, SSH, and OTA uploads) but keeps the link reliable. Raising the MTU brings the stall back. A controller-level fix is future work; the 400-byte MTU is the workaround that ships.

---

## 11. Vendor Source Discoveries (New Insights)

Things the vendor source reveals that we hadn't documented:

### 11a. MCU Firmware (`cortana_mcu.bin` — 13 KB) — PROTOCOL DECODED (Ghidra)

Stock firmware has an MCU firmware update binary. The update protocol is now known:

1. **Data transfer**: Send firmware in 128-byte chunks via MCU command `0x12`
   - Payload: `[0x12, ...128 data bytes...]` (129 bytes total)
   - 10ms delay between chunks
2. **CRC verification**: After all chunks, send command `0x11` with:
   - `[0x11, size_lo, size_hi, crc_lo, crc_hi]`
   - CRC16 computed across entire firmware file (custom polynomial)
3. **Firmware path**: `/usr/share/mcu/`

**Risk assessment**: Flashing bad MCU firmware bricks touch/LED/buttons. Current
MCU firmware works fine with Encore. No reason to update unless adding new MCU features.
The protocol is documented here for completeness.

### 11b. Bluetooth — Character Device Mode (Not Needed)

The bt8xxx driver source reveals a second build mode (`CONFIG_BERLIN_SDIO_BT_8887_CHAR_DRV=y`)
that creates `/dev/mbtchr` instead of an HCI socket. This is **not needed** — the default
HCI socket mode works fine with the kernel's bt8xxx driver. Encore uses raw kernel
sockets directly — no BlueZ daemon needed.

### 11c. dHUB (Audio/Video DMA Engine)

The kernel's `hal_dhub.c` (52 KB) and `dHub.h` (236 KB) reveal the Berlin SoC's
proprietary DMA engine used for audio transfer. Encore doesn't need to touch this
directly — the ALSA PCM driver abstracts it — but understanding it would be
necessary for capture (microphone array) without `arecord`.

### 11d. Audio PLL (`avpll.c` — 24 KB)

The kernel manages audio clock rates via a custom PLL. This explains why the
sample rate is fixed at 48 kHz — the PLL is configured for 48 kHz I2S clock
and changing it would require kernel modifications.

### 11e. Harman Libraries (System)

The stock firmware includes several Harman-specific libraries:
- `libgenericapi.so` (902 KB) — Generic Harman API
- `libJson.so` (1.2 MB) — JSON parser (Harman fork)
- `libreconnect.so` (636 KB) — Reconnection logic
- `libwac_ark.so` (126 KB) — WAC (Wireless Accessory Configuration, Apple)
- `library_fw_update.so` (203 KB) — Firmware update logic
- `library_si_update.so` (167 KB) — SI (System Image) update

None of these are needed by Encore. All functionality has been reimplemented.

### 11f. Hardware HAL Modules

`/system/lib/hw/` contains Android-style HAL modules:
- `bluetooth.default.so` (1.5 MB) — Full BT HAL with HCI management
- `audio.a2dp.default.so` (22 KB) — A2DP audio sink

These are used by the stock `bluetooth` service but not by Encore (which uses raw kernel sockets).

### 11g. WiFi Calibration and Power Limits

Vendor includes WiFi calibration configs in `/lib/firmware/mrvl/`:
- `WlanCalData_ext-LS9-20160725.conf` — For LS9 variant
- `WlanCalData_ext-LS9AD-20160725.conf` — For LS9AD variant
- `txpwrlimit_cfg_8887.bin` — TX power limit table

Plus 29 Marvell WiFi driver config files in `vendor/firmware/misc/config/`.

### 11h. Factory Data

NAND partition 4 (`/factory_setting/`) contains:
- WiFi MAC address (`WIFI_MAC_ADDR`) — stock boot reads this via `ethconfig` if OTP MAC is the Marvell default (`00:50:43:02:fe:01`)
- BT MAC address
- SSL certificates (`factory/client.crt`, `factory/client.key.bin`)
- U-Boot environment vars (device name, Spotify creds)
- General MAC address (`MAC_ADDR`) — used by the stock ramdisk's `srvd.conf` boot process for `eth0` (USB tethering)

---

## 12. Ghidra Decompilation Results

All three priority binaries decompiled via Ghidra 12.0.3 headless analysis.
Output in `vendor/ghidra_output/`.

| Target | Size | Functions | Status | Key Findings |
|--------|------|-----------|--------|-------------|
| `mcu-interface` | 1.15 MB | 2,624 | :white_check_mark: **COMPLETE** | 0x10 not a device, 0x64 unused, MCU FW update decoded, DAC init confirmed |
| `dsp-client` | 700 KB | 2,025 | :white_check_mark: **COMPLETE** | Full 6-command set, no EQ/beamform userspace control, SPI protocol verified |
| `audio-ui` | 1.4 MB | 2,871 | :white_check_mark: **COMPLETE** | ALSA mixer volume path, GStreamer playbin3, WAMP RPC IPC, ducking system |
| `bt8xxx.ko` | 108 KB | — | :no_entry: Not needed | Full GPL source available; recompile with CHAR_DRV=y |
| `dsp-img.ldr` | 157 KB | — | :red_circle: Future | ADSP-21489 SHARC ISA; beamforming/AEC baked in |
| `cortana_mcu.bin` | 13 KB | — | :red_circle: Future | MSP430FR5739 binary; protocol known but firmware internals aren't |
| `bluetooth.default.so` | 1.5 MB | — | :no_entry: Not needed | Stock Android BT HAL; Encore uses raw kernel sockets |

### Ghidra Validation Summary

**Encore implementation confirmed correct for:**
- IO Expander bit assignments (DSP=0, AMP=1, DAC=2)
- IO Expander init sequence (direction→mute AMP→mute DAC→DSP power)
- DAC 10-register initialization (one possible reg/val swap at entry 7, but DAC works)
- DSP SPI message format (5-byte header, checksum, 8-byte alignment)
- DSP volume command (msg_type=0x0000, data=[0x04, level])
- DSP firmware upload (bit-reversal, 1536-byte blocks, 10ms pauses)
- MCU I2C protocol (6-byte commands, volume sequence toggle)

**New capabilities decoded:**
- DSP mic mute command (`[0x09, 0/1]`) — implement for privacy button
- DSP version query (`[0x08]`) — useful for diagnostics
- MCU firmware update protocol (cmd 0x12 data + cmd 0x11 CRC16)
- DSP GPIO sequence (pins 4, 5, 12, 13, 15) for reset/upload
- Audio-ui ducking/fading system (soft volumes for alert overlay)

**Mysteries resolved:**
- I2C 0x10 is NOT a device (bitmask on 0x20)
- I2C 0x64 is never accessed (unused/absent)
- No EQ/beamform/AEC userspace control exists (all in DSP firmware)
- Stock audio-ui uses ALSA mixer, not direct DAC writes (confirms our approach)

---

## 13. Summary Scorecard

### By Category

| Category | Total Components | Ported to Rust | Shell/Stock | Removed | Immutable |
|----------|-----------------|----------------|-------------|---------|-----------|
| Boot chain | 12 | 1 | 5 | 4 | 2 |
| Podium services | 14 | 6 | 0 | 8 | 0 |
| I2C hardware | 5 | 3 | 0 | 2 | 0 |
| Audio pipeline | 8 | 6 | 0 | 0 | 0 |
| Network | 7 | 2 | 5 | 0 | 0 |
| Kernel modules | 7 | 0 | 5 | 0 | 2 |
| Firmware blobs | 8 | 0 | 7 | 1 | 0 |
| External binaries | 11 | 3 | 4 | 0 | 0 |
| Python scripts | 7 | 6 | 0 | 0 | 0 |
| **Totals** | **79** | **27** | **26** | **15** | **4** |

### What's Left to Port

**High priority** (functionality gaps):
1. `arecord` replacement — Direct PCM capture ioctls for Wyoming mic input
2. `ifconfig` replacement — Parse procfs for IP detection
3. iptables cross-compile — Or use Rust netfilter crate for firewall

**Medium priority** (improvements):
5. DSP mic mute command — Add `[0x09, 0/1]` for privacy button (protocol decoded)
6. DSP version query — Add `[0x08]` for diagnostics (protocol decoded)
7. DSP event handling — Read GPIO 3 interrupt + SPI responses (bootup, version, errors)

**Low priority** (works as shell):
8. AP mode setup — hostapd/dnsmasq shell scripts work fine
9. WiFi module loading — insmod in shell is fine
10. DNS fix — Shell resolv.conf write is fine

**Resolved** (no longer needed):
- ~~I2C 0x10 / 0x64 initialization~~ — 0x10 is a bitmask, 0x64 is unused
- ~~Full DSP command set~~ — Only 6 commands exist; volume already implemented
- ~~MCU undocumented commands~~ — All commands now documented

**Resolved**:
11. ~~Bluetooth A2DP~~ — Works; was startup ordering bug, fixed by managing full lifecycle in Encore

---

## 14. NAND Partition Map

From vendor source and U-Boot environment:

| Partition | Offset | Size | Filesystem | Mount Point | Content |
|-----------|--------|------|-----------|-------------|---------|
| 0 | 0x000000 | 4 MB | Raw | — | Bootloader (encrypted) |
| 1 | 0x400000 | 256 KB | Raw | — | U-Boot env (ENV/FENV.bin) |
| 2 | — | ~6.6 MB | CPIO | — | Kernel (67E280, encrypted) |
| 3 | — | ~6.6 MB | CPIO | — | Recovery kernel (EBE480) |
| 4 | — | varies | YAFFS2 | `/factory_setting` | Factory data (MAC, certs) |
| 5-10 | — | varies | — | — | Unknown / reserved |
| 11 | — | ~4 MB | YAFFS2 | `/lsync` | Writable config/data |
| 12+ | — | ~78 MB | SquashFS | `/` (rootfs) | Stock rootfs (121E680) |

---

---

## 15. Audio-UI IPC Protocol (WAMP RPC) — Ghidra Reference

The stock `audio-ui` binary uses WAMP (WebSocket Application Messaging Protocol) RPC
for all inter-process communication. Encore replaces this entirely with its subsystem
architecture and WebSocket-based dashboard. Documented here for completeness.

### Volume Control Endpoints
| WAMP URI | Direction | Purpose |
|----------|-----------|---------|
| `com.harman.volumeGet` | RPC call | Read current volume (0-100) |
| `com.harman.volumeSet` | RPC call | Set volume (0-100) |
| `com.harman.volumeAdjust` | RPC call | Adjust volume by delta |
| `com.harman.volumeChanged` | Publish | Volume changed notification |
| `com.harman.volume.setDuck` | RPC call | Set ducking state |
| `com.harman.musicMuteSet` | RPC call | Set music mute on/off |
| `com.harman.musicMuteToggle` | RPC call | Toggle music mute |
| `com.harman.musicMuteChanged` | Publish | Mute state changed notification |

### Playback Control Endpoints
| WAMP URI | Direction | Purpose |
|----------|-----------|---------|
| `com.harman.music.stop` | RPC call | Stop playback |
| `com.harman.music.pause` | RPC call | Pause playback |
| `com.harman.music.resume` | RPC call | Resume playback |
| `com.harman.music.stateChanged` | Publish | Music state change event |
| `alertPlay` | RPC call | Play alert/notification sound |
| `alertCancel` | RPC call | Cancel current alert |

### Volume Control Architecture (Stock)
```
User volume (0-100) → ALSA mixer → snd_mixer_selem_set_playback_volume_all()
                    ↘ 4 parallel soft-volume tracks:
                      ├── Volume (user-controlled)
                      ├── Limit (system cap)
                      ├── Mute (0 or 100)
                      └── Duck (0-100, env configurable)
                    → Final = min(volume, limit, mute, duck)
                    → fade_step() for smooth transitions
```

### Encore Equivalent
Encore's volume path: MCU volume ring event → `AudioCmd::SetVolume` → DAC register write + DSP volume command. Simpler than stock (no ducking/fading), but functional. Ducking could be added later if needed for Wyoming voice overlay.

---

*Last updated: 2026-02-21 (corrected chip IDs, BT stack description, removed Python references)*
*Generated from vendor source cross-reference + Ghidra decompilation against Encore codebase*
