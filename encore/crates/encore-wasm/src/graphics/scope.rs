//! Oscilloscope — waveform display with green phosphor line.
//!
//! Subtle grid background, green waveform line.

use web_sys::CanvasRenderingContext2d;

const W: f64 = 320.0;
const H: f64 = 80.0;

/// Draw the oscilloscope waveform.
/// `samples`: normalized audio samples in range [-1.0, 1.0].
pub fn draw(ctx: &CanvasRenderingContext2d, samples: &[f32]) {
    ctx.clear_rect(0.0, 0.0, W, H);
    let is_dark = super::theme::is_dark();

    // Background
    ctx.set_fill_style_str(super::theme::canvas_bg(is_dark));
    ctx.fill_rect(0.0, 0.0, W, H);

    // Grid
    let (gr, gg, gb) = super::theme::grid_rgb(is_dark);
    ctx.set_stroke_style_str(&super::rgba_str(gr, gg, gb, 0.12));
    ctx.set_line_width(0.5);
    for i in 1..4 {
        let y = H * i as f64 / 4.0;
        ctx.begin_path();
        ctx.move_to(0.0, y);
        ctx.line_to(W, y);
        ctx.stroke();
    }
    for i in 1..8 {
        let x = W * i as f64 / 8.0;
        ctx.begin_path();
        ctx.move_to(x, 0.0);
        ctx.line_to(x, H);
        ctx.stroke();
    }

    // Center line (0V)
    ctx.set_stroke_style_str(&super::rgba_str(gr, gg, gb, 0.25));
    ctx.set_line_width(1.0);
    ctx.begin_path();
    ctx.move_to(0.0, H / 2.0);
    ctx.line_to(W, H / 2.0);
    ctx.stroke();

    if samples.is_empty() {
        return;
    }

    // Waveform — phosphor green, darkened in the light theme so it reads on cream.
    let (wr, wg, wb) = if is_dark {
        (63, 185, 80)
    } else {
        (26, 127, 55)
    };
    ctx.set_stroke_style_str(&super::rgba_str(wr, wg, wb, 1.0));
    ctx.set_line_width(1.5);
    ctx.set_shadow_color(&super::rgba_str(wr, wg, wb, 0.4));
    ctx.set_shadow_blur(4.0);

    ctx.begin_path();
    let step = W / samples.len().max(1) as f64;
    for (i, &sample) in samples.iter().enumerate() {
        let x = i as f64 * step;
        let y = H / 2.0 - sample as f64 * (H / 2.0 - 4.0);
        if i == 0 {
            ctx.move_to(x, y);
        } else {
            ctx.line_to(x, y);
        }
    }
    ctx.stroke();

    ctx.set_shadow_blur(0.0);
}
