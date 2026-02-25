//! Connect page — speaker discovery and manual connection.
//!
//! Shown when running as a standalone app (Tauri or external browser)
//! and no speaker host has been configured yet. Lets the user enter
//! a speaker IP manually or scan for speakers via mDNS (Tauri only).

use crate::dom;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

pub fn render(container: &web_sys::Element) {
    let wrap = dom::create_div();
    dom::set_class(&wrap, "connect-page");

    // Spacer for the unified hero logo (fixed-position, lands here visually)
    let spacer = dom::create_div();
    dom::set_class(&spacer, "connect-logo-spacer");
    dom::append(&wrap, &spacer);

    let subtitle = dom::el("div", "connect-subtitle",
        Some("Enter the IP or hostname of your Invoke"));
    dom::append(&wrap, &subtitle);

    // Input row
    let input_row = dom::create_div();
    dom::set_class(&input_row, "connect-input-row");

    let input = dom::create_el("input");
    input.set_id("connect-host-input");
    dom::set_class(&input, "text-field connect-input");
    dom::set_attr(&input, "type", "text");
    dom::set_attr(&input, "placeholder", "192.168.43.1");
    dom::set_attr(&input, "autocomplete", "off");
    dom::set_attr(&input, "autocapitalize", "off");
    dom::set_attr(&input, "spellcheck", "false");

    // Pre-fill saved host if any
    if let Some(saved) = dom::get_local("encore_speaker_host") {
        dom::set_attr(&input, "value", &saved);
    }

    // Connect on Enter key
    {
        let cb = Closure::wrap(Box::new(|e: web_sys::KeyboardEvent| {
            if e.key() == "Enter" {
                do_connect_from_input();
            }
        }) as Box<dyn FnMut(_)>);
        input.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref()).ok();
        cb.forget();
    }
    dom::append(&input_row, &input);

    let connect_btn = dom::el("button", "btn btn-primary", Some("Connect"));
    connect_btn.set_id("connect-btn");
    dom::on_click(&connect_btn, || do_connect_from_input());
    dom::append(&input_row, &connect_btn);

    dom::append(&wrap, &input_row);

    // Status message area
    let status = dom::create_div();
    status.set_id("connect-status");
    dom::set_class(&status, "connect-status");
    dom::append(&wrap, &status);

    // mDNS discovery section
    let discovery = dom::create_div();
    dom::set_class(&discovery, "connect-discovery");

    let divider = dom::el("div", "connect-divider", Some("or"));
    dom::append(&discovery, &divider);

    let scan_btn = dom::el("button", "btn connect-scan-btn", Some("Scan Network"));
    scan_btn.set_id("connect-scan-btn");
    dom::on_click(&scan_btn, || scan_for_speakers());
    dom::append(&discovery, &scan_btn);

    let results = dom::create_div();
    results.set_id("connect-results");
    dom::set_class(&results, "connect-results");
    dom::append(&discovery, &results);

    // Show pre-scanned speakers from boot sequence (if any)
    let boot_speakers = crate::state::with(|s| s.boot_speakers.clone());
    if !boot_speakers.is_empty() {
        show_discovered_speakers(boot_speakers);
        if let Some(btn) = dom::get_el("connect-scan-btn") {
            dom::set_text(&btn, "Scan Again");
        }
    }

    dom::append(&wrap, &discovery);

    dom::append(container, &wrap);
}

pub fn update() {
    // Static page — no live updates needed
}

/// Read the input field and connect.
fn do_connect_from_input() {
    if let Some(el) = dom::get_el("connect-host-input") {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            let host = input.value().trim().to_string();
            if host.is_empty() {
                show_status("Enter an IP address or hostname", true);
                return;
            }
            connect_to_speaker(&host);
        }
    }
}

/// Connect to a speaker by host/IP — verifies connection before navigating.
pub fn connect_to_speaker(host: &str) {
    // Strip protocol prefix if user included it
    let host = host
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();

    // Save to localStorage and state
    dom::set_local("encore_speaker_host", &host);
    crate::state::with_mut(|s| s.speaker_host = Some(host.clone()));

    // Reset boot flags for fresh data gate
    crate::state::with_mut(|s| {
        s.boot_config_received = false;
        s.boot_system_received = false;
        s.connected = false;
    });

    // Transition logo to connecting state and hide connect form
    crate::app::set_logo_state("connecting");
    set_connect_form_visible(false);

    // Kick off async connection + verification
    wasm_bindgen_futures::spawn_local(async move {
        verify_connection(host).await;
    });
}

/// Async connection verification — waits for WS + data gate.
async fn verify_connection(host: String) {
    // Show connecting status on logo
    set_logo_status("Connecting...");

    // Connect WebSocket
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

    if !crate::state::with(|s| s.connected) {
        // Connection failed — revert to connect screen
        connection_failed(&host, "Could not reach speaker");
        return;
    }

    // Connected — wait for data
    set_logo_status("Syncing...");

    let mut elapsed = 0;
    loop {
        let (config, system) = crate::state::with(|s| {
            (s.boot_config_received, s.boot_system_received)
        });
        if config && system {
            break;
        }
        if elapsed >= 5000 {
            // Timeout — proceed anyway, data will trickle in
            break;
        }
        sleep(100).await;
        elapsed += 100;
    }

    // Success — navigate to dashboard
    set_logo_status("");
    if let Some(nav) = dom::get_el("tab-nav-wrap") {
        dom::set_style(&nav, "display", "");
    }
    crate::app::set_logo_state("header");
    crate::app::check_setup_status();
    dom::window().location().set_hash("dashboard").ok();
}

/// Revert to connect screen after a failed connection attempt.
fn connection_failed(host: &str, reason: &str) {
    // Clear the broken host
    dom::remove_local("encore_speaker_host");
    crate::state::with_mut(|s| s.speaker_host = None);

    // Return logo to hero state
    set_logo_status("");
    crate::app::set_logo_state("hero");
    set_connect_form_visible(true);

    // Pre-fill the input with what they typed
    if let Some(el) = dom::get_el("connect-host-input") {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            input.set_value(host);
        }
    }

    show_status(reason, true);
}

/// Show/hide the connect form content during connection verification.
fn set_connect_form_visible(visible: bool) {
    if let Some(content) = dom::get_el("content") {
        let style = if visible { "1" } else { "0" };
        let events = if visible { "auto" } else { "none" };
        dom::set_style(&content, "opacity", style);
        dom::set_style(&content, "pointer-events", events);
    }
}

/// Set status text below the logo (for connecting/syncing states).
fn set_logo_status(text: &str) {
    if let Some(el) = dom::get_el("logo-status") {
        dom::set_text(&el, text);
    }
}

/// Async sleep utility.
async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        dom::window()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .ok();
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Show a status message on the connect page.
fn show_status(msg: &str, is_error: bool) {
    if let Some(el) = dom::get_el("connect-status") {
        dom::set_text(&el, msg);
        if is_error {
            dom::set_class(&el, "connect-status error");
        } else {
            dom::set_class(&el, "connect-status");
        }
    }
}

/// Scan for speakers via Tauri mDNS command (or show manual-only message).
fn scan_for_speakers() {
    if let Some(btn) = dom::get_el("connect-scan-btn") {
        dom::set_text(&btn, "Scanning...");
        dom::set_attr(&btn, "disabled", "true");
    }

    // Check if Tauri runtime is available
    let w = dom::window();
    let w_ref: &JsValue = w.as_ref();
    let has_tauri = js_sys::Reflect::get(w_ref, &"__TAURI__".into())
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false);

    if has_tauri {
        wasm_bindgen_futures::spawn_local(async {
            match tauri_discover().await {
                Ok(speakers) => show_discovered_speakers(speakers),
                Err(_) => show_scan_error("Discovery failed"),
            }
            if let Some(btn) = dom::get_el("connect-scan-btn") {
                dom::set_text(&btn, "Scan Again");
                let _ = btn.remove_attribute("disabled");
            }
        });
    } else {
        // No Tauri — can't do mDNS from browser
        if let Some(results) = dom::get_el("connect-results") {
            dom::clear(&results);
            let msg = dom::el("div", "text-muted connect-no-scan", Some(
                "Network scanning requires the Encore desktop or mobile app. \
                 Enter the speaker IP manually above."
            ));
            dom::append(&results, &msg);
        }
        if let Some(btn) = dom::get_el("connect-scan-btn") {
            dom::set_text(&btn, "Scan Network");
            let _ = btn.remove_attribute("disabled");
        }
    }
}

/// Call Tauri's discover_speakers command.
async fn tauri_discover() -> Result<Vec<(String, String)>, JsValue> {
    let w = dom::window();
    let w_ref: &JsValue = w.as_ref();
    let tauri = js_sys::Reflect::get(w_ref, &"__TAURI__".into())?;
    let core = js_sys::Reflect::get(&tauri, &"core".into())?;
    let invoke_fn = js_sys::Reflect::get(&core, &"invoke".into())?;
    let func = invoke_fn.dyn_ref::<js_sys::Function>().ok_or(JsValue::NULL)?;

    let promise: js_sys::Promise = func.call1(&core, &"discover_speakers".into())?.into();
    let result = wasm_bindgen_futures::JsFuture::from(promise).await?;

    // Parse JSON array of { name, host, port }
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
    Ok(speakers)
}

/// Display discovered speakers as clickable items.
fn show_discovered_speakers(speakers: Vec<(String, String)>) {
    let Some(results) = dom::get_el("connect-results") else { return };
    dom::clear(&results);

    if speakers.is_empty() {
        let msg = dom::el("div", "text-muted", Some("No speakers found. Make sure you're on the same network."));
        dom::append(&results, &msg);
        return;
    }

    for (name, host) in speakers {
        let item = dom::create_div();
        dom::set_class(&item, "connect-result-item");

        let label = if name.is_empty() {
            host.clone()
        } else {
            format!("{} ({})", name, host)
        };
        let name_el = dom::el("span", "connect-result-name", Some(&label));
        dom::append(&item, &name_el);

        let btn = dom::el("button", "btn btn-small", Some("Connect"));
        let h = host.clone();
        dom::on_click(&btn, move || {
            connect_to_speaker(&h);
        });
        dom::append(&item, &btn);

        dom::append(&results, &item);
    }
}

fn show_scan_error(msg: &str) {
    if let Some(results) = dom::get_el("connect-results") {
        dom::clear(&results);
        let err = dom::el("div", "text-muted", Some(msg));
        dom::append(&results, &err);
    }
}
