//! Ovation Bloom — the now-playing centerpiece: a real `<img id="sp-art-img">`
//! album center, an `aria-hidden` canvas painting the gold progress arc and an
//! outer ring of audio-reactive "applause" rays, and a `role="slider"` seek
//! overlay. The reactive loop is energy-gated and reduced-motion-gated and
//! reads state per frame with zero String allocation (see the perf budget).

use std::cell::Cell;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

const NUM_RAYS_DEVICE: usize = 32;
#[allow(dead_code)] // desktop ray count is consumed by the P4 viewport-tiered loop
const NUM_RAYS_DESKTOP: usize = 56;

thread_local! {
    /// Epoch for the bloom rAF loop — bumped to park/cancel (the led_ring pattern).
    static OVATION_EPOCH: Cell<u32> = const { Cell::new(0) };
    static LEVELS: Cell<(f32, f32, f32, f32)> = const { Cell::new((0.0, 0.0, 0.0, 0.0)) };
    static SPECTRUM: Cell<Option<[f32; 32]>> = const { Cell::new(None) };
    /// Per-band display value eased toward SPECTRUM each frame, so the modest ~8 Hz
    /// device spectrum renders fluidly at the loop's framerate instead of snapping.
    static SMOOTHED_SPEC: Cell<[f32; 32]> = const { Cell::new([0.0; 32]) };
    static LAST_LEVEL_MS: Cell<f64> = const { Cell::new(0.0) };
    static RUNNING: Cell<bool> = const { Cell::new(false) };
    static ENV: Cell<f32> = const { Cell::new(0.0) };
    static LOW_SINCE_MS: Cell<f64> = const { Cell::new(0.0) };
    static LAST_PAINT_MS: Cell<f64> = const { Cell::new(0.0) };
    static LAST_FLASH_MS: Cell<f64> = const { Cell::new(0.0) };
}

/// Attack/release smoothing toward a target, the vu_meter peak-decay idea: fast
/// attack (rising), slow release (falling). `attack`/`release` are 0..1 blend
/// factors. Pure.
pub fn smooth(cur: f32, target: f32, attack: f32, release: f32) -> f32 {
    let k = if target > cur { attack } else { release };
    cur + (target - cur) * k
}

/// Map a 0..1 shaped energy to a ray length in pixels for radius `r`.
/// `len = r * (0.08 + 0.36 * shaped)`: a calm-but-visible floor (~8% of r) that
/// blooms to ~44% of r at full energy. With the ring base at 0.62*r the tips
/// reach ~0.98*r at peak, so the applause ring radiates strongly to the canvas
/// edge without clipping. Pure.
pub fn ray_length(r: f64, shaped: f32) -> f64 {
    let s = shaped.clamp(0.0, 1.0) as f64;
    r * (0.08 + 0.36 * s)
}

/// Quantize a 0..1 ray energy to one of 4 precomputed gold tiers. Returns a
/// `&'static str` so the frame loop does NOT allocate (no `format!`). The caller
/// sets `set_global_alpha` for continuous alpha; this picks the base RGB tier.
pub fn ray_tier(energy: f32, is_dark: bool) -> &'static str {
    if is_dark {
        if energy >= 0.85 {
            "rgb(255,206,106)" // glow peak (applause)
        } else if energy >= 0.55 {
            "rgb(240,194,103)" // accent-hover
        } else if energy >= 0.25 {
            "rgb(230,179,77)" // accent
        } else {
            "rgb(201,154,58)" // accent-active (dim)
        }
    } else {
        // Light "matinee": the rays sit on the cream stage, so louder reads as a
        // bolder, darker gold instead of the dark theme's brighter highlight.
        if energy >= 0.85 {
            "rgb(116,86,27)" // accent (AA on light)
        } else if energy >= 0.55 {
            "rgb(146,108,40)"
        } else if energy >= 0.25 {
            "rgb(176,134,58)"
        } else {
            "rgb(199,165,108)" // dim (recedes)
        }
    }
}

/// True when an applause flash may fire: env crossed the peak threshold AND the
/// refractory window (>=500ms since the last flash) has elapsed. Pure so the
/// refractory logic is unit-tested (Correction 9). `now`/`last_flash` are ms.
pub fn flash_armed(env: f32, now: f64, last_flash: f64) -> bool {
    env >= 0.85 && (now - last_flash) >= 500.0
}

/// Build the Ovation into `container`: the album `<img id="sp-art-img">` center,
/// an `aria-hidden` canvas for the arc + rays, and a `role="slider"` seek
/// overlay. Paints one static frame; the reactive loop arms from `note_levels`.
pub fn render(container: &web_sys::Element) {
    use crate::dom;

    let stack = dom::create_div();
    dom::set_class(&stack, "ovation");
    dom::set_style(&stack, "position", "relative");
    dom::set_style(&stack, "width", "280px");
    dom::set_style(&stack, "height", "280px");
    dom::set_style(&stack, "margin", "0 auto");

    // Album disc — a circular center that shows the music-note fallback glyph
    // when there is no cover art (idle hero) and the <img> on top when there is.
    let disc = dom::create_div();
    dom::set_class(&disc, "ovation-disc");
    dom::append(&stack, &disc);

    // Album art center (kept id so spotify.rs update() still drives it). The img
    // is hidden by spotify::update() when the cover URL is empty, revealing the
    // fallback glyph on the disc beneath it.
    let img = dom::create_el("img");
    img.set_id("sp-art-img");
    dom::set_class(&img, "ovation-art");
    dom::set_attr(&img, "alt", "");
    dom::set_style(&img, "position", "absolute");
    dom::set_style(&img, "left", "50%");
    dom::set_style(&img, "top", "50%");
    dom::set_style(&img, "width", "150px");
    dom::set_style(&img, "height", "150px");
    dom::set_style(&img, "transform", "translate(-50%,-50%)");
    dom::set_style(&img, "border-radius", "50%");
    dom::set_style(&img, "object-fit", "cover");
    dom::set_style(&img, "display", "none");
    dom::append(&stack, &img);

    // Explicit badge (kept id so spotify::update() still finds it).
    let explicit = dom::el("span", "sp-explicit", Some("E"));
    explicit.set_id("sp-explicit");
    dom::set_style(&explicit, "display", "none");
    dom::append(&stack, &explicit);

    // Conic-gradient progress fallback ring (kept id for the no-canvas fallback
    // and for spotify::update()'s width writes).
    let progress = dom::create_div();
    progress.set_id("sp-progress");
    dom::set_attr(&progress, "aria-hidden", "true");
    dom::append(&stack, &progress);

    // Decorative bloom canvas (arc + rays).
    let (canvas, ctx) = super::create_canvas(280, 280);
    canvas.set_id("ovation-canvas");
    let canvas_el: web_sys::Element = canvas.into();
    dom::set_attr(&canvas_el, "aria-hidden", "true");
    dom::set_style(&canvas_el, "pointer-events", "none");
    dom::set_style(&canvas_el, "position", "absolute");
    dom::set_style(&canvas_el, "inset", "0");
    dom::append(&stack, &canvas_el);

    // Seek slider overlay (the real, focusable control).
    let seek = dom::create_div();
    seek.set_id("sp-seek-overlay");
    dom::set_attr(&seek, "role", "slider");
    dom::set_attr(&seek, "aria-label", "Seek");
    dom::set_attr(&seek, "aria-valuemin", "0");
    dom::set_attr(&seek, "aria-valuemax", "100");
    dom::set_attr(&seek, "aria-valuenow", "0");
    dom::set_attr(&seek, "tabindex", "0");
    dom::set_style(&seek, "position", "absolute");
    dom::set_style(&seek, "inset", "0");
    dom::set_style(&seek, "border-radius", "50%");
    dom::set_style(&seek, "cursor", "pointer");
    dom::append(&stack, &seek);

    dom::append(container, &stack);

    paint_static(&ctx, 280.0);
}

/// Paint one static frame: faint full track + the progress arc, no rays. Used at
/// mount, under reduced motion, and when parked.
fn paint_static(ctx: &CanvasRenderingContext2d, size: f64) {
    let is_dark = super::theme::is_dark();
    ctx.clear_rect(0.0, 0.0, size, size);
    let cx = size / 2.0;
    let cy = size / 2.0;
    let r = size / 2.0 - 14.0;

    // Faint full track.
    let (ar, ag, ab) = super::theme::accent_rgb(is_dark);
    ctx.set_stroke_style_str(&super::rgba_str(ar, ag, ab, 0.14));
    ctx.set_line_width(6.0);
    ctx.set_line_cap("round");
    ctx.begin_path();
    ctx.arc(cx, cy, r, 0.0, std::f64::consts::TAU).ok();
    ctx.stroke();

    // Progress arc from 12 o'clock.
    let frac = crate::state::with(|s| {
        s.spotify_status
            .as_ref()
            .filter(|st| st.duration_ms > 0)
            .map(|st| (st.position_ms as f64 / st.duration_ms as f64).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    });
    if frac > 0.0 {
        let start = -std::f64::consts::FRAC_PI_2;
        ctx.set_stroke_style_str(&super::rgb_str(ar, ag, ab));
        ctx.set_line_width(6.0);
        ctx.begin_path();
        ctx.arc(cx, cy, r, start, start + frac * std::f64::consts::TAU)
            .ok();
        ctx.stroke();
    }
}

/// Feed the latest audio levels (called from the WS dispatcher). Copies the four
/// primitives, then arms the reactive loop if the gate passes — the re-arm guard
/// lives HERE because the AudioLevels handler is page-agnostic.
pub fn note_levels(l_rms: f32, r_rms: f32, l_peak: f32, r_peak: f32) {
    LEVELS.with(|c| c.set((l_rms, r_rms, l_peak, r_peak)));
    LAST_LEVEL_MS.with(|c| c.set(now_ms()));

    let reduce = super::theme::reduce_motion();
    let on_stage = crate::state::with(|s| s.active_page == "home");
    let power_active = crate::state::with(|s| s.audio_power_state == "active");
    let hidden = crate::dom::document().hidden();
    if reduce || !on_stage || !power_active || hidden {
        return;
    }
    // Arm: start the loop, but only if not already running.
    if !RUNNING.with(|c| c.get()) {
        start_loop();
    }
}

/// Copy the spectrum once per arrival (never per frame).
pub fn note_spectrum(bins: &[f32; 32]) {
    SPECTRUM.with(|c| c.set(Some(*bins)));
}

/// Park the loop: stop the rAF and paint one static frame (called by `app.rs`
/// route teardown and by the idle gate).
pub fn park() {
    OVATION_EPOCH.with(|e| e.set(e.get().wrapping_add(1)));
    RUNNING.with(|c| c.set(false));
    if let Some(ctx) = canvas_ctx() {
        paint_static(&ctx, 280.0);
    }
}

fn now_ms() -> f64 {
    crate::dom::window()
        .performance()
        .map(|p| p.now())
        .unwrap_or(0.0)
}

fn canvas_ctx() -> Option<CanvasRenderingContext2d> {
    let canvas = crate::dom::get_el("ovation-canvas")?;
    let canvas = canvas.dyn_ref::<web_sys::HtmlCanvasElement>()?;
    let dpr = crate::dom::window().device_pixel_ratio().clamp(1.0, 2.0);
    let ctx = canvas.get_context("2d").ok()??;
    let ctx: CanvasRenderingContext2d = ctx.unchecked_into();
    ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0).ok();
    Some(ctx)
}

fn start_loop() {
    let epoch = OVATION_EPOCH.with(|e| {
        let next = e.get().wrapping_add(1);
        e.set(next);
        next
    });
    RUNNING.with(|c| c.set(true));
    LOW_SINCE_MS.with(|c| c.set(now_ms()));
    frame(epoch);
}

fn frame(epoch: u32) {
    use wasm_bindgen::prelude::Closure;
    if OVATION_EPOCH.with(|e| e.get()) != epoch {
        RUNNING.with(|c| c.set(false));
        return;
    }

    let now = now_ms();
    // FPS cap (20fps device): bail the paint but keep the schedule via setTimeout.
    let interval = 50.0; // 20fps; desktop tuning is a P4 device check
    let do_paint = now - LAST_PAINT_MS.with(|c| c.get()) >= interval;

    // Idle gate: park if power not active, no levels >1.5s, or env<0.02 >800ms.
    let power_active = crate::state::with(|s| s.audio_power_state == "active");
    let on_stage = crate::state::with(|s| s.active_page == "home");
    let stale = now - LAST_LEVEL_MS.with(|c| c.get()) > 1500.0;
    let hidden = crate::dom::document().hidden();
    if !power_active || !on_stage || stale || hidden {
        park();
        return;
    }

    let (_l_rms, _r_rms, l_peak, r_peak) = LEVELS.with(|c| c.get());
    let target = l_peak.max(r_peak).clamp(0.0, 1.0);
    let env = smooth(ENV.with(|c| c.get()), target, 0.5, 0.12);
    ENV.with(|c| c.set(env));

    // Ease the per-band spectrum toward the latest target each frame (snappy up,
    // gentle down) so the ~8 Hz device feed reads as a fluid bloom, not a stutter.
    if let Some(target) = SPECTRUM.with(|c| c.get()) {
        SMOOTHED_SPEC.with(|c| {
            let mut s = c.get();
            for (cur, &t) in s.iter_mut().zip(target.iter()) {
                *cur = smooth(*cur, t, 0.6, 0.25);
            }
            c.set(s);
        });
    }

    if env < 0.02 {
        let low_since = LOW_SINCE_MS.with(|c| c.get());
        if now - low_since > 800.0 {
            park();
            return;
        }
    } else {
        LOW_SINCE_MS.with(|c| c.set(now));
    }

    if do_paint {
        LAST_PAINT_MS.with(|c| c.set(now));
        if let Some(ctx) = canvas_ctx() {
            paint_rays(&ctx, 280.0, env, now);
        }
    }

    // Schedule next frame via setTimeout so the CPU is not woken at 60Hz to no-op.
    let cb = Closure::once(move || frame(epoch));
    crate::dom::window()
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            interval as i32,
        )
        .ok();
    cb.forget();
}

/// Paint the ray ring + progress arc for a smoothed envelope. Tier-batched,
/// zero String allocation: precompute sin/cos, set fill once per tier, one
/// stroke. `shadow_blur` is reserved for the transient applause flash with a
/// 500ms refractory (Correction 9).
fn paint_rays(ctx: &CanvasRenderingContext2d, size: f64, env: f32, now: f64) {
    paint_static(ctx, size); // track + progress arc underneath
    let cx = size / 2.0;
    let cy = size / 2.0;
    let r = size / 2.0 - 14.0;
    let base = r * 0.62;
    let n = NUM_RAYS_DEVICE;

    // Single tier for a scalar envelope (spectrum mapping mirrors 32 bins onto
    // rays); one stroke for the whole ring keeps it zero-alloc and batched.
    // Eased per-band values when a spectrum is present (smoothed in `frame`); falls
    // back to scalar-envelope wobble when no spectrum has arrived.
    let spectrum = SPECTRUM
        .with(|c| c.get())
        .map(|_| SMOOTHED_SPEC.with(|c| c.get()));
    let tier = ray_tier(env, super::theme::is_dark());
    ctx.set_stroke_style_str(tier);
    // Stronger, more readable ring: a higher alpha floor so even a moderate
    // level reads boldly, and a thicker stroke so the rays carry weight.
    ctx.set_global_alpha((0.5 + 0.5 * env as f64).clamp(0.0, 1.0));
    ctx.set_line_width(3.0);

    // Transient applause flash: shadow_blur on the whole ring stroke, but only on
    // a peak crossing and only once per 500ms refractory so it can't re-arm every
    // frame.
    let last_flash = LAST_FLASH_MS.with(|c| c.get());
    let flash = flash_armed(env, now, last_flash);
    if flash {
        LAST_FLASH_MS.with(|c| c.set(now));
        ctx.set_shadow_color(tier);
        ctx.set_shadow_blur(8.0);
    }

    ctx.begin_path();
    for i in 0..n {
        let ang = (i as f64 / n as f64) * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
        // Scalar mode: one envelope + a small per-ray wobble so a flat signal
        // still shimmers. Spectrum mode (if present): mirror 32 bins onto rays.
        let shaped = match spectrum {
            Some(bins) => {
                let half = n / 2;
                let bi = if i < half { i } else { n - 1 - i } * 32 / half.max(1);
                bins[bi.min(31)]
            }
            None => env * (0.85 + 0.15 * ((i as f32) * 1.7).sin().abs()),
        };
        let len = ray_length(r, shaped);
        let x0 = cx + base * ang.cos();
        let y0 = cy + base * ang.sin();
        let x1 = cx + (base + len) * ang.cos();
        let y1 = cy + (base + len) * ang.sin();
        ctx.move_to(x0, y0);
        ctx.line_to(x1, y1);
    }
    ctx.stroke();

    if flash {
        ctx.set_shadow_blur(0.0);
    }
    ctx.set_global_alpha(1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_attacks_faster_than_releases() {
        // Rising uses attack; from 0 toward 1 with attack 0.5 -> 0.5.
        assert!((smooth(0.0, 1.0, 0.5, 0.1) - 0.5).abs() < 1e-6);
        // Falling uses release; from 1 toward 0 with release 0.1 -> 0.9.
        assert!((smooth(1.0, 0.0, 0.5, 0.1) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn smooth_converges_to_target() {
        let mut v = 0.0f32;
        for _ in 0..200 {
            v = smooth(v, 1.0, 0.5, 0.12);
        }
        assert!((v - 1.0).abs() < 1e-3, "got {}", v);
    }

    #[test]
    fn smooth_equal_when_already_there() {
        assert_eq!(smooth(0.42, 0.42, 0.5, 0.1), 0.42);
    }

    #[test]
    fn ray_length_floor_and_ceiling() {
        // shaped 0 -> 8% of r; shaped 1 -> 44% of r (8% floor + 36% span).
        assert!((ray_length(100.0, 0.0) - 8.0).abs() < 1e-9);
        assert!((ray_length(100.0, 1.0) - 44.0).abs() < 1e-9);
    }

    #[test]
    fn ray_length_clamps_out_of_range() {
        assert_eq!(ray_length(100.0, -1.0), ray_length(100.0, 0.0));
        assert_eq!(ray_length(100.0, 2.0), ray_length(100.0, 1.0));
    }

    #[test]
    fn ray_tier_is_static_and_monotone_in_brightness() {
        // Dark: four distinct tiers, brightest at the top.
        assert_eq!(ray_tier(0.9, true), "rgb(255,206,106)");
        assert_eq!(ray_tier(0.6, true), "rgb(240,194,103)");
        assert_eq!(ray_tier(0.3, true), "rgb(230,179,77)");
        assert_eq!(ray_tier(0.0, true), "rgb(201,154,58)");
        // Boundary checks.
        assert_eq!(ray_tier(0.85, true), "rgb(255,206,106)");
        assert_eq!(ray_tier(0.55, true), "rgb(240,194,103)");
        assert_eq!(ray_tier(0.25, true), "rgb(230,179,77)");
        // Light theme returns a distinct, darker palette (never the dark tiers).
        assert_ne!(ray_tier(0.9, false), ray_tier(0.9, true));
        assert_eq!(ray_tier(0.9, false), "rgb(116,86,27)");
    }

    #[test]
    fn flash_refractory_blocks_rapid_reflash() {
        // First loud frame at t=1000 arms (no prior flash).
        assert!(flash_armed(0.9, 1000.0, 0.0));
        // 100ms later, still loud, but inside the refractory -> blocked.
        assert!(!flash_armed(0.95, 1100.0, 1000.0));
        // 500ms later -> re-armed.
        assert!(flash_armed(0.95, 1500.0, 1000.0));
        // Below the peak threshold never arms regardless of time.
        assert!(!flash_armed(0.84, 5000.0, 0.0));
    }
}
