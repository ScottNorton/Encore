//! Bluetooth page — connection status, paired devices, scanner.

use crate::dom;
use encore_common::protocol::{BtAction, BtEvent, ClientMsg};

pub fn render(container: &web_sys::Element) {
    // Connection status card
    let conn_card = dom::create_div();
    dom::set_class(&conn_card, "card");

    let conn_title = dom::el("div", "card-title", Some("Connection"));
    dom::append(&conn_card, &conn_title);

    let conn_status = dom::create_div();
    conn_status.set_id("bt-conn-status");
    dom::set_class(&conn_status, "text-muted");
    dom::set_text(&conn_status, "No device connected");
    dom::append(&conn_card, &conn_status);

    dom::append(container, &conn_card);

    // Discovered devices card
    let scan_card = dom::create_div();
    dom::set_class(&scan_card, "card");

    let scan_header = dom::create_div();
    dom::set_class(&scan_header, "flex justify-between items-center mb-12");
    let scan_title = dom::el("span", "card-title", Some("Nearby Devices"));
    dom::set_style(&scan_title, "margin-bottom", "0");
    dom::append(&scan_header, &scan_title);

    let scan_btn = dom::el("button", "btn", Some("Scan"));
    scan_btn.set_id("bt-scan-btn");
    dom::on_click(&scan_btn, || {
        crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::StartDiscovery));
        if let Some(btn) = dom::get_el("bt-scan-btn") {
            dom::set_text(&btn, "Scanning...");
            dom::set_attr(&btn, "disabled", "true");
        }
        // Re-enable after 10 seconds
        crate::dom::set_timeout(
            || {
                if let Some(btn) = dom::get_el("bt-scan-btn") {
                    dom::set_text(&btn, "Scan");
                    let _ = btn.remove_attribute("disabled");
                }
            },
            10_000,
        );
    });
    dom::append(&scan_header, &scan_btn);
    dom::append(&scan_card, &scan_header);

    let device_list = dom::create_div();
    device_list.set_id("bt-devices");
    dom::append(&scan_card, &device_list);

    dom::append(container, &scan_card);

    update();
}

pub fn update() {
    crate::state::with(|s| {
        // Update connection status
        let mut connected_name = None;
        let mut connected_addr = None;
        for event in &s.bt_devices {
            match event {
                BtEvent::DeviceConnected { name, addr } => {
                    connected_name = Some(name.clone());
                    connected_addr = Some(addr.clone());
                }
                BtEvent::DeviceDisconnected { addr } => {
                    if connected_addr.as_deref() == Some(addr) {
                        connected_name = None;
                        connected_addr = None;
                    }
                }
                _ => {}
            }
        }

        if let Some(el) = dom::get_el("bt-conn-status") {
            dom::clear(&el);
            if let Some(name) = &connected_name {
                let row = dom::create_div();
                dom::set_class(&row, "flex justify-between items-center");
                let info = dom::el("span", "", Some(name));
                dom::set_style(&info, "font-weight", "600");
                dom::append(&row, &info);

                if let Some(addr) = &connected_addr {
                    let addr_clone = addr.clone();
                    let dc_btn = dom::el("button", "btn btn-danger", Some("Disconnect"));
                    dom::on_click(&dc_btn, move || {
                        crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Disconnect {
                            addr: addr_clone.clone(),
                        }));
                    });
                    dom::append(&row, &dc_btn);
                }
                dom::append(&el, &row);
            } else {
                dom::set_text(&el, "No device connected");
            }
        }

        // Update discovered devices list
        if let Some(list_el) = dom::get_el("bt-devices") {
            dom::clear(&list_el);
            let mut discoveries: Vec<(&str, &str, i16)> = Vec::new();
            for event in &s.bt_devices {
                if let BtEvent::DiscoveryResult { name, addr, rssi } = event {
                    // Deduplicate by addr, keep latest
                    discoveries.retain(|(_, a, _)| *a != addr.as_str());
                    discoveries.push((name, addr, *rssi));
                }
            }

            if discoveries.is_empty() {
                let empty = dom::el(
                    "div",
                    "text-muted text-center",
                    Some("No devices found. Tap Scan to search."),
                );
                dom::append(&list_el, &empty);
            } else {
                for (name, addr, rssi) in &discoveries {
                    let row = dom::create_div();
                    dom::set_class(&row, "flex justify-between items-center mb-8");
                    dom::set_style(&row, "padding", "8px 0");
                    dom::set_style(&row, "border-bottom", "1px solid var(--border)");

                    let info = dom::create_div();
                    let name_el = dom::el(
                        "div",
                        "",
                        Some(if name.is_empty() { "Unknown" } else { name }),
                    );
                    dom::set_style(&name_el, "font-weight", "500");
                    let detail = dom::el(
                        "div",
                        "text-muted text-sm",
                        Some(&format!("{} ({}dBm)", addr, rssi)),
                    );
                    dom::append(&info, &name_el);
                    dom::append(&info, &detail);
                    dom::append(&row, &info);

                    let addr_str = addr.to_string();
                    let pair_btn = dom::el("button", "btn", Some("Pair"));
                    dom::on_click(&pair_btn, move || {
                        crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Pair {
                            addr: addr_str.clone(),
                        }));
                    });
                    dom::append(&row, &pair_btn);
                    dom::append(&list_el, &row);
                }
            }
        }
    });
}
