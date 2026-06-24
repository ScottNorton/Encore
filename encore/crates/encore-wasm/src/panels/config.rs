//! Configuration editor panel.

use crate::components::TextField;
use crate::dom;
use encore_common::protocol::{ClientMsg, EncoreConfig};

pub fn render(container: &web_sys::Element) {
    // Request latest config
    crate::ws::send_msg(&ClientMsg::RequestConfig);

    let form = dom::create_div();
    form.set_id("config-form");

    // Build sections from current config (or defaults)
    let cfg = crate::state::with(|s| s.config.clone().unwrap_or_default());

    // Device section
    section(
        &form,
        "Device",
        &[TextField::create(
            "cfg-name",
            "Device Name",
            &cfg.device_name,
            "",
        )],
    );

    // Audio section
    section(
        &form,
        "Audio",
        &[
            field_range("cfg-master-vol", "Master Volume", cfg.master_volume),
            field_range("cfg-spotify-vol", "Spotify Volume", cfg.spotify_volume),
            field_range("cfg-bt-vol", "Bluetooth Volume", cfg.bluetooth_volume),
            field_range("cfg-tts-duck", "TTS Duck %", cfg.tts_duck_percent),
            field_range(
                "cfg-max-vol-limit",
                "Max Volume Limit %",
                reg_to_limit_pct(cfg.max_volume_reg),
            ),
            field_note(
                "\u{26A0} The volume limit caps how hard the amp drives the speaker. \
                 Raising it gets louder but risks driver or hearing damage — the \
                 default was calibrated by ear. A hard safety floor still applies.",
            ),
        ],
    );

    // Spotify section
    section(
        &form,
        "Spotify",
        &[field_toggle(
            "cfg-spotify-en",
            "Enabled",
            cfg.spotify_enabled,
        )],
    );

    // Bluetooth section
    section(
        &form,
        "Bluetooth",
        &[
            field_toggle("cfg-bt-en", "Enabled", cfg.bluetooth_enabled),
            field_toggle("cfg-bt-disc", "Discoverable", cfg.bluetooth_discoverable),
        ],
    );

    // Network section
    section(
        &form,
        "Network",
        &[
            TextField::create(
                "cfg-wifi-ssid",
                "WiFi SSID",
                cfg.wifi_ssid.as_deref().unwrap_or(""),
                "",
            ),
            TextField::password(
                "cfg-wifi-pass",
                "WiFi Password",
                cfg.wifi_password.as_deref().unwrap_or(""),
                "",
            ),
            field_toggle("cfg-ap-keep-alive", "Keep AP Alive", cfg.ap_keep_alive),
        ],
    );

    // VPN section
    section(
        &form,
        "VPN (WireGuard)",
        &[
            field_toggle("cfg-vpn-en", "Enabled", cfg.vpn_enabled),
            TextField::password(
                "cfg-vpn-key",
                "Private Key",
                cfg.vpn_private_key.as_deref().unwrap_or(""),
                "",
            ),
            TextField::create(
                "cfg-vpn-addr",
                "Address",
                cfg.vpn_address.as_deref().unwrap_or(""),
                "",
            ),
            TextField::create(
                "cfg-vpn-peer-pub",
                "Peer Public Key",
                cfg.vpn_peer_public_key.as_deref().unwrap_or(""),
                "",
            ),
            TextField::password(
                "cfg-vpn-peer-psk",
                "Peer Preshared Key",
                cfg.vpn_peer_preshared_key.as_deref().unwrap_or(""),
                "",
            ),
            TextField::create(
                "cfg-vpn-endpoint",
                "Peer Endpoint",
                cfg.vpn_peer_endpoint.as_deref().unwrap_or(""),
                "",
            ),
            TextField::create(
                "cfg-vpn-allowed",
                "Allowed IPs",
                cfg.vpn_peer_allowed_ips.as_deref().unwrap_or(""),
                "",
            ),
            TextField::create(
                "cfg-vpn-keepalive",
                "Keepalive (s)",
                &cfg.vpn_persistent_keepalive.to_string(),
                "",
            ),
        ],
    );

    // Debug section
    section(
        &form,
        "Debug",
        &[TextField::create(
            "cfg-debug-mode",
            "Mode",
            &cfg.debug_mode,
            "",
        )],
    );

    dom::append(container, &form);

    // Save button
    let save_wrap = dom::create_div();
    dom::set_class(&save_wrap, "mt-16");
    let save_btn = dom::el(
        "button",
        "btn btn-primary w-full",
        Some("Save Configuration"),
    );
    dom::on_click(&save_btn, || {
        save_config();
    });
    dom::append(&save_wrap, &save_btn);
    dom::append(container, &save_wrap);
}

fn section(parent: &web_sys::Element, title: &str, fields: &[web_sys::Element]) {
    let card = dom::create_div();
    dom::set_class(&card, "card mb-12");

    let heading = dom::el("div", "card-title", Some(title));
    dom::append(&card, &heading);

    for field in fields {
        dom::append(&card, field);
    }

    dom::append(parent, &card);
}

fn field_range(id: &str, label: &str, value: u8) -> web_sys::Element {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    let wrap = dom::create_div();
    dom::set_class(&wrap, "mb-12");
    let header = dom::create_div();
    dom::set_class(&header, "flex justify-between");
    let lbl = dom::el("label", "", Some(label));
    let val = dom::el("span", "stat-value text-sm", Some(&format!("{}%", value)));
    val.set_id(&format!("{}-val", id));
    dom::append(&header, &lbl);
    dom::append(&header, &val);
    dom::append(&wrap, &header);
    let input = dom::create_el("input");
    dom::set_attr(&input, "type", "range");
    dom::set_attr(&input, "min", "0");
    dom::set_attr(&input, "max", "100");
    dom::set_attr(&input, "value", &value.to_string());
    input.set_id(id);
    // Live-update display value on drag
    let val_id = format!("{}-val", id);
    let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
        if let Some(input) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            if let Some(el) = dom::get_el(&val_id) {
                dom::set_text(&el, &format!("{}%", input.value()));
            }
        }
    }) as Box<dyn FnMut(_)>);
    input
        .add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();
    dom::append(&wrap, &input);
    wrap
}

/// Map the DAC loudness-cap register to a friendly "max volume limit" percent.
/// The register is inverted (lower = louder): 0x18 (24) is the hard floor = 100%,
/// 0xA0 (160) is very quiet = 0%.
fn reg_to_limit_pct(reg: u8) -> u8 {
    let reg = reg.clamp(24, 160) as u32;
    (((160 - reg) * 100) / 136) as u8
}

/// Inverse of `reg_to_limit_pct`. The firmware re-clamps to its hard safety floor.
fn limit_pct_to_reg(pct: u8) -> u8 {
    let pct = pct.min(100) as u32;
    (160 - (pct * 136) / 100).clamp(24, 160) as u8
}

/// A small inline note/warning rendered under a field.
fn field_note(text: &str) -> web_sys::Element {
    dom::el("div", "text-sm mt-8", Some(text))
}

fn field_toggle(id: &str, label: &str, checked: bool) -> web_sys::Element {
    let wrap = dom::create_div();
    dom::set_class(&wrap, "toggle-wrap");
    let lbl = dom::el("span", "", Some(label));
    dom::append(&wrap, &lbl);
    // Use <label> so clicking anywhere on the toggle toggles the hidden checkbox
    let toggle = dom::create_el("label");
    dom::set_class(&toggle, "toggle");
    let input = dom::create_el("input");
    dom::set_attr(&input, "type", "checkbox");
    input.set_id(id);
    if checked {
        dom::set_attr(&input, "checked", "");
    }
    let track = dom::create_div();
    dom::set_class(&track, "toggle-track");
    let thumb = dom::create_div();
    dom::set_class(&thumb, "toggle-thumb");
    dom::append(&toggle, &input);
    dom::append(&toggle, &track);
    dom::append(&toggle, &thumb);
    dom::append(&wrap, &toggle);
    wrap
}

fn get_input_value(id: &str) -> String {
    use wasm_bindgen::JsCast;
    dom::get_el(id)
        .and_then(|el| el.dyn_ref::<web_sys::HtmlInputElement>().map(|i| i.value()))
        .unwrap_or_default()
}

fn get_checkbox(id: &str) -> bool {
    use wasm_bindgen::JsCast;
    dom::get_el(id)
        .and_then(|el| {
            el.dyn_ref::<web_sys::HtmlInputElement>()
                .map(|i| i.checked())
        })
        .unwrap_or(false)
}

fn save_config() {
    let config = EncoreConfig {
        device_name: get_input_value("cfg-name"),
        master_volume: get_input_value("cfg-master-vol").parse().unwrap_or(70),
        spotify_volume: get_input_value("cfg-spotify-vol").parse().unwrap_or(70),
        bluetooth_volume: get_input_value("cfg-bt-vol").parse().unwrap_or(70),
        tts_duck_percent: get_input_value("cfg-tts-duck").parse().unwrap_or(80),
        max_volume_reg: limit_pct_to_reg(
            get_input_value("cfg-max-vol-limit").parse().unwrap_or(77),
        ),
        volume_ring_step: crate::state::with(|s| {
            s.config.as_ref().map(|c| c.volume_ring_step).unwrap_or(2)
        }),
        spotify_enabled: get_checkbox("cfg-spotify-en"),
        spotify_bitrate: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.spotify_bitrate.clone())
                .unwrap_or_else(|| "320".into())
        }),
        spotify_gapless: crate::state::with(|s| {
            s.config.as_ref().map(|c| c.spotify_gapless).unwrap_or(true)
        }),
        spotify_normalisation: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.spotify_normalisation)
                .unwrap_or(false)
        }),
        spotify_normalisation_type: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.spotify_normalisation_type.clone())
                .unwrap_or_else(|| "auto".into())
        }),
        spotify_normalisation_pregain_db: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.spotify_normalisation_pregain_db)
                .unwrap_or(0.0)
        }),
        bluetooth_enabled: get_checkbox("cfg-bt-en"),
        bluetooth_discoverable: get_checkbox("cfg-bt-disc"),
        // Not in this form — preserve the live value so a config save can't clobber it.
        bluetooth_mesh_enabled: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.bluetooth_mesh_enabled)
                .unwrap_or(false)
        }),
        homeassistant_enabled: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.homeassistant_enabled)
                .unwrap_or(false)
        }),
        mqtt_host: crate::state::with(|s| s.config.as_ref().and_then(|c| c.mqtt_host.clone())),
        mqtt_port: crate::state::with(|s| s.config.as_ref().and_then(|c| c.mqtt_port)),
        mqtt_user: crate::state::with(|s| s.config.as_ref().and_then(|c| c.mqtt_user.clone())),
        mqtt_password: crate::state::with(|s| {
            s.config.as_ref().and_then(|c| c.mqtt_password.clone())
        }),
        wyoming_enabled: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.wyoming_enabled)
                .unwrap_or(false)
        }),
        wyoming_host: crate::state::with(|s| {
            s.config.as_ref().and_then(|c| c.wyoming_host.clone())
        }),
        wyoming_port: crate::state::with(|s| s.config.as_ref().and_then(|c| c.wyoming_port)),
        wifi_ssid: {
            let v = get_input_value("cfg-wifi-ssid");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        wifi_password: {
            let v = get_input_value("cfg-wifi-pass");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        ap_keep_alive: get_checkbox("cfg-ap-keep-alive"),
        ap_ssid: crate::state::with(|s| s.config.as_ref().and_then(|c| c.ap_ssid.clone())),
        ap_password: crate::state::with(|s| s.config.as_ref().and_then(|c| c.ap_password.clone())),
        vpn_enabled: get_checkbox("cfg-vpn-en"),
        vpn_private_key: {
            let v = get_input_value("cfg-vpn-key");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        vpn_address: {
            let v = get_input_value("cfg-vpn-addr");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        vpn_peer_public_key: {
            let v = get_input_value("cfg-vpn-peer-pub");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        vpn_peer_preshared_key: {
            let v = get_input_value("cfg-vpn-peer-psk");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        vpn_peer_endpoint: {
            let v = get_input_value("cfg-vpn-endpoint");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        vpn_peer_allowed_ips: {
            let v = get_input_value("cfg-vpn-allowed");
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        },
        vpn_persistent_keepalive: get_input_value("cfg-vpn-keepalive").parse().unwrap_or(25),
        group_enabled: crate::state::with(|s| {
            s.config.as_ref().map(|c| c.group_enabled).unwrap_or(false)
        }),
        group_name: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.group_name.clone())
                .unwrap_or_else(|| "Living Room".into())
        }),
        group_channel: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.group_channel.clone())
                .unwrap_or_else(|| "stereo".into())
        }),
        group_peers: crate::state::with(|s| {
            s.config
                .as_ref()
                .map(|c| c.group_peers.clone())
                .unwrap_or_default()
        }),
        debug_mode: get_input_value("cfg-debug-mode"),
    };

    crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(config)));
    crate::components::toast::success("Configuration saved");
}
