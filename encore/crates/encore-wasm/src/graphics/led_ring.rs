//! LED Ring — 12 ring LEDs + 1 center LED with glow, RAF animation.

use std::cell::Cell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

use encore_common::protocol::LedAnimation;

const NUM_RING: usize = 12;
const NUM_TOTAL: usize = 13; // 12 ring + 1 center

thread_local! {
    /// Epoch counter — incremented to cancel running animation loops.
    static ANIM_EPOCH: Cell<u32> = Cell::new(0);
}

/// Draw a single frame of the LED ring.
/// `colors`: 13 RGB tuples — indices 0..12 are ring (from 12 o'clock CW), index 12 is center.
/// `size`: canvas logical dimension (square).
pub fn draw_frame(ctx: &CanvasRenderingContext2d, size: f64, colors: &[(u8, u8, u8); NUM_TOTAL]) {
    ctx.clear_rect(0.0, 0.0, size, size);

    let cx = size / 2.0;
    let cy = size / 2.0;
    let ring_r = size / 2.0 - 20.0;
    let dot_r = 7.0;
    let center_r = 10.0;

    // Draw 12 ring LEDs starting from 12 o'clock, clockwise
    for i in 0..NUM_RING {
        let angle = (i as f64 / NUM_RING as f64) * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
        let x = cx + ring_r * angle.cos();
        let y = cy + ring_r * angle.sin();
        let (r, g, b) = colors[i];

        if r > 0 || g > 0 || b > 0 {
            ctx.set_shadow_color(&super::rgba_str(r, g, b, 0.8));
            ctx.set_shadow_blur(12.0);
            ctx.set_fill_style_str(&super::rgb_str(r, g, b));
        } else {
            ctx.set_shadow_blur(0.0);
            ctx.set_fill_style_str("rgba(48,54,61,0.5)");
        }

        ctx.begin_path();
        ctx.arc(x, y, dot_r, 0.0, std::f64::consts::TAU).ok();
        ctx.fill();
    }

    // Draw center LED (larger)
    let (cr, cg, cb) = colors[NUM_RING];
    if cr > 0 || cg > 0 || cb > 0 {
        ctx.set_shadow_color(&super::rgba_str(cr, cg, cb, 0.6));
        ctx.set_shadow_blur(16.0);
        ctx.set_fill_style_str(&super::rgb_str(cr, cg, cb));
    } else {
        ctx.set_shadow_blur(0.0);
        ctx.set_fill_style_str("rgba(48,54,61,0.4)");
    }
    ctx.begin_path();
    ctx.arc(cx, cy, center_r, 0.0, std::f64::consts::TAU).ok();
    ctx.fill();

    ctx.set_shadow_blur(0.0);
}

/// Legacy draw — solid color on all LEDs or off.
pub fn draw(ctx: &CanvasRenderingContext2d, size: f64, color: Option<(u8, u8, u8)>) {
    let c = color.unwrap_or((0, 0, 0));
    let colors = [c; NUM_TOTAL];
    draw_frame(ctx, size, &colors);
}

/// Compute per-LED colors for a given animation at time `t_ms`.
fn compute_frame(anim: &LedAnimation, t_ms: f64) -> [(u8, u8, u8); NUM_TOTAL] {
    match anim {
        LedAnimation::Off => [(0, 0, 0); NUM_TOTAL],
        LedAnimation::Solid { r, g, b } => [(*r, *g, *b); NUM_TOTAL],
        LedAnimation::Breathe { r, g, b, period_ms } => {
            let period = *period_ms as f64;
            let phase = (std::f64::consts::TAU * t_ms / period).sin();
            let intensity = (phase + 1.0) / 2.0; // 0..1
            let ri = (*r as f64 * intensity) as u8;
            let gi = (*g as f64 * intensity) as u8;
            let bi = (*b as f64 * intensity) as u8;
            [(ri, gi, bi); NUM_TOTAL]
        }
        LedAnimation::Spin { r, g, b, speed } => {
            let spd = *speed as f64;
            let rot_period = 1000.0 / spd.max(0.5);
            let pos = (t_ms / rot_period * NUM_RING as f64) % NUM_RING as f64;
            let mut colors = [(0u8, 0u8, 0u8); NUM_TOTAL];
            for i in 0..NUM_RING {
                let dist = {
                    let d = (i as f64 - pos).rem_euclid(NUM_RING as f64);
                    d.min(NUM_RING as f64 - d)
                };
                let tail_len = 4.0;
                let brightness = (1.0 - dist / tail_len).max(0.0);
                colors[i] = (
                    (*r as f64 * brightness) as u8,
                    (*g as f64 * brightness) as u8,
                    (*b as f64 * brightness) as u8,
                );
            }
            // Center follows brightest
            colors[NUM_RING] = (
                (*r as f64 * 0.3) as u8,
                (*g as f64 * 0.3) as u8,
                (*b as f64 * 0.3) as u8,
            );
            colors
        }
        LedAnimation::Pulse { r, g, b } => {
            let cycle = t_ms % 1000.0;
            let intensity = if cycle < 100.0 {
                cycle / 100.0 // sharp attack
            } else {
                1.0 - (cycle - 100.0) / 900.0 // slow decay
            };
            let intensity = intensity.max(0.0);
            let ri = (*r as f64 * intensity) as u8;
            let gi = (*g as f64 * intensity) as u8;
            let bi = (*b as f64 * intensity) as u8;
            [(ri, gi, bi); NUM_TOTAL]
        }
        LedAnimation::VolumeArc { level } => {
            let lit = (*level as f64 / 100.0 * NUM_RING as f64).round() as usize;
            let mut colors = [(0u8, 0u8, 0u8); NUM_TOTAL];
            for i in 0..NUM_RING {
                if i < lit {
                    colors[i] = (255, 255, 255);
                }
            }
            colors
        }
        LedAnimation::BootSurge => {
            let phase = (std::f64::consts::TAU * t_ms / 3000.0).sin();
            let intensity = (phase + 1.0) / 2.0;
            let r = (255.0 * intensity) as u8;
            let g = (180.0 * intensity) as u8;
            let b = (30.0 * intensity) as u8;
            [(r, g, b); NUM_TOTAL]
        }
        LedAnimation::SafeMode => {
            // Amber/blue alternating sectors rotating at ~0.5 rev/sec
            let offset = ((t_ms / 1000.0 * 0.5) * NUM_RING as f64) as usize % NUM_RING;
            let mut colors = [(0u8, 0u8, 0u8); NUM_TOTAL];
            for i in 0..NUM_RING {
                if (i + offset) % NUM_RING < NUM_RING / 2 {
                    colors[i] = (255, 160, 48); // Amber
                } else {
                    colors[i] = (32, 96, 255); // Blue
                }
            }
            // Center: alternates amber/blue every 2 seconds
            let center_amber = ((t_ms / 2000.0) as u32) % 2 == 0;
            colors[NUM_RING] = if center_amber { (255, 160, 48) } else { (32, 96, 255) };
            colors
        }
        LedAnimation::Custom { ref frames } => {
            if frames.is_empty() {
                return [(0, 0, 0); NUM_TOTAL];
            }
            // Advance through frames based on cumulative durations
            let total_dur: f64 = frames.iter().map(|f| f.duration_ms as f64).sum();
            if total_dur <= 0.0 {
                return frames[0].colors;
            }
            let t = t_ms % total_dur;
            let mut accum = 0.0;
            for f in frames {
                accum += f.duration_ms as f64;
                if t < accum {
                    return f.colors;
                }
            }
            frames.last().unwrap().colors
        }
    }
}

/// Start an animation loop on the LED ring canvas (by ID "led-ring-canvas").
/// Cancels any previous animation via epoch counter.
pub fn start_animation(anim: LedAnimation) {
    // Bump epoch to cancel any running loop
    let epoch = ANIM_EPOCH.with(|e| {
        let next = e.get().wrapping_add(1);
        e.set(next);
        next
    });

    run_anim_frame(anim, epoch);
}

fn run_anim_frame(anim: LedAnimation, epoch: u32) {
    // Check if we've been cancelled
    let current = ANIM_EPOCH.with(|e| e.get());
    if current != epoch {
        return;
    }

    // Get canvas and draw
    if let Some(canvas_el) = crate::dom::get_el("led-ring-canvas") {
        if let Some(canvas) = canvas_el.dyn_ref::<web_sys::HtmlCanvasElement>() {
            if let Ok(Some(ctx)) = canvas.get_context("2d") {
                let ctx: CanvasRenderingContext2d = ctx.unchecked_into();
                let now = crate::dom::window()
                    .performance()
                    .map(|p| p.now())
                    .unwrap_or(0.0);
                let colors = compute_frame(&anim, now);
                draw_frame(&ctx, 200.0, &colors);
            }
        }
    }

    // Schedule next frame
    let anim_clone = anim.clone();
    let cb = Closure::once(move || {
        run_anim_frame(anim_clone, epoch);
    });
    crate::dom::window()
        .request_animation_frame(cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();
}

/// Stop any running animation loop.
pub fn stop_animation() {
    ANIM_EPOCH.with(|e| e.set(e.get().wrapping_add(1)));
}
