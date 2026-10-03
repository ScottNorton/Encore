# Feature and Hardware Gaps from RE

A fresh reverse-engineering sweep (2026-06-17) over the stock device assets, looking
specifically for capabilities the device physically has, or behaviours the stock
firmware performs, that Encore does not yet use. It extends
[`encore-porting-status.md`](encore-porting-status.md), which is a point-in-time
(Feb 2026) cross-reference. Where that doc is now wrong, this one corrects it.

Assets examined: decompiled stock binaries (`vendor/ghidra_output/{visual-ui,audio-ui,dsp-client,mcu-interface}.c`),
MCU and DSP disassembly, the stock vendor kernel tree (`vendor/kernel/`), the stock
rootfs (`vendor/firmware/squashfs-root/`), and the captured boot logs
(`vendor/stock_device_dump/`). Every finding below was cross-referenced against the
Encore source (`encore/crates/`); the file:line citations are the evidence and the
grep results that confirmed the gap is real.

These are reference findings, not a commitment to build. Effort and confidence are the
author's estimate. Anything touching `/sys` paths should be re-checked on hardware,
since the live device may run the 6.1 kexec kernel rather than the stock 3.8.13 one.

---

## Priority summary

| # | Gap | Type | Effort | Confidence |
|---|-----|------|--------|-----------|
| 1 | Dashboard temperature reads a sysfs path that does not exist on this kernel | hardware-compat bug | trivial | high |
| 2 | No overheat-protection reboot (stock reboots at 90/95 C) | safety feature | trivial-moderate | high |
| 3 | Mic-mute has no LED indicator | feature | trivial | high |
| 4 | Voice/HA error blanks the LED ring instead of showing an error animation | feature | trivial | high |
| 5 | `tts_duck_percent` is a dead dashboard knob (never applied) | feature | moderate | high |
| 6 | DSP event-drain loop is dead code; unmute is not gated on DSP_BOOTUP | feature | moderate | high |
| 7 | Network lifecycle (wifi-setup / AP) has no LED feedback | feature | moderate | high |
| 8 | ~~Bluetooth AVRCP not advertised~~ — implemented since (see Implementation status) | feature | moderate | high |
| 9 | No per-source mixer gain (no ducking, no per-stream volume) | feature | significant | high |
| 10 | No UI earcons / prompt sounds for hardware events | feature | moderate | medium |
| 11 | No factory-reset capability | feature | moderate | high |
| 12 | No alarm/timer alert state (animation + any-button-cancel) | feature | significant | high |
| 13 | MCU [BT+MIC] combo gesture is undecoded and dropped | dev feature | trivial | high |
| 14 | Apple WAC iOS WiFi setup + MFi coprocessor unused | hardware-compat (license-gated) | infeasible | high |

---

## Implementation status (2026-06-17)

Implemented on this branch. All changes compile clean for armv7 (verified with
`cargo zigbuild`); they have NOT been device-tested yet, so anything touching `/sys`
paths or hardware timing should be confirmed on the speaker.

- **1** Temperature read now uses the hwmon `tsen_temp` path with a `thermal_zone0`
  fallback (`web/api.rs`), so the dashboard gauge works on the stock kernel.
- **2** A thermal-protection task mutes the amp on sustained >= 95 C and unmutes on
  cooldown, held by a throttle flag the audio loop re-asserts each tick
  (`main.rs`, `audio/subsystem.rs`). Deliberately gentler than stock's reboot, so a
  bad sensor read cannot cause a boot loop.
- **3** Mic-mute holds `L_301_d_micoff` on the ring, cleared on unmute
  (`audio/subsystem.rs`, covers both the button and the dashboard toggle).
- **4** Voice/HA error plays `L_108_c_error` instead of blanking the ring
  (`wyoming/mod.rs`).
- **5 / 9** Per-slot Q16 mixer gain with a click-free ramp; `tts_duck_percent` now
  actually ducks music under the voice/TTS source (`audio/mixer.rs`,
  `audio/subsystem.rs`). Unity gain stays exact, so non-ducked audio is unchanged.
- **6** DSP event draining is wired but opt-in behind `ENCORE_DSP_EVENTS=1`
  (`audio/subsystem.rs`): enabling it exports the DSP flow-control GPIOs and changes
  the SPI handshake, which needs on-device validation. Bootup-gated unmute is NOT
  changed (the working unconditional unmute is left intact).
- **7** Entering AP/setup mode lights `L_302_d_wifisetup` (`network/mod.rs`), and the
  animation is cleared once provisioning connects. The connected / no-internet
  animations are still a small follow-up.
- **11** Factory reset is exposed as `POST /api/factory-reset` (`web/api.rs`) and as a
  confirm-guarded button in the Settings > System > Reboot panel: it forgets WiFi and
  resets Encore settings to defaults, then reboots. This is a settings reset, not the
  stock `/data` wipe, and is deliberately not wired to the physical reset button.
- **13** The `[BT+MIC]` long-press combo (key `0x0A`) now decodes to
  `McuEvent::BtMicCombo` and is logged (`mcu/mod.rs`, `led.rs`).
- **8** AVRCP Target (and Controller) is fully implemented and device-verified, built in
  a later pass than this sweep: SDP now advertises both A2DP Sink and AVRCP Target
  (0x110C/0x110E) records (`encore_common::sdp`), and the AVCTP/AV-C codec
  (`encore_common::avrcp`, socket layer in `bluetooth/avrcp.rs`) answers the Unit/Subunit
  handshake, absolute-volume set/notify (both directions), `GetElementAttributes`
  (title/artist/album), `GetPlayStatus`/position notifications, and passthrough
  play/pause/next/prev. The dashboard shows a Now Playing card with transport buttons and
  a progress bar on both the Stage and Devices pages.

Not implemented here, with reason:

- **10 earcons** — needs a small WAV player feeding a dedicated mixer slot, and it is
  also a product call (the device may intentionally favour silent, LED-only feedback).
  Pairs with the ducking work above once that call is made.
- **12 alarm/timer** — there is no trigger source: no Home Assistant or voice alarm
  event reaches Encore today, so the animation + any-button-cancel state would be dead
  code until one does.
- **14 Apple WAC** — requires an Apple MFi license and per-device certificates, so a
  clean-room reimplementation cannot be distributed. Documentation note only.

---

## Thermal: the SoC temperature sensor (findings 1 and 2)

This was surfaced independently by every kernel/hardware/config explorer, so it is the
best-corroborated result of the sweep.

**The sensor.** The Berlin2CDP SoC has an on-die thermal ADC (TSEN). It is enabled in
every board defconfig (`CONFIG_SENSORS_TSEN_ADC33=y`, `CONFIG_HWMON=y`) and declared in
the device tree as `tsen@F7FCD000`, compatible `mrvl,berlin2cdp-tsen-adc33`
(`vendor/kernel/arch/arm/boot/dts/berlin2cdp-a0.dtsi:521`), with its power GPIO on the
product board at `berlin2cdp-a0-acast.dts:53`. The driver
`vendor/kernel/drivers/hwmon/tsen-adc33.c` registers a **hwmon** device only
(`hwmon_device_register`, line 295) and exposes `tsen_temp` / `tsen_temp_raw`. It never
calls `thermal_zone_device_register`. Critically, `# CONFIG_THERMAL is not set` in all
three berlin2cdp amp defconfigs, so `/sys/class/thermal/` does not exist on this kernel
at all. The temperature is reported in **whole degrees Celsius**, not millidegrees
(`tsen_temp_show`, line 184).

**Finding 1: the dashboard temperature is permanently blank (hardware-compat bug).**
`read_temperature_mc()` in [`web/api.rs:331`](../encore/crates/encore-firmware/src/web/api.rs)
reads `/sys/class/thermal/thermal_zone0/temp` and treats the value as millidegrees. On
this kernel that path never exists, so the call always returns `None` and the dashboard
"Temperature" field is permanently N/A. The real sensor is
`/sys/class/hwmon/hwmon0/device/tsen_temp`. Fix is a one-liner: read the hwmon path and
multiply by 1000 to keep the existing millidegree contract that `protocol.rs` and the
WASM dashboard already assume. Keep `thermal_zone0` as a fallback for a future 6.1
kernel that may enable `CONFIG_THERMAL`.

**Finding 2: no overheat protection (missing safety feature).** Stock runs
`device_auto_recovery.sh` as a managed Podium process. It polls the hwmon sensor and
force-reboots the speaker after writing a crash report: immediately at >= 95 C, or after
~5 minutes sustained >= 90 C (`THRESHOLD_TEMP=90`, `MAX_THRESHOLD_TEMP=95`,
`vendor/firmware/squashfs-root/usr/bin/device_auto_recovery.sh:8,54,58,87`). Encore has
no equivalent. It reads temperature only for the dashboard gauge and never compares it
to a threshold; `watchdog.rs` is a pure `/dev/watchdog` liveness pet with zero thermal
logic. Given a Class-D amp and a SHARC DSP in a sealed enclosure, a small async task that
mutes the amp and/or reboots at the stock thresholds is a genuine protection gap. Depends
on finding 1 (read the right sensor first).

This corrects two stale doc lines, fixed in the same commit:
[`encore-porting-status.md:81`](encore-porting-status.md) claimed `watchdog.rs handles
recovery` (it does not handle thermal), and [`hardware.md:177`](hardware.md) mislabelled
the sensor as the unpopulated I2C 0x48 footprint (it is the on-die TSEN ADC, already
driver-backed).

---

## LED ring: visual states (findings 3, 4, 7, 12)

The playback engine is correct: `AnimationCache` (`led.rs:59`) loads every `.bin` file
from `/usr/share/lights` at startup, so all ~25 stock animations are resident. But only
**three** are ever triggered: `L_101_c_listening`, `L_104_c_thinking`,
`L_105_c_cortanaspeaking` (from `main.rs:650`, `wakeword/mod.rs:106`,
`wyoming/mod.rs:430/442/483`). The rest are cached and never requested. The stock
`audio-ui` drives a single state-to-animation map of ~20 UI states
(`audio-ui.c:2018-2325`), each of which also rebinds the physical buttons. Encore has no
equivalent state machine and routes each button to one fixed action regardless of
context. The individual gaps below are instances of that umbrella gap; the call-related
animations (`L_201`..`L_208`) need telephony Encore does not have and are not actionable.

**Finding 3: mic-mute has no LED indicator.** Stock maps `microphone:mute` to
`L_301_d_micoff` (`audio-ui.c:2199`). Encore's mic-toggle path (`main.rs` MicToggle and
the dashboard `SetMicMute`) routes only to `AudioCmd::SetMicMute`, and the audio handler
(`audio/subsystem.rs:758`) emits no `LedCmd`. The asset is cached but never played. The
`PlayBin` mechanism already exists, so this is a one-line `PlayBin{"L_301_d_micoff",
repeat:true}` on mute and a restore on unmute, best placed in the `SetMicMute` handler so
it covers both the button and the dashboard. Trivial.

**Finding 4: voice/HA error blanks the ring.** Stock maps `voice:error` and
`agent:error` to `L_108_c_error` (`audio-ui.c:2067,2229`). Encore's only equivalent
trigger (the Wyoming `error` arm, `wyoming/mod.rs:539`) sends
`LedCmd::Animate(Off)`, blanking the ring instead of showing failure feedback. Swap the
`Off` for `PlayBin{"L_108_c_error", repeat:false}`. Trivial. (`L_106_c_success` and
`L_113_c_unabletoreachinternet` are likewise cached-but-unused, but no Encore state maps
to them yet.)

**Finding 7: network lifecycle has no LED feedback.** Stock animates `L_302_d_wifisetup`
/ `L_402a_o_apconnect` / `L_402b_o_apconnected` across the wifi-setup and AP-connect
transitions (`audio-ui.c:2187,2219,2253`). Encore's `NetworkSubsystem`
(`network/mod.rs:87`) has no `led_tx` channel at all; `EnterApMode` and the WifiSetup
button emit no LED, and the wifi-connected path only logs. Adding an `led_tx` to the
network subsystem and emitting `PlayBin` on the AP/connect transitions would light up
four or five already-present animations. Moderate. (The no-internet animation `L_113` is
the weakest leg: it would also need a connectivity check Encore does not do.)

**Finding 12: no alarm/timer alert state.** Stock has an `alert:playing` state that loops
`L_111_c_alarm` or `L_112_c_timer` with a sound and rebinds **every** button to
`alert-cancel` (`audio-ui.c:2153,103413`). Encore has no alarm/timer/alert concept and no
per-state button override (its buttons do the same thing in every context). Significant,
and partly a Home Assistant integration concern since the alert trigger is realistically
assistant-side; the two assets are ready once a trigger exists.

---

## Audio (findings 5, 9, 10)

EQ, tone, bass/treble were investigated and are **not** gaps: Encore does EQ/tone in
software. A parametric EQ engine (`encore-common::dsp` `StereoEq`, RBJ biquad cascade with
auto anti-clip headroom) runs in the mixer thread (`audio/subsystem.rs`), plus a software
DRC (`DrcProcessor`). The DAC's own Program-5 EQ is unusable (it needs the full PurePath
flow image we don't have), so all tone shaping is software. The real gaps are about
per-source mixing and audible feedback.

**Finding 9: no per-source mixer gain (the backbone).** Stock's `VolumeManager` is a
per-named-source layered softvol: each source (music, voice/TTS, alert, BT) has four
independently-faded layers (volume, limit, mute, duck) and the effective gain is their
minimum, smoothed per tick (`audio-ui.c:112769-112856`); two duck depths let a call duck
music harder than a notification. Encore's mixer sums every active slot flat:
`read_add` (`audio/mixer.rs:85`) does `saturating_add` with no gain factor, `MixerSlot`
carries no volume/duck/mute fields, and the summing loop applies no attenuation. Encore
has only a single global volume plus global EQ/DRC. Significant. A minimal version adds an
atomic Q16 gain per `MixerSlot` and multiplies it in `read_add`; that alone unlocks
ducking. Full parity (four layers, fades, dual duck, groups) is larger.

**Finding 5: `tts_duck_percent` is a dead knob.** The setting exists and round-trips
through config (`encore-common/config.rs:64`, default 80), the protocol
(`protocol.rs:636`), and the dashboard (`pages/audio.rs:195`), and the user can change
it, but **no code in `encore-firmware/src` ever reads it** (grep returns zero firmware
hits). Stock implements exactly this: it ducks music while voice/notifications play and
restores on `EvNotificationDone` (`audio-ui.c:90864`). Today setting the value does
nothing. Moderate, and it depends on finding 9: once a slot gain exists, drop the music
slots to `(100 - tts_duck_percent)%` while the Wyoming/voice slot is active.

**Finding 10: no UI earcons.** Stock plays short confirmation sounds on hardware events
through a dedicated `system` ALSA mixer pin: BT pairing/connected, mic on/off, wifi
setup, volume up/down, plugged-in (`audio-ui.c:99989,100111`; assets in
`/usr/share/sounds/cortana/S_30*_d_*.wav`, also played via `aplay -D system` at
`mcu-interface.c:83137`). Encore gives only LED feedback for these events and never plays
the shipped WAVs. It already has a software mixer with addable sources, so a small WAV
decoder feeding a dedicated system slot would do it without GStreamer; it pairs naturally
with finding 9 so prompts duck music. Confidence is medium because the device may
deliberately favour silent, LED-only feedback. The voice/telephony prompts and the spoken
bug-report-ID feature are out of scope (Cortana/cloud).

---

## DSP events (finding 6)

No direction-of-arrival, beam index, or VAD payload exists anywhere in the DSP responses;
this confirms the prior finding that all such processing is baked into `dsp-img.ldr`.

**Finding 6: the event-drain loop is dead code.** Stock runs a 200 ms loop
(`dsp-client.c:12494`) that drains all DSP-initiated events through `Dsp_msg_handle`.
Encore mirrors this with `Dsp::poll_event()` (`mcu/dsp.rs:442`) and `init_gpio()`
(`:424`), but both have **zero callers**: `init_gpio` is never called from the audio
subsystem, so `self.gpio` is always `None` and `poll_event` returns `Ok(None)`
unconditionally. Every event parse arm (`Bootup`, `TriggerFound`, `ExpectSpeech`,
`CancelTrigger`, `DacGain`, `MicMute`) is inert. The only event consumption is the manual
dashboard `DspPollEvents` debug command, which is a synchronous version query, not an
async drain. Most events are diagnostic, but the load-bearing one is `DSP_BOOTUP`: stock
gates the DAC/amp unmute on it (`call_mcu_unmute` at `dsp-client.c:12341`), whereas Encore
unmutes unconditionally after firmware upload with no bootup wait. Wiring `init_gpio()`
plus a ~200 ms poll task would let Encore properly gate unmute on bootup and surface
DAC-gain and error events. Moderate, low risk (GPIO 12 is already configured as input
during upload). Note: this contradicts the memory note that DSP_BOOTUP already gates
unmute; on the current source it does not.

---

## MCU (finding 13)

**Finding 13: the [BT+MIC] combo gesture is dropped.** The MCU emits one multi-button
gesture: holding BT + MIC together long-press is key event category `0x04`, key code
`0x0A` (`mcu-interface.c:53951`). Stock handles it as a special case, running
`system("send-bugreport DSP_MEM_DUMP")` to capture a developer diagnostic
(`mcu-interface.c:83107-83123`), gated by a per-speaker enable flag. Encore's decoder
(`mcu/mod.rs:149-161`) has no `(0x04, 0x0A)` arm, so the combo decodes to
`McuEvent::Unknown` and is dropped by the catch-all at `led.rs:255`. Encore cannot even
observe it today. Low user value (a developer feature, and the stock `send-bugreport`
binary does not exist in Encore), but it is the only combo gesture the MCU produces, and
Encore already has the DSP mem-dump command (`0x0c`) to wire it to. Trivial.

---

## Bluetooth (finding 8) — implemented

**Finding 8: AVRCP target not advertised.** *(Original finding, since resolved — kept for
context.)* At sweep time Encore explicitly skipped AVRCP: SDP advertised only A2DP sink
(0x110B), and without a target the phone's lock-screen play/pause/next controls did
nothing, no track title or artist showed, and BT volume was decoupled from the phone.

**Current state:** AVRCP Target *and* Controller are both implemented and were verified
live on real hardware. SDP now carries two records — A2DP Sink (0x110B) and AVRCP Target
(0x110C/0x110E), `encore_common::sdp`. The AVCTP/AV-C wire codec
(`encore_common::avrcp`, socket/session layer in `encore/crates/encore-firmware/src/bluetooth/avrcp.rs`)
answers the Unit/Subunit Info handshake, `GetCapabilities`, absolute-volume
`SetAbsoluteVolume`/`RegisterNotification` in both directions (phone slider moves the
speaker, and the speaker's own volume changes notify a registered phone), and — as
Controller — queries `GetElementAttributes` (title/artist/album), `GetPlayStatus` and
live position notifications, and sends passthrough play/pause/next/prev. The dashboard
renders a Now Playing card (title/artist, transport buttons, progress bar) on both the
Stage and Devices pages. All the pure wire-codec logic is host-tested (25 unit tests:
17 in `encore_common::avrcp`, 8 in `encore_common::sdp`).

---

## Boot and recovery (finding 11)

**Finding 11: no factory-reset.** Stock `usr/sbin/factory-reset` shows the MCU
upgrade-mode LED (`i2ccmd_io w 0x36 ... 0x05 0x22`, which is MCU command `0x05` sub `0x22`,
per [`mcu-reference.md:217`](mcu-reference.md)), sets a fast-blink white front LED, stops
Podium, `rm -rf /data/*`, and runs `SetFacDefault`. It is wired into `init.rc` as the
`factory_reset` service. Encore already **decodes** the reset button
(`McuEvent::ResetShort/ResetLong`, `mcu/mod.rs:48`) but deliberately no-ops it
(`led.rs:253`, comment "hardware handles this"). It has no `/data` wipe, no defaults
restore, and never uses the MCU upgrade-LED mode. The button hook and the MCU LED
sub-commands (`0x05/0x20`..`0x22`) are already decoded, so wiring a reset action (clear
`/data/wifi` and config, restart) plus the upgrade-LED animation is the remaining work.
Moderate, app-level.

---

## Apple WAC (finding 14, documentation only)

**Finding 14: iOS WiFi setup and the MFi coprocessor are unused.** Stock ships a complete
Apple Wireless Accessory Configuration stack: `libwac_ark.so` (the WAC/MFiSAP server
advertising `_mfi-config._tcp` over Bonjour), `libacpsharing.so` (an MFi auth-coprocessor
I2C driver, strings reference `/dev/i2c-0`, `ACPMFiPlatform_*`, "MFi auth read register"),
and `libreconnect.so` (the glue: "AnpWac connect wac", "SetAirplayPassword"). Together
they let an iPhone provision the speaker's WiFi and AirPlay password via "Set up new
accessory". The backing hardware is an Apple MFi auth coprocessor on `/dev/i2c-0`, the
same bus Encore already drives for the MCU/DAC/IO-expander, at an untouched slave address.
Encore has zero references to any of it.

This is reported for completeness as a physically-present-but-unused peripheral, not as an
actionable feature: MFiSAP and the per-device certificates require an Apple MFi license, so
a clean-room reimplementation cannot be legally distributed.

---

## Investigated and confirmed not a gap

To save the next investigator the trip, these were checked and ruled out:

- **EQ / tone / bass / treble**: already implemented (software biquad EQ + single-band DRC).
- **DSP EQ / beamforming / AEC userspace control**: does not exist; all baked into
  `dsp-img.ldr`. Confirms the existing porting-status conclusion.
- **7-mic array / DoA**: fully internal to the SHARC. The SoC physically receives only a
  2-channel processed stream, which Encore already captures. No untapped mic channels and
  no direction-of-arrival are exposed over SPI or I2S. (One soft note: a near-field "call"
  channel is captured but unused.)
- **WM8904 codec, gpio-keys keyboard, RTC, leds-class, IIO** in the DT/defconfig: either
  Marvell reference-board leftovers with zero references in the stock rootfs/dmesg, or
  explicitly disabled in every berlin2cdp defconfig. Device controls go through the MCU,
  which is already handled.
- **Google Cast / DLNA / UPnP / A2DP source / Spotify Connect internals**: out of scope or
  proprietary/absent.
- **Bluetooth character-device mode** (`/dev/mbtchr`): an alternate driver build mode, not
  needed; the HCI socket path works.

---

*Sweep date: 2026-06-17. Method: parallel RE of the assets listed at the top, each
finding adversarially re-verified against the Encore source tree before inclusion. When a
finding disagrees with older docs or memory, trust the source tree.*
