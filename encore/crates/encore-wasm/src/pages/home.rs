//! Home — the landing surface. A device-glance strip plus the now-playing card
//! (reused from the Spotify page; the standalone Spotify tab folds into Home).

use crate::dom;
use encore_common::protocol::{BtAction, ClientMsg, LedAnimation};

pub fn render(container: &web_sys::Element) {
    // Pull current BT status so the Stage BT card resolves immediately.
    crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::RequestStatus));

    // Single centered theatre column (CSS .stage caps width on desktop).
    let stage = dom::el("div", "stage", None);
    stage.set_id("stage");

    // ── Source Tuner: backlit dial, active source derived from playback. ──
    // Indicator + navigation, not a switch (no source-switch protocol message).
    let tuner_wrap = crate::components::section_header::section_header("Source");
    let tuner_host = dom::el("div", "stage-tuner", None);
    tuner_host.set_id("stage-tuner");
    crate::graphics::tuner::render(&tuner_host);
    dom::append(&stage, &tuner_wrap);
    dom::append(&stage, &tuner_host);

    // ── Ovation now-playing (rings + transport + player volume + settings). ──
    // spotify::render builds the rings and keeps every sp-* id update() drives.
    let spotify_enabled =
        crate::state::with(|s| s.config.as_ref().map(|c| c.spotify_enabled).unwrap_or(true));
    // Wrap the Spotify card so it can yield to the BT now-playing card when
    // Bluetooth is the active source.
    let sp_wrap = dom::el("div", "", None);
    sp_wrap.set_id("stage-spotify-wrap");
    if spotify_enabled {
        crate::pages::spotify::render(&sp_wrap);
    } else {
        let es = crate::components::async_state::empty(
            "Spotify is off. Turn it on in Settings \u{203a} System \u{203a} Advanced.",
        );
        dom::append(&sp_wrap, &es);
    }
    dom::append(&stage, &sp_wrap);

    // ── Bluetooth now-playing — shown when BT is the active source. ──
    render_bt_nowplaying(&stage);

    // ── Slim status / quick strip. ──
    render_status_quick(&stage);

    dom::append(container, &stage);
    update();
}

pub fn update() {
    let spotify_enabled =
        crate::state::with(|s| s.config.as_ref().map(|c| c.spotify_enabled).unwrap_or(true));
    // When Bluetooth is the active source, surface its now-playing card and let
    // the Spotify card step aside.
    let bt_active = crate::state::with(crate::state::active_source) == "bluetooth";
    if let Some(w) = dom::get_el("stage-spotify-wrap") {
        dom::set_style(&w, "display", if bt_active { "none" } else { "" });
    }
    if spotify_enabled && !bt_active {
        crate::pages::spotify::update();
    }
    update_bt_nowplaying(bt_active);
    refresh_glance();

    // Master quick-chip caption tracks live master volume.
    if let Some(chip) = dom::get_el("stage-master-chip") {
        let v = crate::state::with(|s| s.master_volume);
        dom::set_text(&chip, &master_chip_label(v));
        dom::set_attr(&chip, "aria-valuenow", &v.to_string());
        dom::set_attr(&chip, "aria-valuetext", &format!("{v}%"));
    }
}

/// Build the Stage Bluetooth now-playing card (hidden until BT is active).
fn render_bt_nowplaying(container: &web_sys::Element) {
    let card = dom::el("div", "card", None);
    card.set_id("stage-bt-np");
    dom::set_style(&card, "display", "none");
    dom::append(&card, &dom::el("div", "card-title", Some("Bluetooth")));
    let title = dom::el("div", "", None);
    title.set_id("stage-bt-title");
    dom::set_style(&title, "font-size", "1.15rem");
    dom::set_style(&title, "font-weight", "600");
    dom::append(&card, &title);
    let artist = dom::el("div", "text-muted", None);
    artist.set_id("stage-bt-artist");
    dom::append(&card, &artist);

    crate::components::nowplaying::render(&card, "stage-bt");

    let controls = dom::el("div", "", None);
    dom::set_style(&controls, "margin-top", "12px");
    dom::set_style(&controls, "display", "flex");
    dom::set_style(&controls, "gap", "8px");
    let prev = dom::el("button", "btn", Some("\u{23EE}"));
    dom::set_attr(&prev, "aria-label", "Previous track");
    dom::on_click(&prev, || send_bt_transport("prev"));
    dom::append(&controls, &prev);
    let pp = dom::el("button", "btn", Some("\u{25B6}"));
    pp.set_id("stage-bt-playpause");
    dom::set_attr(&pp, "aria-label", "Play or pause");
    dom::on_click(&pp, || {
        let playing = crate::state::with(|s| s.bt_status.as_ref().is_some_and(|st| st.playing));
        send_bt_transport(if playing { "pause" } else { "play" });
    });
    dom::append(&controls, &pp);
    let next = dom::el("button", "btn", Some("\u{23ED}"));
    dom::set_attr(&next, "aria-label", "Next track");
    dom::on_click(&next, || send_bt_transport("next"));
    dom::append(&controls, &next);
    dom::append(&card, &controls);
    dom::append(container, &card);
}

fn send_bt_transport(key: &str) {
    crate::ws::send_msg(&ClientMsg::BluetoothControl(BtAction::Transport {
        key: key.into(),
    }));
}

/// Show and populate the Stage BT now-playing card when BT is the active source.
fn update_bt_nowplaying(bt_active: bool) {
    let Some(card) = dom::get_el("stage-bt-np") else {
        return;
    };
    if !bt_active {
        dom::set_style(&card, "display", "none");
        return;
    }
    dom::set_style(&card, "display", "");
    crate::state::with(|s| {
        let (title, artist) = match s.bt_track.as_ref() {
            Some(t) if !t.title.is_empty() => (t.title.clone(), t.artist.clone()),
            _ => {
                // Connected but no metadata yet — name the device instead.
                let name = s
                    .bt_status
                    .as_ref()
                    .and_then(|st| st.connected.as_ref())
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "Bluetooth audio".to_string());
                (name, String::new())
            }
        };
        if let Some(el) = dom::get_el("stage-bt-title") {
            dom::set_text(&el, &title);
        }
        if let Some(el) = dom::get_el("stage-bt-artist") {
            dom::set_text(&el, &artist);
            dom::set_style(&el, "display", if artist.is_empty() { "none" } else { "" });
        }
        if let Some(el) = dom::get_el("stage-bt-playpause") {
            let playing = s.bt_status.as_ref().is_some_and(|st| st.playing);
            dom::set_text(&el, if playing { "\u{23F8}" } else { "\u{25B6}" });
        }
        crate::components::nowplaying::update("stage-bt", s.bt_playstatus.as_ref());
    });
}

fn render_status_quick(container: &web_sys::Element) {
    let card = dom::el("div", "card stage-strip", None);

    // Glance chips (online/grouped/CPU+temp) live here; refresh_glance fills them.
    let glance = dom::el("div", "home-glance", None);
    let chips = dom::el("div", "home-glance-chips", None);
    chips.set_id("home-glance-chips");
    dom::append(&glance, &chips);
    let status_link = dom::el("button", "home-glance-link", Some("Status \u{203a}"));
    dom::on_click(&status_link, || {
        dom::window()
            .location()
            .set_hash("settings/system/status")
            .ok();
    });
    dom::append(&glance, &status_link);
    dom::append(&card, &glance);

    // Master quick-chip: the everyday "make it louder" without leaving Stage.
    // Canonical master control still lives on Sound; this is the quick affordance.
    let vol = crate::state::with(|s| s.master_volume);
    let chip = dom::el("button", "stage-master-chip", Some(&master_chip_label(vol)));
    chip.set_id("stage-master-chip");
    dom::set_attr(&chip, "role", "slider");
    dom::set_attr(&chip, "aria-label", "Master volume");
    dom::set_attr(&chip, "aria-valuemin", "0");
    dom::set_attr(&chip, "aria-valuemax", "100");
    dom::set_attr(&chip, "aria-valuenow", &vol.to_string());
    dom::set_attr(&chip, "aria-valuetext", &format!("{vol}%"));
    dom::on_click(&chip, || {
        let next = crate::state::with(|s| {
            let v = s.master_volume;
            if v >= 100 {
                0
            } else {
                (v / 10 + 1) * 10
            }
        });
        crate::state::with_mut(|s| s.master_volume = next);
        crate::ws::send_msg(&ClientMsg::SetMasterVolume(next));
        if let Some(el) = dom::get_el("stage-master-chip") {
            dom::set_text(&el, &master_chip_label(next));
            dom::set_attr(&el, "aria-valuenow", &next.to_string());
            dom::set_attr(&el, "aria-valuetext", &format!("{next}%"));
        }
    });
    // Arrow-key fine control on the chip (keyboard parity with a slider).
    dom::on_keydown(&chip, |e| {
        let delta: i32 = match e.key().as_str() {
            "ArrowUp" | "ArrowRight" => 5,
            "ArrowDown" | "ArrowLeft" => -5,
            _ => return,
        };
        e.prevent_default();
        let next = crate::state::with(|s| (s.master_volume as i32 + delta).clamp(0, 100) as u8);
        crate::state::with_mut(|s| s.master_volume = next);
        crate::ws::send_msg(&ClientMsg::SetMasterVolume(next));
        if let Some(el) = dom::get_el("stage-master-chip") {
            dom::set_text(&el, &master_chip_label(next));
            dom::set_attr(&el, "aria-valuenow", &next.to_string());
            dom::set_attr(&el, "aria-valuetext", &format!("{next}%"));
        }
    });
    dom::append(&card, &chip);

    // Light scenes — one-tap LED ring presets (preserved exactly).
    let scenes_label = dom::el("div", "stat-label mb-8", Some("Lights"));
    dom::append(&card, &scenes_label);
    let row = dom::el("div", "flex gap-8", None);
    for (name, anim) in [
        (
            "Warm",
            LedAnimation::Solid {
                r: 255,
                g: 150,
                b: 60,
            },
        ),
        (
            "Cool",
            LedAnimation::Solid {
                r: 120,
                g: 180,
                b: 255,
            },
        ),
        (
            "Bright",
            LedAnimation::Solid {
                r: 255,
                g: 255,
                b: 255,
            },
        ),
        ("Off", LedAnimation::Off),
    ] {
        let btn = dom::el("button", "btn btn-sm", Some(name));
        dom::set_style(&btn, "flex", "1");
        dom::on_click(&btn, move || {
            crate::ws::send_msg(&ClientMsg::SetLed(anim.clone()));
            crate::components::toast::success(&format!("Lights: {}", name));
        });
        dom::append(&row, &btn);
    }
    dom::append(&card, &row);

    dom::append(container, &card);
}

fn refresh_glance() {
    let Some(chips) = dom::get_el("home-glance-chips") else {
        return;
    };
    dom::clear(&chips);
    let (connected, cpu, temp, grouped) = crate::state::with(|s| {
        let cpu = s
            .cpu_history
            .back()
            .copied()
            .or_else(|| s.system.as_ref().map(|sys| sys.cpu_percent));
        let temp = s
            .system
            .as_ref()
            .and_then(|sys| sys.temperature_mc)
            .map(|mc| format!("{:.0}\u{00B0}C", mc as f64 / 1000.0));
        (s.connected, cpu, temp, s.group_status.is_some())
    });
    add_chip(&chips, if connected { "Online" } else { "Offline" });
    add_chip(&chips, if grouped { "Grouped" } else { "Solo" });
    // One chip for both CPU figures — a bare "77°C" on its own read as an
    // unlabeled mystery; pairing it with load names it as a CPU temperature.
    match (cpu, temp) {
        (Some(c), Some(t)) => add_chip(&chips, &format!("CPU {}% \u{00B7} {}", c, t)),
        (Some(c), None) => add_chip(&chips, &format!("CPU {}%", c)),
        (None, Some(t)) => add_chip(&chips, &format!("CPU {}", t)),
        (None, None) => {}
    }
}

fn add_chip(parent: &web_sys::Element, text: &str) {
    let chip = dom::el("span", "home-chip", Some(text));
    dom::append(parent, &chip);
}

/// Caption for the Stage master-volume quick chip. Pure so it is unit-tested.
fn master_chip_label(vol: u8) -> String {
    if vol == 0 {
        "Master muted".to_string()
    } else {
        format!("Master {}%", vol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_chip_label_formats() {
        assert_eq!(master_chip_label(0), "Master muted");
        assert_eq!(master_chip_label(1), "Master 1%");
        assert_eq!(master_chip_label(30), "Master 30%");
        assert_eq!(master_chip_label(100), "Master 100%");
    }
}
