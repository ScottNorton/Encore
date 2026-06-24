//! Arc Knob — circular dial for master volume control.
//!
//! Touch/mouse draggable. Displays 0-100 value in center.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

/// Draw an arc knob.
/// value: 0-100
/// size: canvas logical dimension (square).
pub fn draw(ctx: &CanvasRenderingContext2d, size: f64, value: u8, label: &str) {
    ctx.clear_rect(0.0, 0.0, size, size);
    let is_dark = super::theme::is_dark();

    let cx = size / 2.0;
    let cy = size / 2.0;
    let radius = size / 2.0 - 12.0;
    let line_w = 8.0;

    // Arc range: from 135deg to 405deg (270deg sweep)
    let start_angle = 135.0_f64.to_radians();
    let end_angle = 405.0_f64.to_radians();
    let sweep = end_angle - start_angle;
    let value_angle = start_angle + sweep * (value as f64 / 100.0);

    // Ember track: a warm low-alpha gold so the unfilled arc reads on the stage.
    let (tr, tg, tb) = super::theme::accent_rgb(is_dark);
    ctx.set_stroke_style_str(&super::rgba_str(tr, tg, tb, 0.18));
    ctx.set_line_width(line_w);
    ctx.set_line_cap("round");
    ctx.begin_path();
    ctx.arc(cx, cy, radius, start_angle, end_angle).ok();
    ctx.stroke();

    // Value arc (fill): brand accent at normal levels, warning hues when loud.
    if value > 0 {
        let accent = {
            let (r, g, b) = super::theme::accent_rgb(is_dark);
            super::rgb_str(r, g, b)
        };
        let color = if value > 80 {
            "rgb(248,81,73)".to_string() // red for loud
        } else if value > 60 {
            "rgb(227,179,65)".to_string() // yellow
        } else {
            accent
        };
        ctx.set_stroke_style_str(&color);
        ctx.set_line_width(line_w);
        ctx.begin_path();
        ctx.arc(cx, cy, radius, start_angle, value_angle).ok();
        ctx.stroke();
    }

    // Center value text
    let (ir, ig, ib) = super::theme::ink_rgb(is_dark);
    ctx.set_fill_style_str(&super::rgba_str(ir, ig, ib, 0.9));
    ctx.set_font(&format!("bold {}px system-ui", (size / 4.0) as u32));
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    ctx.fill_text(&format!("{}", value), cx, cy).ok();

    // Caller-supplied label below the number (e.g. "VOL", "Volume").
    let (lr, lg, lb) = super::theme::ink_rgb(is_dark);
    ctx.set_fill_style_str(&super::rgba_str(lr, lg, lb, 0.55));
    ctx.set_font(&format!("{}px system-ui", (size / 10.0) as u32));
    ctx.fill_text(label, cx, cy + size / 6.0).ok();
}

/// Map a center-relative offset (dx right-positive, dy down-positive) to a
/// 0-100 knob value over a 270deg sweep that starts lower-left (225deg) and
/// runs clockwise to lower-right (135deg). The bottom 90deg is a dead zone that
/// snaps to the nearest end. Pure — operates on the offset, not the DOM, so the
/// overlay path and tests reuse it.
pub fn value_from_offset_xy(dx: f64, dy: f64) -> u8 {
    let mut angle = dy.atan2(dx).to_degrees();
    angle += 90.0; // 0deg = up
    if angle < 0.0 {
        angle += 360.0;
    }
    let offset = (angle - 225.0 + 360.0) % 360.0;
    let value = if offset <= 270.0 {
        (offset / 270.0 * 100.0).clamp(0.0, 100.0)
    } else if offset > 315.0 {
        0.0
    } else {
        100.0
    };
    value as u8
}

/// Step a 0-100 value by `delta`, clamped. Used for arrow-key handling on the
/// `role="slider"` overlay.
pub fn step_value(value: u8, delta: i8) -> u8 {
    (value as i16 + delta as i16).clamp(0, 100) as u8
}

/// Set up mouse/touch/keyboard interaction on a `role="slider"` overlay element
/// (NOT the canvas — the canvas is aria-hidden decoration). The same
/// `on_change` fires for drag and for arrow keys. The overlay must own the
/// current value via its `aria-valuenow` attribute so arrow steps are relative.
pub fn make_interactive(overlay: &web_sys::Element, on_change: impl Fn(u8) + 'static) {
    crate::dom::set_style(overlay, "touch-action", "none");

    let overlay_clone = overlay.clone();
    let prev_threshold = std::rc::Rc::new(std::cell::Cell::new(255u8)); // impossible initial
    let on_change = {
        let pt = prev_threshold.clone();
        std::rc::Rc::new(move |value: u8| {
            // Haptic on threshold crossing (0, 25, 50, 75, 100).
            let bucket = match value {
                0 => 0,
                1..=25 => 25,
                26..=50 => 50,
                51..=75 => 75,
                _ => 100,
            };
            if bucket != pt.get() {
                pt.set(bucket);
                crate::haptic::pulse(8);
            }
            on_change(value);
        })
    };

    let handler: std::rc::Rc<dyn Fn(f64, f64)> = {
        let overlay = overlay_clone.clone();
        let on_change = on_change.clone();
        std::rc::Rc::new(move |client_x: f64, client_y: f64| {
            let rect = overlay.get_bounding_client_rect();
            let cx = rect.width() / 2.0;
            let cy = rect.height() / 2.0;
            let dx = client_x - rect.left() - cx;
            let dy = client_y - rect.top() - cy;
            on_change(value_from_offset_xy(dx, dy));
        })
    };

    // Arrow keys: read the current value off aria-valuenow, step, fire on_change.
    {
        let overlay_kb = overlay_clone.clone();
        let on_change = on_change.clone();
        crate::dom::on_keydown(overlay, move |e: web_sys::KeyboardEvent| {
            let cur: u8 = overlay_kb
                .get_attribute("aria-valuenow")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let delta: i8 = match e.key().as_str() {
                "ArrowUp" | "ArrowRight" => 1,
                "ArrowDown" | "ArrowLeft" => -1,
                "PageUp" => 10,
                "PageDown" => -10,
                "Home" => return on_change(0),
                "End" => return on_change(100),
                _ => return,
            };
            e.prevent_default();
            on_change(step_value(cur, delta));
        });
    }

    // Mouse drag on the overlay.
    let handler_clone = handler.clone();
    let mousedown = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
        handler_clone(e.client_x() as f64, e.client_y() as f64);
    }) as Box<dyn FnMut(_)>);
    overlay
        .add_event_listener_with_callback("mousedown", mousedown.as_ref().unchecked_ref())
        .ok();
    mousedown.forget();

    let handler_clone = handler.clone();
    let mousemove = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
        if e.buttons() & 1 != 0 {
            handler_clone(e.client_x() as f64, e.client_y() as f64);
        }
    }) as Box<dyn FnMut(_)>);
    overlay
        .add_event_listener_with_callback("mousemove", mousemove.as_ref().unchecked_ref())
        .ok();
    mousemove.forget();

    // Touch drag on the overlay.
    let handler_clone = handler;
    let touchmove = Closure::wrap(Box::new(move |e: web_sys::TouchEvent| {
        e.prevent_default();
        if let Some(touch) = e.touches().get(0) {
            handler_clone(touch.client_x() as f64, touch.client_y() as f64);
        }
    }) as Box<dyn FnMut(_)>);
    overlay
        .add_event_listener_with_callback("touchmove", touchmove.as_ref().unchecked_ref())
        .ok();
    overlay
        .add_event_listener_with_callback("touchstart", touchmove.as_ref().unchecked_ref())
        .ok();
    touchmove.forget();
}

#[cfg(test)]
mod tests {
    use super::*;

    // value_from_offset_xy: relative (dx, dy) from the center maps to 0..100
    // over a 270deg sweep starting lower-left (225deg), dead zone at the bottom.
    #[test]
    fn top_is_fifty() {
        // Straight up: dx=0, dy=-1. 270deg sweep -> mid value.
        assert_eq!(value_from_offset_xy(0.0, -1.0), 50);
    }

    #[test]
    fn lower_left_start_is_zero() {
        // 225deg direction = (-0.707, +0.707): the sweep start.
        let v = value_from_offset_xy(-0.707, 0.707);
        assert!(v <= 1, "expected ~0, got {}", v);
    }

    #[test]
    fn lower_right_end_is_hundred() {
        // 135deg direction = (+0.707, +0.707): the sweep end.
        let v = value_from_offset_xy(0.707, 0.707);
        assert!(v >= 99, "expected ~100, got {}", v);
    }

    #[test]
    fn left_is_about_a_sixth() {
        // The sweep starts lower-left (0%) and runs clockwise through top (50%)
        // to lower-right (100%), so straight-left sits 45deg into the 270deg
        // sweep -> ~16.7%. (This preserves the original shipped knob math; the
        // value is symmetric with straight-right about the 50% top.)
        let v = value_from_offset_xy(-1.0, 0.0);
        assert!((14..=20).contains(&v), "got {}", v);
    }

    #[test]
    fn right_is_about_five_sixths() {
        // Straight-right sits 225deg into the 270deg sweep -> ~83.3%, the mirror
        // of straight-left about the 50% top.
        let v = value_from_offset_xy(1.0, 0.0);
        assert!((80..=86).contains(&v), "got {}", v);
    }

    #[test]
    fn dead_zone_snaps_to_nearest_end() {
        // Straight down is the dead-zone center; slightly past start snaps to 0,
        // slightly before end snaps to 100.
        let down = value_from_offset_xy(0.0, 1.0);
        assert!(down == 0 || down == 100, "got {}", down);
    }

    #[test]
    fn arrow_step_clamps() {
        assert_eq!(step_value(0, -1), 0);
        assert_eq!(step_value(100, 1), 100);
        assert_eq!(step_value(50, 5), 55);
        assert_eq!(step_value(2, -5), 0);
    }
}
