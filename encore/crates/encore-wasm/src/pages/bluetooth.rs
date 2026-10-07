//! Bluetooth page — connection status, paired devices, scanner.

use crate::dom;
use encore_common::protocol::{BtAction, BtConnectedDevice, BtEvent, ClientMsg};
use wasm_bindgen::JsCast;

/// Friendly label for a group role string.
fn role_label(role: &str) -> String {
    match role {
        "leader" => "Leader",
        "follower" => "Follower",
        "coordinator" => "Coordinator",
        "standalone" | "" => "Solo",
        other => other,
    }
    .to_string()
}

/// Current text in the "Visible as" name input.
fn name_input_value() -> String {
    dom::get_el("bt-name-input")
        .and_then(|el| el.dyn_ref::<web_sys::HtmlInputElement>().map(|i| i.value()))
        .unwrap_or_default()
}

pub fn render(container: &web_sys::Element) {
    // Pull current status immediately so the page doesn't flash an empty
    // "No device / No paired devices" state while waiting for the next tick.
    crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::RequestStatus));

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

    // Now-playing card (AVRCP metadata). Hidden until a track arrives.
    let np_card = dom::create_div();
    np_card.set_id("bt-nowplaying");
    dom::set_class(&np_card, "card");
    dom::set_style(&np_card, "display", "none");
    let np_title = dom::el("div", "card-title", Some("Now Playing"));
    dom::append(&np_card, &np_title);
    let np_track = dom::create_div();
    np_track.set_id("bt-np-title");
    dom::set_style(&np_track, "font-size", "1.1rem");
    dom::set_style(&np_track, "font-weight", "600");
    dom::append(&np_card, &np_track);
    let np_artist = dom::create_div();
    np_artist.set_id("bt-np-artist");
    dom::set_class(&np_artist, "text-muted");
    dom::append(&np_card, &np_artist);
    let np_album = dom::create_div();
    np_album.set_id("bt-np-album");
    dom::set_class(&np_album, "text-muted text-sm");
    dom::append(&np_card, &np_album);

    // Playback progress (position / duration).
    crate::components::nowplaying::render(&np_card, "bt-np");

    // Transport controls (AVRCP passthrough to the source).
    let np_controls = dom::create_div();
    dom::set_style(&np_controls, "margin-top", "12px");
    dom::set_style(&np_controls, "display", "flex");
    dom::set_style(&np_controls, "gap", "8px");
    let prev_btn = dom::el("button", "btn", Some("\u{23EE}")); // ⏮
    dom::set_attr(&prev_btn, "aria-label", "Previous track");
    dom::on_click(&prev_btn, || {
        crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Transport {
            key: "prev".into(),
        }));
    });
    dom::append(&np_controls, &prev_btn);
    let pp_btn = dom::el("button", "btn", Some("\u{25B6}")); // ▶
    pp_btn.set_id("bt-np-playpause");
    dom::set_attr(&pp_btn, "aria-label", "Play or pause");
    dom::on_click(&pp_btn, || {
        let playing = crate::state::with(|s| s.bt_status.as_ref().is_some_and(|st| st.playing));
        let key = if playing { "pause" } else { "play" };
        crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Transport {
            key: key.into(),
        }));
    });
    dom::append(&np_controls, &pp_btn);
    let next_btn = dom::el("button", "btn", Some("\u{23ED}")); // ⏭
    dom::set_attr(&next_btn, "aria-label", "Next track");
    dom::on_click(&next_btn, || {
        crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Transport {
            key: "next".into(),
        }));
    });
    dom::append(&np_controls, &next_btn);
    dom::append(&np_card, &np_controls);

    dom::append(container, &np_card);

    // Group activity — so the user can see how playback is spread across speakers.
    let group_card = dom::create_div();
    dom::set_class(&group_card, "card");
    let group_title = dom::el("div", "card-title", Some("Group"));
    dom::append(&group_card, &group_title);
    let group_body = dom::create_div();
    group_body.set_id("bt-group");
    dom::append(&group_card, &group_body);
    dom::append(container, &group_card);

    // "Visible as" — the Bluetooth name phones see when pairing.
    let name_card = dom::create_div();
    dom::set_class(&name_card, "card");
    let name_title = dom::el("div", "card-title", Some("Visible As"));
    dom::append(&name_card, &name_title);
    let name_hint = dom::el(
        "div",
        "text-muted text-sm mb-12",
        Some("The name this speaker shows on phones when pairing."),
    );
    name_hint.set_id("bt-name-hint");
    dom::append(&name_card, &name_hint);
    let name_row = dom::create_div();
    dom::set_class(&name_row, "flex justify-between items-center");
    dom::set_style(&name_row, "gap", "8px");
    let name_input = dom::create_el("input");
    name_input.set_id("bt-name-input");
    dom::set_attr(&name_input, "type", "text");
    dom::set_attr(&name_input, "maxlength", "32");
    dom::set_class(&name_input, "input");
    dom::set_style(&name_input, "flex", "1");
    dom::append(&name_row, &name_input);
    let name_save = dom::el("button", "btn", Some("Save"));
    name_save.set_id("bt-name-save");
    dom::on_click(&name_save, || {
        let name = name_input_value();
        let name = name.trim().to_string();
        if !name.is_empty() {
            crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::SetName { name }));
        }
    });
    dom::append(&name_row, &name_save);
    dom::append(&name_card, &name_row);
    dom::append(container, &name_card);

    // Paired devices card
    let paired_card = dom::create_div();
    dom::set_class(&paired_card, "card");
    let paired_title = dom::el("div", "card-title", Some("Paired Devices"));
    dom::append(&paired_card, &paired_title);
    let paired_list = dom::create_div();
    paired_list.set_id("bt-paired");
    dom::append(&paired_card, &paired_list);
    dom::append(container, &paired_card);

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

/// Update only the now-playing card + the "Visible as" field — the incremental
/// parts (set_text / set_value, no DOM rebuild, no click closures), safe to run
/// on every ~1Hz play-status / volume tick without leaking.
pub fn update_now_playing() {
    crate::state::with(|s| {
        // ── Now playing: show whenever a device is connected, so the transport
        // controls stay reachable even when the source sends no AVRCP metadata.
        // Title falls back to the device name (mirrors the Stage card). ──
        if let Some(card) = dom::get_el("bt-nowplaying") {
            let conn = s.bt_status.as_ref().and_then(|st| st.connected.as_ref());
            if let Some(dev) = conn {
                dom::set_style(&card, "display", "");
                let track = s.bt_track.as_ref();
                let title = match track {
                    Some(t) if !t.title.is_empty() => t.title.as_str(),
                    _ if !dev.name.is_empty() => dev.name.as_str(),
                    _ => "Bluetooth audio",
                };
                let artist = track.map(|t| t.artist.as_str()).unwrap_or("");
                let album = track.map(|t| t.album.as_str()).unwrap_or("");
                if let Some(el) = dom::get_el("bt-np-title") {
                    dom::set_text(&el, title);
                }
                if let Some(el) = dom::get_el("bt-np-artist") {
                    dom::set_text(&el, artist);
                    dom::set_style(&el, "display", if artist.is_empty() { "none" } else { "" });
                }
                if let Some(el) = dom::get_el("bt-np-album") {
                    dom::set_text(&el, album);
                    dom::set_style(&el, "display", if album.is_empty() { "none" } else { "" });
                }
                if let Some(el) = dom::get_el("bt-np-playpause") {
                    let playing = s.bt_status.as_ref().is_some_and(|st| st.playing);
                    dom::set_text(&el, if playing { "\u{23F8}" } else { "\u{25B6}" });
                }
                crate::components::nowplaying::update("bt-np", s.bt_playstatus.as_ref());
            } else {
                dom::set_style(&card, "display", "none");
            }
        }

        // Reflect the current "Visible as" name unless the user is editing it
        // (focused), so an external rename shows up without clobbering typing.
        // When meshed, `name` is the shared group name — make the field read-only
        // so a user can't unknowingly rename only this speaker.
        let meshed = s.bt_status.as_ref().is_some_and(|st| st.meshed);
        if let Some(el) = dom::get_el("bt-name-input") {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                let focused = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.active_element())
                    .is_some_and(|a| a.id() == "bt-name-input");
                if !focused {
                    let name = s
                        .bt_status
                        .as_ref()
                        .map(|st| st.name.clone())
                        .unwrap_or_default();
                    if !name.is_empty() && input.value() != name {
                        input.set_value(&name);
                    }
                }
                if meshed {
                    dom::set_attr(&el, "disabled", "true");
                } else {
                    el.remove_attribute("disabled").ok();
                }
            }
        }
        if let Some(save) = dom::get_el("bt-name-save") {
            if meshed {
                dom::set_attr(&save, "disabled", "true");
            } else {
                save.remove_attribute("disabled").ok();
            }
        }
        if let Some(hint) = dom::get_el("bt-name-hint") {
            dom::set_text(
                &hint,
                if meshed {
                    "Showing the group (mesh) name. Turn off Mesh Mode to rename this speaker."
                } else {
                    "The name this speaker shows on phones when pairing."
                },
            );
        }
    });
}

/// Full refresh: the now-playing card plus the connection / group / paired /
/// discovered list sections. The list sections clear+rebuild DOM and register
/// click closures, so this runs only for list-changing messages — never the
/// ~1Hz play-status / volume ticks (those route to update_now_playing).
pub fn update() {
    update_now_playing();
    crate::state::with(|s| {
        // ── Group activity: show how playback is spread across speakers ──
        if let Some(el) = dom::get_el("bt-group") {
            dom::clear(&el);
            let grouped = s
                .group_status
                .as_ref()
                .filter(|g| g.enabled && !g.peers.is_empty());
            match grouped {
                Some(g) => {
                    let total = g.peers.len() + 1; // other peers + this speaker
                    let name = if g.group_name.is_empty() {
                        "Group"
                    } else {
                        &g.group_name
                    };
                    let head = dom::el(
                        "div",
                        "",
                        Some(&format!(
                            "{} · {} speaker{}",
                            name,
                            total,
                            if total == 1 { "" } else { "s" }
                        )),
                    );
                    dom::set_style(&head, "font-weight", "600");
                    dom::append(&el, &head);
                    let sub = dom::el(
                        "div",
                        "text-muted text-sm mb-8",
                        Some(&format!("This speaker: {}", role_label(&g.role))),
                    );
                    dom::append(&el, &sub);

                    // If Bluetooth is streaming, note it's playing to the group.
                    let playing = s.bt_status.as_ref().map(|st| st.playing).unwrap_or(false);
                    if playing {
                        let codec = s
                            .bt_status
                            .as_ref()
                            .and_then(|st| st.connected.as_ref())
                            .map(|d| d.codec.clone())
                            .unwrap_or_default();
                        let txt = if codec.is_empty() {
                            "\u{25B6} Streaming Bluetooth across the group".to_string()
                        } else {
                            format!("\u{25B6} Streaming Bluetooth ({}) across the group", codec)
                        };
                        let np = dom::el("div", "text-sm mb-8", Some(&txt));
                        dom::append(&el, &np);
                    }

                    for p in &g.peers {
                        let row = dom::create_div();
                        dom::set_class(&row, "flex justify-between items-center mb-8");
                        let has_name = !p.name.is_empty();
                        let nm = dom::el(
                            "div",
                            "",
                            Some(if has_name {
                                p.name.as_str()
                            } else {
                                "Unknown device"
                            }),
                        );
                        dom::append(&row, &nm);
                        let role_txt = if p.connected {
                            role_label(&p.role)
                        } else {
                            format!("{} · offline", role_label(&p.role))
                        };
                        let status = if has_name {
                            role_txt
                        } else {
                            format!("{} · {}", p.address, role_txt)
                        };
                        let st = dom::el("div", "text-muted text-sm", Some(&status));
                        dom::append(&row, &st);
                        dom::append(&el, &row);
                    }
                }
                None => {
                    let solo = dom::el(
                        "div",
                        "text-muted",
                        Some("Solo — not in a speaker group. Group speakers on the Groups page."),
                    );
                    dom::append(&el, &solo);
                }
            }
        }

        // ── Connection status: prefer live BtStatus (codec + playing state);
        // fall back to the event log before the first status arrives. ──
        let connected: Option<BtConnectedDevice> = s
            .bt_status
            .as_ref()
            .and_then(|st| st.connected.clone())
            .or_else(|| {
                let mut name = None;
                let mut addr = None;
                for event in &s.bt_devices {
                    match event {
                        BtEvent::DeviceConnected { name: n, addr: a } => {
                            name = Some(n.clone());
                            addr = Some(a.clone());
                        }
                        BtEvent::DeviceDisconnected { addr: a } => {
                            if addr.as_deref() == Some(a.as_str()) {
                                name = None;
                                addr = None;
                            }
                        }
                        _ => {}
                    }
                }
                match (name, addr) {
                    (Some(n), Some(a)) => Some(BtConnectedDevice {
                        name: n,
                        addr: a,
                        codec: String::new(),
                    }),
                    _ => None,
                }
            });
        let playing = s.bt_status.as_ref().map(|st| st.playing).unwrap_or(false);
        let reconnecting = s.bt_status.as_ref().and_then(|st| st.reconnecting.clone());

        if let Some(el) = dom::get_el("bt-conn-status") {
            dom::clear(&el);
            if let Some(dev) = &connected {
                let row = dom::create_div();
                dom::set_class(&row, "flex justify-between items-center");

                let info = dom::create_div();
                let name_el = dom::el("div", "", Some(&dev.name));
                dom::set_style(&name_el, "font-weight", "600");
                dom::append(&info, &name_el);
                // The servo-held stream buffer: how far playback trails the
                // source. Visible only while streaming (0 when idle/paused).
                let latency_ms = s.bt_status.as_ref().map(|st| st.latency_ms).unwrap_or(0);
                let detail = if dev.codec.is_empty() {
                    "Connected".to_string()
                } else {
                    let mut d = format!(
                        "{} · {}",
                        dev.codec,
                        if playing { "Playing" } else { "Paused" }
                    );
                    if playing && latency_ms > 0 {
                        d.push_str(&format!(" · {latency_ms} ms buffer"));
                    }
                    d
                };
                let detail_el = dom::el("div", "text-muted text-sm", Some(&detail));
                dom::append(&info, &detail_el);
                dom::append(&row, &info);

                let addr_clone = dev.addr.clone();
                let dc_btn = dom::el("button", "btn btn-danger", Some("Disconnect"));
                dom::on_click(&dc_btn, move || {
                    crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Disconnect {
                        addr: addr_clone.clone(),
                    }));
                });
                dom::append(&row, &dc_btn);
                dom::append(&el, &row);
            } else if let Some(name) = &reconnecting {
                // Boot-time paging window: tell the user it's actively trying,
                // so the ~15s of silence doesn't read as a fault.
                dom::set_text(&el, &format!("Reconnecting to {name}\u{2026}"));
            } else {
                // Not connected: the paired list below carries the per-device
                // Reconnect buttons, so don't duplicate one here.
                dom::set_text(&el, "No device connected");
            }
        }

        // ── Paired devices: per-device reconnect / forget ──
        if let Some(list_el) = dom::get_el("bt-paired") {
            dom::clear(&list_el);
            let paired = s
                .bt_status
                .as_ref()
                .map(|st| st.paired.clone())
                .unwrap_or_default();
            let connected_addr = connected.as_ref().map(|d| d.addr.clone());

            if paired.is_empty() {
                let empty = dom::el("div", "text-muted text-center", Some("No paired devices."));
                dom::append(&list_el, &empty);
            } else {
                for dev in &paired {
                    // Prefer the speaker-persisted name, then a name seen in this
                    // session's event log, else fall back to the address.
                    let mut name = dev.name.clone();
                    if name.is_empty() {
                        for ev in &s.bt_devices {
                            if let BtEvent::DeviceConnected { name: n, addr: a } = ev {
                                if *a == dev.addr && !n.is_empty() {
                                    name = n.clone();
                                }
                            }
                        }
                    }
                    let has_name = !name.is_empty();
                    if !has_name {
                        name = "Unknown device".to_string();
                    }
                    let is_connected = connected_addr.as_deref() == Some(dev.addr.as_str());

                    let row = dom::create_div();
                    dom::set_class(&row, "flex justify-between items-center mb-8");
                    dom::set_style(&row, "padding", "8px 0");
                    dom::set_style(&row, "border-bottom", "1px solid var(--border)");

                    let info = dom::create_div();
                    let name_el = dom::el("div", "", Some(&name));
                    dom::set_style(&name_el, "font-weight", "500");
                    dom::append(&info, &name_el);
                    // Always show the address as the muted subtitle (the title is
                    // the name, or "Unknown device" when we have none).
                    let status = if is_connected { "Connected" } else { "Paired" };
                    let detail_text = format!("{} \u{00B7} {}", dev.addr, status);
                    let detail = dom::el("div", "text-muted text-sm", Some(&detail_text));
                    dom::append(&info, &detail);
                    dom::append(&row, &info);

                    let btns = dom::create_div();
                    dom::set_class(&btns, "flex");
                    dom::set_style(&btns, "gap", "8px");
                    if !is_connected {
                        let addr_c = dev.addr.clone();
                        let rc = dom::el("button", "btn", Some("Reconnect"));
                        dom::on_click(&rc, move || {
                            crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Connect {
                                addr: addr_c.clone(),
                            }));
                        });
                        dom::append(&btns, &rc);
                    }
                    let addr_f = dev.addr.clone();
                    let fg = dom::el("button", "btn btn-danger", Some("Forget"));
                    dom::on_click(&fg, move || {
                        crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Forget {
                            addr: addr_f.clone(),
                        }));
                    });
                    dom::append(&btns, &fg);
                    dom::append(&row, &btns);
                    dom::append(&list_el, &row);
                }
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
