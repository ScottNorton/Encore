//! Smart boot sequence — connection probing, mDNS scanning, data gate.
//!
//! Called from `lib.rs` instead of `app::init()`. Runs an async boot
//! sequence that updates the loading screen text and ring animation
//! at each phase, then hands off to `app::init_after_boot()`.

use crate::dom;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Entry point — called from lib.rs start().
pub fn run() {
    // Initialize state early so boot can read/write it
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
        dom::set_timeout(move || {
            if let Ok(Some(ring)) = dom::document().query_selector(".ring") {
                dom::remove_class(&ring, "fling");
            }
        }, 800);
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
        let (config, system) = crate::state::with(|s| {
            (s.boot_config_received, s.boot_system_received)
        });
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

/// Check if Tauri runtime is available.
fn has_tauri() -> bool {
    let w = dom::window();
    let w_ref: &JsValue = w.as_ref();
    js_sys::Reflect::get(w_ref, &"__TAURI__".into())
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false)
}

/// Run mDNS discovery via Tauri. Returns discovered (name, host) pairs.
async fn tauri_discover() -> Vec<(String, String)> {
    let w = dom::window();
    let w_ref: &JsValue = w.as_ref();
    let tauri = match js_sys::Reflect::get(w_ref, &"__TAURI__".into()) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let core = match js_sys::Reflect::get(&tauri, &"core".into()) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let invoke_fn = match js_sys::Reflect::get(&core, &"invoke".into()) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let func = match invoke_fn.dyn_ref::<js_sys::Function>() {
        Some(f) => f,
        None => return Vec::new(),
    };
    let promise: js_sys::Promise = match func.call1(&core, &"discover_speakers".into()) {
        Ok(p) => p.into(),
        Err(_) => return Vec::new(),
    };
    let result = match wasm_bindgen_futures::JsFuture::from(promise).await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut speakers = Vec::new();
    if let Some(arr) = result.dyn_ref::<js_sys::Array>() {
        for i in 0..arr.length() {
            let item = arr.get(i);
            let name = js_sys::Reflect::get(&item, &"name".into())
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            let host = js_sys::Reflect::get(&item, &"host".into())
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            if !host.is_empty() {
                speakers.push((name, host));
            }
        }
    }
    speakers
}

/// The main boot sequence.
async fn boot_sequence() {
    // Small delay to let the loading screen render and be visible
    sleep(300).await;

    let is_standalone = dom::is_standalone();

    if is_standalone {
        boot_standalone().await;
    } else {
        boot_device_served().await;
    }
}

/// Boot flow for standalone mode (Tauri or external browser).
async fn boot_standalone() {
    // Check for a saved speaker host
    let saved_host = dom::get_local("encore_speaker_host");

    if let Some(ref host) = saved_host {
        // Try connecting to saved host
        set_status("Connecting...");
        crate::state::with_mut(|s| s.speaker_host = Some(host.clone()));
        crate::ws::connect();

        // Wait briefly for the WebSocket to connect
        sleep(2000).await;

        let connected = crate::state::with(|s| s.connected);
        if connected {
            // Connected — wait for data gate
            set_status("Syncing...");
            wait_for_data_gate(5000).await;
            // Land on dashboard (data may still be arriving)
            crate::app::init_after_boot("dashboard");
            return;
        }

        // Connection failed — clear the broken host, fall through to scan
        crate::state::with_mut(|s| s.speaker_host = None);
    }

    // No host or connection failed — scan for speakers
    if has_tauri() {
        set_status("Scanning...");
        let speakers = tauri_discover().await;
        crate::state::with_mut(|s| s.boot_speakers = speakers);
    }

    // Land on connect screen
    crate::app::init_after_boot("connect");
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
    crate::app::init_after_boot("dashboard");
}
