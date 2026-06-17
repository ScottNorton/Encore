//! Arc Knob — circular dial for master volume control.
//!
//! Touch/mouse draggable. Displays 0-100 value in center.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

/// Draw an arc knob.
/// value: 0-100
/// size: canvas logical dimension (square).
pub fn draw(ctx: &CanvasRenderingContext2d, size: f64, value: u8) {
    ctx.clear_rect(0.0, 0.0, size, size);

    let cx = size / 2.0;
    let cy = size / 2.0;
    let radius = size / 2.0 - 12.0;
    let line_w = 8.0;

    // Arc range: from 135deg to 405deg (270deg sweep)
    let start_angle = 135.0_f64.to_radians();
    let end_angle = 405.0_f64.to_radians();
    let sweep = end_angle - start_angle;
    let value_angle = start_angle + sweep * (value as f64 / 100.0);

    // Background arc (track)
    ctx.set_stroke_style_str("rgba(48,54,61,0.8)");
    ctx.set_line_width(line_w);
    ctx.set_line_cap("round");
    ctx.begin_path();
    ctx.arc(cx, cy, radius, start_angle, end_angle).ok();
    ctx.stroke();

    // Value arc (fill)
    if value > 0 {
        let color = if value > 80 {
            "rgb(248,81,73)" // red for loud
        } else if value > 60 {
            "rgb(227,179,65)" // yellow
        } else {
            "rgb(88,166,255)" // blue
        };
        ctx.set_stroke_style_str(color);
        ctx.set_line_width(line_w);
        ctx.begin_path();
        ctx.arc(cx, cy, radius, start_angle, value_angle).ok();
        ctx.stroke();
    }

    // Center value text
    ctx.set_fill_style_str("rgba(230,237,243,0.9)");
    ctx.set_font(&format!("bold {}px system-ui", (size / 4.0) as u32));
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    ctx.fill_text(&format!("{}", value), cx, cy).ok();

    // "VOL" label below number
    ctx.set_fill_style_str("rgba(139,148,158,0.7)");
    ctx.set_font(&format!("{}px system-ui", (size / 10.0) as u32));
    ctx.fill_text("VOL", cx, cy + size / 6.0).ok();
}

/// Set up mouse/touch drag interaction on a canvas for knob control.
/// Returns a closure that removes the listeners (for cleanup).
/// `on_change` is called with the new value (0-100).
pub fn make_interactive(canvas: &web_sys::HtmlCanvasElement, on_change: impl Fn(u8) + 'static) {
    // Prevent browser scroll/zoom while interacting with the knob
    let el: &web_sys::Element = canvas.as_ref();
    crate::dom::set_style(el, "touch-action", "none");

    let canvas_clone = canvas.clone();
    let prev_threshold = std::rc::Rc::new(std::cell::Cell::new(255u8)); // impossible initial
    let on_change = {
        let pt = prev_threshold.clone();
        std::rc::Rc::new(move |value: u8| {
            // Haptic on threshold crossing (0, 25, 50, 75, 100)
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
        let canvas = canvas_clone.clone();
        let on_change = on_change.clone();
        std::rc::Rc::new(move |client_x: f64, client_y: f64| {
            let rect = canvas.get_bounding_client_rect();
            let cx = rect.width() / 2.0;
            let cy = rect.height() / 2.0;
            let dx = client_x - rect.left() - cx;
            let dy = client_y - rect.top() - cy;

            let mut angle = dy.atan2(dx).to_degrees();
            // Rotate so 0° = top (up)
            angle += 90.0;
            if angle < 0.0 {
                angle += 360.0;
            }

            // Arc starts at 225° (lower-left) and sweeps 270° clockwise to 135° (lower-right).
            // Dead zone is the bottom 90° (from 135° to 225°).
            let offset = (angle - 225.0 + 360.0) % 360.0;
            let value = if offset <= 270.0 {
                (offset / 270.0 * 100.0).clamp(0.0, 100.0)
            } else {
                // In dead zone — snap to nearest end
                if offset > 315.0 {
                    0.0
                } else {
                    100.0
                }
            };

            on_change(value as u8);
        })
    };

    // Mouse drag
    let handler_clone = handler.clone();
    let mousedown = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
        handler_clone(e.client_x() as f64, e.client_y() as f64);
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("mousedown", mousedown.as_ref().unchecked_ref())
        .ok();
    mousedown.forget();

    let handler_clone = handler.clone();
    let mousemove = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
        if e.buttons() & 1 != 0 {
            handler_clone(e.client_x() as f64, e.client_y() as f64);
        }
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("mousemove", mousemove.as_ref().unchecked_ref())
        .ok();
    mousemove.forget();

    // Touch drag
    let handler_clone = handler;
    let touchmove = Closure::wrap(Box::new(move |e: web_sys::TouchEvent| {
        e.prevent_default();
        if let Some(touch) = e.touches().get(0) {
            handler_clone(touch.client_x() as f64, touch.client_y() as f64);
        }
    }) as Box<dyn FnMut(_)>);
    canvas
        .add_event_listener_with_callback("touchmove", touchmove.as_ref().unchecked_ref())
        .ok();
    canvas
        .add_event_listener_with_callback("touchstart", touchmove.as_ref().unchecked_ref())
        .ok();
    touchmove.forget();
}
