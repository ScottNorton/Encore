//! About panel — device info, version, SSH credentials.

use crate::components::stat_row;
use crate::dom;

pub fn render(container: &web_sys::Element) {
    // Wordmark — inline SVG Classical Badge (gold ring + "Encore" + subtitle)
    let wordmark = crate::brand::build_wordmark();
    dom::append(container, &wordmark);

    let card = dom::create_div();
    dom::set_class(&card, "card");

    let (boot_source, safe_mode) = crate::state::with(|s| (s.boot_source.clone(), s.safe_mode));
    let firmware_label = match boot_source.as_str() {
        "next" => format!("Encore v{} (OTA next)", encore_common::VERSION),
        "rootfs" if safe_mode => format!("Encore v{} (safe mode)", encore_common::VERSION),
        "rootfs" => format!("Encore v{} (rootfs)", encore_common::VERSION),
        "lsync" => format!("Encore v{}", encore_common::VERSION),
        "dev" => format!("Encore v{} (dev)", encore_common::VERSION),
        _ => format!("Encore v{}", encore_common::VERSION),
    };
    stat_row(&card, "Firmware", &firmware_label);
    stat_row(&card, "SoC", "Marvell BG2CDP (88DE3006)");
    stat_row(&card, "CPU", "Dual Cortex-A7 @ 1.3GHz");
    stat_row(&card, "RAM", "512 MB");
    stat_row(&card, "Kernel", "Linux 3.8.13");

    dom::append(container, &card);

    // SSH card
    let ssh_card = dom::create_div();
    dom::set_class(&ssh_card, "card mt-12");

    let ssh_title = dom::el("div", "card-title", Some("SSH Access"));
    dom::append(&ssh_card, &ssh_title);

    stat_row(&ssh_card, "User", "root");
    stat_row(&ssh_card, "Password", "ridiculous");
    stat_row(&ssh_card, "Port", "22 (Dropbear)");

    let note = dom::el("div", "text-sm text-muted mt-8", Some("Use sshpass from WSL — Dropbear rejects interactive password auth from some SSH clients."));
    dom::append(&ssh_card, &note);

    dom::append(container, &ssh_card);

    // TLS Certificate card
    let tls_card = dom::create_div();
    dom::set_class(&tls_card, "card mt-12");

    let tls_title = dom::el("div", "card-title", Some("TLS Certificate"));
    dom::append(&tls_card, &tls_title);

    let tls_desc = dom::el("div", "text-sm text-muted", Some("Install the CA certificate for a trusted HTTPS connection and installable PWA."));
    dom::set_style(&tls_desc, "margin-bottom", "12px");
    dom::append(&tls_card, &tls_desc);

    let dl_link = dom::create_el("a");
    dom::set_class(&dl_link, "btn");
    dom::set_style(&dl_link, "display", "inline-block");
    dom::set_style(&dl_link, "text-align", "center");
    dom::set_style(&dl_link, "margin-bottom", "12px");
    dom::set_attr(&dl_link, "href", "/ca.crt");
    dom::set_attr(&dl_link, "download", "");
    dom::set_text(&dl_link, "Download Certificate");
    dom::append(&tls_card, &dl_link);

    let tls_instr = dom::create_div();
    dom::set_style(&tls_instr, "font-size", "12px");
    dom::set_style(&tls_instr, "line-height", "1.5");
    dom::set_class(&tls_instr, "text-muted");
    dom::set_text(&tls_instr, "Windows: Install \u{2192} Local Machine \u{2192} Trusted Root  \u{b7}  \
macOS: Open \u{2192} System keychain \u{2192} Always Trust  \u{b7}  \
Android: Settings \u{2192} Security \u{2192} Install CA cert  \u{b7}  \
iOS: Install profile, then enable in Certificate Trust Settings  \u{b7}  \
Linux: chrome://settings/certificates \u{2192} Authorities \u{2192} Import");
    dom::append(&tls_card, &tls_instr);

    dom::append(container, &tls_card);

    // Project info
    let proj_card = dom::create_div();
    dom::set_class(&proj_card, "card mt-12");

    let proj_title = dom::el("div", "card-title", Some("Project"));
    dom::append(&proj_card, &proj_title);

    let desc = dom::el("div", "", Some("Harman Kardon Invoke Community Firmware"));
    dom::append(&proj_card, &desc);
    let desc2 = dom::el("div", "text-sm text-muted mt-8", Some("Open-source firmware replacement for the Harman Kardon Invoke smart speaker."));
    dom::append(&proj_card, &desc2);

    dom::append(container, &proj_card);

    // Cache management
    let cache_card = dom::create_div();
    dom::set_class(&cache_card, "card mt-12");

    let cache_title = dom::el("div", "card-title", Some("Cache"));
    dom::append(&cache_card, &cache_title);

    let cache_desc = dom::el("div", "text-sm text-muted", Some("Clear cached assets and reload. Useful after firmware updates."));
    dom::append(&cache_card, &cache_desc);

    let clear_btn = dom::el("button", "btn btn-danger w-full mt-12", Some("Clear Cache & Reload"));
    dom::on_click(&clear_btn, || {
        // Clear all caches, unregister service workers, then reload
        let _ = js_sys::eval(
            "caches.keys().then(ks=>Promise.all(ks.map(k=>caches.delete(k)))).then(()=>\
             navigator.serviceWorker.getRegistrations().then(rs=>Promise.all(rs.map(r=>r.unregister())))).then(()=>\
             location.reload())"
        );
    });
    dom::append(&cache_card, &clear_btn);

    dom::append(container, &cache_card);
}
