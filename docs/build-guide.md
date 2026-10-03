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

This downloads Harman's final public OTA2 package from archive.org, extracts the stock
83_IMAGE, and writes it to `firmware/83_IMAGE_stock`. The download is verified by size
(69,481,562 bytes) and SHA256, and the step is skipped if a valid image is already present.

## Step 2: Build Encore

```bash
make encore
```

This builds both the WASM web dashboard and the ARM binary:
1. `wasm-pack` compiles `encore-wasm` to WebAssembly
2. `cargo-zigbuild` cross-compiles `encore-firmware` for `armv7-unknown-linux-musleabihf`

The binary is statically linked against musl libc — no shared library dependencies on the device.

Output: `build/encore` (stripped, statically linked)

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
- `build/firmware/83_IMAGE` — full image for USB boot flashing
- `build/firmware/rootfs.squashfs` — raw SquashFS for web UI OTA updates

### TLS fallback pair

`make firmware` also creates a TLS certificate and private key in `build/tls/` and installs them in the image. They are a spare. The speaker makes its own HTTPS certificate on first boot and normally uses that one (see [HTTPS / TLS Setup](troubleshooting.md#https--tls-setup)). The spare pair is used only when that fails, for example when `/lsync` cannot be written.

| | |
|---|---|
| What it is | `cert.pem` (a self-signed certificate) and `key.pem` (its private key), both PEM files |
| Where the build keeps it | `build/tls/cert.pem` and `build/tls/key.pem`, next to the compiled `build/encore` |
| Where it ends up on the speaker | `/usr/share/encore/tls/cert.pem` and `key.pem` (read-only) |
| What creates it | `scripts/build/gen_tls_fallback.sh`, run by `make firmware` or by `make tls` |

Things worth knowing:

- The key is made on your machine and belongs to your builds only. It is not in the repository, it is not compiled into the Encore binary, and it is not part of any published release.
- Existing files are never overwritten. To use your own certificate, put `cert.pem` and `key.pem` in `build/tls/` before you run `make firmware`. The script checks that the key belongs to the certificate and stops if it does not. The certificate should cover the names and addresses you browse to.
- The firmware loads the files when it starts, not when it is compiled. It checks that the key belongs to the certificate (for key files in the common PKCS#8 layout) and ignores a pair that is missing, unreadable, or does not match.
- If there is no usable pair (for example you updated only the Encore binary over the air on an older image), the web server makes a throwaway certificate in memory each time it starts. The dashboard still comes up, and nothing is written to disk.
- Browsers do not trust any of these certificates. Install the speaker's own CA from `/ca.crt` to get a trusted connection.
- `make clean` removes `build/`, including the pair. The next `make firmware` creates a new one.
- Never commit `build/tls/key.pem`. The repository ignores `*.pem` and `*.key`, and `make verify` fails if a private key is tracked.

The script needs `openssl`. `make tls` runs it on its own if you want to look at or replace the files before building.

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
make test
```

### Verify No Credential Leaks

```bash
make verify
```

This fails if the repository tracks a private key, a certificate or key file, a vendor PDF, a firmware image, a file over 2 MB, or a shell script with Windows line endings. It also looks for private-key blocks and common access-token formats in tracked text. Nothing needs to be set up for these checks, and CI runs them.

To check the whole commit history as well (file names, sizes, private-key blocks, and author trailers in commit messages), run:

```bash
make verify VERIFY_ARGS=--history
```

For values only you know are private, such as your own WiFi name, put one pattern per line in `.verify-patterns` at the repository root. That file is git-ignored.

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

### Building the USB Gadget Kernel Modules

The USB network gadget (RNDIS over the Mini-B port, the device at 10.55.55.1) needs patched `g_ether`/RNDIS kernel modules. They are built separately from Encore. The compiled `.ko` files are checked in under `rootfs/usr/lib/usbgadget/` and the firmware build installs them from there, so you only need to rebuild them if you change the gadget patches.

These modules are GPL-2.0 kernel code. [LEGAL.md](../LEGAL.md#gpl-components-and-source-availability) says where the source for them is.

What you need:

- **The vendor kernel tree.** Linux 3.8.13 as Harman published it for the Invoke (LEGAL.md links a copy). Unpack it so that `vendor/kernel/Makefile` exists (`vendor/` is git-ignored), or point `KDIR` at it.
- **The Linaro GCC 4.9.4 cross compiler**, release 2017.01 (`gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf`). Linaro no longer serves it from its old download address, so you will have to find a copy of that exact release. Point `TOOLCHAIN` at its folder, or put it in `vendor/toolchains/`. A modern gcc builds modules that `insmod` accepts but that fault the 3.8.13 kernel once traffic flows.
- **A prepared kernel tree.** Once, before the first build, run `scripts/build/build_kernel_vendor.sh defconfig` and then `scripts/build/build_kernel_vendor.sh modules_prepare`.

Then run the build (from WSL or Linux):

```bash
export TOOLCHAIN=/path/to/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf
bash scripts/device/build_usb_gadget_modules.sh
```

The script applies the patches in `scripts/device/usb-gadget-patches/` to the kernel tree (a patch that is already applied is skipped), builds only the gadget modules, and collects the four `.ko` files in `build/usb_gadget_modules/`. Copy them into `rootfs/usr/lib/usbgadget/` and rebuild the firmware. `rootfs/sbin/usb_gadget.sh` loads them at boot and `usb_gadget_monitor.sh` reloads them if the link drops.

The patches are plain diffs against the unmodified Harman tree. To change the gadget behavior, edit the tree and then run `PRISTINE=/path/to/unmodified/tree bash scripts/device/gen_gadget_patches.sh` to regenerate them.

The maintainer has built the modules this way. Nobody else has tried it from a clean checkout yet, so expect small problems and please report them.

The MTU is fixed at 400 bytes in the gadget configuration so each frame is a single USB packet. The `mv_udc` controller stalls multi-packet bulk transfers, so raising the MTU brings the stall back. See [usb-access.md](usb-access.md) for the runtime side of this.

## Important Rules

1. **Never use `mksquashfs -all-root`** — it destroys file ownership and breaks boot
2. **Always extract rootfs as root** in WSL to preserve UIDs/GIDs
3. **The rootfs must fit in 84.8MB** — the NAND partition maximum
