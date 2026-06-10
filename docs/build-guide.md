# Build Guide

End-to-end instructions for building custom Harman Kardon Invoke firmware from source.

## Prerequisites

- **Linux** (native Ubuntu 22.04+, or WSL2 on Windows)
- If using WSL: not docker-desktop — check with `wsl -l -v`
- **Rust nightly** — version pinned in `encore/rust-toolchain.toml`
- **cargo-zigbuild** — ARM cross-compilation
- **wasm-pack** — compiles the WASM dashboard
- **USB-A to USB Mini-B cable** for initial flashing

### WSL Setup

Always specify the Ubuntu distro explicitly:

```bash
# Correct — uses Ubuntu
wsl.exe -d Ubuntu -u root -- bash -c "..."

# WRONG — may default to docker-desktop!
wsl.exe -- bash -c "..."
```

### Install Build Dependencies (on Windows)

Only needed for firmware packaging (SquashFS tools):

```bash
wsl.exe -d Ubuntu -u root -- bash -c "
apt-get update
apt-get install -y squashfs-tools python3
"
```

### Install Rust Toolchain

```bash
# Install rustup if not already present
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# cargo-zigbuild for ARM cross-compilation
cargo install cargo-zigbuild

# wasm-pack for dashboard
cargo install wasm-pack

# Tauri CLI for desktop/mobile app (optional)
cargo install tauri-cli@^2
```

The Rust toolchain version and targets are pinned in `encore/rust-toolchain.toml` — rustup will install them automatically on first build.

## Step 1: Obtain Stock Firmware

The stock 83_IMAGE is required as a base. It contains the encrypted, signature-verified kernel, which cannot be rebuilt from source.

```bash
make download
```

This will show instructions for obtaining the stock image. Place it at `firmware/83_IMAGE_stock`.

**Verification**: The original Harman stock firmware is approximately 69MB (69,481,562 bytes).

## Step 2: Build Encore

```bash
make encore
```

This builds both the WASM web dashboard and the ARM binary:
1. `wasm-pack` compiles `encore-wasm` to WebAssembly
2. `cargo-zigbuild` cross-compiles `encore-firmware` for `armv7-unknown-linux-musleabihf`

The binary is statically linked against musl libc — no shared library dependencies on the device.

Output: `encore/target/armv7-unknown-linux-musleabihf/release/encore-firmware` (~6.6 MB stripped)

## Step 2b: Build Desktop/Mobile App (Optional)

The Encore app wraps the WASM dashboard in a native app via Tauri v2. This step is optional — the speaker's built-in web server serves the same dashboard.

### Desktop (Windows)

```bash
make app
```

This builds the WASM dashboard, generates app icons, and produces native installers (MSI + NSIS on Windows). Output: `build/desktop/`.

For development with hot reload:

```bash
make app-dev
```

### Android

Requires Android SDK and NDK. Follow the [Tauri v2 Android prerequisites](https://v2.tauri.app/start/prerequisites/#android).

```bash
make app-android
```

Output: `build/android/Encore.apk` (signed with a debug keystore).

For development:

```bash
make app-android-dev
```

See [app.md](app.md) for full details on platforms and usage.

## Step 3: Build Firmware

```bash
make firmware
```

This runs in WSL and:
1. Extracts the stock rootfs from 83_IMAGE
2. Removes Cortana, Microsoft services, and bloat (~45MB stripped)
3. Copies the rootfs overlay (Encore binary, init scripts, web UI)
4. Builds a new SquashFS (must fit in 84.8MB NAND partition)
5. Packages it into 83_IMAGE with correct CRC32

Output:
- `firmware/83_IMAGE` — full image for USB boot flashing
- `firmware/rootfs.squashfs` — raw SquashFS for web UI OTA updates

## Step 4: Flash

See [Flashing Guide](flashing.md) for all deployment methods — USB boot (first flash), OTA rootfs, and OTA binary updates.

## Development Workflow

The fastest loop is a binary-only update, with no firmware rebuild:

```bash
make encore
```

Then deploy `build/encore` to the device by either:

1. **Web UI** (recommended): upload the binary in the dashboard's Update tab. It is staged
   as `/lsync/encore/encore_next` and promoted on the next boot.
2. **SSH**: stage it manually, then reboot:
   ```bash
   cat build/encore | ssh root@<device-ip> 'cat > /run/encore_next'
   ssh root@<device-ip> 'cp /run/encore_next /lsync/encore/encore_next && sync && reboot'
   ```
   (Upload to `/run` first; writing directly into `/lsync` over SSH can silently truncate.)

The supervisor promotes a staged binary only after it runs stably, and falls back to the
previous binary if it crashes at boot, so a bad build costs you one reboot, not a reflash.

**Do not `kill` the running Encore process to restart it.** A dirty kill leaves WiFi, the
DSP, and the watchdog in undefined states and can require a USB reflash. Always deploy via
staging + reboot and let the supervisor manage the process.

### Run Tests

```bash
cd encore && cargo test --all
```

### Verify No Credential Leaks

```bash
make verify
```

## Build Options

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `WORK` | `~/hkinvoke-build` | Firmware build working directory |
| `STOCK_IMG` | `firmware/83_IMAGE_stock` | Path to stock image |

### CLI Flags

```bash
# Use different working directory
./scripts/build/build_firmware.sh --work /home/user/build
```

## Modifying the Firmware

### Adding/Changing Services

Edit files under `rootfs/`. The build script copies the entire overlay tree onto the stock rootfs:

- `rootfs/usr/bin/` — Encore binary, helper scripts
- `rootfs/etc/init.d/` — Boot-time init scripts (S00, S01, S02)
- `rootfs/sbin/` — System scripts (WiFi, AP, mount_partition, supervisor)

### Boot Sequence

`mount_partition.sh` is the main entry point called by stock init. It launches Encore via the supervisor. See [architecture.md](architecture.md) for details.

## Important Rules

1. **Never use `mksquashfs -all-root`** — it destroys file ownership and breaks boot
2. **Always extract rootfs as root** in WSL to preserve UIDs/GIDs
3. **The rootfs must fit in 84.8MB** — the NAND partition maximum
