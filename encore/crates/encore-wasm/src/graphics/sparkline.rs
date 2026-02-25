//! Sparkline — time-series line chart for CPU history.

use web_sys::CanvasRenderingContext2d;

/// Draw a sparkline chart.
/// data: slice of values 0-100, rendered left to right.
/// w/h: logical pixel dimensions of the canvas.
pub fn draw(ctx: &CanvasRenderingContext2d, data: &[u8], w: f64, h: f64) {
    ctx.clear_rect(0.0, 0.0, w, h);

    if data.is_empty() {
        return;
    }

    let padding = 2.0;
    let chart_h = h - padding * 2.0;
    let chart_w = w - padding * 2.0;

    // Grid lines (25%, 50%, 75%)
    ctx.set_stroke_style_str("rgba(48,54,61,0.5)");
    ctx.set_line_width(0.5);
    for pct in [25.0, 50.0, 75.0] {
        let y = padding + chart_h * (1.0 - pct / 100.0);
        ctx.begin_path();
        ctx.move_to(padding, y);
        ctx.line_to(w - padding, y);
        ctx.stroke();
    }

    let step = if data.len() > 1 {
        chart_w / (data.len() - 1) as f64
    } else {
        chart_w
    };

    // Build path
    ctx.begin_path();
    for (i, &val) in data.iter().enumerate() {
        let x = padding + i as f64 * step;
        let y = padding + chart_h * (1.0 - val as f64 / 100.0);
        if i == 0 {
            ctx.move_to(x, y);
        } else {
            ctx.line_to(x, y);
        }
    }

    // Stroke line
    ctx.set_stroke_style_str("#58a6ff");
    ctx.set_line_width(1.5);
    ctx.stroke();

    // Fill gradient below line
    ctx.line_to(padding + (data.len() - 1) as f64 * step, h - padding);
    ctx.line_to(padding, h - padding);
    ctx.close_path();
    ctx.set_fill_style_str("rgba(88,166,255,0.1)");
    ctx.fill();

    // Current value dot
    if let Some(&last) = data.last() {
        let x = padding + (data.len() - 1) as f64 * step;
        let y = padding + chart_h * (1.0 - last as f64 / 100.0);
        ctx.begin_path();
        ctx.arc(x, y, 3.0, 0.0, std::f64::consts::TAU).ok();
        ctx.set_fill_style_str("#58a6ff");
        ctx.fill();
    }
}
