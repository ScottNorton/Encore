//! Now-playing progress bar (position / duration) shared by the Stage and the
//! Devices Bluetooth cards. `render` builds the elements (hidden); `update`
//! fills them from a [`BtPlayStatus`].

use crate::components::slider::Slider;
use crate::dom;
use encore_common::protocol::{BtPlayStatus, ClientMsg};
use wasm_bindgen::JsCast;

/// Format milliseconds as `m:ss`, or `h:mm:ss` once past an hour. Pure.
pub fn fmt_time(ms: u32) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// Build a progress row into `container`. Element ids: `{id}-progress` (wrapper),
/// `{id}-bar-fill`, `{id}-pos`, `{id}-dur`. Hidden until `update` reveals it.
pub fn render(container: &web_sys::Element, id: &str) {
    let wrap = dom::create_div();
    wrap.set_id(&format!("{}-progress", id));
    dom::set_style(&wrap, "display", "none");
    dom::set_style(&wrap, "margin-top", "10px");

    let bar = dom::create_div();
    dom::set_style(&bar, "height", "4px");
    dom::set_style(&bar, "border-radius", "2px");
    dom::set_style(&bar, "background", "var(--border)");
    dom::set_style(&bar, "overflow", "hidden");
    let fill = dom::create_div();
    fill.set_id(&format!("{}-bar-fill", id));
    dom::set_style(&fill, "height", "100%");
    dom::set_style(&fill, "width", "0%");
    dom::set_style(&fill, "background", "var(--accent, #c8a24a)");
    dom::append(&bar, &fill);
    dom::append(&wrap, &bar);

    let times = dom::create_div();
    dom::set_class(&times, "flex justify-between text-muted text-sm");
    dom::set_style(&times, "margin-top", "4px");
    let pos = dom::el("span", "", Some("0:00"));
    pos.set_id(&format!("{}-pos", id));
    let dur = dom::el("span", "", Some(""));
    dur.set_id(&format!("{}-dur", id));
    dom::append(&times, &pos);
    dom::append(&times, &dur);
    dom::append(&wrap, &times);

    dom::append(container, &wrap);

    // Volume control (drives the master volume = the BT playback volume, and
    // reflects the AVRCP-synced level). Rendered once; `update` sets its value.
    let vol = crate::state::with(|s| s.master_volume) as u32;
    let slider = Slider::create(&format!("{}-vol", id), "Volume", 0, 100, vol, "%", |v| {
        crate::ws::send_msg(&ClientMsg::SetMasterVolume(v as u8))
    });
    dom::set_style(&slider, "margin-top", "12px");
    dom::append(container, &slider);
}

/// Update the progress row. `None` or an unknown duration hides the bar.
pub fn update(id: &str, ps: Option<&BtPlayStatus>) {
    let Some(wrap) = dom::get_el(&format!("{}-progress", id)) else {
        return;
    };
    match ps {
        Some(p) if p.duration_ms > 0 => {
            dom::set_style(&wrap, "display", "");
            let pct = (p.position_ms as f64 / p.duration_ms as f64 * 100.0).clamp(0.0, 100.0);
            if let Some(fill) = dom::get_el(&format!("{}-bar-fill", id)) {
                dom::set_style(&fill, "width", &format!("{:.1}%", pct));
            }
            if let Some(el) = dom::get_el(&format!("{}-pos", id)) {
                dom::set_text(&el, &fmt_time(p.position_ms));
            }
            if let Some(el) = dom::get_el(&format!("{}-dur", id)) {
                dom::set_text(&el, &fmt_time(p.duration_ms));
            }
        }
        _ => dom::set_style(&wrap, "display", "none"),
    }

    // Reflect the live master volume on the slider — unless it's focused (the
    // user is dragging), so we don't fight the gesture.
    let vol_id = format!("{}-vol", id);
    let focused = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .is_some_and(|a| a.id() == vol_id);
    if !focused {
        if let Some(input) =
            dom::get_el(&vol_id).and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            let vol = crate::state::with(|s| s.master_volume);
            input.set_value(&vol.to_string());
            if let Some(val) = dom::get_el(&format!("{}-vol-val", id)) {
                dom::set_text(&val, &format!("{}%", vol));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time() {
        assert_eq!(fmt_time(0), "0:00");
        assert_eq!(fmt_time(42_000), "0:42");
        assert_eq!(fmt_time(125_000), "2:05");
        assert_eq!(fmt_time(3_661_000), "1:01:01");
    }
}
