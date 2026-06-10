//! JSON WebSocket client with auto-reconnect.
//!
//! Connects to the firmware's /ws endpoint. Receives JSON-serialized
//! ServerMsg, sends ClientMsg. Reconnects with exponential backoff
//! on disconnect/error (1s → 2s → 4s → ... → 30s cap).

use encore_common::protocol::{ClientMsg, ServerMsg};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

const RECONNECT_BASE_MS: i32 = 1_000;
const RECONNECT_MAX_MS: i32 = 30_000;

thread_local! {
    static WS: RefCell<Option<WebSocket>> = RefCell::new(None);
    static STOPPED: RefCell<bool> = RefCell::new(false);
}

/// Connect to the firmware WebSocket.
pub fn connect() {
    STOPPED.with(|s| *s.borrow_mut() = false);
    do_connect(RECONNECT_BASE_MS);
}

/// Disconnect and stop auto-reconnect.
pub fn disconnect() {
    STOPPED.with(|s| *s.borrow_mut() = true);
    WS.with(|w| {
        if let Some(ws) = w.borrow_mut().take() {
            ws.set_onclose(None);
            ws.set_onerror(None);
            ws.close().ok();
        }
    });
}

fn do_connect(backoff_ms: i32) {
    let window = crate::dom::window();

    // In standalone mode, connect to the configured speaker host.
    // Otherwise, derive from page origin (device-served mode).
    let (host, protocol) = match crate::state::with(|s| s.speaker_host.clone()) {
        Some(h) => (h, "ws"),
        None => {
            let location = window.location();
            let h = location.host().unwrap_or_else(|_| "localhost".into());
            let p = if location.protocol().unwrap_or_default() == "https:" {
                "wss"
            } else {
                "ws"
            };
            (h, p)
        }
    };
    let url = format!("{}://{}/ws", protocol, host);

    let ws = match WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(e) => {
            web_sys::console::error_1(&format!("WS: connect failed: {:?}", e).into());
            crate::app::set_connection_status(false);
            schedule_reconnect(backoff_ms);
            return;
        }
    };

    // onopen
    let onopen = Closure::wrap(Box::new(|_: JsValue| {
        web_sys::console::log_1(&"WS: connected".into());
        crate::app::set_connection_status(true);
        // Request current config + network state on connect
        send_msg(&ClientMsg::RequestConfig);
        send_msg(&ClientMsg::RequestNetworkState);
    }) as Box<dyn FnMut(JsValue)>);
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();

    // onmessage — JSON text
    let onmessage = Closure::wrap(Box::new(|e: MessageEvent| {
        if let Some(text) = e.data().as_string() {
            match serde_json::from_str::<ServerMsg>(&text) {
                Ok(msg) => dispatch(msg),
                Err(err) => {
                    web_sys::console::warn_1(
                        &format!("WS: JSON parse error: {}", err).into(),
                    );
                }
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    // onclose
    let onclose = Closure::wrap(Box::new(move |_: JsValue| {
        web_sys::console::log_1(&"WS: disconnected".into());
        crate::app::set_connection_status(false);
        WS.with(|w| w.borrow_mut().take());
        schedule_reconnect(RECONNECT_BASE_MS);
    }) as Box<dyn FnMut(JsValue)>);
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onclose.forget();

    // onerror
    let onerror = Closure::wrap(Box::new(|_: JsValue| {
        web_sys::console::error_1(&"WS: error".into());
    }) as Box<dyn FnMut(JsValue)>);
    ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    onerror.forget();

    // Store handle
    WS.with(|w| *w.borrow_mut() = Some(ws));
}

fn schedule_reconnect(delay_ms: i32) {
    if STOPPED.with(|s| *s.borrow()) {
        return;
    }
    let next_delay = (delay_ms * 2).min(RECONNECT_MAX_MS);
    web_sys::console::log_1(&format!("WS: reconnecting in {}ms...", delay_ms).into());

    crate::dom::set_timeout(
        move || {
            if !STOPPED.with(|s| *s.borrow()) {
                do_connect(next_delay);
            }
        },
        delay_ms,
    );
}

/// Send a ClientMsg as JSON over the WebSocket.
pub fn send_msg(msg: &ClientMsg) {
    WS.with(|w| {
        if let Some(ws) = w.borrow().as_ref() {
            if ws.ready_state() == WebSocket::OPEN {
                match serde_json::to_string(msg) {
                    Ok(json) => {
                        ws.send_with_str(&json).ok();
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("WS: serialize error: {}", e).into(),
                        );
                    }
                }
            }
        }
    });
}

/// Dispatch a received ServerMsg — update state and notify active page.
fn dispatch(msg: ServerMsg) {
    // Tag for targeted page updates (avoids refreshing unrelated pages).
    let msg_tag = match &msg {
        ServerMsg::SystemStatus(_) => "SystemStatus",
        ServerMsg::SubsystemStatus(_) => "SubsystemStatus",
        ServerMsg::TrackChanged(_) => "TrackChanged",
        ServerMsg::BluetoothEvent(_) => "BluetoothEvent",
        ServerMsg::VolumeChanged { .. } => "VolumeChanged",
        ServerMsg::NetworkChanged(_) => "NetworkChanged",
        ServerMsg::LedStateChanged(_) => "LedStateChanged",
        ServerMsg::CrashReport(_) => "CrashReport",
        ServerMsg::ConfigLoaded(_) => "ConfigLoaded",
        ServerMsg::LogEntries(_) => "LogEntries",
        ServerMsg::AudioLevels { .. } => "AudioLevels",
        ServerMsg::SpotifyStatus(_) => "SpotifyStatus",
        ServerMsg::EqState(_) => "EqState",
        ServerMsg::DrcState(_) => "DrcState",
        ServerMsg::DspInfo(_) => "DspInfo",
        ServerMsg::DacRegValue { .. } => "DacRegValue",
        ServerMsg::DspSpiResponse { .. } => "DspSpiResponse",
        ServerMsg::DspMemoryDump { .. } => "DspMemoryDump",
        ServerMsg::DspEvent { .. } => "DspEvent",
        ServerMsg::WifiConnectResult(_) => "WifiConnectResult",
        ServerMsg::TimeSynced { .. } => "TimeSynced",
        ServerMsg::AudioSpectrum { .. } => "AudioSpectrum",
        ServerMsg::AudioWaveform { .. } => "AudioWaveform",
        ServerMsg::GroupStatus(_) => "GroupStatus",
        ServerMsg::BootMode { .. } => "BootMode",
        ServerMsg::AudioPowerState { .. } => "AudioPowerState",
        ServerMsg::MicLevels { .. } => "MicLevels",
    };
    match msg {
        ServerMsg::SystemStatus(snap) => {
            crate::state::with_mut(|s| {
                // Compute CPU% from delta for sparkline (accurate, not since-boot)
                let cpu_pct = if s.prev_cores.len() == snap.cores.len() && !snap.cores.is_empty() {
                    let mut total_delta = 0u64;
                    let mut idle_delta = 0u64;
                    for (cur, prev) in snap.cores.iter().zip(s.prev_cores.iter()) {
                        let ct = cur.user + cur.nice + cur.system + cur.idle + cur.iowait + cur.irq + cur.softirq;
                        let pt = prev.user + prev.nice + prev.system + prev.idle + prev.iowait + prev.irq + prev.softirq;
                        total_delta += ct.saturating_sub(pt);
                        idle_delta += cur.idle.saturating_sub(prev.idle);
                    }
                    if total_delta > 0 { ((total_delta - idle_delta) * 100 / total_delta) as u8 } else { 0 }
                } else {
                    snap.cpu_percent // fallback on first update
                };
                if s.cpu_history.len() >= 60 { s.cpu_history.pop_front(); }
                s.cpu_history.push_back(cpu_pct);

                // Compute total RX throughput for network sparkline
                if let Some(old) = &s.system {
                    // Store previous cores/net for next delta
                    s.prev_cores = old.cores.clone();
                    for iface in &old.net_interfaces {
                        s.prev_net.insert(iface.name.clone(), (iface.rx_bytes, iface.tx_bytes));
                    }

                    // Sum RX delta across all interfaces
                    let mut total_rx_delta = 0u64;
                    for iface in &snap.net_interfaces {
                        if let Some((prev_rx, _)) = s.prev_net.get(&iface.name) {
                            total_rx_delta += iface.rx_bytes.saturating_sub(*prev_rx);
                        }
                    }
                    if s.net_rx_history.len() >= 60 { s.net_rx_history.pop_front(); }
                    s.net_rx_history.push_back(total_rx_delta);
                }
                s.boot_system_received = true;
                s.system = Some(snap);
            });
        }
        ServerMsg::SubsystemStatus(snap) => {
            crate::state::with_mut(|s| {
                s.subsystems.insert(snap.name.clone(), snap);
            });
        }
        ServerMsg::TrackChanged(track) => {
            crate::state::with_mut(|s| s.track = Some(track));
        }
        ServerMsg::BluetoothEvent(event) => {
            crate::state::with_mut(|s| s.bt_devices.push(event));
        }
        ServerMsg::VolumeChanged { level, .. } => {
            crate::state::with_mut(|s| {
                s.master_volume = level;
            });
        }
        ServerMsg::NetworkChanged(state) => {
            crate::state::with_mut(|s| s.network = Some(state));
        }
        ServerMsg::LedStateChanged(anim) => {
            let color = crate::brand::led_dominant_color(&anim);
            crate::brand::update_dome_color(&color);
            crate::state::with_mut(|s| s.led = Some(anim));
        }
        ServerMsg::CrashReport(_crash) => {
            // Stored in crash log panel, not main state
        }
        ServerMsg::ConfigLoaded(config) => {
            crate::app::update_device_name(&config.device_name);
            crate::state::with_mut(|s| {
                s.config = Some(*config);
                s.config_dirty = false;
                s.boot_config_received = true;
            });
            crate::app::update_tab_visibility();
        }
        ServerMsg::LogEntries(entries) => {
            crate::pages::logs::append_entries(&entries);
        }
        ServerMsg::AudioLevels { left_rms, right_rms, left_peak, right_peak } => {
            crate::state::with_mut(|s| {
                s.audio_left_rms = left_rms;
                s.audio_right_rms = right_rms;
                s.audio_left_peak = left_peak;
                s.audio_right_peak = right_peak;
            });
        }
        ServerMsg::SpotifyStatus(status) => {
            crate::state::with_mut(|s| {
                // Also keep track in sync for backward compat
                s.track = status.track.clone();
                s.spotify_status = Some(status);
            });
        }
        ServerMsg::EqState(state) => {
            crate::state::with_mut(|s| s.eq_state = Some(state));
        }
        ServerMsg::DrcState(state) => {
            crate::state::with_mut(|s| s.drc_state = Some(state));
        }
        ServerMsg::DspInfo(info) => {
            crate::state::with_mut(|s| s.dsp_info = Some(info));
        }
        ServerMsg::DacRegValue { page, reg, value } => {
            crate::state::with_mut(|s| s.dac_reg_result = Some((page, reg, value)));
        }
        ServerMsg::DspSpiResponse { data } => {
            crate::state::with_mut(|s| s.dsp_spi_result = Some(data));
        }
        ServerMsg::DspMemoryDump { data, .. } => {
            crate::state::with_mut(|s| s.dsp_spi_result = Some(data));
        }
        ServerMsg::DspEvent { .. } => {
            // DSP event notification — logged via tracing, no state to store
        }
        ServerMsg::WifiConnectResult(result) => {
            let success = result.success;
            crate::state::with_mut(|s| s.wifi_connect_result = Some(result));
            // Notify setup wizard if active
            crate::state::with(|s| {
                if s.active_page == "setup" {
                    crate::pages::setup::on_wifi_result(success);
                }
            });
        }
        ServerMsg::TimeSynced { .. } => {
            // Time sync notification — no state to store, just log
        }
        ServerMsg::AudioSpectrum { bins } => {
            if bins.len() == 32 {
                let mut arr = [0.0f32; 32];
                arr.copy_from_slice(&bins);
                crate::state::with_mut(|s| s.audio_spectrum = Some(arr));
            }
        }
        ServerMsg::AudioWaveform { samples } => {
            crate::state::with_mut(|s| s.audio_waveform = Some(samples));
        }
        ServerMsg::GroupStatus(status) => {
            crate::state::with_mut(|s| {
                s.group_status = Some(status);
            });
        }
        ServerMsg::BootMode { safe_mode, boot_source } => {
            crate::state::with_mut(|s| {
                s.safe_mode = safe_mode;
                if !boot_source.is_empty() {
                    s.boot_source = boot_source;
                }
            });
            if safe_mode {
                let already_shown = crate::state::with(|s| s.safe_mode_modal_shown);
                if !already_shown {
                    crate::state::with_mut(|s| s.safe_mode_modal_shown = true);
                    crate::components::modal::Modal::alert(
                        "Safe Mode",
                        "A firmware update failed to start. Your speaker is running \
                         the last stable version. Upload new firmware via Update, \
                         or use Restart on the dashboard to clear safe mode.",
                        "OK",
                    );
                }
            }
        }
        ServerMsg::AudioPowerState { state } => {
            crate::state::with_mut(|s| s.audio_power_state = state);
        }
        ServerMsg::MicLevels { left_rms, right_rms, left_peak, right_peak } => {
            crate::state::with_mut(|s| {
                s.mic_left_rms = left_rms;
                s.mic_right_rms = right_rms;
                s.mic_left_peak = left_peak;
                s.mic_right_peak = right_peak;
            });
        }
    }

    // Notify only the relevant page(s) instead of always refreshing the
    // active page.  This prevents multi-Hz DOM thrashing (SystemStatus +
    // SpotifyStatus + SubsystemStatus = 3+ updates/sec) that causes
    // visible flickering and resets form inputs.
    let relevant_page: Option<&str> = match msg_tag {
        "SystemStatus" | "SubsystemStatus" => Some("dashboard"),
        "TrackChanged" | "SpotifyStatus" => Some("spotify"),
        "BluetoothEvent" => Some("bluetooth"),
        "NetworkChanged" => Some("network"),
        "GroupStatus" => Some("speakers"),
        "BootMode" => Some("dashboard"),
        "WifiConnectResult" => None, // network page + setup wizard both care
        "TimeSynced" => None,
        "LedStateChanged" => Some("lights"),
        "VolumeChanged" => Some("audio"),
        "AudioLevels" => Some("audio"),
        "EqState" | "DrcState" | "DspInfo" | "DacRegValue" | "DspSpiResponse"
        | "AudioSpectrum" | "AudioWaveform" | "AudioPowerState" | "MicLevels" => Some("audio"),
        "ConfigLoaded" => None,  // all pages may care about config
        "LogEntries" => Some("logs"),
        _ => None,
    };

    crate::state::with(|s| {
        let active = s.active_page.as_str();
        let should_update = match relevant_page {
            Some(page) => active == page,
            None => true,  // broadcast to whatever page is active
        };
        if should_update {
            crate::pages::update(active);
        }
    });
}
