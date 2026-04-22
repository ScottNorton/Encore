//! Assistant page — Wyoming Voice + Home Assistant config.

use crate::components::TextField;
use crate::dom;
use encore_common::protocol::ClientMsg;
use wasm_bindgen::JsCast;

pub fn render(container: &web_sys::Element) {
    // Request latest config
    crate::ws::send_msg(&ClientMsg::RequestConfig);

    let cfg = crate::state::with(|s| s.config.clone().unwrap_or_default());

    // ── Wyoming Voice Satellite card ──
    let wy_card = dom::create_div();
    dom::set_class(&wy_card, "card");
    let wy_title = dom::el("div", "card-title", Some("Wyoming Voice Satellite"));
    dom::append(&wy_card, &wy_title);

    let wy_desc = dom::el("div", "text-muted text-sm", Some(
        "Connect to a Wyoming voice server for wake word detection and voice commands."
    ));
    dom::set_style(&wy_desc, "margin-bottom", "12px");
    dom::append(&wy_card, &wy_desc);

    dom::append(&wy_card, &field_toggle("ast-wy-en", "Enabled", cfg.wyoming_enabled));
    dom::append(&wy_card, &TextField::create(
        "ast-wy-host", "Server Host",
        cfg.wyoming_host.as_deref().unwrap_or(""), "homeassistant.local",
    ));
    dom::append(&wy_card, &TextField::create(
        "ast-wy-port", "Server Port",
        &cfg.wyoming_port.map(|p| p.to_string()).unwrap_or_default(), "10300",
    ));

    let wy_save = dom::el("button", "btn btn-primary w-full mt-12", Some("Save"));
    dom::on_click(&wy_save, || save_assistant_config());
    dom::append(&wy_card, &wy_save);

    dom::append(container, &wy_card);

    // ── Home Assistant (MQTT) card ──
    let ha_card = dom::create_div();
    dom::set_class(&ha_card, "card");
    let ha_title = dom::el("div", "card-title", Some("Home Assistant (MQTT)"));
    dom::append(&ha_card, &ha_title);

    let ha_desc = dom::el("div", "text-muted text-sm", Some(
        "Connect to Home Assistant via MQTT for device control and automation."
    ));
    dom::set_style(&ha_desc, "margin-bottom", "12px");
    dom::append(&ha_card, &ha_desc);

    dom::append(&ha_card, &field_toggle("ast-ha-en", "Enabled", cfg.homeassistant_enabled));
    dom::append(&ha_card, &TextField::create(
        "ast-mqtt-host", "MQTT Host",
        cfg.mqtt_host.as_deref().unwrap_or(""), "homeassistant.local",
    ));
    dom::append(&ha_card, &TextField::create(
        "ast-mqtt-port", "MQTT Port",
        &cfg.mqtt_port.map(|p| p.to_string()).unwrap_or_default(), "1883",
    ));
    dom::append(&ha_card, &TextField::create(
        "ast-mqtt-user", "Username",
        cfg.mqtt_user.as_deref().unwrap_or(""), "",
    ));
    dom::append(&ha_card, &TextField::create(
        "ast-mqtt-pass", "Password",
        cfg.mqtt_password.as_deref().unwrap_or(""), "",
    ));

    let ha_save = dom::el("button", "btn btn-primary w-full mt-12", Some("Save"));
    dom::on_click(&ha_save, || save_assistant_config());
    dom::append(&ha_card, &ha_save);

    dom::append(container, &ha_card);
}

pub fn update() {
    crate::state::with(|s| {
        if let Some(ref cfg) = s.config {
            populate_field("ast-wy-host", cfg.wyoming_host.as_deref().unwrap_or(""));
            populate_field("ast-wy-port", &cfg.wyoming_port.map(|p| p.to_string()).unwrap_or_default());
            set_checkbox("ast-wy-en", cfg.wyoming_enabled);
            populate_field("ast-mqtt-host", cfg.mqtt_host.as_deref().unwrap_or(""));
            populate_field("ast-mqtt-port", &cfg.mqtt_port.map(|p| p.to_string()).unwrap_or_default());
            populate_field("ast-mqtt-user", cfg.mqtt_user.as_deref().unwrap_or(""));
            populate_field("ast-mqtt-pass", cfg.mqtt_password.as_deref().unwrap_or(""));
            set_checkbox("ast-ha-en", cfg.homeassistant_enabled);
        }
    });
}

fn field_toggle(id: &str, label: &str, checked: bool) -> web_sys::Element {
    let wrap = dom::create_div();
    dom::set_class(&wrap, "toggle-wrap");
    let lbl = dom::el("span", "", Some(label));
    dom::append(&wrap, &lbl);
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
    dom::get_el(id)
        .and_then(|el| el.dyn_ref::<web_sys::HtmlInputElement>().map(|i| i.value()))
        .unwrap_or_default()
}

fn get_checkbox(id: &str) -> bool {
    dom::get_el(id)
        .and_then(|el| el.dyn_ref::<web_sys::HtmlInputElement>().map(|i| i.checked()))
        .unwrap_or(false)
}

fn opt_input(id: &str) -> Option<String> {
    let v = get_input_value(id);
    if v.is_empty() { None } else { Some(v) }
}

fn populate_field(id: &str, value: &str) {
    if let Some(el) = dom::get_el(id) {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            if input.get_attribute("data-populated").is_none() {
                input.set_value(value);
                dom::set_attr(&el, "data-populated", "1");
            }
        }
    }
}

fn set_checkbox(id: &str, checked: bool) {
    if let Some(el) = dom::get_el(id) {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            if input.get_attribute("data-populated").is_none() {
                input.set_checked(checked);
                dom::set_attr(&el, "data-populated", "1");
            }
        }
    }
}

fn save_assistant_config() {
    let config = crate::state::with(|s| {
        let mut cfg = s.config.clone().unwrap_or_default();
        cfg.wyoming_enabled = get_checkbox("ast-wy-en");
        cfg.wyoming_host = opt_input("ast-wy-host");
        cfg.wyoming_port = get_input_value("ast-wy-port").parse().ok();
        cfg.homeassistant_enabled = get_checkbox("ast-ha-en");
        cfg.mqtt_host = opt_input("ast-mqtt-host");
        cfg.mqtt_port = get_input_value("ast-mqtt-port").parse().ok();
        cfg.mqtt_user = opt_input("ast-mqtt-user");
        cfg.mqtt_password = opt_input("ast-mqtt-pass");
        cfg
    });
    crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(config)));
    web_sys::console::log_1(&"Assistant config saved".into());
}
