# Desktop & Mobile App

The Encore app is a standalone [Tauri v2](https://v2.tauri.app/) wrapper around the same
WASM dashboard that runs on the speaker itself. Instead of opening a browser and navigating
to your speaker's IP, the app gives you a native window with speaker discovery, remembered
connections, and custom window controls -- no browser required.

When connected, the experience is identical to the browser dashboard. The app simply loads
the WASM frontend and points it at your speaker over the local network.

## Platforms

| Platform | WebView | Minimum Version | Installer |
|----------|---------|-----------------|-----------|
| Windows | WebView2 (Edge) | Windows 10+ | MSI + NSIS |
| Android | Android WebView | API 24+ (7.0) | APK |

macOS (WKWebView), Linux (WebKitGTK), and iOS (WKWebView) are supported by Tauri v2 but
have not been tested with Encore. The build system produces installers for the host platform
only -- cross-compilation requires each platform's native toolchain.

## Using the App

On launch, the app shows a **Connect** screen:

1. Enter your speaker's IP address (e.g., `192.168.1.42`) or hostname (`encore.local`
   if mDNS works on your network).
2. Tap **Connect**. The app saves this address in local storage and reconnects automatically
   on next launch.
3. In the Tauri app (not browser), you can also tap **Scan Network** to discover speakers
   via mDNS. Discovered speakers appear as clickable items.

Once connected, you get the full dashboard -- the same 8 tab pages, gear menu, setup wizard,
and real-time WebSocket telemetry as the browser version.

On desktop, the standard title bar is replaced with custom window controls (minimize,
maximize, close) drawn by the WASM layer. The default window size is 420x800, sized
for a phone-like layout but freely resizable.

## TLS Certificates

The speaker generates a self-signed CA on first boot and serves HTTPS on port 443. The
app's webview accepts self-signed certificates automatically, so TLS works out of the box
with no setup.

For browser-based access where you want a trusted green padlock, see the
[HTTPS / TLS Setup](troubleshooting.md#https--tls-setup) section in troubleshooting.

## Building from Source

### Prerequisites

- **Rust nightly** -- version pinned in `encore/rust-toolchain.toml` (installed automatically by rustup)
- **wasm-pack** -- `cargo install wasm-pack`
- **Tauri CLI v2** -- `cargo install tauri-cli@^2`

### Desktop

```bash
make app           # Build WASM + desktop installer
make app-dev       # Dev mode with hot reload
```

Build output goes to `build/desktop/` (MSI and NSIS installer on Windows, DMG on macOS,
deb/AppImage on Linux).

### Android

Additional prerequisites:

- Android SDK + NDK (install via Android Studio or `sdkmanager`)
- Follow the [Tauri v2 Android prerequisites](https://v2.tauri.app/start/prerequisites/#android)

```bash
make app-android       # Build WASM + signed APK
make app-android-dev   # Dev mode on connected device
```

Build output goes to `build/android/Encore.apk`. The Makefile generates a debug keystore
on first build and handles zipalign + apksigner automatically.

### Notes

- WASM must be built before the app, but all `make app*` targets depend on `make wasm`
  and handle this automatically.
- The app crate lives at `encore/crates/encore-app/`. Tauri configuration is in
  `encore/crates/encore-app/tauri.conf.json`.
- The WASM dashboard is assembled into `build/app-dist/` (web assets + branding) which
  Tauri bundles as the frontend.
