//! Interactive EQ frequency response curve — canvas with draggable band dots.
//!
//! X axis: 20Hz–20kHz (log scale), Y axis: -12dB to +12dB.
//! Each of 10 EQ bands appears as a draggable dot. Drag vertically for gain,
//! horizontally for frequency. Tap a dot to select it for detail editing.
//! A smooth frequency response curve is drawn through all active bands.

use encore_common::protocol::{EqBand, FilterType};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

// Layout constants (logical pixels, before DPI scaling)
const W: f64 = 320.0;
const H: f64 = 160.0;
const PAD_L: f64 = 0.0;
const PAD_R: f64 = 0.0;
const PAD_T: f64 = 8.0;
const PAD_B: f64 = 8.0;
const PLOT_W: f64 = W - PAD_L - PAD_R;
const PLOT_H: f64 = H - PAD_T - PAD_B;

// Frequency range (log scale)
const F_MIN: f64 = 20.0;
const F_MAX: f64 = 20000.0;

// Gain range
const G_MIN: f64 = -12.0;
const G_MAX: f64 = 12.0;

const DOT_RADIUS: f64 = 7.0;
const DOT_SELECTED_RADIUS: f64 = 9.0;

/// Map frequency (Hz) to X pixel coordinate (log scale).
fn freq_to_x(hz: f64) -> f64 {
    let log_min = F_MIN.ln();
    let log_max = F_MAX.ln();
    let t = (hz.clamp(F_MIN, F_MAX).ln() - log_min) / (log_max - log_min);
    PAD_L + t * PLOT_W
}

/// Map X pixel coordinate to frequency (Hz).
fn x_to_freq(x: f64) -> f64 {
    let log_min = F_MIN.ln();
    let log_max = F_MAX.ln();
    let t = ((x - PAD_L) / PLOT_W).clamp(0.0, 1.0);
    (log_min + t * (log_max - log_min)).exp()
}

/// Map gain (dB) to Y pixel coordinate.
fn gain_to_y(db: f64) -> f64 {
    let t = (db.clamp(G_MIN, G_MAX) - G_MAX) / (G_MIN - G_MAX);
    PAD_T + t * PLOT_H
}

/// Map Y pixel coordinate to gain (dB).
fn y_to_gain(y: f64) -> f64 {
    let t = ((y - PAD_T) / PLOT_H).clamp(0.0, 1.0);
    G_MAX + t * (G_MIN - G_MAX)
}

/// Compute biquad magnitude response at frequency `f` for a single band.
/// Returns gain in dB. Same cookbook formulas as the firmware uses.
fn band_response_db(band: &EqBand, f: f64) -> f64 {
    if band.gain_cb == 0 {
        return 0.0;
    }

    let fs = 48000.0;
    let freq = band.freq_hz as f64;
    let gain_db = band.gain_cb as f64 / 10.0;
    let q = band.q_x10 as f64 / 10.0;

    let w0 = 2.0 * std::f64::consts::PI * freq / fs;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);
    let a_lin = 10.0_f64.powf(gain_db / 40.0);

    let (b0, b1, b2, a0, a1, a2) = match band.filter_type {
        FilterType::Peak => (
            1.0 + alpha * a_lin,
            -2.0 * cos_w0,
            1.0 - alpha * a_lin,
            1.0 + alpha / a_lin,
            -2.0 * cos_w0,
            1.0 - alpha / a_lin,
        ),
        FilterType::LowShelf => {
            let tsa = 2.0 * a_lin.sqrt() * alpha;
            (
                a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w0 + tsa),
                2.0 * a_lin * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w0),
                a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w0 - tsa),
                (a_lin + 1.0) + (a_lin - 1.0) * cos_w0 + tsa,
                -2.0 * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w0),
                (a_lin + 1.0) + (a_lin - 1.0) * cos_w0 - tsa,
            )
        }
        FilterType::HighShelf => {
            let tsa = 2.0 * a_lin.sqrt() * alpha;
            (
                a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w0 + tsa),
                -2.0 * a_lin * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w0),
                a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w0 - tsa),
                (a_lin + 1.0) - (a_lin - 1.0) * cos_w0 + tsa,
                2.0 * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w0),
                (a_lin + 1.0) - (a_lin - 1.0) * cos_w0 - tsa,
            )
        }
        FilterType::Notch => (
            1.0,
            -2.0 * cos_w0,
            1.0,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        ),
    };

    // Evaluate transfer function H(e^jw) at frequency f
    let w = 2.0 * std::f64::consts::PI * f / fs;
    let cos1 = w.cos();
    let cos2 = (2.0 * w).cos();
    let sin1 = w.sin();
    let sin2 = (2.0 * w).sin();

    let num_re = (b0 / a0) + (b1 / a0) * cos1 + (b2 / a0) * cos2;
    let num_im = -(b1 / a0) * sin1 - (b2 / a0) * sin2;
    let den_re = 1.0 + (a1 / a0) * cos1 + (a2 / a0) * cos2;
    let den_im = -(a1 / a0) * sin1 - (a2 / a0) * sin2;

    let num_mag_sq = num_re * num_re + num_im * num_im;
    let den_mag_sq = den_re * den_re + den_im * den_im;

    if den_mag_sq < 1e-20 {
        return 0.0;
    }
    10.0 * (num_mag_sq / den_mag_sq).log10()
}

/// Compute combined response of all bands at frequency `f` (in dB).
fn total_response_db(bands: &[EqBand; 10], enabled: bool, f: f64) -> f64 {
    if !enabled {
        return 0.0;
    }
    let mut total = 0.0;
    for band in bands {
        total += band_response_db(band, f);
    }
    total
}

/// Draw the EQ frequency response curve and band dots.
pub fn draw(
    ctx: &CanvasRenderingContext2d,
    bands: &[EqBand; 10],
    enabled: bool,
    selected: Option<usize>,
) {
    ctx.clear_rect(0.0, 0.0, W, H);

    // Background
    ctx.set_fill_style_str("rgba(13,17,23,0.6)");
    ctx.begin_path();
    round_rect(ctx, 0.0, 0.0, W, H, 8.0);
    ctx.fill();

    // Grid lines
    ctx.set_stroke_style_str("rgba(48,54,61,0.4)");
    ctx.set_line_width(0.5);

    // Horizontal grid: -12, -6, 0, +6, +12 dB
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = gain_to_y(db);
        ctx.begin_path();
        ctx.move_to(PAD_L, y);
        ctx.line_to(W - PAD_R, y);
        ctx.stroke();
    }

    // 0dB line slightly brighter
    ctx.set_stroke_style_str("rgba(48,54,61,0.8)");
    ctx.set_line_width(1.0);
    let zero_y = gain_to_y(0.0);
    ctx.begin_path();
    ctx.move_to(PAD_L, zero_y);
    ctx.line_to(W - PAD_R, zero_y);
    ctx.stroke();

    // Vertical grid: 100, 1k, 10k Hz
    ctx.set_stroke_style_str("rgba(48,54,61,0.4)");
    ctx.set_line_width(0.5);
    for &freq in &[100.0, 1000.0, 10000.0] {
        let x = freq_to_x(freq);
        ctx.begin_path();
        ctx.move_to(x, PAD_T);
        ctx.line_to(x, H - PAD_B);
        ctx.stroke();
    }

    // Frequency labels
    ctx.set_fill_style_str("rgba(139,148,158,0.5)");
    ctx.set_font("9px system-ui");
    ctx.set_text_align("center");
    ctx.set_text_baseline("top");
    let label_y = H - 2.0;
    for (freq, label) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        ctx.fill_text(label, freq_to_x(freq), label_y).ok();
    }

    // dB labels
    ctx.set_text_align("left");
    ctx.set_text_baseline("middle");
    for (db, label) in [(12.0, "+12"), (0.0, "0"), (-12.0, "-12")] {
        ctx.fill_text(label, 2.0, gain_to_y(db)).ok();
    }

    // Draw frequency response curve (smooth, many sample points)
    if enabled {
        // Filled area under curve
        ctx.begin_path();
        ctx.move_to(PAD_L, zero_y);
        let steps = 200;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let log_f = F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln());
            let f = log_f.exp();
            let db = total_response_db(bands, true, f);
            let x = freq_to_x(f);
            let y = gain_to_y(db);
            ctx.line_to(x, y);
        }
        ctx.line_to(W - PAD_R, zero_y);
        ctx.close_path();
        ctx.set_fill_style_str("rgba(88,166,255,0.08)");
        ctx.fill();

        // Curve line
        ctx.begin_path();
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let log_f = F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln());
            let f = log_f.exp();
            let db = total_response_db(bands, true, f);
            let x = freq_to_x(f);
            let y = gain_to_y(db);
            if i == 0 {
                ctx.move_to(x, y);
            } else {
                ctx.line_to(x, y);
            }
        }
        ctx.set_stroke_style_str("rgba(88,166,255,0.7)");
        ctx.set_line_width(2.0);
        ctx.stroke();
    }

    // Draw band dots
    for (i, band) in bands.iter().enumerate() {
        let x = freq_to_x(band.freq_hz as f64);
        let gain_db = band.gain_cb as f64 / 10.0;
        let y = if enabled { gain_to_y(gain_db) } else { zero_y };

        let is_sel = selected == Some(i);
        let r = if is_sel {
            DOT_SELECTED_RADIUS
        } else {
            DOT_RADIUS
        };

        // Outer glow for selected
        if is_sel {
            ctx.set_fill_style_str("rgba(88,166,255,0.2)");
            ctx.begin_path();
            ctx.arc(x, y, r + 4.0, 0.0, std::f64::consts::TAU).ok();
            ctx.fill();
        }

        // Dot fill
        let color = if !enabled {
            "rgba(139,148,158,0.4)"
        } else if is_sel {
            "rgb(88,166,255)"
        } else if band.gain_cb != 0 {
            "rgba(88,166,255,0.8)"
        } else {
            "rgba(139,148,158,0.6)"
        };
        ctx.set_fill_style_str(color);
        ctx.begin_path();
        ctx.arc(x, y, r, 0.0, std::f64::consts::TAU).ok();
        ctx.fill();

        // Dot border
        ctx.set_stroke_style_str(if is_sel {
            "rgb(88,166,255)"
        } else {
            "rgba(230,237,243,0.3)"
        });
        ctx.set_line_width(if is_sel { 2.0 } else { 1.0 });
        ctx.stroke();

        // Band number label inside dot
        ctx.set_fill_style_str(if is_sel || band.gain_cb != 0 {
            "rgba(255,255,255,0.9)"
        } else {
            "rgba(230,237,243,0.5)"
        });
        ctx.set_font("bold 8px system-ui");
        ctx.set_text_align("center");
        ctx.set_text_baseline("middle");
        ctx.fill_text(&format!("{}", i + 1), x, y).ok();
    }
}

/// Hit-test: which band dot (if any) is at canvas position (x, y)?
pub fn hit_test(bands: &[EqBand; 10], enabled: bool, cx: f64, cy: f64) -> Option<usize> {
    let hit_radius = DOT_RADIUS + 8.0; // generous touch target
    let zero_y = gain_to_y(0.0);

    // Check in reverse order so visually-top dots are hit first
    for i in (0..10).rev() {
        let band = &bands[i];
        let x = freq_to_x(band.freq_hz as f64);
        let gain_db = band.gain_cb as f64 / 10.0;
        let y = if enabled { gain_to_y(gain_db) } else { zero_y };

        let dx = cx - x;
        let dy = cy - y;
        if dx * dx + dy * dy <= hit_radius * hit_radius {
            return Some(i);
        }
    }
    None
}

/// Convert canvas coordinates from a mouse/touch event to EQ band parameters.
pub fn coords_to_band(cx: f64, cy: f64) -> (u16, i16) {
    let freq = x_to_freq(cx).round().clamp(20.0, 20000.0) as u16;
    let gain_db = y_to_gain(cy).clamp(-12.0, 12.0);
    // Convert to centibels, round to nearest 5 for usability
    let gain_cb = ((gain_db * 10.0).round() as i16 / 5) * 5;
    (freq, gain_cb)
}

/// Get canvas logical dimensions.
pub fn dimensions() -> (u32, u32) {
    (W as u32, H as u32)
}

/// Canvas helper: rounded rectangle path.
fn round_rect(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
    ctx.move_to(x + r, y);
    ctx.line_to(x + w - r, y);
    ctx.arc_to(x + w, y, x + w, y + r, r).ok();
    ctx.line_to(x + w, y + h - r);
    ctx.arc_to(x + w, y + h, x + w - r, y + h, r).ok();
    ctx.line_to(x + r, y + h);
    ctx.arc_to(x, y + h, x, y + h - r, r).ok();
    ctx.line_to(x, y + r);
    ctx.arc_to(x, y, x + r, y, r).ok();
}

/// Set up mouse/touch interaction on the EQ canvas.
/// - Tap a dot to select it
/// - Drag a dot vertically for gain, horizontally for frequency
/// - `on_select`: called with band index when a dot is tapped
/// - `on_drag`: called with (band_index, freq_hz, gain_cb) during drag
pub fn make_interactive(
    canvas: &web_sys::HtmlCanvasElement,
    on_select: impl Fn(usize) + 'static,
    on_drag: impl Fn(usize, u16, i16) + 'static,
) {
    use std::cell::Cell;
    use std::rc::Rc;

    // Prevent browser scroll/zoom while interacting with the EQ curve
    let el: &web_sys::Element = canvas.as_ref();
    crate::dom::set_style(el, "touch-action", "none");

    let dragging: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let on_select = Rc::new(move |idx: usize| {
        crate::haptic::pulse(5);
        on_select(idx);
    });
    let on_drag = Rc::new(on_drag);

    // Helper to get canvas-relative coordinates from client coordinates
    let get_canvas_pos = {
        let canvas = canvas.clone();
        move |client_x: f64, client_y: f64| -> (f64, f64) {
            let rect = canvas.get_bounding_client_rect();
            let scale_x = W / rect.width();
            let scale_y = H / rect.height();
            let cx = (client_x - rect.left()) * scale_x;
            let cy = (client_y - rect.top()) * scale_y;
            (cx, cy)
        }
    };

    // Mousedown / touchstart: hit-test, start drag
    {
        let dragging = dragging.clone();
        let on_select = on_select.clone();
        let get_pos = get_canvas_pos.clone();
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            let (cx, cy) = get_pos(e.client_x() as f64, e.client_y() as f64);
            let bands =
                crate::state::with(|s| s.eq_state.as_ref().map(|eq| (eq.bands, eq.enabled)));
            if let Some((bands, enabled)) = bands {
                if let Some(idx) = hit_test(&bands, enabled, cx, cy) {
                    dragging.set(Some(idx));
                    on_select(idx);
                }
            }
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    // Mousemove: drag
    {
        let dragging = dragging.clone();
        let on_drag = on_drag.clone();
        let get_pos = get_canvas_pos.clone();
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            if e.buttons() & 1 == 0 {
                dragging.set(None);
                return;
            }
            if let Some(idx) = dragging.get() {
                let (cx, cy) = get_pos(e.client_x() as f64, e.client_y() as f64);
                let (freq, gain_cb) = coords_to_band(cx, cy);
                on_drag(idx, freq, gain_cb);
            }
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("mousemove", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    // Mouseup: end drag
    {
        let dragging = dragging.clone();
        let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
            dragging.set(None);
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("mouseup", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    // Touch events
    {
        let dragging_t = dragging.clone();
        let on_select_t = on_select;
        let on_drag_t = on_drag;
        let get_pos_t = get_canvas_pos;

        let touchstart = Closure::wrap(Box::new(move |e: web_sys::TouchEvent| {
            e.prevent_default();
            if let Some(touch) = e.touches().get(0) {
                let (cx, cy) = get_pos_t(touch.client_x() as f64, touch.client_y() as f64);
                let bands =
                    crate::state::with(|s| s.eq_state.as_ref().map(|eq| (eq.bands, eq.enabled)));
                if let Some((bands, enabled)) = bands {
                    if let Some(idx) = hit_test(&bands, enabled, cx, cy) {
                        dragging_t.set(Some(idx));
                        on_select_t(idx);
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("touchstart", touchstart.as_ref().unchecked_ref())
            .ok();
        touchstart.forget();

        let dragging_m = dragging.clone();
        let on_drag_m = on_drag_t;
        let canvas_m = canvas.clone();
        let touchmove = Closure::wrap(Box::new(move |e: web_sys::TouchEvent| {
            e.prevent_default();
            if let Some(idx) = dragging_m.get() {
                if let Some(touch) = e.touches().get(0) {
                    let rect = canvas_m.get_bounding_client_rect();
                    let scale_x = W / rect.width();
                    let scale_y = H / rect.height();
                    let cx = (touch.client_x() as f64 - rect.left()) * scale_x;
                    let cy = (touch.client_y() as f64 - rect.top()) * scale_y;
                    let (freq, gain_cb) = coords_to_band(cx, cy);
                    on_drag_m(idx, freq, gain_cb);
                }
            }
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("touchmove", touchmove.as_ref().unchecked_ref())
            .ok();
        touchmove.forget();

        let dragging_e = dragging;
        let touchend = Closure::wrap(Box::new(move |_: web_sys::TouchEvent| {
            dragging_e.set(None);
        }) as Box<dyn FnMut(_)>);
        canvas
            .add_event_listener_with_callback("touchend", touchend.as_ref().unchecked_ref())
            .ok();
        touchend.forget();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_band() -> EqBand {
        EqBand {
            freq_hz: 1000,
            gain_cb: 0,
            q_x10: 10,
            filter_type: FilterType::Peak,
        }
    }

    #[test]
    fn freq_axis_endpoints() {
        // 20 Hz maps to the left edge of the plot, 20 kHz to the right edge.
        assert!((freq_to_x(F_MIN) - PAD_L).abs() < 1e-9);
        assert!((freq_to_x(F_MAX) - (PAD_L + PLOT_W)).abs() < 1e-9);
    }

    #[test]
    fn freq_axis_clamps_out_of_range() {
        // Below F_MIN clamps to the left edge, above F_MAX to the right edge.
        assert!((freq_to_x(5.0) - PAD_L).abs() < 1e-9);
        assert!((freq_to_x(50000.0) - (PAD_L + PLOT_W)).abs() < 1e-9);
    }

    #[test]
    fn freq_x_roundtrip() {
        // f -> x -> f recovers the original frequency within tolerance.
        for &f in &[20.0, 100.0, 440.0, 1000.0, 10000.0, 20000.0] {
            let back = x_to_freq(freq_to_x(f));
            let rel = (back - f).abs() / f;
            assert!(rel < 1e-6, "f={} back={}", f, back);
        }
    }

    #[test]
    fn x_to_freq_endpoints() {
        assert!((x_to_freq(PAD_L) - F_MIN).abs() < 1e-6);
        assert!((x_to_freq(PAD_L + PLOT_W) - F_MAX).abs() < 1e-6);
    }

    #[test]
    fn gain_axis_endpoints_and_center() {
        // +12 dB at the top, -12 dB at the bottom, 0 dB at the vertical midpoint.
        assert!((gain_to_y(G_MAX) - PAD_T).abs() < 1e-9);
        assert!((gain_to_y(G_MIN) - (PAD_T + PLOT_H)).abs() < 1e-9);
        assert!((gain_to_y(0.0) - (PAD_T + PLOT_H / 2.0)).abs() < 1e-9);
    }

    #[test]
    fn gain_y_roundtrip() {
        for &db in &[-12.0, -6.0, 0.0, 3.5, 12.0] {
            let back = y_to_gain(gain_to_y(db));
            assert!((back - db).abs() < 1e-9, "db={} back={}", db, back);
        }
    }

    #[test]
    fn flat_band_has_zero_response() {
        // gain_cb == 0 short-circuits to exactly 0 dB at any frequency.
        let band = flat_band();
        assert_eq!(band_response_db(&band, 1000.0), 0.0);
        assert_eq!(band_response_db(&band, 50.0), 0.0);
        assert_eq!(band_response_db(&band, 15000.0), 0.0);
    }

    #[test]
    fn peak_boost_is_positive_near_center() {
        // A +6 dB peak filter should raise the response near its center frequency.
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: 60,
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        let at_center = band_response_db(&band, 1000.0);
        assert!(at_center > 0.0, "expected boost, got {}", at_center);
        // The peak gain should be close to the requested 6 dB.
        assert!((at_center - 6.0).abs() < 1.0, "got {}", at_center);
    }

    #[test]
    fn peak_cut_is_negative_near_center() {
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: -60,
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        let at_center = band_response_db(&band, 1000.0);
        assert!(at_center < 0.0, "expected cut, got {}", at_center);
    }

    #[test]
    fn total_response_disabled_is_zero() {
        let bands = [flat_band(); 10];
        assert_eq!(total_response_db(&bands, false, 1000.0), 0.0);
    }

    #[test]
    fn total_response_sums_bands() {
        // All-flat bands sum to zero; one active band contributes its own response.
        let mut bands = [flat_band(); 10];
        assert_eq!(total_response_db(&bands, true, 1000.0), 0.0);

        bands[0] = EqBand {
            freq_hz: 1000,
            gain_cb: 60,
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        let single = band_response_db(&bands[0], 1000.0);
        let total = total_response_db(&bands, true, 1000.0);
        assert!(
            (total - single).abs() < 1e-9,
            "total={} single={}",
            total,
            single
        );
    }

    #[test]
    fn coords_to_band_left_bottom() {
        // Far-left x clamps to 20 Hz; the y of the 0 dB line yields 0 centibel gain.
        let (freq, gain_cb) = coords_to_band(PAD_L, gain_to_y(0.0));
        assert_eq!(freq, 20);
        assert_eq!(gain_cb, 0);
    }

    #[test]
    fn coords_to_band_right_top_clamps() {
        // Far-right x clamps to 20000 Hz; top y clamps to +12 dB -> +120 centibel.
        let (freq, gain_cb) = coords_to_band(PAD_L + PLOT_W, gain_to_y(G_MAX));
        assert_eq!(freq, 20000);
        assert_eq!(gain_cb, 120);
    }

    #[test]
    fn coords_to_band_gain_rounds_to_nearest_five() {
        // -12 dB -> -120 centibel, divisible by 5.
        let (_freq, gain_cb) = coords_to_band(PAD_L + PLOT_W / 2.0, gain_to_y(G_MIN));
        assert_eq!(gain_cb, -120);
        assert_eq!(gain_cb % 5, 0);
    }

    #[test]
    fn dimensions_match_constants() {
        assert_eq!(dimensions(), (320, 160));
    }
}
