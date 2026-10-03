# Architecture

## Overview

Flashing a constructed "83 image" via U-boot replaces all vendor services with **Encore** — a single statically-linked Rust binary that manages the device.

Stock vendor components (kernel, bootloader, init, WiFi drivers) are preserved, for now.

## Boot Sequence

```
Power on
  → Bootloader (Marvell, NAND partition 0)
  → Kernel 3.8.13 (encrypted, NAND partition 2)
  → init.rc (Android-style init)
    → servicemanager (binder IPC — wpa_supplicant, adbd, dhcpcd depend on it)
    → startup.sh (stock — launches LibreEnv, LibreManager, yani_service)
    → run-podium.sh (our no-op stub — satisfies init.rc service entry)
    → /etc/init.d/S00audiomute  — mute amplifier via i2c_mute (C binary)
    → /etc/init.d/S01firewall   — iptables rules
    → /etc/init.d/S02bootlog    — persistent boot logging
    → /sbin/mount_partition.sh  — OUR MAIN ENTRY POINT
      → mount /factory_setting and /lsync (persistent storage)
      → wpa_supplicant_setup.sh (stock WiFi init)
      → start_ap.sh &             (backgrounded — always-on AP: Invoke-XXXX)
      → auto_wifi_firewall.sh &   (backgrounded — connect WiFi + activate firewall)
      → usb_gadget.sh &           (backgrounded — USB RNDIS gadget: usb0 at 10.55.55.1)
      → encore_supervisor.sh      (foreground — watchdog + crash monitor + Encore launch)
```

### Why Keep Stock Services?

`servicemanager` is critical — it provides the binder IPC that `wpa_supplicant`, `adbd` (ADB daemon), and `dhcpcd` use. Removing it breaks WiFi and remote access. Other stock services (`LibreEnv`, `LibreManager`, `yani_service`) are harmless behind the firewall and may have undocumented dependencies.

`run-podium.sh` is replaced with a no-op (`exec sleep 999999`) to satisfy init.rc's service entry without actually starting the Harman service orchestrator.

## Encore Subsystems

Encore is a single binary that handles all device functionality. Each subsystem implements an async `Subsystem` trait with independent lifecycle management. Vital subsystems auto-restart on crash.

| Subsystem | Purpose | Vital |
|-----------|---------|-------|
| `mcu/` | I2C hardware: MCU (0x36), IO Expander (0x20), DAC (0x4C), DSP SPI upload | no |
| `audio/` | Direct ALSA PCM (48kHz S32_LE), lock-free mixer, resampler | yes |
| `network/` | wpa_supplicant control, firewall, AP mode, mDNS (`encore.local`), NTP, DNS management, RPS | yes |
| `web/` | Axum HTTPS server, REST API, WebSocket, embedded WASM dashboard | yes |
| `watchdog.rs` | SoC `/dev/watchdog` (10s pet) — MCU has no watchdog | yes |
| `spotify.rs` | librespot-core integration, mDNS discovery | no |
| `bluetooth/` | A2DP sink (aptX HD + aptX + SBC), raw kernel sockets (no BlueZ) | no |
| `wyoming/` | Wyoming voice satellite (TCP port 10700) | no |
| `homeassistant.rs` | MQTT bridge via rumqttc | no |
| `vpn/` | WireGuard via boringtun (userspace TUN) | no |
| `led.rs` | LED animation (13 of 15 LEDs controlled, 30fps), MCU events | no |
| `group/` | Multi-speaker sync: mDNS discovery, leader/follower, clock sync, jitter buffer | no |
| `wakeword/` | Wake word framework (pluggable detector trait, stub currently) | no |

### Hardware Constraints

The Marvell 88W8887 wireless module (inside a Libre Wireless LS9AD combo module) is a **single-radio chip** shared between WiFi station (STA) mode and the access point (AP). This creates a fundamental constraint:

- **Band conflict**: If the WiFi STA connects on 5 GHz, the 2.4 GHz AP dies after approximately 35 seconds. The radio cannot simultaneously operate on two different frequency bands.
- **Auto band matching**: The AP startup script (`start_ap.sh`) detects which band the STA is connected on and starts the AP on the same frequency range. If WiFi is on 5 GHz, the AP also uses 5 GHz (same channel range), avoiding the cross-band conflict.
- **AP-only mode**: When no WiFi STA is connected (first boot or no saved credentials), the AP runs on 2.4 GHz by default for maximum client compatibility.
- **Practical impact**: Clients that only support 2.4 GHz cannot reach the AP while the speaker is connected to a 5 GHz WiFi network. Use a 2.4 GHz WiFi network if you need the AP accessible from all devices.

### USB Network Gadget

Alongside WiFi and the AP, the device brings up a USB RNDIS network gadget at boot. `mount_partition.sh` backgrounds `rootfs/sbin/usb_gadget.sh`, which creates the `usb0` interface at 10.55.55.1 and runs a small DHCP server on that link (pool 10.55.55.10-50). Plugging a USB cable from the speaker's USB Mini-B port into a computer makes the speaker appear as a USB network adapter; on Windows it binds the built-in RNDIS driver automatically. The gadget advertises no gateway and no DNS, so plugging in does not disturb the computer's existing internet connection.

Because it does not depend on the WiFi radio and comes up automatically, the gadget gives WiFi-independent access to SSH (`root@10.55.55.1`) and the dashboard (`http://10.55.55.1/`), and doubles as a recovery channel when WiFi or the AP is unavailable. `usb_gadget.sh` spawns `usb_gadget_monitor.sh`, which restarts the gadget if the link drops.

This relies on patched `g_ether`/RNDIS kernel modules (baked into the rootfs at `/usr/lib/usbgadget/`). The Marvell `mv_udc` controller stalls multi-packet bulk transfers, so the gadget MTU is fixed at 400 bytes to keep every frame in a single USB packet. This caps throughput (measured around 7 MB/s, enough for the dashboard, SSH, and OTA uploads) but keeps the link reliable.

### Crash Recovery

The supervisor (`encore_supervisor.sh`) manages Encore's lifecycle:

1. Opens `/dev/watchdog` immediately (prevents reboot from prior crash)
2. 5-second safety delay (SSH window for emergency disable)
3. Releases watchdog to Encore (magic close)
4. Runs primary binary (staged OTA → dev binary → rootfs binary)
5. If primary crashes fast (<5 min) → quarantine it, try fallback (rootfs binary)
6. If fallback also crashes fast → **safe mode** (supervisor pets watchdog, device stays alive for AP SSH recovery)
7. If Encore runs 5+ minutes → stable run, auto-restart on exit, promote staged OTA binary

Safe mode keeps the device accessible via SSH so you can upload a new binary or investigate logs.
The AP is kept active for remote access.

## Persistent Storage

| Mount Point | Filesystem | Purpose |
|-------------|-----------|---------|
| `/factory_setting` | yaffs2 | Factory calibration data (read-only in practice) |
| `/lsync` | yaffs2 | User data: config, logs, WiFi credentials, Encore binary |
| `/data` | tmpfs | Runtime data: Spotify cache, WiFi state |

Configuration: `/lsync/encore/config.toml` (TOML format, read by Encore at startup).

## Firewall

The firewall (`S01firewall` + Encore's `network/firewall.rs`) implements:
- **Outbound**: ALLOW ALL — needed for Spotify CDN, MQTT broker, NTP
- **Inbound**: DROP by default, ACCEPT from LAN ranges (192.168.x.x, 10.x.x.x, 172.16-31.x.x)
- **DHCP and mDNS**: Always allowed (Spotify Connect discovery)

**Note**: The `iptables` binary is missing from the stock rootfs. Firewall rules in `S01firewall` and Encore's `firewall.rs` silently fail if iptables is not present. The device still benefits from having no listening services on non-LAN ports, but active packet filtering is not enforced without iptables.

## REST API

The web subsystem exposes a REST API on HTTPS port 443. Port 80 serves the same
routes only to peers on the AP subnet (192.168.43.0/24) or the USB RNDIS link
(10.55.55.0/24), where a self-signed cert would strand the browser; every other
client gets `/ca.crt` plus a 308 redirect to HTTPS, so the API never runs in
cleartext on the LAN. The API has no authentication, so browser requests from
origins other than the device itself, the desktop app, or localhost are
rejected, and CORS is limited to that same allowlist.

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/system` | GET | System metrics (uptime, CPU, memory, firmware version) |
| `/api/config` | GET | Read config file (passwords and VPN keys are redacted) |
| `/api/config` | POST | Save config file (absent or blank secrets keep their stored value) |
| `/api/logs` | GET | Log history as JSON array |
| `/api/crashes` | GET | All crash reports |
| `/api/crashes/{subsystem}` | GET | Last crash for a subsystem |
| `/api/wifi/scan` | GET | Trigger WiFi scan |
| `/api/update` | POST | Upload Encore binary (staged to `/lsync/encore/encore_next`) |
| `/api/firmware/flash` | POST | Upload SquashFS rootfs and flash to NAND |
| `/api/reboot` | POST | Sync filesystems and reboot |
| `/api/setup` | GET | Check setup mode status |
| `/api/setup/complete` | POST | Finalize first-boot setup |
| `/ca.crt` | GET | Download TLS CA certificate |
| `/api/ws` | GET | WebSocket upgrade for real-time telemetry |

Debug endpoints are available under `/api/debug/` — see [troubleshooting.md](troubleshooting.md).

## WebSocket Protocol

The WebSocket at `/api/ws` carries bidirectional messages:

- **ServerMsg** (firmware → dashboard): system status, subsystem health, audio levels,
  spectrum/waveform data, track changes, Bluetooth events, LED state, crash reports,
  EQ/DRC state, DSP info, group sync status, boot mode, WiFi connect results,
  time sync status, network info with signal quality
- **ClientMsg** (dashboard → firmware): volume control, Spotify playback, Bluetooth
  management, LED animations, WiFi config, EQ/DRC parameters, DSP register access,
  speaker group settings, group channel assignment, debug mode, subsystem restart

Messages use JSON encoding. See `encore-common/src/protocol.rs` for the complete
type definitions.

## WASM Dashboard

The web dashboard is a single-page WASM application (`encore-wasm` crate) embedded in the Encore binary via `rust-embed`. It provides:

- 8 tab pages: dashboard, assistant, spotify, audio, lights, bluetooth, speakers, network — the spotify, bluetooth, and speakers tabs are hidden when their subsystems are disabled in config. Setup is a separate full-screen mode (hides tab navigation) shown on first boot.
- 7 gear-menu panels: config, health, crashes, logs, update, reboot, about
- Canvas 2D graphics: LED ring visualization, VU meter, EQ curve, sparkline, knob
- Real-time telemetry via WebSocket (`/api/ws`)
- PWA with offline support

## Wake Word Framework

Encore includes a pluggable wake word detection framework in the `wakeword/` subsystem. It runs continuously on microphone input and triggers voice sessions when a wake word is detected.

### WakeWordDetector Trait

The detection engine is abstracted behind a `WakeWordDetector` trait that accepts 16 kHz mono S16 (signed 16-bit) audio samples and returns an optional detection result. Any engine that implements this trait can be plugged into Encore without modifying the subsystem wiring.

The current implementation uses a **stub detector** that always returns `None` -- it never triggers. This allows the framework to compile and run without pulling in a real inference engine.

### Voice Session State Machine

When a wake word is detected, Encore transitions through three states:

```
Idle  -->  Active  -->  Responding  -->  Idle
       detect      STT complete      TTS complete
```

- **Idle**: Microphone audio flows to the wake word detector. LED ring is in its normal state.
- **Active**: Wake word detected. LED ring shows a listening animation. Microphone audio is streamed to the Wyoming voice pipeline (or other STT backend) for speech-to-text processing.
- **Responding**: STT result received and sent to the assistant. TTS audio plays through the speaker. LED ring shows a speaking animation. Once playback completes, the session returns to Idle.

### Implementing a Custom Engine

To add a real wake word engine:

1. Implement the `WakeWordDetector` trait in a new module under `wakeword/`.
2. The `detect` method receives a slice of `i16` samples (16 kHz, mono) and returns `Option<Detection>` with the wake word label and confidence score.
3. Register the new detector in the subsystem initialization (replace the stub).
4. The engine should be lightweight enough to run continuously on a single Cortex-A7 core alongside all other subsystems.

### Potential Engines

- **openWakeWord (ONNX)**: Pre-trained ONNX models for common wake words. Could be integrated via the `ort` (ONNX Runtime) crate. Models are small (~2 MB) and run efficiently on ARM.
- **Porcupine**: Picovoice's wake word engine has ARM support but requires a license key.
- **Custom trained**: Record samples on the Invoke's own microphone array and train a small model for best accuracy with the specific hardware acoustics.

## Multi-Speaker Group Sync

The `group/` subsystem enables synchronized audio playback across multiple Encore speakers on the same network. Speakers discover each other via mDNS, elect a leader, and stream audio from leader to followers with clock-synchronized playback.

### Discovery

Speakers advertise and browse for `_encore-group._tcp.local.` mDNS services. TXT records carry the peer ID, group name, and channel assignment. The mDNS responder parses response packets for PTR, SRV, TXT, and A/AAAA records, with DNS name compression (0xC0 pointer) support. Active PTR queries are sent every 30 seconds. Speakers filter out their own advertisements.

For cross-subnet discovery (where mDNS doesn't reach), static bootstrap peers can be listed in `config.toml` under `[group] peers = ["ip:port", ...]`. These are reconnected automatically using a gossip protocol that propagates peer lists.

### Leader Election

When audio starts playing on a speaker, it triggers a leader election. The election uses RTT-based scoring — the speaker with the lowest network latency to its peers wins. Uptime and stability bonuses prevent unnecessary failover. Elections have a cooldown period to avoid rapid re-elections. Only Spotify and Bluetooth audio sources trigger elections; Wyoming TTS playback does not.

### Audio Streaming

The leader captures audio from the mixer and sends it as `AudioChunk` packets (480 samples per chunk, 48kHz stereo S32_LE) to all followers via TCP port 48200. Followers use a jitter buffer with channel routing (stereo, left-only, or right-only). On packet loss the follower fills the gap with exact-length silence so the beat never slips, and chunks are de-duplicated and stale ones rejected by sequence number. There is no audio time-stretching. Clocks are kept in sync with a min-RTT plus skew estimator (offset and RTT smoothed across samples), so playback timing tracks the leader without resampling. A relay tree is present in the wire protocol but inactive in v1: the leader does not currently compute or send relay assignments.

### Limitations

- **No WiFi Direct / P2P**: The Marvell 88W8887 is a single-radio chip. WiFi Direct requires simultaneous STA + P2P group owner operation on the same radio, which this hardware does not support. All group communication uses the existing WiFi network infrastructure.
- **Same-subnet only for auto-discovery**: mDNS is link-local. Cross-subnet groups require manual bootstrap peer configuration.
- **Single-radio band constraint**: If speakers are on different WiFi bands (2.4 GHz vs 5 GHz), they cannot discover each other via mDNS. All grouped speakers should be on the same WiFi network and band.

See [config-reference.md](config-reference.md) for the full configuration schema.
