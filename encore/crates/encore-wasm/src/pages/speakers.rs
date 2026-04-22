//! Groups page — multi-speaker group status, controls, and peer details.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use std::cell::RefCell;
use std::collections::HashSet;

use crate::components::segmented::{SegmentedControl, SegmentedMode};
use crate::components::toggle::Toggle;
use crate::components::text_field::TextField;
use crate::dom;
use encore_common::protocol::ClientMsg;

thread_local! {
    /// Track which peer cards are expanded (by peer_id) so updates don't collapse them.
    static EXPANDED_PEERS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub fn render(container: &web_sys::Element) {
    // Request fresh group status on page open
    crate::ws::send_msg(&ClientMsg::RequestGroupStatus);

    // ── This Speaker card ──
    let status_card = dom::create_div();
    dom::set_class(&status_card, "card");
    let title = dom::el("div", "card-title", Some("This Speaker"));
    dom::append(&status_card, &title);

    let status_body = dom::create_div();
    status_body.set_id("group-status");
    let placeholder = dom::el("div", "text-muted", Some("Loading group status..."));
    dom::append(&status_body, &placeholder);
    dom::append(&status_card, &status_body);
    dom::append(container, &status_card);

    // ── Controls card ──
    let ctrl_card = dom::create_div();
    dom::set_class(&ctrl_card, "card");
    let ctrl_title = dom::el("div", "card-title", Some("Controls"));
    dom::append(&ctrl_card, &ctrl_title);

    // Enable toggle
    let enable_on = crate::state::with(|s| {
        s.group_status.as_ref().map_or(false, |g| g.enabled)
    });
    let enable_toggle = Toggle::create("group-enable", "Group Mode", enable_on, |on| {
        crate::ws::send_msg(&ClientMsg::SetGroupEnabled(on));
    });
    dom::set_class(&enable_toggle, "toggle-wrap mb-12");
    dom::append(&ctrl_card, &enable_toggle);

    // Party mode toggle
    let party_on = crate::state::with(|s| {
        s.group_status.as_ref().map_or(false, |g| g.party_mode)
    });
    let party_toggle = Toggle::create("group-party", "Party Mode", party_on, |on| {
        crate::ws::send_msg(&ClientMsg::SetPartyMode(on));
    });
    dom::set_class(&party_toggle, "toggle-wrap mb-12");
    dom::append(&ctrl_card, &party_toggle);

    // Channel select
    let ch_row = dom::create_div();
    dom::set_class(&ch_row, "flex justify-between items-center mb-12");
    let ch_label = dom::el("span", "", Some("Channel"));
    dom::append(&ch_row, &ch_label);

    let current_ch = crate::state::with(|s| {
        s.group_status.as_ref().map_or("stereo".to_string(), |g| g.channel.clone())
    });
    let ch_seg = SegmentedControl::create(
        "group-ch",
        &[("stereo", "Stereo"), ("left", "Left"), ("right", "Right")],
        &[current_ch.as_str()],
        SegmentedMode::Single(Box::new(|val| {
            crate::ws::send_msg(&ClientMsg::SetGroupChannel(val.to_string()));
        })),
    );
    dom::append(&ch_row, &ch_seg);
    dom::append(&ctrl_card, &ch_row);

    // Group name field
    let current_name = crate::state::with(|s| {
        s.group_status.as_ref().map_or(String::new(), |g| g.group_name.clone())
    });
    let name_field = TextField::create("group-name", "Group Name", &current_name, "living-room");
    dom::append(&ctrl_card, &name_field);

    // Save group name button
    let save_btn = dom::el("button", "btn btn-sm", Some("Save Name"));
    dom::set_style(&save_btn, "margin-top", "4px");
    dom::on_click(&save_btn, || {
        if let Some(el) = dom::get_el("group-name") {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                let name = input.value();
                if !name.is_empty() {
                    crate::ws::send_msg(&ClientMsg::SetGroupName(name));
                }
            }
        }
    });
    dom::append(&ctrl_card, &save_btn);

    // Buffer depth slider
    let buf_ms = crate::state::with(|s| {
        s.group_status.as_ref().map_or(80, |g| g.buffer_ms)
    });
    let buf_row = dom::create_div();
    dom::set_class(&buf_row, "flex justify-between items-center mt-12");
    let buf_label = dom::el("span", "", Some("Buffer"));
    dom::append(&buf_row, &buf_label);

    let buf_right = dom::create_div();
    dom::set_class(&buf_right, "flex items-center gap-8");

    let buf_value = dom::el("span", "text-muted", Some(&format!("{} ms", buf_ms)));
    buf_value.set_id("group-buffer-value");
    dom::append(&buf_right, &buf_value);

    let buf_slider = dom::create_el("input");
    buf_slider.set_id("group-buffer-slider");
    dom::set_attr(&buf_slider, "type", "range");
    dom::set_attr(&buf_slider, "min", "20");
    dom::set_attr(&buf_slider, "max", "500");
    dom::set_attr(&buf_slider, "step", "10");
    dom::set_attr(&buf_slider, "value", &buf_ms.to_string());
    dom::set_class(&buf_slider, "slider");

    let buf_cb = Closure::wrap(Box::new(|e: web_sys::Event| {
        use wasm_bindgen::JsCast;
        if let Some(target) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            if let Ok(ms) = target.value().parse::<u16>() {
                if let Some(el) = dom::get_el("group-buffer-value") {
                    el.set_text_content(Some(&format!("{} ms", ms)));
                }
                crate::ws::send_msg(&ClientMsg::SetGroupBufferMs(ms));
            }
        }
    }) as Box<dyn FnMut(_)>);
    buf_slider.add_event_listener_with_callback("input", buf_cb.as_ref().unchecked_ref()).ok();
    buf_cb.forget();

    dom::append(&buf_right, &buf_slider);
    dom::append(&buf_row, &buf_right);
    dom::append(&ctrl_card, &buf_row);

    // Group volume slider
    let vol_row = dom::create_div();
    dom::set_class(&vol_row, "flex justify-between items-center mt-12");
    let vol_label = dom::el("span", "", Some("Volume"));
    dom::append(&vol_row, &vol_label);

    let vol_right = dom::create_div();
    dom::set_class(&vol_right, "flex items-center gap-8");

    let current_vol = crate::state::with(|s| {
        s.group_status.as_ref().map_or(70u8, |g| g.volume)
    });
    let vol_value = dom::el("span", "text-muted", Some(&format!("{}%", current_vol)));
    vol_value.set_id("group-vol-value");
    dom::append(&vol_right, &vol_value);

    let vol_slider = dom::create_el("input");
    vol_slider.set_id("group-vol-slider");
    dom::set_attr(&vol_slider, "type", "range");
    dom::set_attr(&vol_slider, "min", "0");
    dom::set_attr(&vol_slider, "max", "100");
    dom::set_attr(&vol_slider, "value", &current_vol.to_string());
    dom::set_class(&vol_slider, "slider");

    let vol_cb = Closure::wrap(Box::new(|e: web_sys::Event| {
        use wasm_bindgen::JsCast;
        if let Some(target) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            if let Ok(vol) = target.value().parse::<u8>() {
                if let Some(el) = dom::get_el("group-vol-value") {
                    el.set_text_content(Some(&format!("{}%", vol)));
                }
                crate::ws::send_msg(&ClientMsg::SetGroupVolume(vol));
            }
        }
    }) as Box<dyn FnMut(_)>);
    vol_slider.add_event_listener_with_callback("input", vol_cb.as_ref().unchecked_ref()).ok();
    vol_cb.forget();

    dom::append(&vol_right, &vol_slider);
    dom::append(&vol_row, &vol_right);
    dom::append(&ctrl_card, &vol_row);

    dom::append(container, &ctrl_card);

    // ── Bootstrap Peers card ──
    let boot_card = dom::create_div();
    dom::set_class(&boot_card, "card");
    let boot_title = dom::el("div", "card-title", Some("Bootstrap Peers"));
    dom::append(&boot_card, &boot_title);
    let boot_desc = dom::el("div", "text-muted text-sm", Some(
        "Add speaker IPs for cross-subnet discovery."
    ));
    dom::set_style(&boot_desc, "margin-bottom", "8px");
    dom::append(&boot_card, &boot_desc);

    let boot_list = dom::create_div();
    boot_list.set_id("bootstrap-peers");
    dom::append(&boot_card, &boot_list);

    // Add peer row
    let add_row = dom::create_div();
    dom::set_class(&add_row, "flex gap-8 mt-8");
    let peer_input = dom::create_el("input");
    peer_input.set_id("bootstrap-peer-input");
    dom::set_attr(&peer_input, "type", "text");
    dom::set_attr(&peer_input, "placeholder", "192.168.1.x");
    dom::set_class(&peer_input, "input");
    dom::set_style(&peer_input, "flex", "1");
    dom::append(&add_row, &peer_input);

    let add_btn = dom::el("button", "btn btn-sm", Some("Add"));
    dom::on_click(&add_btn, || add_bootstrap_peer());
    dom::append(&add_row, &add_btn);
    dom::append(&boot_card, &add_row);

    dom::append(container, &boot_card);

    // ── Connected Peers card ──
    let peers_card = dom::create_div();
    dom::set_class(&peers_card, "card");
    let peers_title = dom::el("div", "card-title", Some("Connected Peers"));
    dom::append(&peers_card, &peers_title);

    let peers_list = dom::create_div();
    peers_list.set_id("group-peers");
    let peers_placeholder = dom::el("div", "text-muted text-center", Some("No peers discovered"));
    dom::append(&peers_list, &peers_placeholder);
    dom::append(&peers_card, &peers_list);
    dom::append(container, &peers_card);

    update();
}

pub fn update() {
    crate::state::with(|state| {

    if let Some(status) = &state.group_status {
        // ── Update enable toggle ──
        if let Some(el) = dom::get_el("group-enable") {
            dom::toggle_set(&el, status.enabled);
        }

        // Update party mode toggle
        if let Some(el) = dom::get_el("group-party") {
            dom::toggle_set(&el, status.party_mode);
        }

        // Update volume slider + label
        if let Some(el) = dom::get_el("group-vol-slider") {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                input.set_value(&status.volume.to_string());
            }
        }
        if let Some(el) = dom::get_el("group-vol-value") {
            el.set_text_content(Some(&format!("{}%", status.volume)));
        }

        // ── Update buffer slider + label ──
        if let Some(el) = dom::get_el("group-buffer-slider") {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                input.set_value(&status.buffer_ms.to_string());
            }
        }
        if let Some(el) = dom::get_el("group-buffer-value") {
            el.set_text_content(Some(&format!("{} ms", status.buffer_ms)));
        }

        // ── Update channel segmented control ──
        if let Some(parent) = dom::get_el("group-ch") {
            let children = parent.children();
            for i in 0..children.length() {
                if let Some(child) = children.item(i) {
                    let btn_id = child.id();
                    let expected = format!("group-ch-{}", status.channel);
                    if btn_id == expected {
                        dom::set_class(&child, "seg-btn active");
                    } else {
                        dom::set_class(&child, "seg-btn");
                    }
                }
            }
        }

        // ── Update status display ──
        if let Some(container) = dom::get_el("group-status") {
            container.set_inner_html("");

            // Role badge row
            let role_row = dom::create_div();
            dom::set_class(&role_row, "flex items-center gap-8 mb-8");

            let (role_text, badge_cls) = match status.role.as_str() {
                "leader" => ("Leader", "badge badge-blue"),
                "follower" => ("Follower", "badge badge-green"),
                _ => ("Standalone", "badge badge-muted"),
            };
            let badge = dom::el("span", badge_cls, Some(role_text));
            dom::append(&role_row, &badge);

            if !status.group_name.is_empty() {
                let group_lbl = dom::el("span", "text-muted", Some(&format!("Group: {}", status.group_name)));
                dom::append(&role_row, &group_lbl);
            }
            dom::append(&container, &role_row);

            // Stats row
            let stats = dom::el("div", "text-muted text-sm", Some(&format!(
                "Channel: {} | Buffer: {} ms | {} peer(s)",
                status.channel, status.buffer_ms, status.peers.len()
            )));
            dom::append(&container, &stats);
        }

        // ── Update peer list ──
        if let Some(list) = dom::get_el("group-peers") {
            list.set_inner_html("");

            if status.peers.is_empty() {
                let empty = dom::el("div", "text-muted text-center", Some("No peers discovered"));
                dom::append(&list, &empty);
            } else {
                for peer in status.peers.iter() {
                    let card = dom::create_div();
                    dom::set_class(&card, "peer-card mb-8");
                    dom::set_style(&card, "border", "1px solid var(--border)");
                    dom::set_style(&card, "border-radius", "8px");
                    dom::set_style(&card, "overflow", "hidden");

                    // ── Summary row (always visible, clickable) ──
                    let summary = dom::create_div();
                    dom::set_class(&summary, "flex justify-between items-center p-8");
                    dom::set_style(&summary, "cursor", "pointer");

                    let left = dom::create_div();
                    dom::set_class(&left, "flex items-center gap-8");

                    // Health indicator dot
                    let dot_cls = if peer.instability_score < 10 {
                        "health-dot health-good"
                    } else if peer.instability_score < 50 {
                        "health-dot health-warn"
                    } else {
                        "health-dot health-bad"
                    };
                    let dot = dom::el("span", dot_cls, None);
                    dom::append(&left, &dot);

                    // Peer name (fall back to peer_id if name is empty)
                    let display_name = if peer.name.is_empty() {
                        &peer.peer_id[..8.min(peer.peer_id.len())]
                    } else {
                        &peer.name
                    };
                    let name = dom::el("span", "text-bold", Some(display_name));
                    dom::append(&left, &name);

                    // Role badge
                    let (peer_role, peer_badge) = match peer.role.as_str() {
                        "leader" => ("Leader", "badge badge-blue badge-sm"),
                        "follower" => ("Follower", "badge badge-green badge-sm"),
                        _ => ("Standalone", "badge badge-muted badge-sm"),
                    };
                    let rbadge = dom::el("span", peer_badge, Some(peer_role));
                    dom::append(&left, &rbadge);

                    dom::append(&summary, &left);

                    // RTT on the right side
                    let rtt_text = if peer.latency_us == 0 {
                        "\u{2014}".to_string() // em-dash for no data
                    } else if peer.latency_us.abs() < 1000 {
                        format!("{} \u{00B5}s", peer.latency_us)
                    } else {
                        format!("{:.1} ms", peer.latency_us as f64 / 1000.0)
                    };
                    let rtt = dom::el("span", "text-muted text-sm", Some(&rtt_text));
                    dom::append(&summary, &rtt);

                    // Click handler to toggle detail — keyed by peer_id for stability
                    let peer_id_key = peer.peer_id.clone();
                    let detail_id = format!("peer-detail-{}", peer.peer_id);
                    let detail_id_c = detail_id.clone();
                    dom::on_click(&summary, move || {
                        let peer_id_c = peer_id_key.clone();
                        if let Some(el) = dom::get_el(&detail_id_c) {
                            let cur = el.dyn_ref::<web_sys::HtmlElement>()
                                .and_then(|h| Some(h.style().get_property_value("display").unwrap_or_default()))
                                .unwrap_or_default();
                            if cur == "none" {
                                dom::set_style(&el, "display", "block");
                                EXPANDED_PEERS.with(|ep| ep.borrow_mut().insert(peer_id_c));
                            } else {
                                dom::set_style(&el, "display", "none");
                                EXPANDED_PEERS.with(|ep| ep.borrow_mut().remove(&peer_id_c));
                            }
                        }
                    });

                    dom::append(&card, &summary);

                    // ── Expandable detail section ──
                    let is_expanded = EXPANDED_PEERS.with(|ep| ep.borrow().contains(&peer.peer_id));
                    let detail = dom::create_div();
                    detail.set_id(&detail_id);
                    dom::set_class(&detail, "p-8");
                    dom::set_style(&detail, "display", if is_expanded { "block" } else { "none" });
                    dom::set_style(&detail, "border-top", "1px solid var(--border)");
                    dom::set_style(&detail, "background", "var(--bg-card-alt, rgba(255,255,255,0.02))");

                    // Detail grid
                    let grid = dom::create_div();
                    dom::set_class(&grid, "text-sm");
                    dom::set_style(&grid, "display", "grid");
                    dom::set_style(&grid, "grid-template-columns", "1fr 1fr");
                    dom::set_style(&grid, "gap", "4px 12px");

                    // Helper: add a label+value pair to the grid
                    fn add_detail(grid: &web_sys::Element, label: &str, value: &str) {
                        let l = dom::el("span", "text-muted", Some(label));
                        let v = dom::el("span", "", Some(value));
                        dom::append(grid, &l);
                        dom::append(grid, &v);
                    }

                    add_detail(&grid, "Address", &peer.address);
                    add_detail(&grid, "Channel", &peer.channel);
                    let rtt_detail = if peer.latency_us == 0 {
                        "No data".to_string()
                    } else {
                        format!("{} \u{00B5}s", peer.latency_us)
                    };
                    add_detail(&grid, "RTT", &rtt_detail);
                    add_detail(&grid, "Packet Loss", &format!("{:.1}%", peer.packet_loss_pct));
                    let offset_detail = if peer.clock_offset_us == 0 && peer.latency_us == 0 {
                        "Syncing...".to_string()
                    } else {
                        format!("{} \u{00B5}s", peer.clock_offset_us)
                    };
                    add_detail(&grid, "Clock Offset", &offset_detail);
                    add_detail(&grid, "Buffer Health", &format!("{}%", peer.buffer_health));
                    add_detail(&grid, "Hop Count", &format!("{}", peer.hop_count));
                    if peer.is_relay {
                        add_detail(&grid, "Relay", "Yes");
                    }
                    add_detail(&grid, "Instability", &format!("{}", peer.instability_score));

                    dom::append(&detail, &grid);
                    dom::append(&card, &detail);
                    dom::append(&list, &card);
                }
            }
        }
    }

    // ── Update bootstrap peers list ──
    if let Some(list_el) = dom::get_el("bootstrap-peers") {
        list_el.set_inner_html("");
        if let Some(ref cfg) = state.config {
            if cfg.group_peers.is_empty() {
                let empty = dom::el("div", "text-muted text-center text-sm", Some("None configured"));
                dom::append(&list_el, &empty);
            } else {
                for peer_ip in &cfg.group_peers {
                    let row = dom::create_div();
                    dom::set_class(&row, "flex justify-between items-center mb-4");
                    let ip_text = dom::el("span", "text-sm", Some(peer_ip));
                    dom::append(&row, &ip_text);
                    let ip_clone = peer_ip.clone();
                    let rm_btn = dom::el("button", "btn btn-sm", Some("\u{2715}"));
                    dom::set_style(&rm_btn, "padding", "2px 8px");
                    dom::set_style(&rm_btn, "font-size", "10px");
                    dom::on_click(&rm_btn, move || remove_bootstrap_peer(&ip_clone));
                    dom::append(&row, &rm_btn);
                    dom::append(&list_el, &row);
                }
            }
        }
    }

    });
}

fn add_bootstrap_peer() {
    let ip = dom::get_el("bootstrap-peer-input")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|i| i.value())
        .unwrap_or_default()
        .trim()
        .to_string();
    if ip.is_empty() { return; }

    crate::state::with_mut(|s| {
        if let Some(ref mut cfg) = s.config {
            if !cfg.group_peers.contains(&ip) {
                cfg.group_peers.push(ip);
            }
            crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(cfg.clone())));
        }
    });
    if let Some(el) = dom::get_el("bootstrap-peer-input") {
        if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            input.set_value("");
        }
    }
    crate::pages::update("speakers");
}

fn remove_bootstrap_peer(ip: &str) {
    let ip_owned = ip.to_string();
    crate::state::with_mut(|s| {
        if let Some(ref mut cfg) = s.config {
            cfg.group_peers.retain(|p| p != &ip_owned);
            crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(cfg.clone())));
        }
    });
    crate::pages::update("speakers");
}
