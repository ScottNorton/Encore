//! Source Tuner — a horizontal backlit dial bar with data-driven stations
//! (Spotify / Bluetooth / Voice). It is an INDICATOR plus navigation, not a
//! switch: the active station is derived from playback state
//! (`crate::state::active_source`) and tapping a station navigates to its
//! controls. The accessible primary is a `radiogroup` of `role="radio"` buttons
//! laid over the bar; a glowing gold needle div glides to the active station.
//!
//! The bar is built from DOM (a rounded-rect panel with a `repeating-linear-
//! gradient` tick texture and an absolutely-positioned needle) rather than
//! canvas — a crisper, simpler shape for a flat horizontal dial.

use std::cell::Cell;

/// The v1 source list (data-driven so AirPlay/Cast/line-in can be added later).
/// `(id, label, glyph)` — `id` matches `crate::state::active_source` return
/// values; `glyph` is the small source icon painted in the row above the bar.
pub const STATIONS: &[(&str, &str, &str)] = &[
    ("spotify", "Spotify", "\u{266B}"),     // ♫ note
    ("bluetooth", "Bluetooth", "\u{0192}"), // ƒ (Bluetooth-ish rune)
    ("voice", "Voice", "\u{1F3A4}"),        // 🎤 mic
];

thread_local! {
    /// Epoch for the needle glide loop — bumped on each glide start and on stop
    /// so a new target cancels the previous glide (the led_ring pattern).
    static TUNER_EPOCH: Cell<u32> = const { Cell::new(0) };
}

/// Index of the station whose id matches `source`, or 0 if unknown.
pub fn station_index(source: &str) -> usize {
    STATIONS
        .iter()
        .position(|(id, _, _)| *id == source)
        .unwrap_or(0)
}

/// Map a horizontal tap (fraction 0.0..1.0 across the dial width) to a station
/// index. Evenly partitions the width into `STATIONS.len()` bands.
///
// `#[allow(dead_code)]`: the tap-to-station hit path consumes this in P4 (the
// current `render` navigates via per-station button clicks). Exercised by tests.
#[allow(dead_code)]
pub fn station_from_x(frac: f64) -> usize {
    let n = STATIONS.len();
    if n == 0 {
        return 0;
    }
    let clamped = frac.clamp(0.0, 0.999_999);
    (clamped * n as f64) as usize
}

/// Horizontal position of a station's center as a percentage (0..100) across the
/// bar width. With `n` stations they sit at the center of `n` even bands, so the
/// needle and labels share one geometry. Pure.
pub fn needle_pct(index: usize) -> f64 {
    let n = STATIONS.len();
    if n == 0 {
        return 50.0;
    }
    let i = index.min(n - 1);
    (i as f64 + 0.5) / n as f64 * 100.0
}

/// Build the Tuner into `container`: a source-icon row, then a backlit dial bar
/// (a `radiogroup` of station buttons over a tick-textured panel) with a gold
/// needle div. The active station is derived (not stored). Tapping a station
/// navigates to its controls.
pub fn render(container: &web_sys::Element) {
    use crate::dom;

    let active = crate::state::with(crate::state::active_source);
    let active_idx = station_index(active);

    let stack = dom::create_div();
    dom::set_class(&stack, "tuner");

    // ── Source-icon row (small icons; the active one gold). ──
    let icons = dom::create_div();
    dom::set_class(&icons, "tuner-icons");
    dom::set_attr(&icons, "aria-hidden", "true");
    for (i, (id, _label, glyph)) in STATIONS.iter().enumerate() {
        let icon = dom::el("span", "tuner-icon", Some(glyph));
        icon.set_id(&format!("tuner-icon-{}", id));
        if i == active_idx {
            dom::add_class(&icon, "active");
        }
        dom::append(&icons, &icon);
    }
    dom::append(&stack, &icons);

    // ── Backlit dial bar: tick-textured panel + needle + station radiogroup. ──
    let bar = dom::create_div();
    dom::set_class(&bar, "tuner-bar");

    // Tick texture layer (decorative; the repeating gradient lives in CSS).
    let ticks = dom::create_div();
    dom::set_class(&ticks, "tuner-ticks");
    dom::set_attr(&ticks, "aria-hidden", "true");
    dom::append(&bar, &ticks);

    // Glowing gold needle, positioned over the active station.
    let needle = dom::create_div();
    needle.set_id("tuner-needle");
    dom::set_class(&needle, "tuner-needle");
    dom::set_attr(&needle, "aria-hidden", "true");
    dom::set_style(&needle, "left", &format!("{:.4}%", needle_pct(active_idx)));
    dom::append(&bar, &needle);

    // Accessible radiogroup of station labels laid across the bar.
    let group = dom::create_div();
    dom::set_class(&group, "tuner-stations");
    dom::set_attr(&group, "role", "radiogroup");
    dom::set_attr(&group, "aria-label", "Audio source");

    for (i, (id, label, _glyph)) in STATIONS.iter().enumerate() {
        let btn = dom::el("button", "tuner-station", Some(label));
        btn.set_id(&format!("tuner-station-{}", id));
        dom::set_attr(&btn, "role", "radio");
        let is_active = i == active_idx;
        dom::set_attr(
            &btn,
            "aria-checked",
            if is_active { "true" } else { "false" },
        );
        dom::set_attr(&btn, "tabindex", if is_active { "0" } else { "-1" });
        if is_active {
            dom::add_class(&btn, "active");
        }
        let id_owned = id.to_string();
        dom::on_click(&btn, move || navigate_to_source(&id_owned));
        dom::append(&group, &btn);
    }
    dom::append(&bar, &group);
    dom::append(&stack, &bar);
    dom::append(container, &stack);

    glide_to(active_idx);
}

/// Navigate to a source's controls. No source-switch message exists; the Tuner
/// is navigation only (design v2 resolution).
fn navigate_to_source(id: &str) {
    let hash = match id {
        "bluetooth" => "#settings/devices", // Bluetooth lives on the Devices page
        "voice" => "#settings/integrations", // voice/assistant lives under Integrations
        _ => "#home",                        // Spotify controls live on Stage
    };
    crate::dom::window().location().set_hash(hash).ok();
}

/// Glide the needle to `target_idx` over a short ease (or jump under reduced
/// motion). The DOM needle has a CSS transition, so this just sets the final
/// position; the epoch is still bumped so `stop()` can cancel a pending glide.
fn glide_to(target_idx: usize) {
    TUNER_EPOCH.with(|e| e.set(e.get().wrapping_add(1)));
    set_needle(target_idx);
}

/// Position the needle div over `target_idx` and recolor the icon row.
fn set_needle(target_idx: usize) {
    use crate::dom;
    if let Some(needle) = dom::get_el("tuner-needle") {
        dom::set_style(&needle, "left", &format!("{:.4}%", needle_pct(target_idx)));
    }
    for (i, (id, _label, _glyph)) in STATIONS.iter().enumerate() {
        if let Some(icon) = dom::get_el(&format!("tuner-icon-{}", id)) {
            if i == target_idx {
                dom::add_class(&icon, "active");
            } else {
                dom::remove_class(&icon, "active");
            }
        }
    }
}

/// Stop any pending needle glide (called by `app.rs` route teardown). The DOM
/// bar has no rAF loop, so this only invalidates the epoch; kept so the route
/// teardown contract is unchanged.
pub fn stop() {
    TUNER_EPOCH.with(|e| e.set(e.get().wrapping_add(1)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_matches_ids() {
        assert_eq!(station_index("spotify"), 0);
        assert_eq!(station_index("bluetooth"), 1);
        assert_eq!(station_index("voice"), 2);
        assert_eq!(station_index("nonsense"), 0);
    }

    #[test]
    fn x_partitions_into_bands() {
        // 3 stations -> [0,1/3) spotify, [1/3,2/3) bluetooth, [2/3,1) voice.
        assert_eq!(station_from_x(0.0), 0);
        assert_eq!(station_from_x(0.32), 0);
        assert_eq!(station_from_x(0.34), 1);
        assert_eq!(station_from_x(0.5), 1);
        assert_eq!(station_from_x(0.67), 2);
        assert_eq!(station_from_x(1.0), 2); // clamped, never out of range
    }

    #[test]
    fn x_never_out_of_range() {
        for i in 0..=100 {
            let idx = station_from_x(i as f64 / 100.0);
            assert!(idx < STATIONS.len());
        }
        assert!(station_from_x(-5.0) < STATIONS.len());
        assert!(station_from_x(99.0) < STATIONS.len());
    }

    #[test]
    fn needle_centers_each_band() {
        // 3 stations -> centers at 1/6, 1/2, 5/6 of the bar width.
        assert!((needle_pct(0) - 100.0 / 6.0).abs() < 1e-9);
        assert!((needle_pct(1) - 50.0).abs() < 1e-9);
        assert!((needle_pct(2) - 500.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn needle_clamps_out_of_range_index() {
        // Out-of-range index clamps to the last station, never overruns 100%.
        assert_eq!(needle_pct(99), needle_pct(STATIONS.len() - 1));
        assert!(needle_pct(99) < 100.0);
    }
}
