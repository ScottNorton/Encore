//! FFT Spectrum Analyzer — 32-bar frequency display with gradient coloring.
//!
//! Green→yellow→red by level, peak hold with decay.

use std::cell::RefCell;
use web_sys::CanvasRenderingContext2d;

const W: f64 = 320.0;
const H: f64 = 80.0;
const NUM_BARS: usize = 32;
const BAR_GAP: f64 = 2.0;
const PEAK_DECAY: f32 = 0.02;

thread_local! {
    static PEAKS: RefCell<[f32; NUM_BARS]> = const { RefCell::new([0.0; NUM_BARS]) };
}

/// Draw the spectrum analyzer.
/// `bins`: 32 magnitude values in range [0.0, 1.0].
pub fn draw(ctx: &CanvasRenderingContext2d, bins: &[f32; NUM_BARS]) {
    ctx.clear_rect(0.0, 0.0, W, H);

    // Background
    ctx.set_fill_style_str("rgba(13,17,23,0.6)");
    ctx.fill_rect(0.0, 0.0, W, H);

    let bar_w = (W - BAR_GAP * (NUM_BARS as f64 - 1.0)) / NUM_BARS as f64;

    PEAKS.with(|p| {
        let mut peaks = p.borrow_mut();

        for (i, &level) in bins.iter().enumerate() {
            let x = i as f64 * (bar_w + BAR_GAP);
            let bar_h = level as f64 * (H - 4.0);
            let y = H - bar_h;

            // Color gradient based on level
            let color = if level > 0.8 {
                "rgb(248,81,73)" // red
            } else if level > 0.5 {
                "rgb(227,179,65)" // yellow
            } else {
                "rgb(63,185,80)" // green
            };

            ctx.set_fill_style_str(color);
            ctx.fill_rect(x, y, bar_w, bar_h);

            // Peak hold
            if level > peaks[i] {
                peaks[i] = level;
            } else {
                peaks[i] = (peaks[i] - PEAK_DECAY).max(0.0);
            }

            // Draw peak line
            let peak_y = H - peaks[i] as f64 * (H - 4.0);
            ctx.set_fill_style_str("rgba(230,237,243,0.8)");
            ctx.fill_rect(x, peak_y - 1.0, bar_w, 2.0);
        }
    });
}
