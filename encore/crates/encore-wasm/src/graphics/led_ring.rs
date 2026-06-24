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
    static ANIM_EPOCH: Cell<u32> = const { Cell::new(0) };
    /// performance.now() of the last painted frame, for the fps cap.
    static LAST_PAINT: Cell<f64> = const { Cell::new(0.0) };
}

/// Target frame interval in ms for a given fps cap.
fn frame_interval_ms(fps: f64) -> f64 {
    1000.0 / fps
}

/// fps cap for the current viewport: 30 on desktop (>640px), 20 on device/mobile.
fn target_fps() -> f64 {
    let wide = crate::dom::window()
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .map(|w| w > 640.0)
        .unwrap_or(false);
    if wide {
        30.0
    } else {
        20.0
    }
}

/// True for animations that produce one fixed frame, so the loop can paint once
/// and stop instead of rescheduling forever.
fn is_static(anim: &LedAnimation) -> bool {
    matches!(
        anim,
        LedAnimation::Off | LedAnimation::Solid { .. } | LedAnimation::VolumeArc { .. }
    )
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

    // Off-LED well color tracks the theme so the ring reads on a light stage too.
    let (wr, wg, wb) = super::theme::grid_rgb(super::theme::is_dark());
    let off_well = super::rgba_str(wr, wg, wb, 0.18);

    // Draw 12 ring LEDs starting from 12 o'clock, clockwise
    for i in 0..NUM_RING {
        let angle =
            (i as f64 / NUM_RING as f64) * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
        let x = cx + ring_r * angle.cos();
        let y = cy + ring_r * angle.sin();
        let (r, g, b) = colors[i];

        if r > 0 || g > 0 || b > 0 {
            ctx.set_shadow_color(&super::rgba_str(r, g, b, 0.8));
            ctx.set_shadow_blur(4.0);
            ctx.set_fill_style_str(&super::rgb_str(r, g, b));
        } else {
            ctx.set_shadow_blur(0.0);
            ctx.set_fill_style_str(&off_well);
        }

        ctx.begin_path();
        ctx.arc(x, y, dot_r, 0.0, std::f64::consts::TAU).ok();
        ctx.fill();
    }

    // Draw center LED (larger)
    let (cr, cg, cb) = colors[NUM_RING];
    if cr > 0 || cg > 0 || cb > 0 {
        ctx.set_shadow_color(&super::rgba_str(cr, cg, cb, 0.6));
        ctx.set_shadow_blur(4.0);
        ctx.set_fill_style_str(&super::rgb_str(cr, cg, cb));
    } else {
        ctx.set_shadow_blur(0.0);
        ctx.set_fill_style_str(&off_well);
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
            let center_amber = ((t_ms / 2000.0) as u32).is_multiple_of(2);
            colors[NUM_RING] = if center_amber {
                (255, 160, 48)
            } else {
                (32, 96, 255)
            };
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
    // Bump epoch to cancel any running loop.
    let epoch = ANIM_EPOCH.with(|e| {
        let next = e.get().wrapping_add(1);
        e.set(next);
        next
    });
    LAST_PAINT.with(|p| p.set(0.0));

    // Static variants, and every variant under reduced motion, paint exactly one
    // frame and never reschedule. Listen for the reduced-motion setting changing
    // so toggling the OS preference restarts/stops the loop without a reload.
    install_motion_listener();
    if is_static(&anim) || super::theme::reduce_motion() {
        draw_once(&anim);
        return;
    }

    run_anim_frame(anim, epoch);
}

/// Paint a single static frame (no rescheduling).
fn draw_once(anim: &LedAnimation) {
    if let Some(canvas_el) = crate::dom::get_el("led-ring-canvas") {
        if let Some(canvas) = canvas_el.dyn_ref::<web_sys::HtmlCanvasElement>() {
            if let Ok(Some(ctx)) = canvas.get_context("2d") {
                let ctx: CanvasRenderingContext2d = ctx.unchecked_into();
                let now = crate::dom::window()
                    .performance()
                    .map(|p| p.now())
                    .unwrap_or(0.0);
                let colors = compute_frame(anim, now);
                draw_frame(&ctx, 200.0, &colors);
            }
        }
    }
}

/// Register a one-time `change` listener on the reduced-motion media query so a
/// running loop stops (and a stopped one is not silently left animating) when
/// the user toggles the OS setting. Idempotent via a thread-local guard.
fn install_motion_listener() {
    thread_local! {
        static INSTALLED: Cell<bool> = const { Cell::new(false) };
    }
    if INSTALLED.with(|i| i.replace(true)) {
        return;
    }
    if let Ok(Some(mql)) = crate::dom::window().match_media("(prefers-reduced-motion: reduce)") {
        let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            // On change, cancel the current loop. The next start_animation (route
            // re-render) re-evaluates the gate; meanwhile a static frame remains.
            stop_animation();
        }) as Box<dyn FnMut(_)>);
        mql.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }
}

fn run_anim_frame(anim: LedAnimation, epoch: u32) {
    // Cancelled?
    let current = ANIM_EPOCH.with(|e| e.get());
    if current != epoch {
        return;
    }

    let now = crate::dom::window()
        .performance()
        .map(|p| p.now())
        .unwrap_or(0.0);

    // fps cap: only repaint once the target interval has elapsed. We still wake
    // on rAF, but we skip the expensive canvas work in between.
    let interval = frame_interval_ms(target_fps());
    let last = LAST_PAINT.with(|p| p.get());
    if now - last >= interval {
        LAST_PAINT.with(|p| p.set(now));
        if let Some(canvas_el) = crate::dom::get_el("led-ring-canvas") {
            if let Some(canvas) = canvas_el.dyn_ref::<web_sys::HtmlCanvasElement>() {
                if let Ok(Some(ctx)) = canvas.get_context("2d") {
                    let ctx: CanvasRenderingContext2d = ctx.unchecked_into();
                    let colors = compute_frame(&anim, now);
                    draw_frame(&ctx, 200.0, &colors);
                }
            }
        }
    }

    // Schedule next frame.
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

#[cfg(test)]
mod tests {
    use super::*;
    use encore_common::protocol::LedFrame;

    #[test]
    fn off_is_all_black() {
        let frame = compute_frame(&LedAnimation::Off, 0.0);
        assert_eq!(frame, [(0, 0, 0); NUM_TOTAL]);
    }

    #[test]
    fn off_is_black_at_any_time() {
        let frame = compute_frame(&LedAnimation::Off, 12345.678);
        assert!(frame.iter().all(|&c| c == (0, 0, 0)));
    }

    #[test]
    fn solid_sets_every_led() {
        let frame = compute_frame(
            &LedAnimation::Solid {
                r: 12,
                g: 34,
                b: 56,
            },
            999.0,
        );
        assert_eq!(frame.len(), NUM_TOTAL);
        assert!(frame.iter().all(|&c| c == (12, 34, 56)));
    }

    #[test]
    fn solid_independent_of_time() {
        let a = compute_frame(&LedAnimation::Solid { r: 1, g: 2, b: 3 }, 0.0);
        let b = compute_frame(&LedAnimation::Solid { r: 1, g: 2, b: 3 }, 5000.0);
        assert_eq!(a, b);
    }

    #[test]
    fn breathe_half_intensity_at_t_zero() {
        // sin(0) == 0 -> intensity = (0 + 1) / 2 = 0.5
        let frame = compute_frame(
            &LedAnimation::Breathe {
                r: 200,
                g: 100,
                b: 50,
                period_ms: 1000,
            },
            0.0,
        );
        // floor(200 * 0.5) = 100, floor(100 * 0.5) = 50, floor(50 * 0.5) = 25
        assert!(frame.iter().all(|&c| c == (100, 50, 25)));
    }

    #[test]
    fn breathe_peak_at_quarter_period() {
        // t = period/4 -> sin(TAU * 0.25) = sin(PI/2) = 1 -> intensity = 1.0
        let frame = compute_frame(
            &LedAnimation::Breathe {
                r: 200,
                g: 100,
                b: 50,
                period_ms: 1000,
            },
            250.0,
        );
        assert!(frame.iter().all(|&c| c == (200, 100, 50)));
    }

    #[test]
    fn volume_arc_full_lights_all_ring() {
        // level 100 -> lit = round(1.0 * 12) = 12, every ring LED is white.
        let frame = compute_frame(&LedAnimation::VolumeArc { level: 100 }, 0.0);
        for i in 0..NUM_RING {
            assert_eq!(frame[i], (255, 255, 255), "ring led {}", i);
        }
        // Center LED is not part of the arc.
        assert_eq!(frame[NUM_RING], (0, 0, 0));
    }

    #[test]
    fn volume_arc_zero_lights_nothing() {
        let frame = compute_frame(&LedAnimation::VolumeArc { level: 0 }, 0.0);
        assert!(frame.iter().all(|&c| c == (0, 0, 0)));
    }

    #[test]
    fn volume_arc_half_lights_six() {
        // level 50 -> lit = round(0.5 * 12) = 6 ring LEDs.
        let frame = compute_frame(&LedAnimation::VolumeArc { level: 50 }, 0.0);
        for i in 0..NUM_RING {
            let expected = if i < 6 { (255, 255, 255) } else { (0, 0, 0) };
            assert_eq!(frame[i], expected, "ring led {}", i);
        }
    }

    #[test]
    fn pulse_attack_start_is_dark() {
        // t = 0 -> cycle 0 -> intensity = 0/100 = 0 -> all off.
        let frame = compute_frame(
            &LedAnimation::Pulse {
                r: 100,
                g: 100,
                b: 100,
            },
            0.0,
        );
        assert!(frame.iter().all(|&c| c == (0, 0, 0)));
    }

    #[test]
    fn pulse_peak_at_attack_end() {
        // t = 100 -> cycle 100 -> decay branch with (100-100)/900 = 0 -> intensity 1.0.
        let frame = compute_frame(
            &LedAnimation::Pulse {
                r: 100,
                g: 80,
                b: 60,
            },
            100.0,
        );
        assert!(frame.iter().all(|&c| c == (100, 80, 60)));
    }

    #[test]
    fn custom_empty_is_black() {
        let frame = compute_frame(&LedAnimation::Custom { frames: vec![] }, 0.0);
        assert_eq!(frame, [(0, 0, 0); NUM_TOTAL]);
    }

    #[test]
    fn custom_single_frame_returned() {
        let colors = [(7, 8, 9); NUM_TOTAL];
        let anim = LedAnimation::Custom {
            frames: vec![LedFrame {
                colors,
                duration_ms: 100,
            }],
        };
        // Any time within the frame returns that frame's colors.
        assert_eq!(compute_frame(&anim, 0.0), colors);
        assert_eq!(compute_frame(&anim, 50.0), colors);
        // After the single frame's duration, wraps back to it.
        assert_eq!(compute_frame(&anim, 250.0), colors);
    }

    #[test]
    fn custom_selects_frame_by_cumulative_duration() {
        let a = [(1, 1, 1); NUM_TOTAL];
        let b = [(2, 2, 2); NUM_TOTAL];
        let anim = LedAnimation::Custom {
            frames: vec![
                LedFrame {
                    colors: a,
                    duration_ms: 100,
                },
                LedFrame {
                    colors: b,
                    duration_ms: 100,
                },
            ],
        };
        // t in [0,100) -> first frame; t in [100,200) -> second frame.
        assert_eq!(compute_frame(&anim, 0.0), a);
        assert_eq!(compute_frame(&anim, 99.0), a);
        assert_eq!(compute_frame(&anim, 100.0), b);
        assert_eq!(compute_frame(&anim, 150.0), b);
    }

    #[test]
    fn custom_zero_total_duration_returns_first() {
        let a = [(5, 5, 5); NUM_TOTAL];
        let anim = LedAnimation::Custom {
            frames: vec![LedFrame {
                colors: a,
                duration_ms: 0,
            }],
        };
        assert_eq!(compute_frame(&anim, 1234.0), a);
    }

    #[test]
    fn draw_legacy_color_is_uniform_via_compute() {
        // Solid mirrors the legacy `draw(Some(color))` fill of all 13 LEDs.
        let frame = compute_frame(&LedAnimation::Solid { r: 255, g: 0, b: 0 }, 0.0);
        assert_eq!(frame, [(255, 0, 0); NUM_TOTAL]);
    }

    #[test]
    fn frame_interval_caps_at_target_fps() {
        // 20 fps -> 50 ms; 30 fps -> ~33.33 ms.
        assert_eq!(frame_interval_ms(20.0), 50.0);
        assert!((frame_interval_ms(30.0) - 33.3333).abs() < 0.01);
    }

    #[test]
    fn static_variants_do_not_reschedule() {
        assert!(is_static(&LedAnimation::Off));
        assert!(is_static(&LedAnimation::Solid { r: 1, g: 2, b: 3 }));
        assert!(is_static(&LedAnimation::VolumeArc { level: 50 }));
        assert!(!is_static(&LedAnimation::Breathe {
            r: 1,
            g: 2,
            b: 3,
            period_ms: 1000
        }));
        assert!(!is_static(&LedAnimation::Spin {
            r: 1,
            g: 2,
            b: 3,
            speed: 1
        }));
    }
}
