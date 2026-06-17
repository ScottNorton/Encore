//! Setup wizard — first-boot flow (WiFi + device name).

use crate::dom;
use encore_common::protocol::{ClientMsg, NetworkState, WifiCredentials};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

pub fn render(container: &web_sys::Element) {
    let step = crate::state::with(|s| s.setup_step);
    match step {
        0 => render_welcome(container),
        1 => render_wifi(container),
        2 => render_name(container),
        3 => render_trust_cert(container),
        4 => render_success(container),
        _ => render_welcome(container),
    }
}

fn render_welcome(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");
    dom::set_style(&card, "text-align", "center");
    dom::set_style(&card, "padding", "32px 24px");

    let heading = dom::el("div", "", Some("Welcome to Invoke"));
    dom::set_style(&heading, "font-size", "24px");
    dom::set_style(&heading, "font-weight", "700");
    dom::set_style(&heading, "margin-bottom", "16px");
    dom::append(&card, &heading);

    let desc = dom::el(
        "div",
        "text-muted",
        Some("Let's get your speaker connected to WiFi."),
    );
    dom::set_style(&desc, "margin-bottom", "24px");
    dom::append(&card, &desc);

    let btn = dom::el("button", "btn", Some("Scan for Networks"));
    dom::set_style(&btn, "font-size", "16px");
    dom::set_style(&btn, "padding", "12px 24px");
    dom::on_click(&btn, || {
        crate::state::with_mut(|s| s.setup_step = 1);
        if let Some(content) = dom::get_el("content") {
            dom::clear(&content);
            render(&content);
        }
    });
    dom::append(&card, &btn);
    dom::append(container, &card);
}

fn render_wifi(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    let heading = dom::el("div", "card-title", Some("Step 1: Connect to WiFi"));
    dom::append(&card, &heading);

    // Network list
    let network_list = dom::create_div();
    network_list.set_id("setup-wifi-networks");
    let loading = dom::create_div();
    dom::set_class(&loading, "flex items-center gap-8");
    dom::set_style(&loading, "padding", "16px 0");
    let spinner = dom::create_div();
    dom::set_class(&spinner, "spinner");
    dom::append(&loading, &spinner);
    let text = dom::el("span", "text-muted", Some("Scanning for networks..."));
    dom::append(&loading, &text);
    dom::append(&network_list, &loading);
    dom::append(&card, &network_list);

    // Password input area (hidden initially)
    let password_area = dom::create_div();
    password_area.set_id("setup-password-area");
    dom::set_style(&password_area, "display", "none");
    dom::append(&card, &password_area);

    // Connection status area
    let status_area = dom::create_div();
    status_area.set_id("setup-connect-status");
    dom::append(&card, &status_area);

    // Scan button
    let scan_btn = dom::el("button", "btn", Some("Rescan"));
    scan_btn.set_id("setup-scan-btn");
    dom::set_style(&scan_btn, "margin-top", "12px");
    dom::on_click(&scan_btn, scan_wifi_setup);
    dom::append(&card, &scan_btn);

    dom::append(container, &card);

    // Auto-scan on render
    scan_wifi_setup();
}

fn scan_wifi_setup() {
    if let Some(btn) = dom::get_el("setup-scan-btn") {
        dom::set_text(&btn, "Scanning...");
        dom::set_attr(&btn, "disabled", "true");
    }

    wasm_bindgen_futures::spawn_local(async {
        let origin = dom::api_origin();
        let url = format!("{}/api/wifi/scan", origin);

        match fetch_json(&url).await {
            Ok(text) => {
                if let Some(list_el) = dom::get_el("setup-wifi-networks") {
                    dom::clear(&list_el);
                    if let Ok(networks) =
                        serde_json::from_str::<Vec<encore_common::protocol::WifiNetwork>>(&text)
                    {
                        if networks.is_empty() {
                            let empty = dom::el(
                                "div",
                                "text-muted text-center",
                                Some("No networks found. Try again."),
                            );
                            dom::append(&list_el, &empty);
                        } else {
                            for net in &networks {
                                if net.ssid.is_empty() {
                                    continue;
                                }
                                let row = dom::create_div();
                                dom::set_class(&row, "flex justify-between items-center");
                                dom::set_style(&row, "padding", "10px 0");
                                dom::set_style(&row, "border-bottom", "1px solid var(--border)");
                                dom::set_style(&row, "cursor", "pointer");

                                let info = dom::create_div();
                                let ssid_el = dom::el("div", "", Some(&net.ssid));
                                dom::set_style(&ssid_el, "font-weight", "500");
                                let detail = dom::el(
                                    "div",
                                    "text-muted text-sm",
                                    Some(&format!("{}dBm  {}", net.signal_dbm, net.security)),
                                );
                                dom::append(&info, &ssid_el);
                                dom::append(&info, &detail);
                                dom::append(&row, &info);

                                let ssid_clone = net.ssid.clone();
                                let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
                                    show_password_input(&ssid_clone);
                                })
                                    as Box<dyn FnMut(_)>);
                                row.add_event_listener_with_callback(
                                    "click",
                                    cb.as_ref().unchecked_ref(),
                                )
                                .ok();
                                cb.forget();

                                dom::append(&list_el, &row);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                if let Some(list_el) = dom::get_el("setup-wifi-networks") {
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

        if let Some(btn) = dom::get_el("setup-scan-btn") {
            dom::set_text(&btn, "Rescan");
            let _ = btn.remove_attribute("disabled");
        }
    });
}

fn show_password_input(ssid: &str) {
    if let Some(area) = dom::get_el("setup-password-area") {
        dom::clear(&area);
        dom::set_style(&area, "display", "block");
        dom::set_style(&area, "margin-top", "16px");
        dom::set_style(&area, "padding", "16px");
        dom::set_style(&area, "background", "var(--card-bg)");
        dom::set_style(&area, "border-radius", "8px");
        dom::set_style(&area, "border", "1px solid var(--accent)");

        let label = dom::el("div", "", Some(&format!("Join \"{}\"", ssid)));
        dom::set_style(&label, "font-weight", "600");
        dom::set_style(&label, "margin-bottom", "8px");
        dom::append(&area, &label);

        let input = dom::create_el("input");
        input.set_id("setup-wifi-password");
        dom::set_attr(&input, "type", "password");
        dom::set_attr(
            &input,
            "placeholder",
            "Password (leave empty for open networks)",
        );
        dom::set_class(&input, "input");
        dom::set_style(&input, "width", "100%");
        dom::set_style(&input, "margin-bottom", "12px");
        dom::append(&area, &input);

        let btn_row = dom::create_div();
        dom::set_class(&btn_row, "flex gap-8");

        let connect_btn = dom::el("button", "btn", Some("Connect"));
        let ssid_owned = ssid.to_string();
        dom::on_click(&connect_btn, move || {
            let password = get_input_value("setup-wifi-password");
            connect_wifi_setup(&ssid_owned, &password);
        });
        dom::append(&btn_row, &connect_btn);

        let cancel_btn = dom::el("button", "btn", Some("Cancel"));
        dom::set_style(&cancel_btn, "background", "var(--bg)");
        dom::set_style(&cancel_btn, "color", "var(--text)");
        dom::on_click(&cancel_btn, || {
            if let Some(area) = dom::get_el("setup-password-area") {
                dom::set_style(&area, "display", "none");
            }
        });
        dom::append(&btn_row, &cancel_btn);

        dom::append(&area, &btn_row);
    }
}

fn connect_wifi_setup(ssid: &str, password: &str) {
    // Show connecting spinner
    if let Some(status) = dom::get_el("setup-connect-status") {
        dom::clear(&status);
        dom::set_style(&status, "margin-top", "16px");
        let row = dom::create_div();
        dom::set_class(&row, "flex items-center gap-8");
        let spinner = dom::create_div();
        dom::set_class(&spinner, "spinner");
        dom::append(&row, &spinner);
        let text = dom::el("span", "", Some(&format!("Connecting to {}...", ssid)));
        dom::append(&row, &text);
        dom::append(&status, &row);
    }

    // Hide password area
    if let Some(area) = dom::get_el("setup-password-area") {
        dom::set_style(&area, "display", "none");
    }

    // Send WiFi credentials
    crate::ws::send_msg(&ClientMsg::SetWifi(WifiCredentials {
        ssid: ssid.to_string(),
        password: password.to_string(),
    }));
}

fn render_name(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");
    dom::set_style(&card, "padding", "24px");

    let heading = dom::el("div", "card-title", Some("Step 2: Name Your Speaker"));
    dom::append(&card, &heading);

    let desc = dom::el(
        "div",
        "text-muted",
        Some("This name appears on your network as {name}.local"),
    );
    dom::set_style(&desc, "margin-bottom", "16px");
    dom::append(&card, &desc);

    let input = dom::create_el("input");
    input.set_id("setup-device-name");
    dom::set_attr(&input, "type", "text");
    dom::set_attr(&input, "value", "Encore");
    dom::set_attr(&input, "placeholder", "Device name");
    dom::set_class(&input, "input");
    dom::set_style(&input, "width", "100%");
    dom::set_style(&input, "margin-bottom", "16px");
    dom::append(&card, &input);

    let btn = dom::el("button", "btn", Some("Continue"));
    dom::set_style(&btn, "font-size", "16px");
    dom::set_style(&btn, "padding", "12px 24px");
    dom::on_click(&btn, || {
        crate::state::with_mut(|s| s.setup_step = 3);
        if let Some(content) = dom::get_el("content") {
            dom::clear(&content);
            render(&content);
        }
    });
    dom::append(&card, &btn);
    dom::append(container, &card);
}

fn render_trust_cert(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");
    dom::set_style(&card, "padding", "24px");

    let heading = dom::el("div", "card-title", Some("Step 3: Secure Connection"));
    dom::append(&card, &heading);

    // Explanation
    let desc = dom::el("div", "text-muted", Some(
        "Your speaker generated a unique security certificate. Install it to get a trusted HTTPS connection and enable the installable app."
    ));
    dom::set_style(&desc, "margin-bottom", "16px");
    dom::append(&card, &desc);

    // Download button
    let dl_link = dom::create_el("a");
    dom::set_class(&dl_link, "btn");
    dom::set_style(&dl_link, "display", "inline-block");
    dom::set_style(&dl_link, "text-align", "center");
    dom::set_style(&dl_link, "margin-bottom", "20px");
    dom::set_style(&dl_link, "font-size", "16px");
    dom::set_style(&dl_link, "padding", "12px 24px");
    dom::set_attr(&dl_link, "href", "/ca.crt");
    dom::set_attr(&dl_link, "download", "");
    dom::set_text(&dl_link, "Download Certificate");
    dom::append(&card, &dl_link);

    // Platform instructions
    let instr = dom::create_div();
    dom::set_style(&instr, "background", "var(--bg)");
    dom::set_style(&instr, "border-radius", "8px");
    dom::set_style(&instr, "padding", "16px");
    dom::set_style(&instr, "margin-bottom", "20px");
    dom::set_style(&instr, "font-size", "13px");
    dom::set_style(&instr, "line-height", "1.6");

    let instr_title = dom::el("div", "", Some("Installation Instructions"));
    dom::set_style(&instr_title, "font-weight", "600");
    dom::set_style(&instr_title, "margin-bottom", "12px");
    dom::append(&instr, &instr_title);

    cert_instruction(&instr, "Windows",
        "Open the file \u{2192} Install Certificate \u{2192} Local Machine \u{2192} Place in \"Trusted Root Certification Authorities\"");
    cert_instruction(&instr, "macOS",
        "Open the file \u{2192} Keychain Access opens \u{2192} select \"System\" keychain \u{2192} double-click cert \u{2192} Trust \u{2192} \"Always Trust\"");
    cert_instruction(&instr, "Android",
        "Settings \u{2192} Security \u{2192} Encryption & Credentials \u{2192} Install a certificate \u{2192} CA certificate");
    cert_instruction(&instr, "iOS",
        "Settings \u{2192} Profile Downloaded \u{2192} Install, then Settings \u{2192} General \u{2192} About \u{2192} Certificate Trust Settings \u{2192} enable");
    cert_instruction(
        &instr,
        "Linux (Chrome)",
        "chrome://settings/certificates \u{2192} Authorities \u{2192} Import",
    );

    dom::append(&card, &instr);

    // Button row
    let btn_row = dom::create_div();
    dom::set_class(&btn_row, "flex gap-8");

    let skip_btn = dom::el("button", "btn", Some("Skip for Now"));
    dom::set_style(&skip_btn, "background", "var(--bg)");
    dom::set_style(&skip_btn, "color", "var(--text)");
    dom::on_click(&skip_btn, || {
        crate::state::with_mut(|s| s.setup_step = 4);
        if let Some(content) = dom::get_el("content") {
            dom::clear(&content);
            render(&content);
        }
    });
    dom::append(&btn_row, &skip_btn);

    let done_btn = dom::el("button", "btn", Some("I've Installed It \u{2014} Continue"));
    dom::on_click(&done_btn, || {
        crate::state::with_mut(|s| s.setup_step = 4);
        if let Some(content) = dom::get_el("content") {
            dom::clear(&content);
            render(&content);
        }
    });
    dom::append(&btn_row, &done_btn);

    dom::append(&card, &btn_row);
    dom::append(container, &card);
}

fn cert_instruction(parent: &web_sys::Element, platform: &str, steps: &str) {
    let row = dom::create_div();
    dom::set_style(&row, "margin-bottom", "8px");
    let label = dom::el("span", "", Some(&format!("{}: ", platform)));
    dom::set_style(&label, "font-weight", "600");
    dom::append(&row, &label);
    let text = dom::el("span", "text-muted", Some(steps));
    dom::append(&row, &text);
    dom::append(parent, &row);
}

fn render_success(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");
    dom::set_style(&card, "text-align", "center");
    dom::set_style(&card, "padding", "32px 24px");

    // Checkmark
    let check = dom::el("div", "", Some("\u{2714}")); // checkmark
    dom::set_style(&check, "font-size", "48px");
    dom::set_style(&check, "color", "var(--green)");
    dom::set_style(&check, "margin-bottom", "16px");
    dom::append(&card, &check);

    let heading = dom::el("div", "", Some("Setup Complete!"));
    dom::set_style(&heading, "font-size", "20px");
    dom::set_style(&heading, "font-weight", "700");
    dom::set_style(&heading, "margin-bottom", "16px");
    dom::append(&card, &heading);

    // Show connection info
    let info = dom::create_div();
    dom::set_style(&info, "text-align", "left");
    dom::set_style(&info, "margin-bottom", "24px");

    crate::state::with(|s| {
        if let Some(
            NetworkState::Connected {
                ssid, ip, hostname, ..
            }
            | NetworkState::ConnectedWithAp {
                ssid, ip, hostname, ..
            },
        ) = &s.network
        {
            setup_info_row(&info, "WiFi", ssid);
            setup_info_row(&info, "IP Address", ip);
            if !hostname.is_empty() {
                setup_info_row(&info, "Hostname", hostname);
            }
        }
    });
    dom::append(&card, &info);

    // AP keep alive toggle
    let toggle_row = dom::create_div();
    dom::set_class(&toggle_row, "flex justify-between items-center");
    dom::set_style(&toggle_row, "margin-bottom", "8px");
    let toggle_label = dom::el("span", "", Some("Keep Access Point running"));
    dom::append(&toggle_row, &toggle_label);

    let toggle = dom::create_el("input");
    toggle.set_id("setup-ap-keep-alive");
    dom::set_attr(&toggle, "type", "checkbox");
    dom::set_attr(&toggle, "checked", "true");
    dom::append(&toggle_row, &toggle);
    dom::append(&card, &toggle_row);

    let toggle_desc = dom::el("div", "text-muted text-sm", Some(
        "The AP provides emergency access if WiFi fails. You can change this later in Network settings."
    ));
    dom::set_style(&toggle_desc, "margin-bottom", "24px");
    dom::append(&card, &toggle_desc);

    // Finish button
    let btn = dom::el("button", "btn", Some("Finish Setup"));
    dom::set_style(&btn, "font-size", "16px");
    dom::set_style(&btn, "padding", "12px 24px");
    dom::on_click(&btn, || {
        finish_setup();
    });
    dom::append(&card, &btn);
    dom::append(container, &card);
}

fn setup_info_row(parent: &web_sys::Element, label: &str, value: &str) {
    let row = dom::create_div();
    dom::set_class(&row, "stat-row");
    let lbl = dom::el("span", "stat-label", Some(label));
    let val = dom::el("span", "stat-value", Some(value));
    dom::append(&row, &lbl);
    dom::append(&row, &val);
    dom::append(parent, &row);
}

fn finish_setup() {
    let device_name = get_input_value("setup-device-name");
    let device_name = if device_name.is_empty() {
        "Encore".to_string()
    } else {
        device_name
    };
    let ap_keep_alive = get_checkbox("setup-ap-keep-alive");

    // POST /api/setup/complete
    wasm_bindgen_futures::spawn_local(async move {
        let window = dom::window();
        let origin = dom::api_origin();
        let url = format!("{}/api/setup/complete", origin);

        let body = serde_json::json!({
            "device_name": device_name,
            "ap_keep_alive": ap_keep_alive,
        });

        let opts = web_sys::RequestInit::new();
        opts.set_method("POST");
        opts.set_body(&JsValue::from_str(&body.to_string()));

        let headers = web_sys::Headers::new().unwrap();
        headers.set("Content-Type", "application/json").ok();
        opts.set_headers(&headers);

        let request = web_sys::Request::new_with_str_and_init(&url, &opts).unwrap();
        let _ = JsFuture::from(window.fetch_with_request(&request)).await;

        // Mark setup complete and navigate to dashboard
        crate::state::with_mut(|s| {
            s.setup_complete = true;
            s.setup_step = 0;
        });

        // Show tab bar again
        if let Some(nav) = dom::get_el("tab-nav-wrap") {
            dom::set_style(&nav, "display", "");
        }

        // Navigate to dashboard
        window.location().set_hash("dashboard").ok();
    });
}

fn get_input_value(id: &str) -> String {
    dom::get_el(id)
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_default()
}

fn get_checkbox(id: &str) -> bool {
    dom::get_el(id)
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|input| input.checked())
        .unwrap_or(true)
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

/// Called by the WebSocket dispatcher when a WifiConnectResult arrives during setup.
pub fn on_wifi_result(success: bool) {
    let step = crate::state::with(|s| s.setup_step);
    if step != 1 {
        return;
    }

    if success {
        // Move to name step
        crate::state::with_mut(|s| s.setup_step = 2);
        if let Some(content) = dom::get_el("content") {
            dom::clear(&content);
            render(&content);
        }
    } else {
        // Show error in status area
        if let Some(status) = dom::get_el("setup-connect-status") {
            dom::clear(&status);
            let err = dom::el(
                "div",
                "",
                Some("Connection failed. Try again or select another network."),
            );
            dom::set_style(&err, "color", "var(--red)");
            dom::set_style(&err, "margin-top", "12px");
            dom::append(&status, &err);
        }
    }
}

pub fn update() {
    // Re-render to reflect any state changes
}
