//! Boot sequence — try connecting to known or default speaker host.
//!
//! Called from `lib.rs` instead of `app::init()`. Runs an async boot
//! sequence that updates the loading screen, then hands off to
//! `app::init_after_boot()`.

use crate::dom;

/// Entry point — called from lib.rs start().
pub fn run() {
    let _state = crate::state::init();

    wasm_bindgen_futures::spawn_local(async {
        boot_sequence().await;
    });
}

/// Update the loading screen status text and fling the ring.
fn set_status(text: &str) {
    if let Some(el) = dom::get_el("loading-status") {
        dom::set_text(&el, text);
    }
    fling_ring();
}

/// Briefly speed up the loading ring rotation for visual feedback.
fn fling_ring() {
    let doc = dom::document();
    if let Ok(Some(ring)) = doc.query_selector(".ring") {
        dom::add_class(&ring, "fling");
        dom::set_timeout(
            move || {
                if let Ok(Some(ring)) = dom::document().query_selector(".ring") {
                    dom::remove_class(&ring, "fling");
                }
            },
            800,
        );
    }
}

/// Sleep for the given number of milliseconds.
async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        dom::window()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .ok();
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Wait for both ConfigLoaded and SystemStatus, with a timeout.
async fn wait_for_data_gate(timeout_ms: i32) -> bool {
    let step_ms = 100;
    let mut elapsed = 0;
    loop {
        let (config, system) =
            crate::state::with(|s| (s.boot_config_received, s.boot_system_received));
        if config && system {
            return true;
        }
        if elapsed >= timeout_ms {
            return false;
        }
        sleep(step_ms).await;
        elapsed += step_ms;
    }
}

/// The main boot sequence.
async fn boot_sequence() {
    // Small delay to let the loading screen render and be visible
    sleep(300).await;

    // Demo mount: `?demo` (or `?demo=1`) seeds representative state and mounts
    // immediately, skipping all WebSocket connection. Lets the Stage be iterated
    // in a plain browser without a live device. Production boot is unchanged.
    if demo_requested() {
        boot_demo();
        return;
    }

    let is_standalone = dom::is_standalone();

    if is_standalone {
        boot_standalone().await;
    } else {
        boot_device_served().await;
    }
}

/// True when the page URL query string asks for the demo mount (`?demo`).
fn demo_requested() -> bool {
    dom::window()
        .location()
        .search()
        .map(|q| q.contains("demo"))
        .unwrap_or(false)
}

/// Seed representative state and mount the dashboard with no WebSocket. Gated
/// entirely behind `demo_requested()`, so it never runs in production.
fn boot_demo() {
    use encore_common::protocol::{EncoreConfig, SpotifyStatus, SystemSnapshot, TrackInfo};

    crate::state::with_mut(|s| {
        s.connected = true;

        // EncoreConfig::default() already has spotify_enabled = true.
        s.config = Some(EncoreConfig::default());

        s.system = Some(SystemSnapshot {
            uptime_secs: 3600,
            cpu_percent: 8,
            ram_used_kb: 96_000,
            ram_total_kb: 256_000,
            cores: Vec::new(),
            load_avg: [0.12, 0.10, 0.08],
            ram_free_kb: 140_000,
            ram_buffers_kb: 6_000,
            ram_cached_kb: 14_000,
            temperature_mc: Some(42_000),
            disks: Vec::new(),
            net_interfaces: Vec::new(),
            process_count: 48,
        });

        s.spotify_status = Some(SpotifyStatus {
            is_playing: true,
            shuffle: false,
            repeat_context: false,
            repeat_track: false,
            // u16 0..65535 volume scale; ~30% ≈ 19660.
            volume: 19_660,
            position_ms: 84_000,
            duration_ms: 331_000,
            track: Some(TrackInfo {
                title: "Teardrop".into(),
                artist: "Massive Attack".into(),
                album: "Mezzanine".into(),
                duration_ms: 331_000,
                position_ms: 84_000,
                cover_url: String::new(), // empty -> music-note fallback glyph
                uri: String::new(),
                is_explicit: false,
            }),
            connected_user: Some("demo".into()),
        });

        s.master_volume = 42;
        s.boot_config_received = true;
        s.boot_system_received = true;
    });

    crate::app::init_after_boot("home");

    // Keep the Ovation bloom alive for iteration: a repeating pump (demo path
    // only, so production is unaffected) that feeds gently oscillating levels so
    // the rays animate continuously. A sine over wall-clock time varies rms/peak
    // ~0.3..0.9; note_levels re-arms the reactive loop each tick.
    let phase = std::cell::Cell::new(0.0f32);
    crate::graphics::ovation::note_levels(0.5, 0.5, 0.7, 0.7);
    dom::set_interval(
        move || {
            let t = phase.get();
            phase.set(t + 0.18);
            // Map sine (-1..1) -> 0.3..0.9; peak rides a touch above rms.
            let rms = 0.6 + 0.3 * t.sin();
            let peak = (rms + 0.08).min(1.0);
            crate::graphics::ovation::note_levels(rms, rms, peak, peak);
        },
        100,
    );
}

/// Boot flow for standalone mode (Tauri or external browser).
///
/// 1. Try saved host from localStorage (previous successful connection)
/// 2. mDNS discovery via Tauri backend (finds any Encore on the network)
/// 3. Fall through to connect page for manual entry
async fn boot_standalone() {
    // 1. Try saved host
    if let Some(host) = dom::get_local("encore_speaker_host") {
        set_status("Connecting...");
        if try_connect(&host, 8000).await {
            set_status("Syncing...");
            wait_for_data_gate(5000).await;
            crate::app::init_after_boot("home");
            return;
        }
        crate::ws::disconnect();
        crate::state::with_mut(|s| s.speaker_host = None);
        dom::remove_local("encore_speaker_host");
    }

    // 2. mDNS discovery (Tauri desktop only)
    if dom::has_tauri() {
        set_status("Searching...");
        let speakers = dom::tauri_discover_speakers().await;
        if let Some((_name, host)) = speakers.first() {
            set_status("Connecting...");
            if try_connect(host, 5000).await {
                dom::set_local("encore_speaker_host", host);
                set_status("Syncing...");
                wait_for_data_gate(5000).await;
                crate::app::init_after_boot("home");
                return;
            }
            crate::ws::disconnect();
            crate::state::with_mut(|s| s.speaker_host = None);
        }
    }

    // 3. Nothing found — show connect page
    crate::app::init_after_boot("connect");
}

/// Try connecting to a host via WebSocket. Returns true if connected within timeout_ms.
async fn try_connect(host: &str, timeout_ms: i32) -> bool {
    crate::state::with_mut(|s| s.speaker_host = Some(host.to_string()));
    crate::ws::connect();

    let mut elapsed = 0;
    while elapsed < timeout_ms {
        if crate::state::with(|s| s.connected) {
            return true;
        }
        sleep(200).await;
        elapsed += 200;
    }
    false
}

/// Boot flow for device-served mode (dashboard loaded from speaker itself).
async fn boot_device_served() {
    set_status("Connecting...");
    crate::ws::connect();

    // Wait for WebSocket to connect (up to 3s)
    let mut elapsed = 0;
    while elapsed < 3000 {
        if crate::state::with(|s| s.connected) {
            break;
        }
        sleep(100).await;
        elapsed += 100;
    }

    if crate::state::with(|s| s.connected) {
        set_status("Syncing...");
        wait_for_data_gate(5000).await;
    }

    // Proceed to dashboard regardless (WS auto-reconnects)
    crate::app::init_after_boot("home");
}
