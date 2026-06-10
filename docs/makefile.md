# Makefile Reference

The top-level `Makefile` orchestrates all builds. On Windows it routes Linux-only
tasks through WSL automatically; on Linux (native or CI) everything runs directly.

Run `make` with no arguments to see the target list.

## Build Targets

### Core

| Target | Environment | Dependencies | Output | Description |
|--------|-------------|-------------|--------|-------------|
| `make encore` | Any (Rust) | `wasm` | `build/encore` | WASM dashboard + ARM binary — the main firmware binary |
| `make wasm` | Any (Rust) | — | `encore/web/pkg/` | WASM dashboard only (via `wasm-pack`) |
| `make firmware` | Linux/WSL | `encore` | `build/firmware/83_IMAGE`, `build/firmware/rootfs.squashfs` | Full flashable firmware image (extracts stock, merges overlay, packages SquashFS) |

`make encore` depends on `make wasm` — the WASM dashboard is always built first
and embedded in the ARM binary at compile time via `rust-embed`.

`make firmware` depends on `make encore` — the ARM binary must exist before
packaging the rootfs.

### Desktop & Mobile App

| Target | Environment | Dependencies | Output | Description |
|--------|-------------|-------------|--------|-------------|
| `make app` | Any (Rust, Tauri) | `app-dist`, `app-icons` | `build/desktop/*.msi`, `*.exe`, `*.dmg`, `*.deb`, `*.AppImage` | Production desktop app build |
| `make app-dev` | Any (Rust, Tauri) | `app-dist`, `app-icons` | — | Launch desktop app in dev mode (hot reload) |
| `make app-android` | Windows (Android SDK) | `app-dist`, `app-icons` | `build/android/Encore.apk`, `*.aab` | Signed Android APK + AAB |
| `make app-android-dev` | Windows (Android SDK) | `app-dist`, `app-icons` | — | Launch on connected Android device |
| `make app-dist` | Any | `wasm` | `build/app-dist/` | Assemble web dashboard + branding for Tauri |
| `make app-icons` | Any (Rust, Tauri) | — | `encore/crates/encore-app/icons/` | Generate platform icons from branding source |

`make app` clears the WebView2 cache (`%LOCALAPPDATA%/com.encore.speaker/`) before
building to prevent stale frontend content.

### Kernel & Kexec

| Target | Environment | Dependencies | Output | Description |
|--------|-------------|-------------|--------|-------------|
| `make kernel` | Linux/WSL | — | `build/kernel/zImage-dtb` | Build Linux 6.1 kernel for kexec boot |
| `make kernel-vendor` | Linux/WSL | — | — | Build stock vendor 3.8.13 kernel |
| `make kexec` | Linux/WSL | — | `build/kexec/` | Build kexec kernel module + userspace tools |

### Setup & Verification

| Target | Environment | Dependencies | Output | Description |
|--------|-------------|-------------|--------|-------------|
| `make download` | Linux/WSL | — | `firmware/83_IMAGE_stock` | Download the stock Harman firmware image (~69 MB) |
| `make verify` | Any | — | — | Check for credential leaks and line-ending issues |

### Clean Targets

| Target | What it removes |
|--------|----------------|
| `make clean` | `build/`, Cargo artifacts, `encore/web/pkg/` |
| `make clean-kernel` | 6.1 kernel build artifacts |
| `make clean-kernel-vendor` | Vendor 3.8.13 kernel build artifacts |
| `make clean-kexec` | Kexec module build artifacts |
| `make clean-all` | All of the above |
| `make distclean` | Everything + WSL build directory (`~/hkinvoke-build`) |

## Dependency Graph

```
firmware ──► encore ──► wasm
app ──► app-dist ──► wasm
     └► app-icons
app-android ──► app-dist ──► wasm
            └► app-icons
```

## Build Outputs

All build outputs go to `build/` in the repo root:

```
build/
├── encore                  ARM binary (statically linked)
├── firmware/
│   ├── 83_IMAGE            Full flashable image
│   └── rootfs.squashfs     OTA-flashable rootfs
├── desktop/
│   ├── *.msi               Windows installer
│   ├── *-setup.exe         Windows NSIS installer
│   ├── *.dmg               macOS disk image
│   ├── *.deb               Linux Debian package
│   └── *.AppImage          Linux AppImage
├── android/
│   ├── Encore.apk          Signed Android APK
│   └── *.aab               Android App Bundle
└── kernel/
    └── zImage-dtb           6.1 kernel for kexec
```

## Windows / WSL Routing

The Makefile auto-detects the OS:

- **Windows (Git Bash / MSYS2)**: Linux-only targets (`firmware`, `kernel`, `kexec`,
  `download`, `distclean`) are routed through `wsl.exe -d Ubuntu -u root`. The
  `check-wsl` target verifies WSL is available before proceeding.
- **Linux (native or CI)**: All commands run directly with `bash -lc`.
- **Rust targets** (`encore`, `wasm`, `app`, `verify`): Run natively on any OS —
  no WSL required.

The WSL path is resolved with `wslpath` and cached in `REPO_WSL`. If detection
fails, you can override it: `make REPO_WSL=/mnt/g/HKInvoke firmware`.

## CI Workflows

The GitHub Actions workflows use the same Makefile targets:

### CI (`ci.yml`)

Runs on every push and PR to `main`. Three parallel jobs:

| Job | What it runs | Purpose |
|-----|-------------|---------|
| **Test** | `cargo test --workspace --exclude encore-app` | Unit tests (host-only) |
| **Build** | `wasm-pack build` → `cargo zigbuild --release` → `llvm-strip` | Verify ARM binary compiles, upload as artifact |
| **Verify** | `make verify` | Credential leak + line ending checks |

### Release (`release.yml`)

Triggered by pushing a `v*` tag. Three sequential jobs:

| Job | What it runs | Purpose |
|-----|-------------|---------|
| **Build** | Same as CI build job | Produces the Encore ARM binary |
| **Firmware** | Downloads stock image → `build_firmware.sh` | Verification gate only: proves the combined image builds end-to-end |
| **Release** | Collects the binary, generates a SHA256 checksum | Creates the GitHub release |

The only published release asset is the Encore ARM binary (`encore-<tag>-arm`, for OTA
binary updates). The firmware job intentionally does **not** upload `83_IMAGE` or
`rootfs.squashfs`: those contain stock Harman rootfs components and are not ours to
redistribute. Users run `make firmware` against their own stock image to produce a
flashable image.

The stock firmware image is cached between CI runs (keyed by SHA256 `b2e12178...`).
