//! System health panel — subsystem status table.

use crate::dom;
use encore_common::protocol::ClientMsg;

pub fn render(container: &web_sys::Element) {
    let table = dom::create_div();
    table.set_id("health-table");
    dom::append(container, &table);
    refresh_health();
}

fn refresh_health() {
    crate::state::with(|s| {
        if let Some(el) = dom::get_el("health-table") {
            dom::clear(&el);

            if s.subsystems.is_empty() {
                let msg = dom::el("div", "text-muted text-center", Some("No subsystem data yet"));
                dom::append(&el, &msg);
                return;
            }

            let mut names: Vec<&String> = s.subsystems.keys().collect();
            names.sort();

            for name in names {
                if let Some(snap) = s.subsystems.get(name.as_str()) {
                    let row = dom::create_div();
                    dom::set_class(&row, "flex justify-between items-center mb-8");
                    dom::set_style(&row, "padding", "8px 0");
                    dom::set_style(&row, "border-bottom", "1px solid var(--border)");

                    let info = dom::create_div();
                    let name_el = dom::el("div", "", Some(&snap.name));
                    dom::set_style(&name_el, "font-weight", "600");
                    dom::append(&info, &name_el);

                    let state_class = match snap.state {
                        encore_common::protocol::SubsystemState::Running => "badge-running",
                        encore_common::protocol::SubsystemState::Crashed => "badge-crashed",
                        encore_common::protocol::SubsystemState::Degraded => "badge-degraded",
                        _ => "badge-stopped",
                    };
                    let badge = dom::el("span", &format!("badge {}", state_class), None);
                    let dot = dom::el("span", "badge-dot", None);
                    dom::append(&badge, &dot);
                    let state_text = dom::el("span", "", Some(&format!("{:?}", snap.state)));
                    dom::append(&badge, &state_text);
                    dom::append(&info, &badge);

                    let detail = dom::el("div", "text-muted text-sm",
                        Some(&format!("Restarts: {} | Msgs: {} | Up: {}s",
                            snap.restart_count, snap.msg_count, snap.uptime_secs)));
                    dom::append(&info, &detail);
                    dom::append(&row, &info);

                    // Restart button
                    let subsystem_name = snap.name.clone();
                    let restart_btn = dom::el("button", "btn", Some("Restart"));
                    dom::set_style(&restart_btn, "flex-shrink", "0");
                    dom::on_click(&restart_btn, move || {
                        crate::ws::send_msg(&ClientMsg::RestartSubsystem(subsystem_name.clone()));
                    });
                    dom::append(&row, &restart_btn);
                    dom::append(&el, &row);
                }
            }
        }
    });
}
