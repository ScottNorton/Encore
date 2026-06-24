//! Sound-page visualization loop.
//!
//! The device sends audio levels at ~2 Hz and the spectrum at ~8 Hz (both kept low
//! on purpose — high WebSocket rates cause WiFi backpressure on the speaker, see
//! the mixer's LEVELS_INTERVAL). This loop redraws the VU meter and spectrum bars at
//! ~20 fps, easing each toward the latest values so the low-rate feed reads as fluid
//! motion instead of a stutter. The oscilloscope is redrawn with the latest waveform
//! (a waveform has nothing to ease toward). It runs only while the Sound page is the
//! active surface.

use std::cell::{Cell, RefCell};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

const N_BANDS: usize = 32;
const FRAME_MS: i32 = 50; // ~20 fps

thread_local! {
    // The active setInterval handle + its closure (kept alive). One closure total,
    // so there is no per-frame allocation leak.
    static TIMER: RefCell<Option<(i32, Closure<dyn FnMut()>)>> = const { RefCell::new(None) };
    static SPEC: Cell<[f32; N_BANDS]> = const { Cell::new([0.0; N_BANDS]) };
    static VU: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
}

/// Attack/release ease toward a target (fast rise, gentle fall).
fn ease(cur: f32, target: f32, attack: f32, release: f32) -> f32 {
    let k = if target > cur { attack } else { release };
    cur + (target - cur) * k
}

fn ctx_of(id: &str) -> Option<CanvasRenderingContext2d> {
    let el = crate::dom::get_el(id)?;
    let canvas = el.dyn_ref::<web_sys::HtmlCanvasElement>()?;
    canvas
        .get_context("2d")
        .ok()
        .flatten()
        .map(|c| c.unchecked_into())
}

/// Start (or restart) the loop. Idempotent — called whenever the Sound page renders.
pub fn start() {
    stop();
    let cb = Closure::wrap(Box::new(frame) as Box<dyn FnMut()>);
    if let Ok(id) = crate::dom::window().set_interval_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        FRAME_MS,
    ) {
        TIMER.with(|t| *t.borrow_mut() = Some((id, cb)));
    }
}

/// Stop the loop and release its timer.
pub fn stop() {
    TIMER.with(|t| {
        if let Some((id, _cb)) = t.borrow_mut().take() {
            crate::dom::window().clear_interval_with_handle(id);
        }
    });
}

fn frame() {
    // Fully stop once the Sound page is no longer the active surface; render() re-arms
    // it on return. Tab-hidden only skips drawing so visibility toggles don't kill it.
    if crate::state::with(|s| s.active_page != "sound") {
        stop();
        return;
    }
    if crate::dom::document().hidden() {
        return;
    }

    let (mode, lr, rr, lp, rp, spec, wave) = crate::state::with(|s| {
        (
            s.viz_mode.clone(),
            s.audio_left_rms,
            s.audio_right_rms,
            s.audio_left_peak,
            s.audio_right_peak,
            s.audio_spectrum,
            s.audio_waveform.clone(),
        )
    });

    // VU meter: ease the rms bars; peak hold/decay lives inside vu_meter::draw.
    let (mut clr, mut crr) = VU.with(|c| c.get());
    clr = ease(clr, lr, 0.5, 0.2);
    crr = ease(crr, rr, 0.5, 0.2);
    VU.with(|c| c.set((clr, crr)));
    if let Some(ctx) = ctx_of("vu-meter") {
        crate::graphics::vu_meter::draw(&ctx, 320.0, 64.0, clr, crr, lp, rp);
    }

    // Spectrum bars: ease each band toward the latest target (snappy up, gentle down).
    if let Some(target) = spec {
        SPEC.with(|c| {
            let mut s = c.get();
            for (cur, &t) in s.iter_mut().zip(target.iter()) {
                *cur = ease(*cur, t, 0.6, 0.25);
            }
            c.set(s);
        });
        if mode == "spectrum" || mode == "both" {
            if let Some(ctx) = ctx_of("spectrum-canvas") {
                let bars = SPEC.with(|c| c.get());
                crate::graphics::spectrum::draw(&ctx, &bars);
            }
        }
    }

    // Oscilloscope: the latest waveform, nothing to ease.
    if mode == "scope" || mode == "both" {
        if let (Some(wave), Some(ctx)) = (wave, ctx_of("scope-canvas")) {
            crate::graphics::scope::draw(&ctx, &wave);
        }
    }
}
