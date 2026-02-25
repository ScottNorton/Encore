# Contributing

Thanks for your interest in this project. The Harman Kardon Invoke community firmware gives
discontinued speakers a second life, and contributions of all kinds are welcome — code,
documentation, hardware research, testing, bug reports.

## Getting Started

Before diving in, read these:

- [Architecture](docs/architecture.md) — boot sequence, subsystem lifecycle, hardware overview
- [Build Guide](docs/build-guide.md) — full setup instructions for building Encore and firmware images
- [Hardware Reference](docs/hardware.md) — SoC peripherals, I2C bus, audio path
- [Flashing Guide](docs/flashing.md) — how to get firmware onto the device

Most contributors only need Rust — `make encore` builds from any OS without WSL.
See the [Build Guide](docs/build-guide.md) for prerequisites and instructions.

## How to Contribute

1. Fork the repository
2. Create a feature branch from `main`
3. Make your changes
4. Run tests: `cd encore && cargo test --all`
5. Run verification: `make verify`
6. Submit a pull request with a clear description of the change

### Commit Messages

Plain English, imperative mood:

```
Add AirPlay audio source
Prevent DAC pop on boot when volume > 80%
Add teardown photos for hardware revision 2
```

### Code Style

- **Rust**: `rustfmt` defaults, `clippy` clean
- **Shell scripts**: POSIX-compatible (device runs BusyBox ash), **LF line
  endings only** — Windows CRLF will break scripts on device
- **C**: minimal use (`i2c_mute` boot helper), cross-compiled with `zig cc`

## Things You Should Never Do

These rules exist because they've each bricked a device at least once:

- **Never use `mksquashfs -all-root`** — destroys file ownership, breaks boot
- **Never update MCU firmware unless you fully understand the protocol** — a
  bad I2C flash permanently bricks the touch ring, LED ring, and button
  subsystem. See [MCU Reference](docs/mcu-reference.md) for the full protocol

## Areas Where Help Is Valuable

- **Audio sources**: AirPlay, DLNA, Chromecast Audio protocols
- **DSP audio processing**: the ADSP-21489 SHARC has significant untapped
  capability for EQ, room correction, and custom audio effects
- **Multi-room audio**: synchronized playback across multiple Invoke speakers
- **Voice**: wake word engines, alternative STT/TTS integrations
- **Hardware research**: undocumented peripherals, UART interfaces, DSP internals
- **Documentation**: setup guides, teardown walkthroughs, translations
- **Testing**: different hardware revisions, network configurations, edge cases

## Bug Reports

When filing a bug report, include:

- What you were doing when the issue occurred
- Device log output (`/lsync/encore/encore.log` or the web dashboard Logs tab)
- Your network setup (WiFi, AP mode, Ethernet-over-USB)
- Build information (commit hash, `make encore` output)

## Questions?

Open an issue on the repository.
