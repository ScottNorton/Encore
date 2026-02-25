//! Canvas 2D graphics engine.
//!
//! All visualizations rendered via Canvas 2D API.

pub mod sparkline;
pub mod vu_meter;
pub mod led_ring;
pub mod knob;
pub mod device;
pub mod eq_curve;
pub mod color_wheel;
pub mod spectrum;
pub mod scope;

use web_sys::HtmlCanvasElement;

/// HSL to RGB conversion. h: 0-360, s: 0-1, l: 0-1.
#[allow(dead_code)]
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let h = h / 360.0;
    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

#[allow(dead_code)]
fn hue_to_rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 { t += 1.0; }
    if t > 1.0 { t -= 1.0; }
    if t < 1.0 / 6.0 { return p + (q - p) * 6.0 * t; }
    if t < 0.5 { return q; }
    if t < 2.0 / 3.0 { return p + (q - p) * (2.0 / 3.0 - t) * 6.0; }
    p
}

/// Create a canvas element with DPI scaling for sharp rendering.
pub fn create_canvas(width: u32, height: u32) -> (HtmlCanvasElement, web_sys::CanvasRenderingContext2d) {
    let dpr = crate::dom::window()
        .device_pixel_ratio()
        .max(1.0);
    let (canvas, ctx) = crate::dom::canvas(
        (width as f64 * dpr) as u32,
        (height as f64 * dpr) as u32,
    );
    // Set CSS size to logical pixels
    canvas.style().set_property("width", &format!("{}px", width)).ok();
    canvas.style().set_property("height", &format!("{}px", height)).ok();
    ctx.scale(dpr, dpr).ok();
    (canvas, ctx)
}

/// Format RGB as CSS color string.
pub fn rgb_str(r: u8, g: u8, b: u8) -> String {
    format!("rgb({},{},{})", r, g, b)
}

/// Format RGBA as CSS color string.
pub fn rgba_str(r: u8, g: u8, b: u8, a: f64) -> String {
    format!("rgba({},{},{},{:.2})", r, g, b, a)
}
