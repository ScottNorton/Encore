//! HSL color wheel — 160x160 canvas with hue ring.
//!
//! Click/drag on the ring to select a color, returns RGB.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

const SIZE: f64 = 160.0;
const OUTER_R: f64 = 74.0;
const INNER_R: f64 = 54.0;

/// Draw the color wheel.
pub fn draw(ctx: &CanvasRenderingContext2d) {
    ctx.clear_rect(0.0, 0.0, SIZE, SIZE);
    let cx = SIZE / 2.0;
    let cy = SIZE / 2.0;
    let mid_r = (OUTER_R + INNER_R) / 2.0;
    let ring_w = OUTER_R - INNER_R;

    // Draw 360 arc segments
    for deg in 0..360 {
        let angle_start = (deg as f64 - 0.5).to_radians();
        let angle_end = (deg as f64 + 1.5).to_radians();

        let (r, g, b) = super::hsl_to_rgb(deg as f64, 1.0, 0.5);
        ctx.set_stroke_style_str(&super::rgb_str(r, g, b));
        ctx.set_line_width(ring_w + 2.0); // slight overlap to avoid gaps
        ctx.begin_path();
        ctx.arc(cx, cy, mid_r, angle_start, angle_end).ok();
        ctx.stroke();
    }

    // Inner circle — solid stage color so the wheel hole matches the page.
    let is_dark = super::theme::is_dark();
    let (sr, sg, sb) = super::theme::stage_rgb(is_dark);
    ctx.set_fill_style_str(&super::rgb_str(sr, sg, sb));
    ctx.begin_path();
    ctx.arc(cx, cy, INNER_R - 1.0, 0.0, std::f64::consts::TAU)
        .ok();
    ctx.fill();

    // Center label
    let (mr, mg, mb) = super::theme::muted_rgb(is_dark);
    ctx.set_fill_style_str(&super::rgba_str(mr, mg, mb, 0.9));
    ctx.set_font("11px system-ui");
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    ctx.fill_text("HUE", cx, cy).ok();
}

/// Convert canvas (x, y) to hue (0-360), returns None if not on the ring.
fn xy_to_hue(x: f64, y: f64) -> Option<f64> {
    let cx = SIZE / 2.0;
    let cy = SIZE / 2.0;
    let dx = x - cx;
    let dy = y - cy;
    let dist = (dx * dx + dy * dy).sqrt();

    if !(INNER_R - 8.0..=OUTER_R + 8.0).contains(&dist) {
        return None;
    }

    let angle = dy.atan2(dx).to_degrees();
    let hue = (angle + 360.0) % 360.0;
    Some(hue)
}

/// Set up click/drag interaction on the color wheel canvas.
/// `on_color`: called with (r, g, b) when user picks a color.
pub fn make_interactive(
    canvas: &web_sys::HtmlCanvasElement,
    on_color: impl Fn(u8, u8, u8) + 'static,
) {
    use std::cell::Cell;
    use std::rc::Rc;

    // Prevent browser scroll/zoom while interacting with the wheel
    let el: &web_sys::Element = canvas.as_ref();
    crate::dom::set_style(el, "touch-action", "none");

    let on_color = Rc::new(on_color);
    let dragging = Rc::new(Cell::new(false));

    let get_canvas_pos = {
        let canvas = canvas.clone();
        move |client_x: f64, client_y: f64| -> (f64, f64) {
            let rect = canvas.get_bounding_client_rect();
            let scale_x = SIZE / rect.width();
            let scale_y = SIZE / rect.height();
            (
                (client_x - rect.left()) * scale_x,
                (client_y - rect.top()) * scale_y,
            )
        }
    };

    // Mousedown
    {
        let on_color = on_color.clone();
        let dragging = dragging.clone();
        let get_pos = get_canvas_pos.clone();
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            let (x, y) = get_pos(e.client_x() as f64, e.client_y() as f64);
            if let Some(hue) = xy_to_hue(x, y) {
                dragging.set(true);
                let (r, g, b) = super::hsl_to_rgb(hue, 1.0, 0.5);
                on_color(r, g, b);
            }
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    // Mousemove
    {
        let on_color = on_color.clone();
        let dragging = dragging.clone();
        let get_pos = get_canvas_pos.clone();
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            if dragging.get() && e.buttons() & 1 != 0 {
                let (x, y) = get_pos(e.client_x() as f64, e.client_y() as f64);
                if let Some(hue) = xy_to_hue(x, y) {
                    let (r, g, b) = super::hsl_to_rgb(hue, 1.0, 0.5);
                    on_color(r, g, b);
                }
            } else {
                dragging.set(false);
            }
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("mousemove", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    // Mouseup
    {
        let dragging_up = dragging.clone();
        let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
            dragging_up.set(false);
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("mouseup", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    // Touch events
    {
        let on_color_t = on_color;
        let canvas_t = canvas.clone();
        let touchmove = Closure::wrap(Box::new(move |e: web_sys::TouchEvent| {
            e.prevent_default();
            if let Some(touch) = e.touches().get(0) {
                let rect = canvas_t.get_bounding_client_rect();
                let scale_x = SIZE / rect.width();
                let scale_y = SIZE / rect.height();
                let x = (touch.client_x() as f64 - rect.left()) * scale_x;
                let y = (touch.client_y() as f64 - rect.top()) * scale_y;
                if let Some(hue) = xy_to_hue(x, y) {
                    let (r, g, b) = super::hsl_to_rgb(hue, 1.0, 0.5);
                    on_color_t(r, g, b);
                }
            }
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("touchstart", touchmove.as_ref().unchecked_ref())
            .ok();
        canvas
            .add_event_listener_with_callback("touchmove", touchmove.as_ref().unchecked_ref())
            .ok();
        touchmove.forget();
    }
}

/// Get the canvas size.
pub fn size() -> u32 {
    SIZE as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER: f64 = SIZE / 2.0; // 80.0
                                    // A radius safely inside the ring band [INNER_R-8, OUTER_R+8] = [46, 82].
    const ON_RING: f64 = 64.0;

    #[test]
    fn size_matches_const() {
        assert_eq!(size(), 160);
    }

    #[test]
    fn xy_to_hue_right_is_zero() {
        // Point to the right of center: angle 0 deg => hue 0.
        let hue = xy_to_hue(CENTER + ON_RING, CENTER).expect("on ring");
        assert!((hue - 0.0).abs() < 1e-6, "hue was {hue}");
    }

    #[test]
    fn xy_to_hue_down_is_ninety() {
        // Canvas y grows downward, so +dy is "down" => atan2 gives +90 deg.
        let hue = xy_to_hue(CENTER, CENTER + ON_RING).expect("on ring");
        assert!((hue - 90.0).abs() < 1e-6, "hue was {hue}");
    }

    #[test]
    fn xy_to_hue_left_is_one_eighty() {
        let hue = xy_to_hue(CENTER - ON_RING, CENTER).expect("on ring");
        assert!((hue - 180.0).abs() < 1e-6, "hue was {hue}");
    }

    #[test]
    fn xy_to_hue_up_is_two_seventy() {
        // -dy ("up") gives -90 deg, wrapped into [0,360) => 270.
        let hue = xy_to_hue(CENTER, CENTER - ON_RING).expect("on ring");
        assert!((hue - 270.0).abs() < 1e-6, "hue was {hue}");
    }

    #[test]
    fn xy_to_hue_center_is_off_ring() {
        // The dead center is inside INNER_R, so it returns None.
        assert!(xy_to_hue(CENTER, CENTER).is_none());
    }

    #[test]
    fn xy_to_hue_far_outside_is_off_ring() {
        // Far beyond OUTER_R+8 returns None.
        assert!(xy_to_hue(CENTER + 100.0, CENTER).is_none());
    }
}
