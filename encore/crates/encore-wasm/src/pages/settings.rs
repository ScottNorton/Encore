//! Settings hub. `#settings` shows a menu; deeper paths render a specific
//! configure/diagnose surface into the content area. Replaces the old
//! gear-menu slide-over panels with real, back-button-friendly hash routes.

use crate::dom;

struct Link {
    seg: &'static str,
    label: &'static str,
    desc: &'static str,
    experimental: bool,
}

const TOP: &[Link] = &[
    Link {
        seg: "network",
        label: "Network",
        desc: "WiFi, access point, and VPN",
        experimental: false,
    },
    Link {
        seg: "devices",
        label: "Devices",
        desc: "Bluetooth pairing",
        experimental: false,
    },
    Link {
        seg: "integrations",
        label: "Integrations",
        desc: "Home Assistant and voice",
        experimental: true,
    },
    Link {
        seg: "system",
        label: "System",
        desc: "Status, logs, updates, and device info",
        experimental: false,
    },
];

const SYSTEM: &[Link] = &[
    Link {
        seg: "status",
        label: "Status",
        desc: "Live CPU, memory, and network",
        experimental: false,
    },
    Link {
        seg: "health",
        label: "Health",
        desc: "Subsystem status and restart",
        experimental: false,
    },
    Link {
        seg: "logs",
        label: "Logs",
        desc: "Live system log stream",
        experimental: false,
    },
    Link {
        seg: "crashes",
        label: "Crash log",
        desc: "Recorded subsystem crashes",
        experimental: false,
    },
    Link {
        seg: "update",
        label: "Firmware update",
        desc: "Install a new firmware build",
        experimental: false,
    },
    Link {
        seg: "reboot",
        label: "Reboot",
        desc: "Restart the speaker",
        experimental: false,
    },
    Link {
        seg: "about",
        label: "About",
        desc: "Device, version, and SSH access",
        experimental: false,
    },
    Link {
        seg: "theme",
        label: "Appearance",
        desc: "Dark, light, or follow the device",
        experimental: false,
    },
    Link {
        seg: "advanced",
        label: "Advanced",
        desc: "Full configuration editor",
        experimental: false,
    },
];

/// Render the settings surface for `segments` (everything after "settings").
/// `[]` -> top menu, `["system"]` -> system submenu, `["network"]` -> Network.
pub fn render(segments: &[String], container: &web_sys::Element) {
    match segments.first().map(|s| s.as_str()) {
        None => {
            section_title(container, "Settings", None);
            render_menu(container, TOP, "settings");
        }
        Some("system") => render_system(&segments[1..], container),
        Some("network") => sub_page(
            container,
            "Network",
            "settings",
            crate::pages::network::render,
        ),
        Some("devices") => sub_page(
            container,
            "Devices",
            "settings",
            crate::pages::bluetooth::render,
        ),
        Some("integrations") => sub_page(
            container,
            "Integrations",
            "settings",
            crate::pages::assistant::render,
        ),
        Some(other) => not_found(container, other),
    }
}

/// Update hook for the live settings sub-surfaces (others are static forms).
pub fn update(segments: &[String]) {
    match segments.last().map(|s| s.as_str()) {
        Some("network") => crate::pages::network::update(),
        Some("devices") => crate::pages::bluetooth::update(),
        Some("integrations") => crate::pages::assistant::update(),
        Some("status") => crate::pages::dashboard::update(),
        _ => {}
    }
}

fn render_system(segments: &[String], container: &web_sys::Element) {
    let render_panel = |id: &'static str| move |c: &web_sys::Element| crate::panels::render(id, c);
    match segments.first().map(|s| s.as_str()) {
        None => {
            section_title(container, "System", Some("settings"));
            render_menu(container, SYSTEM, "settings/system");
        }
        Some("status") => sub_page(
            container,
            "Status",
            "settings/system",
            crate::pages::dashboard::render,
        ),
        Some("health") => sub_page(
            container,
            "Health",
            "settings/system",
            render_panel("health"),
        ),
        Some("logs") => sub_page(container, "Logs", "settings/system", render_panel("logs")),
        Some("crashes") => sub_page(
            container,
            "Crash log",
            "settings/system",
            render_panel("crashes"),
        ),
        Some("update") => sub_page(
            container,
            "Firmware update",
            "settings/system",
            render_panel("update"),
        ),
        Some("reboot") => sub_page(
            container,
            "Reboot",
            "settings/system",
            render_panel("reboot"),
        ),
        Some("about") => sub_page(container, "About", "settings/system", render_panel("about")),
        Some("theme") => sub_page(
            container,
            "Appearance",
            "settings/system",
            render_panel("theme"),
        ),
        Some("advanced") => sub_page(
            container,
            "Advanced",
            "settings/system",
            render_panel("config"),
        ),
        Some(other) => not_found(container, other),
    }
}

/// Render a back header for `title` (linking up to `back`), then the page body.
fn sub_page(
    container: &web_sys::Element,
    title: &str,
    back: &'static str,
    render_body: impl Fn(&web_sys::Element),
) {
    section_title(container, title, Some(back));
    let body = dom::el("div", "settings-body", None);
    render_body(&body);
    dom::append(container, &body);
}

/// A page title row. When `back` is set, the title is a tappable "‹ title".
fn section_title(container: &web_sys::Element, title: &str, back: Option<&'static str>) {
    let header = dom::el("div", "settings-header", None);
    match back {
        Some(target) => {
            let btn = dom::el(
                "button",
                "settings-back",
                Some(&format!("\u{2039} {}", title)),
            );
            let target = target.to_string();
            dom::on_click(&btn, move || {
                dom::window().location().set_hash(&target).ok();
            });
            dom::append(&header, &btn);
        }
        None => {
            let h = dom::el("div", "settings-title", Some(title));
            dom::append(&header, &h);
        }
    }
    dom::append(container, &header);
}

fn render_menu(container: &web_sys::Element, items: &[Link], base: &str) {
    let list = dom::el("div", "settings-list", None);
    for item in items {
        let row = dom::create_el("button");
        dom::set_class(&row, "settings-row");

        // Label + description stacked, so each row says what's inside.
        let main = dom::el("div", "settings-row-main", None);
        let topline = dom::el("div", "settings-row-topline", None);
        let label = dom::el("span", "settings-row-label", Some(item.label));
        dom::append(&topline, &label);
        if item.experimental {
            let badge = dom::el("span", "badge-experimental", Some("experimental"));
            dom::append(&topline, &badge);
        }
        dom::append(&main, &topline);
        if !item.desc.is_empty() {
            let desc = dom::el("div", "settings-row-desc", Some(item.desc));
            dom::append(&main, &desc);
        }
        dom::append(&row, &main);

        let chev = dom::el("span", "settings-row-chev", Some("\u{203a}"));
        dom::append(&row, &chev);
        let target = format!("{}/{}", base, item.seg);
        dom::on_click(&row, move || {
            dom::window().location().set_hash(&target).ok();
        });
        dom::append(&list, &row);
    }
    dom::append(container, &list);
}

fn not_found(container: &web_sys::Element, what: &str) {
    let el = crate::components::async_state::empty(&format!("Unknown settings page: {}", what));
    dom::append(container, &el);
}
