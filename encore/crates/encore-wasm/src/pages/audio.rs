//! Audio page — master volume knob, per-source sliders, EQ, DRC, DSP engine.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::{SegmentedControl, SegmentedMode};
use crate::dom;
use encore_common::protocol::{
    ClientMsg, SourceId, EqPreset, DrcPreset, DrcBand, DrcBandConfig, FilterType,
};

fn freq_display(hz: u16) -> String {
    if hz >= 1000 {
        let k = hz as f32 / 1000.0;
        if k == k.floor() { format!("{}kHz", k as u16) }
        else { format!("{:.1}kHz", k) }
    } else {
        format!("{}Hz", hz)
    }
}

fn gain_label(cb: i16) -> String {
    let db = cb as f32 / 10.0;
    if db == 0.0 {
        "0 dB".into()
    } else {
        format!("{:+.1} dB", db)
    }
}

pub fn render(container: &web_sys::Element) {
    // ── Volume Row (master knob + source sliders side by side) ──
    let vol_row = dom::create_div();
    dom::set_class(&vol_row, "vol-row");
    render_volume_knob(&vol_row);
    render_source_volumes(&vol_row);
    dom::append(container, &vol_row);

    // ── VU Meter ──
    render_vu_meter(container);

    // ── Audio Visualization ──
    render_visualization(container);

    // ── Equalizer ──
    render_eq(container);

    // ── Dynamics (DRC) ──
    render_drc(container);

    // ── DSP Engine ──
    render_dsp(container);

    update();
}

// ─────────────────────── Master Volume ───────────────────────

fn render_volume_knob(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card text-center");

    let title = dom::el("div", "card-title", Some("Master Volume"));
    dom::append(&card, &title);

    let (knob_canvas, knob_ctx) = crate::graphics::create_canvas(180, 180);
    knob_canvas.set_id("vol-knob");
    let knob_el: web_sys::Element = knob_canvas.clone().into();
    dom::append(&card, &knob_el);

    let vol = crate::state::with(|s| {
        s.config.as_ref().map(|c| c.master_volume).unwrap_or(70)
    });
    crate::graphics::knob::draw(&knob_ctx, 180.0, vol);

    crate::graphics::knob::make_interactive(&knob_canvas, move |new_vol| {
        crate::ws::send_msg(&ClientMsg::SetMasterVolume(new_vol));
        if let Some(canvas) = dom::get_el("vol-knob") {
            if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                if let Ok(Some(ctx)) = canvas.get_context("2d") {
                    let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                    crate::graphics::knob::draw(&ctx, 180.0, new_vol);
                }
            }
        }
    });

    dom::append(container, &card);
}

// ─────────────────────── Source Volumes ───────────────────────

fn render_source_volumes(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    let title = dom::el("div", "card-title", Some("Source Volumes"));
    dom::append(&card, &title);

    for (name, id, source) in [
        ("Spotify", "spotify", SourceId::Spotify),
        ("Bluetooth", "bluetooth", SourceId::Bluetooth),
        ("Voice", "wyoming", SourceId::Wyoming),
    ] {
        let row = dom::create_div();
        row.set_id(&format!("mix-row-{}", id));
        dom::set_class(&row, "mb-12");
        dom::set_style(&row, "display", "none");

        let label = dom::create_div();
        dom::set_class(&label, "flex justify-between mb-8");
        let name_el = dom::el("span", "stat-label", Some(name));
        let val_el = dom::el("span", "stat-value text-sm", None);
        val_el.set_id(&format!("vol-{}-val", id));
        dom::append(&label, &name_el);
        dom::append(&label, &val_el);
        dom::append(&row, &label);

        let slider = dom::create_el("input");
        dom::set_attr(&slider, "type", "range");
        dom::set_attr(&slider, "min", "0");
        dom::set_attr(&slider, "max", "100");
        dom::set_attr(&slider, "value", "70");
        slider.set_id(&format!("vol-{}", id));

        let val_id = format!("vol-{}-val", id);
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                let val: u8 = input.value().parse().unwrap_or(70);
                if let Some(el) = dom::get_el(&val_id) {
                    dom::set_text(&el, &format!("{}%", val));
                }
                crate::ws::send_msg(&ClientMsg::SetVolume { source, level: val });
            }
        }) as Box<dyn FnMut(_)>);
        slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref()).ok();
        cb.forget();

        dom::append(&row, &slider);
        dom::append(&card, &row);
    }

    // TTS Volume Ducking (inline in source card)
    let duck_sep = dom::create_div();
    duck_sep.set_id("tts-duck-sep");
    dom::set_style(&duck_sep, "border-top", "1px solid var(--border)");
    dom::set_style(&duck_sep, "margin", "12px 0 8px");
    dom::append(&card, &duck_sep);

    let duck_header = dom::create_div();
    dom::set_class(&duck_header, "flex justify-between mb-4");
    let duck_name = dom::el("span", "stat-label", Some("TTS Duck Level"));
    let duck_val = dom::el("span", "stat-value text-sm", None);
    duck_val.set_id("tts-duck-val");
    dom::append(&duck_header, &duck_name);
    dom::append(&duck_header, &duck_val);
    dom::append(&card, &duck_header);

    let duck_desc = dom::el("div", "text-sm text-muted mb-8",
        Some("Music volume during voice announcements"));
    dom::append(&card, &duck_desc);

    let duck_slider = dom::create_el("input");
    dom::set_attr(&duck_slider, "type", "range");
    dom::set_attr(&duck_slider, "min", "0");
    dom::set_attr(&duck_slider, "max", "100");
    dom::set_attr(&duck_slider, "value", "80");
    duck_slider.set_id("tts-duck");

    let duck_cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
        if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            let val: u8 = input.value().parse().unwrap_or(80);
            if let Some(el) = dom::get_el("tts-duck-val") {
                dom::set_text(&el, &format!("{}%", val));
            }
            // Save via config update
            crate::state::with(|s| {
                if let Some(ref cfg) = s.config {
                    let mut new_cfg = cfg.clone();
                    new_cfg.tts_duck_percent = val;
                    crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(new_cfg)));
                }
            });
        }
    }) as Box<dyn FnMut(_)>);
    duck_slider.add_event_listener_with_callback("input", duck_cb.as_ref().unchecked_ref()).ok();
    duck_cb.forget();
    dom::append(&card, &duck_slider);

    // Volume Ring Sensitivity
    let ring_sep = dom::create_div();
    dom::set_style(&ring_sep, "border-top", "1px solid var(--border)");
    dom::set_style(&ring_sep, "margin", "12px 0 8px");
    dom::append(&card, &ring_sep);

    let ring_header = dom::create_div();
    dom::set_class(&ring_header, "flex justify-between mb-4");
    let ring_name = dom::el("span", "stat-label", Some("Volume Ring Sensitivity"));
    let ring_val = dom::el("span", "stat-value text-sm", None);
    ring_val.set_id("vol-ring-step-val");
    dom::append(&ring_header, &ring_name);
    dom::append(&ring_header, &ring_val);
    dom::append(&card, &ring_header);

    let ring_desc = dom::el("div", "text-sm text-muted mb-8",
        Some("Percent change per detent click (1\u{2013}5)"));
    dom::append(&card, &ring_desc);

    let ring_slider = dom::create_el("input");
    dom::set_attr(&ring_slider, "type", "range");
    dom::set_attr(&ring_slider, "min", "1");
    dom::set_attr(&ring_slider, "max", "5");
    dom::set_attr(&ring_slider, "value", "2");
    ring_slider.set_id("vol-ring-step");

    let ring_cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
        if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            let val: u8 = input.value().parse().unwrap_or(2);
            if let Some(el) = dom::get_el("vol-ring-step-val") {
                dom::set_text(&el, &format!("{}%", val));
            }
            crate::state::with(|s| {
                if let Some(ref cfg) = s.config {
                    let mut new_cfg = cfg.clone();
                    new_cfg.volume_ring_step = val;
                    crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(new_cfg)));
                }
            });
        }
    }) as Box<dyn FnMut(_)>);
    ring_slider.add_event_listener_with_callback("input", ring_cb.as_ref().unchecked_ref()).ok();
    ring_cb.forget();
    dom::append(&card, &ring_slider);

    dom::append(container, &card);
}

// ─────────────────────── VU Meter ───────────────────────

fn render_vu_meter(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    let title = dom::el("div", "card-title", Some("Audio Levels"));
    dom::append(&card, &title);

    let (vu_canvas, _) = crate::graphics::create_canvas(320, 64);
    vu_canvas.set_id("vu-meter");
    let vu_el: web_sys::Element = vu_canvas.into();
    dom::set_style(&vu_el, "width", "100%");
    dom::append(&card, &vu_el);

    dom::append(container, &card);
}

// ─────────────────────── Visualization ───────────────────────

fn render_visualization(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    let header = dom::create_div();
    dom::set_class(&header, "flex justify-between items-center mb-8");

    let title = dom::el("div", "card-title", Some("Audio Visualization"));
    dom::set_style(&title, "margin-bottom", "0");
    dom::append(&header, &title);

    // View toggle: Spectrum / Scope / Both
    let current_mode = crate::state::with(|s| s.viz_mode.clone());
    let toggle = SegmentedControl::create(
        "viz-toggle",
        &[("spectrum", "Spectrum"), ("scope", "Scope"), ("both", "Both")],
        &[&current_mode],
        SegmentedMode::Single(Box::new(|mode| {
            let mode_str = mode.to_string();
            crate::state::with_mut(|s| s.viz_mode = mode_str.clone());
            if let Some(spec) = dom::get_el("spectrum-canvas") {
                let show = mode_str == "spectrum" || mode_str == "both";
                if let Some(p) = spec.parent_element() {
                    dom::set_style(&p, "display", if show { "block" } else { "none" });
                }
            }
            if let Some(scope) = dom::get_el("scope-canvas") {
                let show = mode_str == "scope" || mode_str == "both";
                if let Some(p) = scope.parent_element() {
                    dom::set_style(&p, "display", if show { "block" } else { "none" });
                }
            }
        })),
    );
    dom::append(&header, &toggle);
    dom::append(&card, &header);

    let viz_mode = crate::state::with(|s| s.viz_mode.clone());

    // Spectrum canvas
    let spec_wrap = dom::create_div();
    spec_wrap.set_id("spectrum-wrap");
    dom::set_style(&spec_wrap, "display", if viz_mode == "spectrum" || viz_mode == "both" { "block" } else { "none" });
    let (spec_canvas, _) = crate::graphics::create_canvas(320, 80);
    spec_canvas.set_id("spectrum-canvas");
    let spec_el: web_sys::Element = spec_canvas.into();
    dom::set_style(&spec_el, "width", "100%");
    dom::set_style(&spec_el, "margin-bottom", "8px");
    dom::append(&spec_wrap, &spec_el);
    dom::append(&card, &spec_wrap);

    // Scope canvas
    let scope_wrap = dom::create_div();
    scope_wrap.set_id("scope-wrap");
    dom::set_style(&scope_wrap, "display", if viz_mode == "scope" || viz_mode == "both" { "block" } else { "none" });
    let (scope_canvas, _) = crate::graphics::create_canvas(320, 80);
    scope_canvas.set_id("scope-canvas");
    let scope_el: web_sys::Element = scope_canvas.into();
    dom::set_style(&scope_el, "width", "100%");
    dom::append(&scope_wrap, &scope_el);
    dom::append(&card, &scope_wrap);

    dom::append(container, &card);
}

// ─────────────────────── Equalizer ───────────────────────

/// Map Hz to log-scale slider value (0-1000).
fn freq_to_slider(hz: u16) -> i32 {
    let log_min = 20.0_f64.ln();
    let log_max = 20000.0_f64.ln();
    let t = ((hz as f64).clamp(20.0, 20000.0).ln() - log_min) / (log_max - log_min);
    (t * 1000.0) as i32
}

/// Map log-scale slider value (0-1000) to Hz.
fn slider_to_freq(val: i32) -> u16 {
    let log_min = 20.0_f64.ln();
    let log_max = 20000.0_f64.ln();
    let t = (val as f64 / 1000.0).clamp(0.0, 1.0);
    (log_min + t * (log_max - log_min)).exp().round() as u16
}

/// Redraw the EQ canvas from current state.
fn redraw_eq_canvas() {
    crate::state::with(|s| {
        if let Some(canvas) = dom::get_el("eq-curve") {
            if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                if let Ok(Some(ctx)) = canvas.get_context("2d") {
                    let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                    if let Some(ref eq) = s.eq_state {
                        crate::graphics::eq_curve::draw(&ctx, &eq.bands, eq.enabled, s.eq_selected_band);
                    }
                }
            }
        }
    });
}

/// Update the selected band detail row from current state.
fn update_eq_detail() {
    crate::state::with(|s| {
        let detail = match dom::get_el("eq-detail") {
            Some(el) => el,
            None => return,
        };
        let (idx, eq) = match (s.eq_selected_band, s.eq_state.as_ref()) {
            (Some(idx), Some(eq)) if idx < 10 => (idx, eq),
            _ => {
                dom::set_style(&detail, "display", "none");
                return;
            }
        };
        dom::set_style(&detail, "display", "block");
        let band = &eq.bands[idx];

        if let Some(lbl) = dom::get_el("eq-detail-label") {
            dom::set_text(&lbl, &format!("Band {}", idx + 1));
        }
        for (id, ft) in [
            ("eq-ft-peak", FilterType::Peak),
            ("eq-ft-lsh", FilterType::LowShelf),
            ("eq-ft-hsh", FilterType::HighShelf),
            ("eq-ft-notch", FilterType::Notch),
        ] {
            if let Some(btn) = dom::get_el(id) {
                dom::set_class(&btn, if band.filter_type == ft { "seg-btn active" } else { "seg-btn" });
            }
        }
        if let Some(el) = dom::get_el("eq-freq") {
            dom::set_attr(&el, "value", &freq_to_slider(band.freq_hz).to_string());
        }
        if let Some(el) = dom::get_el("eq-freq-val") {
            dom::set_text(&el, &freq_display(band.freq_hz));
        }
        if let Some(el) = dom::get_el("eq-gain-detail") {
            dom::set_attr(&el, "value", &band.gain_cb.to_string());
        }
        if let Some(el) = dom::get_el("eq-gain-detail-val") {
            dom::set_text(&el, &gain_label(band.gain_cb));
        }
        if let Some(el) = dom::get_el("eq-q") {
            dom::set_attr(&el, "value", &band.q_x10.to_string());
        }
        if let Some(el) = dom::get_el("eq-q-val") {
            dom::set_text(&el, &format!("{:.1}", band.q_x10 as f32 / 10.0));
        }
    });
}

fn render_eq(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    // Header: title + toggle
    let header = dom::create_div();
    dom::set_class(&header, "flex justify-between items-center mb-12");
    let title = dom::el("div", "card-title", Some("Equalizer"));
    dom::set_style(&title, "margin-bottom", "0");
    let toggle_wrap = crate::components::Toggle::create("eq-toggle", "", true, |checked| {
        crate::ws::send_msg(&ClientMsg::SetEqEnabled(checked));
    });
    dom::append(&header, &title);
    dom::append(&header, &toggle_wrap);
    dom::append(&card, &header);

    // Preset pills
    let presets = SegmentedControl::create(
        "eq-presets",
        &[
            ("flat", "Flat"),
            ("bass", "Bass+"),
            ("vocal", "Vocal"),
            ("warm", "Warm"),
            ("night", "Night"),
        ],
        &[],
        SegmentedMode::Single(Box::new(|value| {
            let preset = match value {
                "flat" => EqPreset::Flat,
                "bass" => EqPreset::BassBoost,
                "vocal" => EqPreset::VocalClarity,
                "warm" => EqPreset::Warm,
                "night" => EqPreset::LateNight,
                _ => return,
            };
            crate::ws::send_msg(&ClientMsg::SetEqPreset(preset));
        })),
    );
    dom::set_style(&presets, "margin", "12px 0");
    dom::append(&card, &presets);

    // EQ curve canvas
    let (eq_w, eq_h) = crate::graphics::eq_curve::dimensions();
    let (eq_canvas, eq_ctx) = crate::graphics::create_canvas(eq_w, eq_h);
    eq_canvas.set_id("eq-curve");
    let canvas_el: web_sys::Element = eq_canvas.clone().into();
    dom::set_style(&canvas_el, "width", "100%");
    dom::set_style(&canvas_el, "cursor", "pointer");
    dom::set_style(&canvas_el, "margin-bottom", "4px");
    dom::append(&card, &canvas_el);

    // Initial draw
    let (bands, enabled, selected) = crate::state::with(|s| {
        match s.eq_state.as_ref() {
            Some(eq) => (eq.bands, eq.enabled, s.eq_selected_band),
            None => (encore_common::protocol::EqState::default().bands, true, None),
        }
    });
    crate::graphics::eq_curve::draw(&eq_ctx, &bands, enabled, selected);

    // Make canvas interactive (tap to select, drag for freq/gain)
    crate::graphics::eq_curve::make_interactive(
        &eq_canvas,
        |idx| {
            crate::state::with_mut(|s| { s.eq_selected_band = Some(idx); });
            redraw_eq_canvas();
            update_eq_detail();
        },
        |idx, freq_hz, gain_cb| {
            let config = crate::state::with_mut(|s| {
                if let Some(ref mut eq) = s.eq_state {
                    eq.bands[idx].freq_hz = freq_hz;
                    eq.bands[idx].gain_cb = gain_cb;
                    eq.preset = None;
                    Some(eq.bands[idx])
                } else {
                    None
                }
            });
            if let Some(config) = config {
                crate::ws::send_msg(&ClientMsg::SetEqBand { band: idx as u8, config });
            }
            redraw_eq_canvas();
            update_eq_detail();
        },
    );

    // Selected band detail row (initially hidden)
    let detail = dom::create_div();
    detail.set_id("eq-detail");
    dom::set_class(&detail, "eq-detail");
    dom::set_style(&detail, "display", "none");

    // Header: "Band N" + filter type buttons
    let detail_header = dom::create_div();
    dom::set_class(&detail_header, "eq-detail-header");
    let band_label = dom::el("span", "eq-detail-band-label", Some("Band 1"));
    band_label.set_id("eq-detail-label");
    let filter_btns = SegmentedControl::create(
        "eq-ft",
        &[
            ("peak", "Peak"),
            ("lsh", "LSh"),
            ("hsh", "HSh"),
            ("notch", "Notch"),
        ],
        &[],
        SegmentedMode::Single(Box::new(|value| {
            let ft = match value {
                "peak" => FilterType::Peak,
                "lsh" => FilterType::LowShelf,
                "hsh" => FilterType::HighShelf,
                "notch" => FilterType::Notch,
                _ => return,
            };
            let config = crate::state::with_mut(|s| {
                if let Some(idx) = s.eq_selected_band {
                    if let Some(ref mut eq) = s.eq_state {
                        eq.bands[idx].filter_type = ft;
                        eq.preset = None;
                        return Some((idx, eq.bands[idx]));
                    }
                }
                None
            });
            if let Some((idx, config)) = config {
                crate::ws::send_msg(&ClientMsg::SetEqBand { band: idx as u8, config });
                redraw_eq_canvas();
                update_eq_detail();
            }
        })),
    );
    dom::append(&detail_header, &band_label);
    dom::append(&detail_header, &filter_btns);
    dom::append(&detail, &detail_header);

    // Frequency slider (log-scale: 0-1000 maps to 20Hz-20kHz)
    {
        let row = dom::create_div();
        dom::set_class(&row, "eq-param-row");
        dom::append(&row, &dom::el("label", "", Some("Freq")));
        let slider = dom::create_el("input");
        dom::set_attr(&slider, "type", "range");
        dom::set_attr(&slider, "min", "0");
        dom::set_attr(&slider, "max", "1000");
        dom::set_attr(&slider, "value", "500");
        slider.set_id("eq-freq");
        let val_el = dom::el("span", "eq-param-val", Some("1kHz"));
        val_el.set_id("eq-freq-val");
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                let slider_val: i32 = input.value().parse().unwrap_or(500);
                let freq_hz = slider_to_freq(slider_val);
                let config = crate::state::with_mut(|s| {
                    if let Some(idx) = s.eq_selected_band {
                        if let Some(ref mut eq) = s.eq_state {
                            eq.bands[idx].freq_hz = freq_hz;
                            eq.preset = None;
                            return Some((idx, eq.bands[idx]));
                        }
                    }
                    None
                });
                if let Some((idx, config)) = config {
                    crate::ws::send_msg(&ClientMsg::SetEqBand { band: idx as u8, config });
                    redraw_eq_canvas();
                    update_eq_detail();
                }
            }
        }) as Box<dyn FnMut(_)>);
        slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref()).ok();
        cb.forget();
        dom::append(&row, &slider);
        dom::append(&row, &val_el);
        dom::append(&detail, &row);
    }

    // Gain slider (-120..120 centibels = -12..+12 dB)
    {
        let row = dom::create_div();
        dom::set_class(&row, "eq-param-row");
        dom::append(&row, &dom::el("label", "", Some("Gain")));
        let slider = dom::create_el("input");
        dom::set_attr(&slider, "type", "range");
        dom::set_attr(&slider, "min", "-120");
        dom::set_attr(&slider, "max", "120");
        dom::set_attr(&slider, "value", "0");
        slider.set_id("eq-gain-detail");
        let val_el = dom::el("span", "eq-param-val", Some("0 dB"));
        val_el.set_id("eq-gain-detail-val");
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                let gain_cb: i16 = input.value().parse().unwrap_or(0);
                let config = crate::state::with_mut(|s| {
                    if let Some(idx) = s.eq_selected_band {
                        if let Some(ref mut eq) = s.eq_state {
                            eq.bands[idx].gain_cb = gain_cb;
                            eq.preset = None;
                            return Some((idx, eq.bands[idx]));
                        }
                    }
                    None
                });
                if let Some((idx, config)) = config {
                    crate::ws::send_msg(&ClientMsg::SetEqBand { band: idx as u8, config });
                    redraw_eq_canvas();
                    update_eq_detail();
                }
            }
        }) as Box<dyn FnMut(_)>);
        slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref()).ok();
        cb.forget();
        dom::append(&row, &slider);
        dom::append(&row, &val_el);
        dom::append(&detail, &row);
    }

    // Q slider (q_x10: 1-100, display as 0.1-10.0)
    {
        let row = dom::create_div();
        dom::set_class(&row, "eq-param-row");
        dom::append(&row, &dom::el("label", "", Some("Q")));
        let slider = dom::create_el("input");
        dom::set_attr(&slider, "type", "range");
        dom::set_attr(&slider, "min", "1");
        dom::set_attr(&slider, "max", "100");
        dom::set_attr(&slider, "value", "14");
        slider.set_id("eq-q");
        let val_el = dom::el("span", "eq-param-val", Some("1.4"));
        val_el.set_id("eq-q-val");
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                let q_x10: u16 = input.value().parse().unwrap_or(14);
                let config = crate::state::with_mut(|s| {
                    if let Some(idx) = s.eq_selected_band {
                        if let Some(ref mut eq) = s.eq_state {
                            eq.bands[idx].q_x10 = q_x10;
                            eq.preset = None;
                            return Some((idx, eq.bands[idx]));
                        }
                    }
                    None
                });
                if let Some((idx, config)) = config {
                    crate::ws::send_msg(&ClientMsg::SetEqBand { band: idx as u8, config });
                    redraw_eq_canvas();
                    update_eq_detail();
                }
            }
        }) as Box<dyn FnMut(_)>);
        slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref()).ok();
        cb.forget();
        dom::append(&row, &slider);
        dom::append(&row, &val_el);
        dom::append(&detail, &row);
    }

    dom::append(&card, &detail);
    dom::append(container, &card);
}

// ─────────────────────── Dynamics (DRC) ───────────────────────

fn render_drc(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    // Header: title + toggle
    let header = dom::create_div();
    dom::set_class(&header, "flex justify-between items-center mb-12");
    let title = dom::el("div", "card-title", Some("Dynamics"));
    dom::set_style(&title, "margin-bottom", "0");

    let toggle_wrap = crate::components::Toggle::create("drc-toggle", "", false, |checked| {
        crate::ws::send_msg(&ClientMsg::SetDrcEnabled(checked));
    });
    dom::append(&header, &title);
    dom::append(&header, &toggle_wrap);
    dom::append(&card, &header);

    // Preset pills
    let presets = SegmentedControl::create(
        "drc-presets",
        &[
            ("off", "Off"),
            ("gentle", "Gentle"),
            ("night", "Night"),
            ("protect", "Protect"),
        ],
        &[],
        SegmentedMode::Single(Box::new(|value| {
            let preset = match value {
                "off" => DrcPreset::Off,
                "gentle" => DrcPreset::Gentle,
                "night" => DrcPreset::LateNight,
                "protect" => DrcPreset::Protect,
                _ => return,
            };
            crate::ws::send_msg(&ClientMsg::SetDrcPreset(preset));
        })),
    );
    dom::set_style(&presets, "margin", "12px 0");
    dom::append(&card, &presets);

    // 3-band DRC controls
    let bands = dom::create_div();
    dom::set_style(&bands, "display", "grid");
    dom::set_style(&bands, "grid-template-columns", "repeat(3, 1fr)");
    dom::set_style(&bands, "gap", "12px");

    for (label, band_enum, idx) in [
        ("Low", DrcBand::Low, 0u8),
        ("Mid", DrcBand::Mid, 1u8),
        ("High", DrcBand::High, 2u8),
    ] {
        let col = dom::create_div();
        dom::set_class(&col, "drc-band-col");

        let band_label = dom::el("div", "drc-band-title", Some(label));
        dom::append(&col, &band_label);

        // Helper to create a DRC parameter slider row
        for (param_label, param_id, min, max, default_val, suffix) in [
            ("Thresh", format!("drc-thresh-{}", idx), -60i32, 0, -20, "dB"),
            ("Ratio", format!("drc-ratio-{}", idx), 10, 100, 20, ""),
            ("Attack", format!("drc-attack-{}", idx), 1, 200, 10, "ms"),
            ("Release", format!("drc-release-{}", idx), 10, 2000, 200, "ms"),
        ] {
            let row = dom::create_div();
            dom::set_class(&row, "drc-param-row");

            let lbl = dom::el("label", "", Some(param_label));
            dom::append(&row, &lbl);

            let slider = dom::create_el("input");
            dom::set_attr(&slider, "type", "range");
            dom::set_attr(&slider, "min", &min.to_string());
            dom::set_attr(&slider, "max", &max.to_string());
            dom::set_attr(&slider, "value", &default_val.to_string());
            slider.set_id(&param_id);

            let val_el = dom::el("span", "drc-val", Some(&format!("{}{}", default_val, suffix)));
            let val_id = format!("{}-val", param_id);
            val_el.set_id(&val_id);

            let band_e = band_enum;
            let band_i = idx as usize;
            let suffix_s = suffix.to_string();
            let val_id_c = val_id.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
                if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                    let raw_val: i32 = input.value().parse().unwrap_or(default_val);
                    if let Some(el) = dom::get_el(&val_id_c) {
                        dom::set_text(&el, &format!("{}{}", raw_val, suffix_s));
                    }
                    // Read current band config from state, override the changed param
                    let config = crate::state::with(|s| {
                        let base = s.drc_state.as_ref()
                            .map(|d| d.bands[band_i])
                            .unwrap_or_default();
                        read_drc_band_from_dom(band_i, base)
                    });
                    crate::ws::send_msg(&ClientMsg::SetDrc { band: band_e, config });
                }
            }) as Box<dyn FnMut(_)>);
            slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref()).ok();
            cb.forget();

            dom::append(&row, &slider);
            dom::append(&row, &val_el);
            dom::append(&col, &row);
        }

        dom::append(&bands, &col);
    }
    dom::append(&card, &bands);

    // Crossover frequency sliders
    let xover = dom::create_div();
    dom::set_class(&xover, "drc-crossover mt-12");

    let xover_title = dom::el("div", "text-sm text-muted mb-8", Some("Crossover Frequencies"));
    dom::append(&xover, &xover_title);

    // Low/Mid crossover: 80-500 Hz
    let low_mid = crate::components::Slider::create("drc-xover-lm", "Low/Mid", 80, 500, 200, "Hz", |v| {
        let mid_high: u16 = dom::get_el("drc-xover-mh")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value().parse().unwrap_or(2000))
            .unwrap_or(2000);
        crate::ws::send_msg(&ClientMsg::SetDrcCrossover {
            low_mid_hz: v as u16,
            mid_high_hz: mid_high,
        });
    });
    dom::append(&xover, &low_mid);

    // Mid/High crossover: 1000-8000 Hz
    let mid_high = crate::components::Slider::create("drc-xover-mh", "Mid/High", 1000, 8000, 2000, "Hz", |v| {
        let low_mid: u16 = dom::get_el("drc-xover-lm")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value().parse().unwrap_or(200))
            .unwrap_or(200);
        crate::ws::send_msg(&ClientMsg::SetDrcCrossover {
            low_mid_hz: low_mid,
            mid_high_hz: v as u16,
        });
    });
    dom::append(&xover, &mid_high);

    dom::append(&card, &xover);

    dom::append(container, &card);
}

/// Read the current DRC band config from DOM slider values.
fn read_drc_band_from_dom(idx: usize, fallback: DrcBandConfig) -> DrcBandConfig {
    let threshold_db = dom::get_el(&format!("drc-thresh-{}", idx))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .and_then(|i| i.value().parse::<i8>().ok())
        .unwrap_or(fallback.threshold_db);
    let ratio_x10 = dom::get_el(&format!("drc-ratio-{}", idx))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .and_then(|i| i.value().parse::<u8>().ok())
        .unwrap_or(fallback.ratio_x10);
    let attack_ms = dom::get_el(&format!("drc-attack-{}", idx))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .and_then(|i| i.value().parse::<u16>().ok())
        .unwrap_or(fallback.attack_ms);
    let release_ms = dom::get_el(&format!("drc-release-{}", idx))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .and_then(|i| i.value().parse::<u16>().ok())
        .unwrap_or(fallback.release_ms);
    DrcBandConfig { threshold_db, ratio_x10, attack_ms, release_ms }
}

// ─────────────────────── DSP Engine ───────────────────────

fn render_dsp(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");

    let dsp_title = dom::el("div", "card-title", Some("DSP Engine"));
    dsp_title.set_id("dsp-title");
    dom::append(&card, &dsp_title);

    // DSP info row
    let info_row = dom::create_div();
    dom::set_class(&info_row, "flex justify-between mb-12");
    let ver = dom::el("span", "stat-label", Some("Version: \u{2014}"));
    ver.set_id("dsp-version");
    let hf = dom::el("span", "stat-value text-sm", Some("HF: \u{2014}"));
    hf.set_id("dsp-hybridflow");
    dom::append(&info_row, &ver);
    dom::append(&info_row, &hf);
    dom::append(&card, &info_row);

    // Mic mute toggle
    let mic_toggle = crate::components::Toggle::create("mic-mute-toggle", "Mic Mute", false, |muted| {
        crate::ws::send_msg(&ClientMsg::SetMicMute(muted));
    });
    dom::set_class(&mic_toggle, "toggle-wrap mb-12");
    dom::append(&card, &mic_toggle);

    // DSP volume slider
    let dsp_vol = crate::components::Slider::create("dsp-vol", "DSP Volume", 1, 100, 50, "%", |v| {
        crate::ws::send_msg(&ClientMsg::SetDspVolume(v as u8));
    });
    dom::append(&card, &dsp_vol);

    // Hidden explorer section (triple-tap to reveal)
    let explorer = dom::create_div();
    explorer.set_id("hw-explorer");
    dom::set_style(&explorer, "display", "none");

    let exp_title = dom::el("div", "card-title text-sm", Some("Hardware Explorer"));
    dom::set_style(&exp_title, "margin-top", "12px");
    dom::append(&explorer, &exp_title);

    // I2C register read row
    let i2c_row = dom::create_div();
    dom::set_class(&i2c_row, "flex gap-8 mb-8 items-center");
    let page_input = dom::create_el("input");
    dom::set_attr(&page_input, "type", "number");
    dom::set_attr(&page_input, "placeholder", "Page");
    dom::set_attr(&page_input, "min", "0");
    dom::set_attr(&page_input, "max", "253");
    dom::set_style(&page_input, "width", "60px");
    page_input.set_id("i2c-page");
    let reg_input = dom::create_el("input");
    dom::set_attr(&reg_input, "type", "number");
    dom::set_attr(&reg_input, "placeholder", "Reg");
    dom::set_attr(&reg_input, "min", "0");
    dom::set_attr(&reg_input, "max", "255");
    dom::set_style(&reg_input, "width", "60px");
    reg_input.set_id("i2c-reg");
    let read_btn = dom::el("button", "btn", Some("Read"));
    dom::on_click(&read_btn, || {
        let page: u8 = dom::get_el("i2c-page")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value().parse().unwrap_or(0))
            .unwrap_or(0);
        let reg: u8 = dom::get_el("i2c-reg")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value().parse().unwrap_or(0))
            .unwrap_or(0);
        crate::ws::send_msg(&ClientMsg::DacRegRead { page, reg });
    });
    let i2c_result = dom::el("span", "stat-value text-sm", Some("\u{2014}"));
    i2c_result.set_id("i2c-result");
    dom::append(&i2c_row, &page_input);
    dom::append(&i2c_row, &reg_input);
    dom::append(&i2c_row, &read_btn);
    dom::append(&i2c_row, &i2c_result);
    dom::append(&explorer, &i2c_row);

    // I2C register write row
    let i2c_write_row = dom::create_div();
    dom::set_class(&i2c_write_row, "flex gap-8 mb-8 items-center");

    let val_input = dom::create_el("input");
    dom::set_attr(&val_input, "type", "text");
    dom::set_attr(&val_input, "placeholder", "Value (hex)");
    dom::set_style(&val_input, "width", "80px");
    val_input.set_id("i2c-write-val");

    let write_btn = dom::el("button", "btn", Some("Write"));
    dom::on_click(&write_btn, || {
        let page: u8 = dom::get_el("i2c-page")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value().parse().unwrap_or(0))
            .unwrap_or(0);
        let reg: u8 = dom::get_el("i2c-reg")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value().parse().unwrap_or(0))
            .unwrap_or(0);
        let val_str = dom::get_el("i2c-write-val")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value())
            .unwrap_or_default();
        let value = u8::from_str_radix(val_str.trim_start_matches("0x"), 16).unwrap_or(0);
        crate::ws::send_msg(&ClientMsg::DacRegWrite { page, reg, value });
    });

    dom::append(&i2c_write_row, &val_input);
    dom::append(&i2c_write_row, &write_btn);
    dom::append(&explorer, &i2c_write_row);

    // SPI command row
    let spi_row = dom::create_div();
    dom::set_class(&spi_row, "flex gap-8 mb-8 items-center");
    let spi_type_input = dom::create_el("input");
    dom::set_attr(&spi_type_input, "type", "text");
    dom::set_attr(&spi_type_input, "placeholder", "Type (hex)");
    dom::set_style(&spi_type_input, "width", "80px");
    spi_type_input.set_id("spi-type");
    let spi_data_input = dom::create_el("input");
    dom::set_attr(&spi_data_input, "type", "text");
    dom::set_attr(&spi_data_input, "placeholder", "Data (hex)");
    dom::set_style(&spi_data_input, "width", "120px");
    spi_data_input.set_id("spi-data");
    let spi_btn = dom::el("button", "btn", Some("Send"));
    dom::on_click(&spi_btn, || {
        let msg_type_str = dom::get_el("spi-type")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value())
            .unwrap_or_default();
        let data_str = dom::get_el("spi-data")
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.value())
            .unwrap_or_default();
        let msg_type = u16::from_str_radix(msg_type_str.trim_start_matches("0x"), 16).unwrap_or(0);
        let data: Vec<u8> = data_str.split_whitespace()
            .filter_map(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .collect();
        crate::ws::send_msg(&ClientMsg::DspSpiSend { msg_type, data });
    });
    let spi_result = dom::el("div", "text-xs text-muted", Some("\u{2014}"));
    spi_result.set_id("spi-result");
    dom::append(&spi_row, &spi_type_input);
    dom::append(&spi_row, &spi_data_input);
    dom::append(&spi_row, &spi_btn);
    dom::append(&explorer, &spi_row);
    dom::append(&explorer, &spi_result);
    dom::append(&card, &explorer);

    // Triple-tap to reveal explorer
    {
        use std::cell::Cell;
        use std::rc::Rc;
        let tap_count = Rc::new(Cell::new(0u32));
        let tap_count_c = tap_count.clone();
        let tap_cb = Closure::wrap(Box::new(move |_: web_sys::Event| {
            let count = tap_count_c.get() + 1;
            tap_count_c.set(count);
            if count >= 3 {
                tap_count_c.set(0);
                if let Some(el) = dom::get_el("hw-explorer") {
                    let current = el.get_attribute("style").unwrap_or_default();
                    if current.contains("display: none") || current.contains("display:none") {
                        dom::set_style(&el, "display", "block");
                    } else {
                        dom::set_style(&el, "display", "none");
                    }
                }
            }
            let tc = tap_count_c.clone();
            crate::dom::set_timeout(move || {
                if tc.get() > 0 && tc.get() < 3 {
                    tc.set(0);
                }
            }, 1000);
        }) as Box<dyn FnMut(_)>);
        if let Some(title_el) = dom::get_el("dsp-title") {
            title_el.add_event_listener_with_callback("click", tap_cb.as_ref().unchecked_ref()).ok();
        }
        tap_cb.forget();
    }

    dom::append(container, &card);
}

// ─────────────────────── Update ───────────────────────

pub fn update() {
    crate::state::with(|s| {
        // Master volume knob
        if let Some(canvas) = dom::get_el("vol-knob") {
            if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                if let Ok(Some(ctx)) = canvas.get_context("2d") {
                    let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                    crate::graphics::knob::draw(&ctx, 180.0, s.master_volume);
                }
            }
        }

        // Show/hide source volume rows
        for (row_id, subsystem_name) in [
            ("mix-row-spotify", "spotify"),
            ("mix-row-bluetooth", "bluetooth"),
            ("mix-row-wyoming", "wyoming"),
        ] {
            let active = s.subsystems.get(subsystem_name)
                .map(|snap| matches!(snap.state, encore_common::protocol::SubsystemState::Running))
                .unwrap_or(false);
            if let Some(el) = dom::get_el(row_id) {
                dom::set_style(&el, "display", if active { "block" } else { "none" });
            }
        }

        if let Some(ref cfg) = s.config {
            for (id, val) in [
                ("vol-spotify", cfg.spotify_volume),
                ("vol-bluetooth", cfg.bluetooth_volume),
            ] {
                if let Some(el) = dom::get_el(id) {
                    dom::set_attr(&el, "value", &val.to_string());
                }
                if let Some(el) = dom::get_el(&format!("{}-val", id)) {
                    dom::set_text(&el, &format!("{}%", val));
                }
            }
            if let Some(el) = dom::get_el("tts-duck") {
                dom::set_attr(&el, "value", &cfg.tts_duck_percent.to_string());
            }
            if let Some(el) = dom::get_el("tts-duck-val") {
                dom::set_text(&el, &format!("{}%", cfg.tts_duck_percent));
            }
            if let Some(el) = dom::get_el("vol-ring-step") {
                dom::set_attr(&el, "value", &cfg.volume_ring_step.to_string());
            }
            if let Some(el) = dom::get_el("vol-ring-step-val") {
                dom::set_text(&el, &format!("{}%", cfg.volume_ring_step));
            }
        }

        // VU meter
        if let Some(canvas) = dom::get_el("vu-meter") {
            if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                if let Ok(Some(ctx)) = canvas.get_context("2d") {
                    let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                    crate::graphics::vu_meter::draw(
                        &ctx, 320.0, 64.0,
                        s.audio_left_rms, s.audio_right_rms,
                        s.audio_left_peak, s.audio_right_peak,
                    );
                }
            }
        }

        // Spectrum analyzer
        if let Some(ref bins) = s.audio_spectrum {
            if s.viz_mode == "spectrum" || s.viz_mode == "both" {
                if let Some(canvas) = dom::get_el("spectrum-canvas") {
                    if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                        if let Ok(Some(ctx)) = canvas.get_context("2d") {
                            let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                            crate::graphics::spectrum::draw(&ctx, bins);
                        }
                    }
                }
            }
        }

        // Oscilloscope
        if let Some(ref waveform) = s.audio_waveform {
            if s.viz_mode == "scope" || s.viz_mode == "both" {
                if let Some(canvas) = dom::get_el("scope-canvas") {
                    if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                        if let Ok(Some(ctx)) = canvas.get_context("2d") {
                            let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                            crate::graphics::scope::draw(&ctx, waveform);
                        }
                    }
                }
            }
        }

        // EQ state — canvas + toggle + presets
        if let Some(ref eq) = s.eq_state {
            if let Some(el) = dom::get_el("eq-toggle") {
                if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                    input.set_checked(eq.enabled);
                }
            }
            if let Some(canvas) = dom::get_el("eq-curve") {
                if let Some(canvas) = canvas.dyn_ref::<web_sys::HtmlCanvasElement>() {
                    if let Ok(Some(ctx)) = canvas.get_context("2d") {
                        let ctx: web_sys::CanvasRenderingContext2d = ctx.unchecked_into();
                        crate::graphics::eq_curve::draw(&ctx, &eq.bands, eq.enabled, s.eq_selected_band);
                    }
                }
            }
            update_eq_preset(eq.preset);
        }

        // DRC state
        if let Some(ref drc) = s.drc_state {
            // Toggle
            if let Some(el) = dom::get_el("drc-toggle") {
                if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                    input.set_checked(drc.enabled);
                }
            }
            // Band parameters
            for (i, band) in drc.bands.iter().enumerate() {
                set_drc_slider(&format!("drc-thresh-{}", i), band.threshold_db as i32, "dB");
                set_drc_slider(&format!("drc-ratio-{}", i), band.ratio_x10 as i32, "");
                set_drc_slider(&format!("drc-attack-{}", i), band.attack_ms as i32, "ms");
                set_drc_slider(&format!("drc-release-{}", i), band.release_ms as i32, "ms");
            }
            // Preset highlighting
            update_drc_preset(drc.preset);
            // Crossover sliders
            set_drc_slider("drc-xover-lm", drc.low_mid_hz as i32, "Hz");
            set_drc_slider("drc-xover-mh", drc.mid_high_hz as i32, "Hz");
        }

        // DSP info
        if let Some(ref dsp) = s.dsp_info {
            if let Some(el) = dom::get_el("dsp-version") {
                dom::set_text(&el, &format!("Version: {}", dsp.version));
            }
            if let Some(el) = dom::get_el("dsp-hybridflow") {
                dom::set_text(&el, &format!("HF: {}", dsp.hybridflow));
            }
            if let Some(el) = dom::get_el("mic-mute-toggle") {
                if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                    input.set_checked(dsp.mic_muted);
                }
            }
            if let Some(el) = dom::get_el("dsp-vol") {
                dom::set_attr(&el, "value", &dsp.dsp_volume.to_string());
            }
            if let Some(el) = dom::get_el("dsp-vol-val") {
                dom::set_text(&el, &format!("{}%", dsp.dsp_volume));
            }
        }

        // Explorer results
        if let Some((page, reg, value)) = s.dac_reg_result {
            if let Some(el) = dom::get_el("i2c-result") {
                dom::set_text(&el, &format!("P{} R0x{:02X} = 0x{:02X}", page, reg, value));
            }
        }
        if let Some(ref data) = s.dsp_spi_result {
            if let Some(el) = dom::get_el("spi-result") {
                let hex: Vec<String> = data.iter().map(|b| format!("{:02X}", b)).collect();
                dom::set_text(&el, &hex.join(" "));
            }
        }
    });
    // EQ detail row (needs separate state access to avoid nested RefCell borrow)
    update_eq_detail();
}

fn set_drc_slider(id: &str, val: i32, suffix: &str) {
    if let Some(el) = dom::get_el(id) {
        dom::set_attr(&el, "value", &val.to_string());
    }
    if let Some(el) = dom::get_el(&format!("{}-val", id)) {
        dom::set_text(&el, &format!("{}{}", val, suffix));
    }
}

fn update_eq_preset(active: Option<EqPreset>) {
    let presets = [
        ("eq-presets-flat", EqPreset::Flat),
        ("eq-presets-bass", EqPreset::BassBoost),
        ("eq-presets-vocal", EqPreset::VocalClarity),
        ("eq-presets-warm", EqPreset::Warm),
        ("eq-presets-night", EqPreset::LateNight),
    ];
    for (id, preset) in presets {
        if let Some(el) = dom::get_el(id) {
            if active == Some(preset) {
                dom::set_class(&el, "seg-btn active");
            } else {
                dom::set_class(&el, "seg-btn");
            }
        }
    }
}

fn update_drc_preset(active: Option<DrcPreset>) {
    let presets = [
        ("drc-presets-off", DrcPreset::Off),
        ("drc-presets-gentle", DrcPreset::Gentle),
        ("drc-presets-night", DrcPreset::LateNight),
        ("drc-presets-protect", DrcPreset::Protect),
    ];
    for (id, preset) in presets {
        if let Some(el) = dom::get_el(id) {
            if active == Some(preset) {
                dom::set_class(&el, "seg-btn active");
            } else {
                dom::set_class(&el, "seg-btn");
            }
        }
    }
}
