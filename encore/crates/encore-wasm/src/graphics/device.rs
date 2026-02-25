//! Device header graphic — dot-particle Harman Kardon Invoke with golden ring.
//!
//! Procedural 3D-projected cylinder rendered as points. 35-degree
//! elevated third-person perspective. Golden ring at top with
//! glow animation. Lives in the header background.

use std::cell::Cell;
use std::f64::consts::TAU;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

// Ring color — synced with LED ring state, default golden.
thread_local! {
    static ENCORE_R: Cell<u8> = Cell::new(255);
    static ENCORE_G: Cell<u8> = Cell::new(200);
    static ENCORE_B: Cell<u8> = Cell::new(50);
}

/// Set the ring color (syncs with LED ring state from WebSocket).
pub fn set_encore_color(r: u8, g: u8, b: u8) {
    ENCORE_R.with(|c| c.set(r));
    ENCORE_G.with(|c| c.set(g));
    ENCORE_B.with(|c| c.set(b));
}

/// Create the device graphic canvas and start the animation loop.
/// Returns the canvas element to be inserted into the DOM.

pub fn create(width: u32, height: u32) -> web_sys::HtmlCanvasElement {
    let (canvas, ctx) = super::create_canvas(width, height);
    canvas.set_id("device-canvas");
    canvas.style().set_property("position", "absolute").ok();
    canvas.style().set_property("top", "0").ok();
    canvas.style().set_property("right", "0").ok();
    canvas.style().set_property("opacity", "0.3").ok();
    canvas.style().set_property("pointer-events", "none").ok();

    // Start animation
    let w = width as f64;
    let h = height as f64;
    animate(ctx, w, h, 0.0);

    canvas
}


fn animate(ctx: CanvasRenderingContext2d, w: f64, h: f64, time: f64) {
    draw(&ctx, w, h, time);

    let ctx_clone = ctx.clone();
    let cb = Closure::once(move || {
        let now = crate::dom::window()
            .performance()
            .map(|p| p.now())
            .unwrap_or(time + 16.0);
        animate(ctx_clone, w, h, now);
    });
    crate::dom::window()
        .request_animation_frame(cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();
}


fn draw(ctx: &CanvasRenderingContext2d, w: f64, h: f64, time: f64) {
    ctx.clear_rect(0.0, 0.0, w, h);

    // 3D projection parameters
    let view_angle = 35.0_f64.to_radians(); // elevation
    let cos_view = view_angle.cos();
    let sin_view = view_angle.sin();

    // Cylinder parameters (the Harman Kardon Invoke is a tall cylinder)
    let cyl_radius = w * 0.18;
    let cyl_height = h * 0.55;
    let cx = w * 0.7; // offset right
    let cy = h * 0.55; // center vertically

    // Generate dots on cylinder surface
    let rings = 16;
    let dots_per_ring = 24;
    let dot_size = 1.5;

    for ring in 0..rings {
        let t = ring as f64 / (rings - 1) as f64; // 0=top, 1=bottom
        let y3d = -cyl_height / 2.0 + t * cyl_height;

        for dot in 0..dots_per_ring {
            let angle = dot as f64 / dots_per_ring as f64 * TAU;
            let x3d = cyl_radius * angle.cos();
            let z3d = cyl_radius * angle.sin();

            // Apply perspective projection (35-degree view from above)
            let proj_x = cx + x3d;
            let proj_y = cy + y3d * cos_view - z3d * sin_view;

            // Only draw front-facing dots (z > 0 in view space)
            let view_z = z3d * cos_view + y3d * sin_view;
            if view_z < -cyl_radius * 0.3 {
                continue; // behind the cylinder
            }

            // Depth-based alpha (closer dots brighter)
            let depth_alpha = 0.2 + 0.6 * ((view_z + cyl_radius) / (2.0 * cyl_radius)).clamp(0.0, 1.0);

            ctx.set_fill_style_str(&format!("rgba(200,200,210,{:.2})", depth_alpha));
            ctx.begin_path();
            ctx.arc(proj_x, proj_y, dot_size, 0.0, TAU).ok();
            ctx.fill();
        }
    }

    // Top ellipse (viewing from above, the top is an ellipse)
    let top_y_center = cy - cyl_height / 2.0 * cos_view;
    let top_dots = 32;
    for i in 0..top_dots {
        let angle = i as f64 / top_dots as f64 * TAU;
        for ring_r in (1..=4).map(|n| cyl_radius * n as f64 / 4.0) {
            let x = cx + ring_r * angle.cos();
            let y = top_y_center + ring_r * angle.sin() * sin_view;
            ctx.set_fill_style_str("rgba(180,180,190,0.15)");
            ctx.begin_path();
            ctx.arc(x, y, 1.0, 0.0, TAU).ok();
            ctx.fill();
        }
    }

    // Golden ring at the top
    let (hr, hg, hb) = (
        ENCORE_R.with(|c| c.get()),
        ENCORE_G.with(|c| c.get()),
        ENCORE_B.with(|c| c.get()),
    );

    let glow_pulse = 0.5 + 0.3 * (time / 2000.0).sin(); // subtle pulse
    let ring_y = top_y_center;

    // Glow effect — multiple passes with increasing blur
    for pass in 0..3 {
        let alpha = glow_pulse * (0.3 - pass as f64 * 0.08);
        let spread = 4.0 + pass as f64 * 6.0;

        ctx.set_stroke_style_str(&super::rgba_str(hr, hg, hb, alpha));
        ctx.set_line_width(spread);
        ctx.set_line_cap("round");
        ctx.begin_path();
        ctx.ellipse(
            cx, ring_y,
            cyl_radius + 2.0, (cyl_radius + 2.0) * sin_view,
            0.0, 0.0, TAU,
        ).ok();
        ctx.stroke();
    }

    // Solid ring line
    ctx.set_stroke_style_str(&super::rgba_str(hr, hg, hb, glow_pulse));
    ctx.set_line_width(2.0);
    ctx.begin_path();
    ctx.ellipse(
        cx, ring_y,
        cyl_radius + 2.0, (cyl_radius + 2.0) * sin_view,
        0.0, 0.0, TAU,
    ).ok();
    ctx.stroke();
}
