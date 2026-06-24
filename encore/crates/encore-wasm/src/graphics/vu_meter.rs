//! VU Meter — segmented LED-style audio level bars with dB scaling and peak hold.

use std::cell::Cell;
use web_sys::CanvasRenderingContext2d;

const SEGMENTS: usize = 40;
const DB_MIN: f64 = -60.0;
const DB_MAX: f64 = 0.0;
const DB_RANGE: f64 = 60.0; // DB_MAX - DB_MIN

// Client-side peak hold with decay (WASM is single-threaded)
thread_local! {
    static PEAK_HOLD_L: Cell<f32> = const { Cell::new(0.0) };
    static PEAK_HOLD_R: Cell<f32> = const { Cell::new(0.0) };
    static MIC_PEAK_HOLD_L: Cell<f32> = const { Cell::new(0.0) };
    static MIC_PEAK_HOLD_R: Cell<f32> = const { Cell::new(0.0) };
}

/// Convert linear amplitude (0.0-1.0) to dB.
fn linear_to_db(v: f64) -> f64 {
    if v < 1e-6 {
        DB_MIN
    } else {
        (20.0 * v.log10()).clamp(DB_MIN, DB_MAX)
    }
}

/// Map dB value to meter position (0.0-1.0).
fn db_to_pos(db: f64) -> f64 {
    ((db - DB_MIN) / DB_RANGE).clamp(0.0, 1.0)
}

/// Get segment color based on its position along the meter and the theme.
fn segment_color(seg: usize, lit: bool, is_dark: bool) -> &'static str {
    let t = seg as f64 / SEGMENTS as f64;
    crate::graphics::theme::level_color(t, lit, is_dark)
}

/// Draw horizontal VU meter bars (segmented LED style, dB-proportional).
/// left/right: RMS values 0.0-1.0
/// left_peak/right_peak: instantaneous peak values 0.0-1.0
pub fn draw(
    ctx: &CanvasRenderingContext2d,
    w: f64,
    h: f64,
    left: f32,
    right: f32,
    left_peak: f32,
    right_peak: f32,
) {
    PEAK_HOLD_L.with(|lh| {
        PEAK_HOLD_R.with(|rh| {
            draw_impl(
                ctx, w, h, left, right, left_peak, right_peak, lh, rh, "L", "R",
            );
        })
    });
}

/// Draw horizontal VU meter bars for microphone input.
/// Uses separate peak hold state from output meters.
pub fn draw_mic(
    ctx: &CanvasRenderingContext2d,
    w: f64,
    h: f64,
    left: f32,
    right: f32,
    left_peak: f32,
    right_peak: f32,
) {
    MIC_PEAK_HOLD_L.with(|lh| {
        MIC_PEAK_HOLD_R.with(|rh| {
            draw_impl(
                ctx, w, h, left, right, left_peak, right_peak, lh, rh, "Far", "Near",
            );
        })
    });
}

fn draw_impl(
    ctx: &CanvasRenderingContext2d,
    w: f64,
    h: f64,
    left: f32,
    right: f32,
    left_peak: f32,
    right_peak: f32,
    peak_hold_l: &Cell<f32>,
    peak_hold_r: &Cell<f32>,
    label_l: &str,
    label_r: &str,
) {
    ctx.clear_rect(0.0, 0.0, w, h);

    // Convert linear to dB-proportional meter position
    let l_pos = db_to_pos(linear_to_db(left as f64));
    let r_pos = db_to_pos(linear_to_db(right as f64));
    let l_peak_pos = db_to_pos(linear_to_db(left_peak as f64));
    let r_peak_pos = db_to_pos(linear_to_db(right_peak as f64));

    // Peak hold: latch new peaks, decay old ones (~10Hz update rate)
    let mut lv = peak_hold_l.get();
    let mut rv = peak_hold_r.get();
    if l_peak_pos as f32 > lv {
        lv = l_peak_pos as f32;
    } else {
        lv *= 0.93;
    }
    if r_peak_pos as f32 > rv {
        rv = r_peak_pos as f32;
    } else {
        rv *= 0.93;
    }
    if lv < 0.005 {
        lv = 0.0;
    }
    if rv < 0.005 {
        rv = 0.0;
    }
    peak_hold_l.set(lv);
    peak_hold_r.set(rv);
    let (l_hold, r_hold) = (lv as f64, rv as f64);

    // Layout
    let label_w = if label_l.len() > 1 { 28.0 } else { 14.0 };
    let bar_x = label_w;
    let bar_w = w - label_w - 4.0;
    let scale_h = 14.0;
    let gap = 4.0;
    let pad_y = 2.0;
    let bar_h = (h - scale_h - gap - pad_y * 2.0) / 2.0;
    let y_l = pad_y;
    let y_r = pad_y + bar_h + gap;
    let scale_y = y_r + bar_h + 6.0;

    // Segment geometry
    let total_gaps = (SEGMENTS as f64 - 1.0) * 1.5;
    let seg_w = (bar_w - total_gaps) / SEGMENTS as f64;

    // Draw segmented bars
    draw_segmented_bar(ctx, bar_x, y_l, bar_w, bar_h, seg_w, l_pos, l_hold);
    draw_segmented_bar(ctx, bar_x, y_r, bar_w, bar_h, seg_w, r_pos, r_hold);

    // Channel labels
    let (mr, mg, mb) = crate::graphics::theme::muted_rgb(crate::graphics::theme::is_dark());
    ctx.set_fill_style_str(&crate::graphics::rgba_str(mr, mg, mb, 0.95));
    ctx.set_font("bold 10px system-ui");
    ctx.set_text_align("left");
    ctx.set_text_baseline("middle");
    ctx.fill_text(label_l, 2.0, y_l + bar_h / 2.0).ok();
    ctx.fill_text(label_r, 2.0, y_r + bar_h / 2.0).ok();

    // dB scale labels
    ctx.set_font("9px system-ui");
    ctx.set_fill_style_str(&crate::graphics::rgba_str(mr, mg, mb, 0.7));
    ctx.set_text_align("center");
    ctx.set_text_baseline("top");
    for &(db, label) in &[
        (-48.0, "-48"),
        (-24.0, "-24"),
        (-12.0, "-12"),
        (-6.0, "-6"),
        (-3.0, "-3"),
        (0.0, "0"),
    ] {
        let x = bar_x + db_to_pos(db) * bar_w;
        ctx.fill_text(label, x, scale_y).ok();
        // Subtle tick mark
        ctx.set_stroke_style_str(&crate::graphics::rgba_str(mr, mg, mb, 0.2));
        ctx.set_line_width(0.5);
        ctx.begin_path();
        ctx.move_to(x, scale_y - 2.0);
        ctx.line_to(x, scale_y);
        ctx.stroke();
    }
}

fn draw_segmented_bar(
    ctx: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    _w: f64,
    h: f64,
    seg_w: f64,
    level: f64,
    peak_hold: f64,
) {
    let lit_count = (level * SEGMENTS as f64).ceil() as usize;
    let peak_seg = if peak_hold > 0.01 {
        Some((peak_hold * (SEGMENTS as f64 - 1.0)).round() as usize)
    } else {
        None
    };
    let seg_gap = 1.5;
    let radius = 1.5;
    let is_dark = crate::graphics::theme::is_dark();

    for i in 0..SEGMENTS {
        let sx = x + i as f64 * (seg_w + seg_gap);
        let lit = i < lit_count;
        let is_peak_hold = peak_seg == Some(i) && !lit;

        if is_peak_hold {
            // Peak hold: high-contrast marker against the current theme
            ctx.set_fill_style_str(if is_dark {
                "rgba(255,255,255,0.8)"
            } else {
                "rgba(0,0,0,0.55)"
            });
        } else {
            ctx.set_fill_style_str(segment_color(i, lit, is_dark));
        }

        ctx.begin_path();
        rounded_rect(ctx, sx, y, seg_w, h, radius);
        ctx.fill();
    }
}

fn rounded_rect(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
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
