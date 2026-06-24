# Configuration Reference

Encore stores its configuration at `/lsync/encore/config.toml` on the device. This
file persists across reboots and firmware updates.

You can edit the config in three ways:

1. **Web dashboard** -- most settings are editable from the Settings panel
2. **SSH** -- `vi /lsync/encore/config.toml` (password: `ridiculous`)
3. **OTA** -- some settings can be changed via the REST API

The file uses [TOML](https://toml.io/) syntax. Every field has a built-in default, so
you only need to include the settings you want to change. An empty file (or no file at
all) is valid and will use all defaults.

> All subsystem defaults are consistent whether the config section is present or absent.
> Spotify is enabled by default. To disable it, set `enabled = false` explicitly.

---

## Complete Example

This file shows every section with its default values:

```toml
[device]
name = "Encore"

[audio]
master_volume = 70
spotify_volume = 70
bluetooth_volume = 70
tts_duck_percent = 80
volume_ring_step = 2
idle_timeout_secs = 5
standby_timeout_secs = 60
dsp_power_gate = false

[eq]
enabled = true
bands = []

[drc]
enabled = false
low_mid_hz = 200
mid_high_hz = 2000
bands = []

[spotify]
enabled = true
bitrate = "320"
gapless = true
normalisation = false
normalisation_type = "auto"
normalisation_pregain_db = 0.0
# cache_path is unset by default

[bluetooth]
enabled = true
discoverable = true

[homeassistant]
enabled = false
# mqtt_host = "192.168.1.100"
# mqtt_port = 1883
# mqtt_user = "user"
# mqtt_password = "password"

[wyoming]
enabled = false
# server_host = "ha.local"
# server_port = 10300

[network]
# wifi_ssid = "MyNetwork"
# wifi_password = "secret"
ap_keep_alive = true
# ap_ssid = "Invoke-XXXX"     # override auto-generated AP SSID
# ap_password = "ridiculous"   # override default AP password

[vpn]
enabled = false
persistent_keepalive = 25
# private_key = "base64..."
# address = "10.0.0.2/24"
# peer_public_key = "base64..."
# peer_preshared_key = "base64..."
# peer_endpoint = "vpn.example.com:51820"
# peer_allowed_ips = "0.0.0.0/0"

[group]
enabled = false
group_name = "Home"
channel = "stereo"
# peer_id is auto-generated on first boot

[debug]
default_mode = "production"
# overrides = { audio = "hold", network = "trace" }
```

---

## Sections

### `[device]`

General device identity.

| Field  | Type   | Default    | Description |
|--------|--------|------------|-------------|
| `name` | string | `"Encore"` | Display name shown in Spotify Connect, Bluetooth pairing, mDNS, and the web dashboard. |

Changes take effect on reboot (mDNS and Spotify Connect names are set at startup).

---

### `[audio]`

Volume levels and mixing behavior. All volumes are 0-100 (percent).

| Field              | Type | Default | Description |
|--------------------|------|---------|-------------|
| `master_volume`    | u8   | `70`    | Master output volume applied after all source mixing. |
| `spotify_volume`   | u8   | `70`    | Spotify Connect source volume. |
| `bluetooth_volume` | u8   | `70`    | Bluetooth A2DP source volume. |
| `tts_duck_percent` | u8   | `80`    | How much to duck music volume during TTS playback (percent of current volume). |
| `volume_ring_step` | u8   | `2`     | Volume change per MCU volume ring tick. |
| `idle_timeout_secs` | u32 | `5`     | Seconds of silence before muting the amp (Active to Idle). |
| `standby_timeout_secs` | u32 | `60` | Seconds in Idle before DAC standby and mixer park (Idle to Standby). |
| `dsp_power_gate` | bool | `false` | Power-gate the DSP in Standby. Saves ~500 mW-1W but adds ~3.5s to resume. |

Volume changes take effect immediately (the mixer reads these at runtime). Values are
persisted to config on change so they survive reboot.

---

### `[eq]`

Software parametric equalizer.

| Field     | Type            | Default | Description |
|-----------|-----------------|---------|-------------|
| `enabled` | bool            | `true`  | Enable/disable the EQ processing chain. |
| `bands`   | array of tables | `[]`    | EQ band definitions (see below). Empty = flat response. |

Each entry in `[[eq.bands]]`:

| Field         | Type   | Default   | Description |
|---------------|--------|-----------|-------------|
| `freq_hz`     | u16    | `1000`    | Center frequency in Hz. |
| `gain_cb`     | i16    | `0`       | Gain in centibels (1/10 dB). +30 = +3.0 dB, -50 = -5.0 dB. |
| `q_x10`       | u16    | `10`      | Q factor times 10. `10` = Q of 1.0, `14` = Q of 1.4. |
| `filter_type` | string | `"peak"` | Filter type: `"peak"`, `"lowshelf"`, `"highshelf"`, `"notch"`. |

Example with three bands:

```toml
[eq]
enabled = true

[[eq.bands]]
freq_hz = 80
gain_cb = 30
q_x10 = 7
filter_type = "lowshelf"

[[eq.bands]]
freq_hz = 2500
gain_cb = -20
q_x10 = 14
filter_type = "peak"

[[eq.bands]]
freq_hz = 10000
gain_cb = 15
q_x10 = 10
filter_type = "highshelf"
```

The `[eq]` block is applied at boot. If you set `[[eq.bands]]` here (or set
`enabled = false`), they take precedence over the `[audio]` `eq_boot_preset`
convenience. Dashboard EQ changes apply immediately but are runtime-only: they
are not written back to this file, so edit `[eq]` here to make a custom EQ
persist across reboots.

---

### `[drc]`

Single-band dynamic range compressor. The dashboard exposes one set of
threshold/ratio/attack/release controls.

| Field        | Type            | Default | Description |
|--------------|-----------------|---------|-------------|
| `enabled`    | bool            | `false` | Enable/disable DRC processing. |
| `bands`      | array of tables | `[]`    | Compressor settings. Provide one entry; it is applied as the single-band compressor (see below). |
| `low_mid_hz` | u16             | `200`   | Reserved. Persisted but not used by the single-band engine. |
| `mid_high_hz`| u16             | `2000`  | Reserved. Persisted but not used by the single-band engine. |

The compressor is single-band: only the middle band's settings reach the audio
path. The `low_mid_hz`/`mid_high_hz` crossover frequencies and any low/high band
entries are kept for wire and config compatibility and as scaffolding for a
possible future multiband engine, but they have no audible effect today.

Provide one entry in `[[drc.bands]]` (the single-band compressor). If you give
several, the engine applies the second:

| Field          | Type | Default | Description |
|----------------|------|---------|-------------|
| `threshold_db` | i8   | `-20`   | Compression threshold in dB (negative = below 0 dBFS). |
| `ratio_x10`    | u8   | `10`    | Compression ratio times 10. `10` = 1.0:1 (no compression), `40` = 4.0:1. |
| `attack_ms`    | u16  | `10`    | Attack time in milliseconds. |
| `release_ms`   | u16  | `200`   | Release time in milliseconds. |

The `[drc]` block is applied at boot, so edits here take effect on startup.
Dashboard DRC changes apply immediately but are runtime-only: they are not
written back to this file, so set `[drc]` here to make a compressor persist
across reboots.

---

### `[spotify]`

Spotify Connect integration via librespot.

| Field                      | Type           | Default   | Description |
|----------------------------|----------------|-----------|-------------|
| `enabled`                  | bool           | `true`    | Enable Spotify Connect. Device appears in Spotify app when enabled. |
| `cache_path`               | string or null | *unset*   | Directory for cached Spotify audio files. `null`/omitted = no caching. |
| `bitrate`                  | string         | `"320"`   | Audio bitrate: `"96"`, `"160"`, or `"320"` (kbps). |
| `gapless`                  | bool           | `true`    | Enable gapless playback between tracks. |
| `normalisation`            | bool           | `false`   | Enable volume normalization (ReplayGain). |
| `normalisation_type`       | string         | `"auto"`  | Normalization mode: `"auto"`, `"album"`, or `"track"`. |
| `normalisation_pregain_db` | f32            | `0.0`     | Pre-gain offset in dB applied before normalization. |

Some Spotify changes require a reboot. Disabling Spotify removes the Spotify tab from the dashboard.

---

### `[bluetooth]`

Bluetooth A2DP sink (aptX HD, aptX, SBC). Uses raw kernel sockets.

| Field          | Type | Default | Description |
|----------------|------|---------|-------------|
| `enabled`      | bool | `true`  | Enable Bluetooth A2DP sink. |
| `discoverable` | bool | `true`  | Whether the device is visible for Bluetooth pairing. |

Bluetooth changes take effect immediately.

---

### `[homeassistant]`

Home Assistant integration via MQTT. See [home-assistant.md](home-assistant.md) for
setup instructions.

| Field           | Type           | Default  | Description |
|-----------------|----------------|----------|-------------|
| `enabled`       | bool           | `false`  | Enable the MQTT bridge to Home Assistant. |
| `mqtt_host`     | string or null | *unset*  | MQTT broker hostname or IP address. |
| `mqtt_port`     | u16 or null    | *unset*  | MQTT broker port (typically `1883`). |
| `mqtt_user`     | string or null | *unset*  | MQTT username for authentication. |
| `mqtt_password` | string or null | *unset*  | MQTT password for authentication. |

Home Assistant changes currently require a reboot.

---

### `[wyoming]`

Wyoming voice satellite protocol for Home Assistant voice pipelines.

| Field         | Type           | Default | Description |
|---------------|----------------|---------|-------------|
| `enabled`     | bool           | `false` | Enable the Wyoming voice satellite. |
| `server_host` | string or null | *unset* | Wyoming server hostname (your Home Assistant instance). |
| `server_port` | u16 or null    | *unset* | Wyoming server port (typically `10300`). |

Wyoming changes currently require a reboot.

---

### `[network]`

WiFi client and access point configuration.

| Field           | Type           | Default | Description |
|-----------------|----------------|---------|-------------|
| `wifi_ssid`     | string or null | *unset* | WiFi network name to connect to. |
| `wifi_password` | string or null | *unset* | WiFi network password (WPA2). |
| `ap_keep_alive` | bool           | `true`  | Keep the setup AP running even after WiFi connects. Alias: `ap_fallback`. |
| `ap_ssid`       | string or null | *auto*  | Override the AP SSID. Defaults to `Invoke-XXXX` (last 4 hex of WiFi MAC). |
| `ap_password`   | string or null | `"ridiculous"` | Override the AP password. |

When `wifi_ssid` is unset, the device stays in AP mode (SSID `Invoke-XXXX` where XXXX
is derived from the WiFi MAC, password `ridiculous`). When WiFi credentials are set, the device connects to WiFi on boot. If
`ap_keep_alive` is `true`, the AP remains active alongside WiFi for recovery access
(note: 5 GHz WiFi will conflict with the 2.4 GHz AP on the single-radio chip).

Some network changes currently require a reboot.

---

### `[vpn]`

WireGuard VPN via userspace boringtun (TUN interface).

| Field                  | Type           | Default | Description |
|------------------------|----------------|---------|-------------|
| `enabled`              | bool           | `false` | Enable the WireGuard tunnel. |
| `private_key`          | string or null | *unset* | WireGuard private key (base64). Generate with `wg genkey`. |
| `address`              | string or null | *unset* | Tunnel interface address in CIDR notation (e.g. `"10.0.0.2/24"`). |
| `peer_public_key`      | string or null | *unset* | Peer's public key (base64). |
| `peer_preshared_key`   | string or null | *unset* | Optional pre-shared key for additional security (base64). |
| `peer_endpoint`        | string or null | *unset* | Peer endpoint as `"host:port"` (e.g. `"vpn.example.com:51820"`). |
| `peer_allowed_ips`     | string or null | *unset* | Allowed IPs for the tunnel (e.g. `"0.0.0.0/0"` for full tunnel, `"10.0.0.0/24"` for split). |
| `persistent_keepalive` | u16            | `25`    | Keepalive interval in seconds. `0` = disabled. |

VPN changes currently require a reboot.

---

### `[group]`

Multi-speaker synchronization. Groups Encore devices on the local network for
synchronized audio playback.

| Field        | Type           | Default     | Description |
|--------------|----------------|-------------|-------------|
| `enabled`    | bool           | `false`     | Enable multi-speaker group sync. |
| `group_name` | string         | `"Home"`    | Name of the speaker group. All devices with the same group name sync together. |
| `channel`    | string         | `"stereo"` | Channel assignment: `"stereo"`, `"left"`, or `"right"`. |
| `peer_id`    | string or null | *unset*     | Unique peer identifier (UUID v4). Auto-generated on first boot -- do not edit manually. |
| `peers`      | string[]       | `[]`        | Bootstrap peer IPs for cross-subnet discovery (where mDNS doesn't reach). |
| `party_mode` | bool           | `false`     | Accept audio streams from any group name, not just the configured one. |

Most group changes require a reboot. Channel assignment can be changed at runtime from the
dashboard.

---

### `[debug]`

Crash capture and logging behavior.

| Field          | Type                 | Default        | Description |
|----------------|----------------------|----------------|-------------|
| `default_mode` | string               | `"production"` | Default crash capture mode. Values: `"production"` (minimal logging), `"trace"` (verbose), `"hold"` (pause on crash for debugging). |
| `overrides`    | map of string:string | `{}`           | Per-subsystem mode overrides. Keys are subsystem names (e.g. `"audio"`, `"network"`), values are mode strings. |

Example:

```toml
[debug]
default_mode = "production"

[debug.overrides]
audio = "hold"
network = "trace"
```

Debug changes take effect on reboot.

---

## Related Documentation

- [architecture.md](architecture.md) -- subsystem overview, boot process, and system design
- [home-assistant.md](home-assistant.md) -- MQTT broker setup and Home Assistant integration guide
- [build-guide.md](build-guide.md) -- building firmware and deploying to the device
- [troubleshooting.md](troubleshooting.md) -- common issues and recovery procedures
