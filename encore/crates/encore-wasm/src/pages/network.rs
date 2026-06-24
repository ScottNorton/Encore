//! Network page — WiFi status, available networks, VPN, interface stats.

use crate::components::modal::Modal;
use crate::dom;
use encore_common::protocol::{ClientMsg, NetworkState, WifiCredentials};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

pub fn render(container: &web_sys::Element) {
    // Server sends cached state on connect. Single fallback request
    // in case state changed since the WebSocket connected.
    crate::ws::send_msg(&ClientMsg::RequestNetworkState);

    // ── WiFi Status card ──
    let wifi_card = dom::create_div();
    dom::set_class(&wifi_card, "card");
    let wifi_title = dom::el("div", "card-title", Some("WiFi Status"));
    dom::append(&wifi_card, &wifi_title);
    let wifi_body = dom::create_div();
    wifi_body.set_id("wifi-info");
    dom::append(&wifi_card, &wifi_body);
    dom::append(container, &wifi_card);

    // ── Result Banner (hidden initially) ──
    let banner = dom::create_div();
    banner.set_id("wifi-result-banner");
    dom::set_class(&banner, "result-banner hidden");
    dom::append(container, &banner);

    // ── WiFi Configuration card ──
    let cfg_card = dom::create_div();
    dom::set_class(&cfg_card, "card");
    let cfg_title = dom::el("div", "card-title", Some("WiFi Configuration"));
    dom::append(&cfg_card, &cfg_title);

    let cfg_desc = dom::el(
        "div",
        "text-muted text-sm",
        Some("Configure the WiFi network and access point. Changes require a reboot."),
    );
    dom::set_style(&cfg_desc, "margin-bottom", "12px");
    dom::append(&cfg_card, &cfg_desc);

    // SSID input
    let ssid_label = dom::el("label", "stat-label", Some("WiFi SSID"));
    dom::set_style(&ssid_label, "display", "block");
    dom::set_style(&ssid_label, "margin-bottom", "4px");
    dom::append(&cfg_card, &ssid_label);
    let ssid_input = dom::create_el("input");
    ssid_input.set_id("net-cfg-ssid");
    dom::set_attr(&ssid_input, "type", "text");
    dom::set_attr(&ssid_input, "placeholder", "Network name");
    dom::set_class(&ssid_input, "input");
    dom::set_style(&ssid_input, "width", "100%");
    dom::set_style(&ssid_input, "margin-bottom", "12px");
    dom::append(&cfg_card, &ssid_input);

    // Password input
    let pass_label = dom::el("label", "stat-label", Some("WiFi Password"));
    dom::set_style(&pass_label, "display", "block");
    dom::set_style(&pass_label, "margin-bottom", "4px");
    dom::append(&cfg_card, &pass_label);
    let pass_input = dom::create_el("input");
    pass_input.set_id("net-cfg-password");
    dom::set_attr(&pass_input, "type", "password");
    dom::set_attr(&pass_input, "placeholder", "Password");
    dom::set_attr(&pass_input, "autocomplete", "off");
    dom::set_class(&pass_input, "input");
    dom::set_style(&pass_input, "width", "100%");
    dom::set_style(&pass_input, "margin-bottom", "16px");
    dom::append(&cfg_card, &pass_input);

    // AP keep-alive toggle
    let ap_row = dom::create_div();
    dom::set_class(&ap_row, "flex justify-between items-center");
    dom::set_style(&ap_row, "margin-bottom", "4px");
    let ap_label = dom::el("span", "stat-label", Some("Keep AP running"));
    dom::append(&ap_row, &ap_label);
    let ap_toggle = dom::create_el("input");
    ap_toggle.set_id("net-cfg-ap-keep");
    dom::set_attr(&ap_toggle, "type", "checkbox");
    dom::append(&ap_row, &ap_toggle);
    dom::append(&cfg_card, &ap_row);
    let ap_hint = dom::el(
        "div",
        "text-muted text-sm",
        Some("AP stays active alongside WiFi for recovery access. Disable to free the radio."),
    );
    dom::set_style(&ap_hint, "margin-bottom", "16px");
    dom::append(&cfg_card, &ap_hint);

    // Save button
    let save_btn = dom::el("button", "btn", Some("Save & Reboot"));
    save_btn.set_id("net-cfg-save");
    dom::on_click(&save_btn, save_network_config);
    dom::append(&cfg_card, &save_btn);

    // Save result feedback
    let save_result = dom::create_div();
    save_result.set_id("net-cfg-result");
    dom::set_style(&save_result, "margin-top", "8px");
    dom::append(&cfg_card, &save_result);

    dom::append(container, &cfg_card);

    // ── Available Networks card ──
    let scan_card = dom::create_div();
    dom::set_class(&scan_card, "card");

    let scan_header = dom::create_div();
    dom::set_class(&scan_header, "flex justify-between items-center mb-12");
    let scan_title = dom::el("span", "card-title", Some("Available Networks"));
    dom::set_style(&scan_title, "margin-bottom", "0");
    dom::append(&scan_header, &scan_title);

    let scan_btn = dom::el("button", "btn", Some("Scan"));
    scan_btn.set_id("wifi-scan-btn");
    dom::on_click(&scan_btn, scan_wifi);
    dom::append(&scan_header, &scan_btn);
    dom::append(&scan_card, &scan_header);

    let network_list = dom::create_div();
    network_list.set_id("wifi-networks");
    let placeholder = dom::el(
        "div",
        "text-muted text-center",
        Some("Tap Scan to search for networks"),
    );
    dom::append(&network_list, &placeholder);
    dom::append(&scan_card, &network_list);
    dom::append(container, &scan_card);

    // ── VPN Status card ──
    let vpn_card = dom::create_div();
    vpn_card.set_id("vpn-status-card");
    dom::set_class(&vpn_card, "card");
    let vpn_title = dom::el("div", "card-title", Some("VPN"));
    dom::append(&vpn_card, &vpn_title);
    let vpn_body = dom::create_div();
    vpn_body.set_id("vpn-info");
    dom::append(&vpn_card, &vpn_body);
    dom::append(container, &vpn_card);

    // ── Interface Stats card ──
    let iface_card = dom::create_div();
    dom::set_class(&iface_card, "card");
    let iface_title = dom::el("div", "card-title", Some("Interface Stats"));
    dom::append(&iface_card, &iface_title);
    let iface_body = dom::create_div();
    iface_body.set_id("iface-stats");
    dom::append(&iface_card, &iface_body);
    dom::append(container, &iface_card);

    // ── Developer (collapsed by default) ──
    // Dev-only subsystem holds, off the common WiFi path, so they start hidden.
    let dev_card = dom::create_div();
    dom::set_class(&dev_card, "card");
    let (dev_title, dev_body) = crate::components::collapsible::collapsible("Developer", false);
    dom::append(&dev_card, &dev_title);

    let subsystems = [
        "audio",
        "spotify",
        "bluetooth",
        "wyoming",
        "network",
        "homeassistant",
    ];
    for name in subsystems {
        let row = dom::create_div();
        dom::set_class(&row, "flex justify-between items-center mb-8");

        let label = dom::el("span", "stat-label", Some(name));
        dom::append(&row, &label);

        let select = dom::create_el("select");
        select.set_id(&format!("debug-mode-{}", name));
        dom::set_class(&select, "select-sm");

        for (mode_label, mode_val) in [("Production", "Production"), ("Hold", "Hold")] {
            let opt = dom::create_el("option");
            dom::set_attr(&opt, "value", mode_val);
            dom::set_text(&opt, mode_label);
            dom::append(&select, &opt);
        }

        let name_owned = name.to_string();
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(target) = e
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
            {
                let mode = match target.value().as_str() {
                    "Hold" => encore_common::protocol::DebugMode::Hold,
                    _ => encore_common::protocol::DebugMode::Production,
                };
                crate::ws::send_msg(&ClientMsg::SetDebugMode {
                    subsystem: name_owned.clone(),
                    mode,
                });
            }
        }) as Box<dyn FnMut(_)>);
        select
            .add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();

        dom::append(&row, &select);
        dom::append(&dev_body, &row);
    }
    dom::append(&dev_card, &dev_body);
    dom::append(container, &dev_card);

    update();
}

fn scan_wifi() {
    if let Some(btn) = dom::get_el("wifi-scan-btn") {
        dom::set_text(&btn, "Scanning...");
        dom::set_attr(&btn, "disabled", "true");
    }

    wasm_bindgen_futures::spawn_local(async {
        let origin = dom::api_origin();
        let url = format!("{}/api/wifi/scan", origin);

        match fetch_json(&url).await {
            Ok(text) => {
                if let Some(list_el) = dom::get_el("wifi-networks") {
                    dom::clear(&list_el);
                    if let Ok(networks) =
                        serde_json::from_str::<Vec<encore_common::protocol::WifiNetwork>>(&text)
                    {
                        if networks.is_empty() {
                            let empty =
                                dom::el("div", "text-muted text-center", Some("No networks found"));
                            dom::append(&list_el, &empty);
                        } else {
                            for net in &networks {
                                let row = dom::create_div();
                                dom::set_class(&row, "flex justify-between items-center mb-8");
                                dom::set_style(&row, "padding", "8px 0");
                                dom::set_style(&row, "border-bottom", "1px solid var(--border)");

                                let info = dom::create_div();
                                let ssid_text = if net.ssid.is_empty() {
                                    "(hidden)"
                                } else {
                                    &net.ssid
                                };
                                let ssid = dom::el("div", "", Some(ssid_text));
                                dom::set_style(&ssid, "font-weight", "500");
                                let band = if net.frequency_mhz >= 5000 {
                                    "5G"
                                } else {
                                    "2.4G"
                                };
                                let detail = dom::el(
                                    "div",
                                    "text-muted text-sm",
                                    Some(&format!(
                                        "{}  {}dBm  {}",
                                        band, net.signal_dbm, net.security
                                    )),
                                );
                                dom::append(&info, &ssid);
                                dom::append(&info, &detail);
                                dom::append(&row, &info);

                                let right = dom::create_div();
                                dom::set_class(&right, "flex items-center gap-8");

                                // Signal bars
                                let bars = signal_bars_el(net.signal_dbm);
                                dom::append(&right, &bars);

                                // Join button
                                if !net.ssid.is_empty() {
                                    let ssid_clone = net.ssid.clone();
                                    let freq = net.frequency_mhz;
                                    let connect_btn = dom::el("button", "btn", Some("Join"));
                                    dom::set_style(&connect_btn, "font-size", "11px");
                                    dom::set_style(&connect_btn, "padding", "4px 10px");
                                    dom::on_click(&connect_btn, move || {
                                        prompt_wifi_password(&ssid_clone, freq);
                                    });
                                    dom::append(&right, &connect_btn);
                                }
                                dom::append(&row, &right);
                                dom::append(&list_el, &row);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                if let Some(list_el) = dom::get_el("wifi-networks") {
                    dom::clear(&list_el);
                    let err = dom::el(
                        "div",
                        "text-muted text-center",
                        Some(&format!("Scan failed: {}", e)),
                    );
                    dom::append(&list_el, &err);
                }
            }
        }

        if let Some(btn) = dom::get_el("wifi-scan-btn") {
            dom::set_text(&btn, "Scan");
            let _ = btn.remove_attribute("disabled");
        }
    });
}

async fn fetch_json(url: &str) -> Result<String, String> {
    let window = dom::window();
    let resp_val = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: web_sys::Response = resp_val.unchecked_into();
    let text_val = JsFuture::from(resp.text().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;
    text_val.as_string().ok_or_else(|| "not a string".into())
}

fn save_network_config() {
    use wasm_bindgen::JsCast;

    let ssid_val = dom::get_el("net-cfg-ssid")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|i| i.value())
        .unwrap_or_default();
    let pass_val = dom::get_el("net-cfg-password")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|i| i.value())
        .unwrap_or_default();
    let ap_keep = dom::get_el("net-cfg-ap-keep")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|i| i.checked())
        .unwrap_or(true);

    // Build a full config from current state, overriding network fields
    let config = crate::state::with(|s| {
        let mut cfg = s.config.clone().unwrap_or_default();
        cfg.wifi_ssid = if ssid_val.is_empty() {
            None
        } else {
            Some(ssid_val.clone())
        };
        cfg.wifi_password = if pass_val.is_empty() {
            None
        } else {
            Some(pass_val)
        };
        cfg.ap_keep_alive = ap_keep;
        cfg
    });

    crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(config)));
    crate::components::toast::info("Saved. The speaker is rebooting\u{2026}");

    // If SSID changed, also send SetWifi to connect immediately
    if !ssid_val.is_empty() {
        let password = dom::get_el("net-cfg-password")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value())
            .unwrap_or_default();
        crate::ws::send_msg(&ClientMsg::SetWifi(WifiCredentials {
            ssid: ssid_val,
            password,
        }));
    }

    // Visual feedback
    if let Some(result_el) = dom::get_el("net-cfg-result") {
        dom::set_text(&result_el, "Config saved. Rebooting...");
        dom::set_style(&result_el, "color", "var(--green)");
    }
    if let Some(btn) = dom::get_el("net-cfg-save") {
        dom::set_attr(&btn, "disabled", "true");
        dom::set_text(&btn, "Saved");
    }
}

fn prompt_wifi_password(ssid: &str, target_freq: u32) {
    let ssid_owned = ssid.to_string();
    let target_is_5g = target_freq >= 5000;

    // Check if current AP is on a different band
    let ap_is_5g = crate::state::with(|s| match s.network.as_ref() {
        Some(NetworkState::ApMode {
            ap_frequency_mhz, ..
        }) => *ap_frequency_mhz >= 5000,
        Some(NetworkState::ConnectedWithAp {
            ap_frequency_mhz, ..
        }) => *ap_frequency_mhz >= 5000,
        _ => false,
    });

    let band_switch = target_is_5g != ap_is_5g && target_freq > 0;

    if band_switch {
        let ssid_for_warning = ssid_owned.clone();
        let band_label = if target_is_5g { "5 GHz" } else { "2.4 GHz" };
        Modal::confirm(
            &format!(
                "\"{}\" is on {}. The speaker's hotspot will briefly \
                 restart on the new band. You may need to reconnect to \
                 the speaker's WiFi to see the result.",
                ssid_for_warning, band_label
            ),
            "Continue",
            move || {
                show_password_modal(ssid_owned.clone());
            },
            || {},
        );
    } else {
        show_password_modal(ssid_owned);
    }
}

fn show_password_modal(ssid_owned: String) {
    let ssid_display = ssid_owned.clone();
    Modal::input(
        &format!("Join \"{}\"", ssid_display),
        "Password",
        "Join",
        move |password| {
            // Fill the config form with the selected network
            fill_config_input("net-cfg-ssid", &ssid_owned);
            fill_config_input("net-cfg-password", &password);

            crate::state::with_mut(|s| {
                s.wifi_connect_result = None;
            });
            crate::pages::update("network");

            crate::ws::send_msg(&ClientMsg::SetWifi(WifiCredentials {
                ssid: ssid_owned.clone(),
                password,
            }));
        },
        || {},
    );
}

/// Force-set an input field value (overrides user edits).
fn fill_config_input(id: &str, value: &str) {
    use wasm_bindgen::JsCast;
    if let Some(el) = dom::get_el(id) {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            input.set_value(value);
            dom::set_attr(&el, "data-populated", "1");
        }
    }
}

/// Create signal strength bars element (4 bars)
fn signal_bars_el(dbm: i16) -> web_sys::Element {
    let active_count = if dbm > -50 {
        4
    } else if dbm > -60 {
        3
    } else if dbm > -70 {
        2
    } else {
        1
    };
    let container = dom::create_div();
    dom::set_class(&container, "signal-bars");
    for i in 0..4 {
        let bar = dom::create_div();
        let height = format!("{}px", 4 + i * 3);
        dom::set_style(&bar, "height", &height);
        if i < active_count {
            dom::set_class(&bar, "signal-bar active");
        } else {
            dom::set_class(&bar, "signal-bar");
        }
        dom::append(&container, &bar);
    }
    container
}

pub fn update() {
    crate::state::with(|s| {
        // ── WiFi Status ──
        if let Some(el) = dom::get_el("wifi-info") {
            dom::clear(&el);
            match s.network.as_ref() {
                Some(NetworkState::Connected {
                    ssid,
                    ip,
                    signal,
                    hostname,
                    ..
                }) => {
                    stat_row(&el, "Status", "Connected");
                    wifi_info_rows(&el, ssid, ip, *signal, hostname);
                }
                Some(NetworkState::ConnectedWithAp {
                    ssid,
                    ip,
                    signal,
                    hostname,
                    ap_ssid,
                    ap_clients,
                    ..
                }) => {
                    stat_row(&el, "Status", "Connected + AP");
                    wifi_info_rows(&el, ssid, ip, *signal, hostname);
                    // AP info section
                    let sep = dom::create_div();
                    dom::set_style(&sep, "border-top", "1px solid var(--border)");
                    dom::set_style(&sep, "margin-top", "8px");
                    dom::set_style(&sep, "padding-top", "8px");
                    dom::append(&el, &sep);
                    stat_row(&el, "AP SSID", ap_ssid);
                    stat_row(&el, "AP Password", "ridiculous");
                    stat_row(&el, "AP Clients", &ap_clients.to_string());
                }
                Some(NetworkState::Connecting { ssid }) => {
                    let row = dom::create_div();
                    dom::set_class(&row, "flex items-center gap-8");
                    dom::set_style(&row, "padding", "8px 0");
                    let spinner = dom::create_div();
                    dom::set_class(&spinner, "spinner");
                    dom::append(&row, &spinner);
                    let text = dom::el("span", "", Some(&format!("Connecting to {}...", ssid)));
                    dom::append(&row, &text);
                    dom::append(&el, &row);
                }
                Some(NetworkState::Disconnected) => {
                    stat_row(&el, "Status", "Disconnected");
                }
                Some(NetworkState::ApMode { ssid, clients, .. }) => {
                    stat_row(&el, "Mode", "Access Point");
                    stat_row(&el, "SSID", ssid);
                    stat_row(&el, "Password", "ridiculous");
                    stat_row(&el, "Clients", &clients.to_string());
                }
                None => {
                    let loading = dom::create_div();
                    dom::set_class(&loading, "flex items-center gap-8");
                    dom::set_style(&loading, "padding", "8px 0");
                    let spinner = dom::create_div();
                    dom::set_class(&spinner, "spinner");
                    dom::append(&loading, &spinner);
                    let text = dom::el("span", "text-muted", Some("Loading..."));
                    dom::append(&loading, &text);
                    dom::append(&el, &loading);
                }
            }

            // Show configured SSID from config (below status)
            if let Some(ref cfg) = s.config {
                if let Some(ref configured_ssid) = cfg.wifi_ssid {
                    let configured = dom::el(
                        "div",
                        "text-muted text-sm",
                        Some(&format!("Configured: {}", configured_ssid)),
                    );
                    dom::set_style(&configured, "margin-top", "8px");
                    dom::set_style(&configured, "padding-top", "8px");
                    dom::set_style(&configured, "border-top", "1px solid var(--border)");
                    dom::append(&el, &configured);
                }
            }
        }

        // ── Result Banner ──
        if let Some(ref banner_el) = dom::get_el("wifi-result-banner") {
            if let Some(ref result) = s.wifi_connect_result {
                dom::remove_class(banner_el, "hidden");
                if result.success {
                    dom::set_class(banner_el, "result-banner success");
                    dom::set_text(banner_el, &format!("Connected to {}", result.ssid));
                } else {
                    dom::set_class(banner_el, "result-banner error");
                    let msg = result.error.as_deref().unwrap_or("Connection failed");
                    dom::set_text(
                        banner_el,
                        &format!("Failed to join {}: {}", result.ssid, msg),
                    );
                }
                // Auto-hide after 10s
                dom::set_timeout(
                    || {
                        if let Some(b) = dom::get_el("wifi-result-banner") {
                            dom::add_class(&b, "hidden");
                        }
                        crate::state::with_mut(|s| s.wifi_connect_result = None);
                    },
                    10_000,
                );
            } else {
                dom::add_class(banner_el, "hidden");
            }
        }

        // ── VPN Status ──
        if let Some(el) = dom::get_el("vpn-info") {
            dom::clear(&el);
            if let Some(ref cfg) = s.config {
                if cfg.vpn_enabled {
                    // Show subsystem state from 1Hz snapshots
                    if let Some(snap) = s.subsystems.get("vpn") {
                        let state_str = match snap.state {
                            encore_common::protocol::SubsystemState::Running => "Running",
                            encore_common::protocol::SubsystemState::Starting => "Starting",
                            encore_common::protocol::SubsystemState::Degraded => "Degraded",
                            encore_common::protocol::SubsystemState::Crashed => "Crashed",
                            encore_common::protocol::SubsystemState::Stopped => "Stopped",
                            encore_common::protocol::SubsystemState::Held => "Held",
                        };
                        let color = match snap.state {
                            encore_common::protocol::SubsystemState::Running => "var(--green)",
                            encore_common::protocol::SubsystemState::Degraded => "var(--orange)",
                            encore_common::protocol::SubsystemState::Crashed => "var(--red)",
                            _ => "var(--text-secondary)",
                        };
                        let row = dom::create_div();
                        dom::set_class(&row, "stat-row");
                        let lbl = dom::el("span", "stat-label", Some("Status"));
                        dom::append(&row, &lbl);
                        let val = dom::el("span", "stat-value", Some(state_str));
                        dom::set_style(&val, "color", color);
                        dom::append(&row, &val);
                        dom::append(&el, &row);
                    } else {
                        stat_row(&el, "Status", "Not started");
                    }
                    if let Some(ref endpoint) = cfg.vpn_peer_endpoint {
                        stat_row(&el, "Endpoint", endpoint);
                    }
                    if let Some(ref addr) = cfg.vpn_address {
                        stat_row(&el, "Address", addr);
                    }
                } else {
                    let muted = dom::el("div", "text-muted", Some("VPN not configured"));
                    dom::append(&el, &muted);
                }
            } else {
                let muted = dom::el("div", "text-muted", Some("Loading config..."));
                dom::append(&el, &muted);
            }
        }

        // ── Interface Stats ──
        if let Some(ref sys) = s.system {
            if let Some(el) = dom::get_el("iface-stats") {
                dom::clear(&el);
                for iface in &sys.net_interfaces {
                    let row = dom::create_div();
                    dom::set_class(&row, "mb-8");
                    let name = dom::el("div", "stat-label", Some(&iface.name));
                    dom::set_style(&name, "font-weight", "600");
                    dom::set_style(&name, "margin-bottom", "4px");
                    dom::append(&row, &name);

                    let stats = dom::create_div();
                    dom::set_class(&stats, "flex gap-16 text-sm text-muted");

                    let rx_mb = iface.rx_bytes as f64 / 1_048_576.0;
                    let tx_mb = iface.tx_bytes as f64 / 1_048_576.0;
                    let rx = dom::el("span", "", Some(&format!("\u{2193} {:.1} MB", rx_mb)));
                    let tx = dom::el("span", "", Some(&format!("\u{2191} {:.1} MB", tx_mb)));
                    dom::append(&stats, &rx);
                    dom::append(&stats, &tx);
                    dom::append(&row, &stats);
                    dom::append(&el, &row);
                }
            }
        }

        // ── Populate WiFi config form (once, when config first arrives) ──
        if let Some(ref cfg) = s.config {
            populate_config_field("net-cfg-ssid", cfg.wifi_ssid.as_deref().unwrap_or(""));
            populate_config_field(
                "net-cfg-password",
                cfg.wifi_password.as_deref().unwrap_or(""),
            );
            set_checkbox_from_config("net-cfg-ap-keep", cfg.ap_keep_alive);
        }
    });
}

/// Set an input field value only if it's currently empty (avoids overwriting user edits).
fn populate_config_field(id: &str, value: &str) {
    use wasm_bindgen::JsCast;
    if let Some(el) = dom::get_el(id) {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            // Only populate if the field hasn't been touched by the user
            if input.get_attribute("data-populated").is_none() {
                input.set_value(value);
                dom::set_attr(&el, "data-populated", "1");
            }
        }
    }
}

/// Set a checkbox from config (only on first load).
fn set_checkbox_from_config(id: &str, checked: bool) {
    use wasm_bindgen::JsCast;
    if let Some(el) = dom::get_el(id) {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            if input.get_attribute("data-populated").is_none() {
                input.set_checked(checked);
                dom::set_attr(&el, "data-populated", "1");
            }
        }
    }
}

/// Classify signal strength into a quality label.
fn signal_quality(dbm: i8) -> (&'static str, &'static str) {
    if dbm > -50 {
        ("Excellent", "var(--green)")
    } else if dbm > -65 {
        ("Good", "var(--green)")
    } else if dbm > -75 {
        ("Fair", "var(--orange)")
    } else {
        ("Poor", "var(--red)")
    }
}

/// Render WiFi info rows (SSID with signal bars, IP, signal + quality, hostname).
fn wifi_info_rows(parent: &web_sys::Element, ssid: &str, ip: &str, signal: i8, hostname: &str) {
    // SSID + signal bars
    let ssid_row = dom::create_div();
    dom::set_class(&ssid_row, "stat-row");
    let lbl = dom::el("span", "stat-label", Some("SSID"));
    dom::append(&ssid_row, &lbl);
    let right = dom::create_div();
    dom::set_class(&right, "flex items-center gap-8");
    let val = dom::el("span", "stat-value", Some(ssid));
    dom::append(&right, &val);
    let bars = signal_bars_el(signal as i16);
    dom::append(&right, &bars);
    dom::append(&ssid_row, &right);
    dom::append(parent, &ssid_row);

    stat_row(parent, "IP Address", ip);

    // Signal + quality label
    let (quality, color) = signal_quality(signal);
    let sig_row = dom::create_div();
    dom::set_class(&sig_row, "stat-row");
    let sig_lbl = dom::el("span", "stat-label", Some("Signal"));
    dom::append(&sig_row, &sig_lbl);
    let sig_right = dom::create_div();
    dom::set_class(&sig_right, "flex items-center gap-8");
    let sig_val = dom::el("span", "stat-value", Some(&format!("{} dBm", signal)));
    dom::append(&sig_right, &sig_val);
    let qual = dom::el("span", "text-sm", Some(quality));
    dom::set_style(&qual, "color", color);
    dom::append(&sig_right, &qual);
    dom::append(&sig_row, &sig_right);
    dom::append(parent, &sig_row);

    if !hostname.is_empty() {
        stat_row(parent, "Hostname", hostname);
    }
}

fn stat_row(parent: &web_sys::Element, label: &str, value: &str) {
    let row = dom::create_div();
    dom::set_class(&row, "stat-row");
    let lbl = dom::el("span", "stat-label", Some(label));
    let val = dom::el("span", "stat-value", Some(value));
    dom::append(&row, &lbl);
    dom::append(&row, &val);
    dom::append(parent, &row);
}
