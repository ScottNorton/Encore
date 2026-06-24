//! Spotify page — full player with album art, seek, transport, shuffle/repeat.
//!
//! All DOM updates are surgical: only elements whose backing data changed get
//! touched. The client-side position timer interpolates at 1 Hz; the server
//! does NOT broadcast position-only updates.
//!
//! Controls are server-driven: click handlers only send commands via WebSocket.
//! The server broadcasts SpotifyStatus after each command, and update() applies
//! the confirmed state to the DOM.

use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::dom;
use encore_common::protocol::{ClientMsg, SpotifyAction};

thread_local! {
    static POSITION_TIMER: RefCell<Option<i32>> = const { RefCell::new(None) };
    static SETTINGS_LOADED: RefCell<bool> = const { RefCell::new(false) };
    static LAST_COVER_URL: RefCell<String> = const { RefCell::new(String::new()) };
    static VOL_DRAGGING: RefCell<bool> = const { RefCell::new(false) };
}

fn format_time(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// "M:SS of M:SS" caption for the seek slider's `aria-valuetext` (spec a11y:
/// the time lives on the slider on demand, not in the live region). Pure.
fn seek_valuetext(position_ms: u32, duration_ms: u32) -> String {
    format!(
        "{} of {}",
        format_time(position_ms),
        format_time(duration_ms)
    )
}

/// Step the seek position by `delta_secs` seconds (signed), clamped to
/// `[0, duration_ms]`. Used by the seek overlay's arrow keys. Pure.
fn seek_step_ms(position_ms: u32, duration_ms: u32, delta_secs: i32) -> u32 {
    let delta_ms = delta_secs * 1000;
    let next = position_ms as i64 + delta_ms as i64;
    next.clamp(0, duration_ms as i64) as u32
}

/// Wire the Ovation seek overlay (`#sp-seek-overlay`, a `role="slider"` built by
/// `ovation::render`): a pointer click maps to a seek position; arrow keys nudge
/// by 5s. Both send `SpotifyAction::Seek`. This page owns the Seek message and
/// the time formatting; `update()` keeps `aria-valuenow`/`aria-valuetext` synced.
fn wire_seek_overlay() {
    let Some(overlay) = dom::get_el("sp-seek-overlay") else {
        return;
    };

    // Pointer: horizontal fraction across the ring maps to a seek position.
    {
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            if let Some(o) = dom::get_el("sp-seek-overlay") {
                let rect = o.get_bounding_client_rect();
                if rect.width() <= 0.0 {
                    return;
                }
                let pct = ((e.client_x() as f64 - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let duration = crate::state::with(|s| {
                    s.spotify_status
                        .as_ref()
                        .map(|st| st.duration_ms)
                        .unwrap_or(0)
                });
                if duration > 0 {
                    let pos = (pct * duration as f64) as u32;
                    crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Seek {
                        position_ms: pos,
                    }));
                }
            }
        }) as Box<dyn FnMut(_)>);
        overlay
            .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    // Arrow keys: nudge by 5s (Home/End jump to the ends). Reads live position
    // from state so steps are relative to where the track actually is.
    dom::on_keydown(&overlay, |e: web_sys::KeyboardEvent| {
        let (position, duration) = crate::state::with(|s| {
            s.spotify_status
                .as_ref()
                .map(|st| (st.position_ms, st.duration_ms))
                .unwrap_or((0, 0))
        });
        if duration == 0 {
            return;
        }
        let pos = match e.key().as_str() {
            "ArrowRight" | "ArrowUp" => seek_step_ms(position, duration, 5),
            "ArrowLeft" | "ArrowDown" => seek_step_ms(position, duration, -5),
            "Home" => 0,
            "End" => duration,
            _ => return,
        };
        e.prevent_default();
        crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Seek {
            position_ms: pos,
        }));
    });
}

fn start_position_timer() {
    stop_position_timer();
    let id = dom::set_interval(
        || {
            // Only increment if server says we're playing
            let should_update = crate::state::with_mut(|s| {
                if let Some(ref mut st) = s.spotify_status {
                    if st.is_playing && st.position_ms < st.duration_ms {
                        st.position_ms = (st.position_ms + 1000).min(st.duration_ms);
                        return true;
                    }
                }
                false
            });
            if should_update {
                update_progress_only();
            }
        },
        1000,
    );
    POSITION_TIMER.with(|t| *t.borrow_mut() = Some(id));
}

fn stop_position_timer() {
    POSITION_TIMER.with(|t| {
        if let Some(id) = t.borrow_mut().take() {
            dom::window().clear_interval_with_handle(id);
        }
    });
}

fn update_progress_only() {
    crate::state::with(|s| {
        if let Some(st) = s.spotify_status.as_ref() {
            let duration = st.duration_ms;
            let position = st.position_ms;
            if duration > 0 {
                let pct = (position as f64 / duration as f64 * 100.0).min(100.0);
                if let Some(el) = dom::get_el("sp-progress") {
                    dom::set_style(&el, "width", &format!("{:.1}%", pct));
                }
            }
            set_text_if_changed("sp-time-cur", &format_time(position));
            sync_seek_overlay(position, duration);
        }
    });
}

/// Keep the seek slider overlay's `aria-valuenow` (0-100) and `aria-valuetext`
/// ("M:SS of M:SS") in step with the server/interpolated position.
fn sync_seek_overlay(position_ms: u32, duration_ms: u32) {
    if let Some(el) = dom::get_el("sp-seek-overlay") {
        let pct = if duration_ms > 0 {
            (position_ms as f64 / duration_ms as f64 * 100.0).min(100.0) as u32
        } else {
            0
        };
        dom::set_attr(&el, "aria-valuenow", &pct.to_string());
        dom::set_attr(
            &el,
            "aria-valuetext",
            &seek_valuetext(position_ms, duration_ms),
        );
    }
}

fn get_input_value(id: &str) -> Option<String> {
    dom::get_el(id).and_then(|el| {
        js_sys::Reflect::get(&el, &"value".into())
            .ok()
            .and_then(|v| v.as_string())
    })
}

fn get_checkbox_checked(id: &str) -> bool {
    dom::get_el(id)
        .and_then(|el| {
            js_sys::Reflect::get(&el, &"checked".into())
                .ok()
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
}

fn set_select_value(id: &str, val: &str) {
    if let Some(el) = dom::get_el(id) {
        let _ = js_sys::Reflect::set(&el, &"value".into(), &val.into());
    }
}

fn set_checkbox_checked(id: &str, checked: bool) {
    if let Some(el) = dom::get_el(id) {
        let _ = js_sys::Reflect::set(&el, &"checked".into(), &checked.into());
    }
}

fn set_text_if_changed(id: &str, text: &str) {
    if let Some(el) = dom::get_el(id) {
        if el.text_content().as_deref() != Some(text) {
            dom::set_text(&el, text);
        }
    }
}

/// Swap a transport button's inline-SVG glyph, but only when it actually
/// changed (the icon swap path: play↔pause, repeat↔repeat-one). Avoids
/// re-parsing identical SVG markup every update tick.
fn set_icon_if_changed(id: &str, svg: &str) {
    if let Some(el) = dom::get_el(id) {
        if el.inner_html() != svg {
            el.set_inner_html(svg);
        }
    }
}

// ── Transport icons (inline SVG, currentColor so CSS tints them gold/ember) ──
// 24x24 viewBox to match the app's nav icons. Filled glyphs for the transport
// affordances; the repeat/shuffle line glyphs read on the ghost buttons.
const ICON_SHUFFLE: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 7h3.5l3 4.5"/><path d="M14.5 16.5l1 1.5H21"/><path d="M3 17h3.5l11-13H21"/><path d="M18.5 2.5 21 4l-2.5 1.5"/><path d="M18.5 16 21 17.5 18.5 19"/></svg>"#;
const ICON_PREV: &str = r#"<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 5.5a1 1 0 0 0-2 0v13a1 1 0 0 0 2 0V13l9.4 5.6a1 1 0 0 0 1.6-.86V6.26a1 1 0 0 0-1.6-.86L7 11V5.5z"/></svg>"#;
const ICON_NEXT: &str = r#"<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M17 5.5a1 1 0 0 1 2 0v13a1 1 0 0 1-2 0V13l-9.4 5.6A1 1 0 0 1 6 17.74V6.26a1 1 0 0 1 1.6-.86L17 11V5.5z"/></svg>"#;
const ICON_PLAY: &str = r#"<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5.14v13.72a1 1 0 0 0 1.54.84l10.3-6.86a1 1 0 0 0 0-1.68L9.54 4.3A1 1 0 0 0 8 5.14z"/></svg>"#;
const ICON_PAUSE: &str = r#"<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="6" y="4.5" width="4" height="15" rx="1"/><rect x="14" y="4.5" width="4" height="15" rx="1"/></svg>"#;
const ICON_REPEAT: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 2.5 20.5 6 17 9.5"/><path d="M3.5 11V9.5a3.5 3.5 0 0 1 3.5-3.5h13.5"/><path d="M7 21.5 3.5 18 7 14.5"/><path d="M20.5 13v1.5a3.5 3.5 0 0 1-3.5 3.5H3.5"/></svg>"#;
const ICON_REPEAT_ONE: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 2.5 20.5 6 17 9.5"/><path d="M3.5 11V9.5a3.5 3.5 0 0 1 3.5-3.5h13.5"/><path d="M7 21.5 3.5 18 7 14.5"/><path d="M20.5 13v1.5a3.5 3.5 0 0 1-3.5 3.5H3.5"/><path d="M11.3 16.5v-5l-1.4 1" stroke-width="1.6"/></svg>"#;

pub fn render(container: &web_sys::Element) {
    stop_position_timer();
    SETTINGS_LOADED.with(|s| *s.borrow_mut() = false);
    LAST_COVER_URL.with(|s| s.borrow_mut().clear());
    VOL_DRAGGING.with(|d| *d.borrow_mut() = false);
    // spotify::render is reused by Stage; park any stale bloom loop from a prior
    // mount before ovation::render rebuilds the canvas.
    crate::graphics::ovation::park();

    // ── Main player card ──
    let card = dom::create_div();
    dom::set_class(&card, "card glass-card sp-card");
    card.set_id("spotify-card");

    // Ovation rings: builds <img id="sp-art-img"> (album center), an aria-hidden
    // canvas for the gold progress arc + audio-reactive ray bloom, a conic
    // #sp-progress fallback ring, the #sp-explicit badge, and a role="slider"
    // #sp-seek-overlay. spotify::update() still finds every id it drives.
    crate::graphics::ovation::render(&card);

    // Seek a11y: the overlay carries the keyboard/pointer seek + aria-valuetext.
    // ovation::render builds the overlay element but leaves the Seek message to
    // this page, which owns SpotifyAction::Seek and the time formatting.
    wire_seek_overlay();

    // ── Track info (aria-live so track changes are announced; time stays out) ──
    let meta = dom::create_div();
    meta.set_id("sp-meta");
    dom::set_class(&meta, "sp-meta text-center");
    dom::set_attr(&meta, "aria-live", "polite");
    dom::set_attr(&meta, "aria-atomic", "true");

    let title = dom::create_div();
    title.set_id("sp-title");
    dom::set_class(&title, "sp-title");
    dom::append(&meta, &title);

    let artist = dom::create_div();
    artist.set_id("sp-artist");
    dom::set_class(&artist, "sp-artist text-muted");
    dom::append(&meta, &artist);

    let album_el = dom::create_div();
    album_el.set_id("sp-album");
    dom::set_class(&album_el, "sp-album text-muted text-sm mt-4");
    dom::append(&meta, &album_el);

    dom::append(&card, &meta);

    // ── Time labels (below the ring; the seek arc itself is in Ovation) ──
    let time_row = dom::create_div();
    dom::set_class(
        &time_row,
        "flex justify-between text-muted text-sm sp-time-row",
    );
    let time_cur = dom::create_div();
    time_cur.set_id("sp-time-cur");
    dom::set_text(&time_cur, "0:00");
    let time_dur = dom::create_div();
    time_dur.set_id("sp-time-dur");
    dom::set_text(&time_dur, "0:00");
    dom::append(&time_row, &time_cur);
    dom::append(&time_row, &time_dur);
    dom::append(&card, &time_row);

    // ── Transport controls ──
    let controls = dom::create_div();
    dom::set_class(&controls, "sp-transport mt-16");

    // Shuffle — ghost button, gold when active. Inline SVG (currentColor) so the
    // theme tints it, not a colored emoji.
    let shuffle_btn = dom::el("button", "sp-btn", None);
    shuffle_btn.set_inner_html(ICON_SHUFFLE);
    shuffle_btn.set_id("sp-shuffle");
    dom::on_click(&shuffle_btn, || {
        let current = crate::state::with(|s| {
            s.spotify_status
                .as_ref()
                .map(|st| st.shuffle)
                .unwrap_or(false)
        });
        crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Shuffle {
            enabled: !current,
        }));
    });
    dom::append(&controls, &shuffle_btn);

    // Previous — skip button
    let prev_btn = dom::el("button", "sp-btn-skip", None);
    prev_btn.set_inner_html(ICON_PREV);
    dom::on_click(&prev_btn, || {
        crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Previous));
    });
    dom::append(&controls, &prev_btn);

    // Play/Pause — gold spotlight circle. update() swaps inner_html between the
    // play triangle and pause bars.
    let play_btn = dom::el("button", "sp-btn-play", None);
    play_btn.set_inner_html(ICON_PLAY);
    play_btn.set_id("sp-playpause");
    dom::on_click(&play_btn, || {
        let is_playing = crate::state::with(|s| {
            s.spotify_status
                .as_ref()
                .map(|st| st.is_playing)
                .unwrap_or(false)
        });
        let action = if is_playing {
            SpotifyAction::Pause
        } else {
            SpotifyAction::Play
        };
        crate::ws::send_msg(&ClientMsg::SpotifyControl(action));
    });
    dom::append(&controls, &play_btn);

    // Next — skip button
    let next_btn = dom::el("button", "sp-btn-skip", None);
    next_btn.set_inner_html(ICON_NEXT);
    dom::on_click(&next_btn, || {
        crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Next));
    });
    dom::append(&controls, &next_btn);

    // Repeat — ghost button, gold when active, cycles off→context→track→off.
    // update() swaps in the "repeat one" SVG (with a small 1) for track mode.
    let repeat_btn = dom::el("button", "sp-btn", None);
    repeat_btn.set_inner_html(ICON_REPEAT);
    repeat_btn.set_id("sp-repeat");
    dom::on_click(&repeat_btn, || {
        let (rc, rt) = crate::state::with(|s| {
            s.spotify_status
                .as_ref()
                .map(|st| (st.repeat_context, st.repeat_track))
                .unwrap_or((false, false))
        });
        if !rc && !rt {
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Repeat {
                enabled: true,
            }));
        } else if rc && !rt {
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Repeat {
                enabled: false,
            }));
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::RepeatTrack {
                enabled: true,
            }));
        } else {
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::RepeatTrack {
                enabled: false,
            }));
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Repeat {
                enabled: false,
            }));
        }
    });
    dom::append(&controls, &repeat_btn);

    dom::append(&card, &controls);

    // ── Player volume: ONE clean horizontal slider (matches the preview) ──
    // A speaker icon + a gold slider + a % readout. The on_input handler sends
    // SetVolume; VOL_DRAGGING guards against server overwrites mid-drag.
    let vol_section = dom::create_div();
    vol_section.set_id("sp-vol-section");
    dom::set_class(&vol_section, "sp-vol-row mt-16");

    let vol_icon = dom::create_div();
    vol_icon.set_id("sp-vol-icon");
    dom::set_class(&vol_icon, "sp-vol-icon");
    dom::set_attr(&vol_icon, "aria-hidden", "true");
    vol_icon.set_inner_html(vol_icon_svg(50));

    let vol_slider = dom::create_el("input");
    vol_slider.set_id("sp-vol-slider");
    dom::set_attr(&vol_slider, "type", "range");
    dom::set_attr(&vol_slider, "min", "0");
    dom::set_attr(&vol_slider, "max", "100");
    dom::set_attr(&vol_slider, "value", "50");
    dom::set_attr(&vol_slider, "aria-label", "Player volume");
    dom::set_style(&vol_slider, "--sp-vol-fill", "50%");

    // Track drag state to prevent server overwrites during thumb drag
    {
        let down = Closure::wrap(Box::new(|_: web_sys::Event| {
            VOL_DRAGGING.with(|d| *d.borrow_mut() = true);
        }) as Box<dyn FnMut(_)>);
        vol_slider
            .add_event_listener_with_callback("mousedown", down.as_ref().unchecked_ref())
            .ok();
        vol_slider
            .add_event_listener_with_callback("touchstart", down.as_ref().unchecked_ref())
            .ok();
        down.forget();
    }
    {
        let up = Closure::wrap(Box::new(|_: web_sys::Event| {
            VOL_DRAGGING.with(|d| *d.borrow_mut() = false);
        }) as Box<dyn FnMut(_)>);
        vol_slider
            .add_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref())
            .ok();
        vol_slider
            .add_event_listener_with_callback("touchend", up.as_ref().unchecked_ref())
            .ok();
        up.forget();
    }

    // Input event: update label + send volume command
    {
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(input) = e
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                let val = input.value_as_number() as u8;
                if let Some(el) = dom::get_el("sp-vol-label") {
                    dom::set_text(&el, &format!("{}%", val));
                }
                set_vol_icon(val);
                set_vol_fill(val);
                crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::SetVolume {
                    level: val,
                }));
            }
        }) as Box<dyn FnMut(_)>);
        vol_slider
            .add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }

    let vol_label = dom::create_div();
    vol_label.set_id("sp-vol-label");
    dom::set_class(&vol_label, "text-muted text-sm");
    dom::set_style(&vol_label, "min-width", "36px");
    dom::set_style(&vol_label, "text-align", "right");
    dom::set_text(&vol_label, "50%");

    dom::append(&vol_section, &vol_icon);
    dom::append(&vol_section, &vol_slider);
    dom::append(&vol_section, &vol_label);
    dom::append(&card, &vol_section);

    // ── Connection badge ──
    let conn_badge = dom::create_div();
    conn_badge.set_id("sp-connected");
    dom::set_class(&conn_badge, "text-center text-muted text-sm mt-16");
    dom::append(&card, &conn_badge);

    // ── Settings toggle ──
    let settings_toggle = dom::el("button", "sp-settings-btn", Some("Spotify settings"));
    dom::on_click(&settings_toggle, || {
        if let Some(el) = dom::get_el("sp-settings") {
            let hidden = js_sys::Reflect::get(&el, &"hidden".into())
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let _ = js_sys::Reflect::set(&el, &"hidden".into(), &(!hidden).into());
        }
    });
    dom::append(&card, &settings_toggle);

    // ── Settings panel ──
    let settings = dom::create_div();
    settings.set_id("sp-settings");
    let _ = js_sys::Reflect::set(&settings, &"hidden".into(), &true.into());
    dom::set_style(&settings, "border-top", "1px solid var(--border)");
    dom::set_style(&settings, "padding-top", "12px");
    dom::set_style(&settings, "margin-top", "12px");

    // Bitrate
    let bitrate_row = setting_row("Bitrate");
    let bitrate_sel = dom::create_el("select");
    bitrate_sel.set_id("sp-cfg-bitrate");
    dom::set_class(&bitrate_sel, "input");
    dom::set_style(&bitrate_sel, "width", "120px");
    for (val, label) in &[("96", "96 kbps"), ("160", "160 kbps"), ("320", "320 kbps")] {
        let o = dom::create_el("option");
        dom::set_attr(&o, "value", val);
        dom::set_text(&o, label);
        dom::append(&bitrate_sel, &o);
    }
    dom::append(&bitrate_row, &bitrate_sel);
    dom::append(&settings, &bitrate_row);

    // Gapless
    let gapless_row = setting_row("Gapless playback");
    let gapless_cb = dom::create_el("input");
    gapless_cb.set_id("sp-cfg-gapless");
    dom::set_attr(&gapless_cb, "type", "checkbox");
    dom::append(&gapless_row, &gapless_cb);
    dom::append(&settings, &gapless_row);

    // Normalisation
    let norm_row = setting_row("Volume normalisation");
    let norm_cb = dom::create_el("input");
    norm_cb.set_id("sp-cfg-normalisation");
    dom::set_attr(&norm_cb, "type", "checkbox");
    dom::append(&norm_row, &norm_cb);
    dom::append(&settings, &norm_row);

    // Normalisation type
    let ntype_row = setting_row("Normalisation type");
    let ntype_sel = dom::create_el("select");
    ntype_sel.set_id("sp-cfg-norm-type");
    dom::set_class(&ntype_sel, "input");
    dom::set_style(&ntype_sel, "width", "120px");
    for (val, label) in &[("auto", "Auto"), ("album", "Album"), ("track", "Track")] {
        let o = dom::create_el("option");
        dom::set_attr(&o, "value", val);
        dom::set_text(&o, label);
        dom::append(&ntype_sel, &o);
    }
    dom::append(&ntype_row, &ntype_sel);
    dom::append(&settings, &ntype_row);

    // Pregain
    let pregain_row = setting_row("Pregain");
    let pregain_wrap = dom::create_div();
    dom::set_class(&pregain_wrap, "flex items-center gap-8");
    let pregain_slider = dom::create_el("input");
    pregain_slider.set_id("sp-cfg-pregain");
    dom::set_attr(&pregain_slider, "type", "range");
    dom::set_attr(&pregain_slider, "min", "-10");
    dom::set_attr(&pregain_slider, "max", "10");
    dom::set_attr(&pregain_slider, "step", "0.5");
    dom::set_attr(&pregain_slider, "value", "0");
    dom::set_style(&pregain_slider, "width", "80px");
    dom::on_input(&pregain_slider, |val| {
        if let Some(el) = dom::get_el("sp-cfg-pregain-label") {
            dom::set_text(&el, &format!("{} dB", val));
        }
    });
    let pregain_label = dom::create_div();
    pregain_label.set_id("sp-cfg-pregain-label");
    dom::set_class(&pregain_label, "text-muted text-sm");
    dom::set_style(&pregain_label, "min-width", "44px");
    dom::set_text(&pregain_label, "0 dB");
    dom::append(&pregain_wrap, &pregain_slider);
    dom::append(&pregain_wrap, &pregain_label);
    dom::append(&pregain_row, &pregain_wrap);
    dom::append(&settings, &pregain_row);

    // Note
    let note = dom::create_div();
    dom::set_class(&note, "text-muted text-sm mt-8 mb-8");
    dom::set_text(&note, "Changes apply on next Spotify restart");
    dom::append(&settings, &note);

    // Save button
    let save_btn = dom::el("button", "sp-save-btn", Some("Save"));
    dom::on_click(&save_btn, save_spotify_settings);
    dom::append(&settings, &save_btn);

    dom::append(&card, &settings);
    dom::append(container, &card);

    update();
}

/// Paint the META area for the idle (no-track) hero: the connect prompt stands
/// in for title/artist, the album/cover glyph falls back, and the explicit badge
/// hides. The Ovation bloom keeps its calm fallback ring (it reads no cover).
fn apply_idle_meta() {
    set_text_if_changed("sp-title", "Connect with Spotify");
    set_text_if_changed(
        "sp-artist",
        "Open Spotify and select \"Invoke\" as playback device",
    );
    set_text_if_changed("sp-album", "");
    // No cover: hide the <img> so the music-note fallback glyph shows through.
    let url_changed = LAST_COVER_URL.with(|prev| !prev.borrow().is_empty());
    if url_changed {
        if let Some(img) = dom::get_el("sp-art-img") {
            dom::set_style(&img, "display", "none");
        }
        LAST_COVER_URL.with(|prev| prev.borrow_mut().clear());
    }
    if let Some(el) = dom::get_el("sp-explicit") {
        dom::set_style(&el, "display", "none");
    }
}

/// Zero the transport/progress/volume readouts when there is no SpotifyStatus at
/// all. The controls stay present (just dimmed via .sp-idle) so the composition
/// matches the preview whether or not the server has reported playback yet.
fn apply_idle_transport() {
    if let Some(el) = dom::get_el("sp-progress") {
        dom::set_style(&el, "width", "0%");
    }
    set_text_if_changed("sp-time-cur", "0:00");
    set_text_if_changed("sp-time-dur", "0:00");
    sync_seek_overlay(0, 0);
    set_icon_if_changed("sp-playpause", ICON_PLAY);
    set_text_if_changed("sp-connected", "");
}

pub fn update() {
    let is_playing = crate::state::with(|s| {
        let status = s.spotify_status.as_ref();
        let has_track = status.map(|st| st.track.is_some()).unwrap_or(false);

        if let Some(st) = status {
            if let Some(ref track) = st.track {
                set_text_if_changed("sp-title", &track.title);
                set_text_if_changed("sp-artist", &track.artist);
                set_text_if_changed("sp-album", &track.album);

                // Album art — load directly from Spotify CDN.
                // Only update <img> src when the cover URL actually changes.
                let url_changed = LAST_COVER_URL.with(|prev| {
                    let prev = prev.borrow();
                    *prev != track.cover_url
                });
                if url_changed {
                    if let Some(img) = dom::get_el("sp-art-img") {
                        if !track.cover_url.is_empty() {
                            dom::set_attr(&img, "src", &track.cover_url);
                            dom::set_style(&img, "display", "block");
                        } else {
                            dom::set_style(&img, "display", "none");
                        }
                    }
                    LAST_COVER_URL.with(|prev| {
                        *prev.borrow_mut() = track.cover_url.clone();
                    });
                }

                // Explicit badge
                if let Some(el) = dom::get_el("sp-explicit") {
                    dom::set_style(
                        &el,
                        "display",
                        if track.is_explicit { "block" } else { "none" },
                    );
                }
            } else {
                // Idle (no track) but Spotify is up — keep the full hero and put
                // the connect prompt in the META area in place of title/artist.
                apply_idle_meta();
            }

            // Progress + time
            let duration = st.duration_ms;
            let position = st.position_ms;
            if duration > 0 {
                let pct = (position as f64 / duration as f64 * 100.0).min(100.0);
                if let Some(el) = dom::get_el("sp-progress") {
                    dom::set_style(&el, "width", &format!("{:.1}%", pct));
                }
            } else if let Some(el) = dom::get_el("sp-progress") {
                dom::set_style(&el, "width", "0%");
            }
            set_text_if_changed("sp-time-cur", &format_time(position));
            set_text_if_changed("sp-time-dur", &format_time(duration));
            sync_seek_overlay(position, duration);

            // Play/Pause icon — driven by server state only. Swap the inline
            // SVG (play triangle ↔ pause bars), not text content.
            set_icon_if_changed(
                "sp-playpause",
                if st.is_playing { ICON_PAUSE } else { ICON_PLAY },
            );

            // Shuffle highlight — toggle .active class for green color
            if let Some(el) = dom::get_el("sp-shuffle") {
                dom::set_class(
                    &el,
                    if st.shuffle {
                        "sp-btn active"
                    } else {
                        "sp-btn"
                    },
                );
            }

            // Repeat highlight + icon — swap inline SVG ("repeat one" carries a
            // small 1 for track mode) and toggle the gold .active class.
            if let Some(el) = dom::get_el("sp-repeat") {
                if st.repeat_track {
                    set_icon_if_changed("sp-repeat", ICON_REPEAT_ONE);
                    dom::set_class(&el, "sp-btn active");
                } else if st.repeat_context {
                    set_icon_if_changed("sp-repeat", ICON_REPEAT);
                    dom::set_class(&el, "sp-btn active");
                } else {
                    set_icon_if_changed("sp-repeat", ICON_REPEAT);
                    dom::set_class(&el, "sp-btn");
                }
            }

            // Volume slider — skip server updates while user is dragging
            let dragging = VOL_DRAGGING.with(|d| *d.borrow());
            if !dragging {
                let vol_pct = (st.volume as u32 * 100 / 65535u32).min(100) as u8;
                if let Some(el) = dom::get_el("sp-vol-slider") {
                    let _ = js_sys::Reflect::set(&el, &"value".into(), &vol_pct.to_string().into());
                }
                set_text_if_changed("sp-vol-label", &format!("{}%", vol_pct));
                set_vol_icon(vol_pct);
                set_vol_fill(vol_pct);
            }

            // Connection badge
            if let Some(ref user) = st.connected_user {
                set_text_if_changed("sp-connected", &format!("Connected as {}", user));
            } else {
                set_text_if_changed("sp-connected", "");
            }
        }

        // Settings from config — once per page visit
        let already_loaded = SETTINGS_LOADED.with(|s| *s.borrow());
        if !already_loaded {
            if let Some(ref config) = s.config {
                set_select_value("sp-cfg-bitrate", &config.spotify_bitrate);
                set_checkbox_checked("sp-cfg-gapless", config.spotify_gapless);
                set_checkbox_checked("sp-cfg-normalisation", config.spotify_normalisation);
                set_select_value("sp-cfg-norm-type", &config.spotify_normalisation_type);
                if let Some(el) = dom::get_el("sp-cfg-pregain") {
                    let _ = js_sys::Reflect::set(
                        &el,
                        &"value".into(),
                        &config.spotify_normalisation_pregain_db.to_string().into(),
                    );
                }
                if let Some(el) = dom::get_el("sp-cfg-pregain-label") {
                    dom::set_text(
                        &el,
                        &format!("{} dB", config.spotify_normalisation_pregain_db),
                    );
                }
                SETTINGS_LOADED.with(|s| *s.borrow_mut() = true);
            }
        }

        // No status at all (server hasn't sent SpotifyStatus yet): still paint
        // the full hero — the idle prompt in META, zeroed transport/volume.
        if status.is_none() {
            apply_idle_meta();
            apply_idle_transport();
        }

        // The Stage hero is ALWAYS visible (track or not); we only dim the
        // transport when idle via the .sp-idle class. The card itself never
        // collapses — the Ovation bloom + transport + volume always render.
        if let Some(card) = dom::get_el("spotify-card") {
            dom::set_style(&card, "display", "block");
            if has_track {
                dom::remove_class(&card, "sp-idle");
            } else {
                dom::add_class(&card, "sp-idle");
            }
        }

        status.map(|st| st.is_playing).unwrap_or(false)
    });

    // Position timer — starts/stops based on server-confirmed is_playing
    let timer_active = POSITION_TIMER.with(|t| t.borrow().is_some());
    if is_playing && !timer_active {
        start_position_timer();
    } else if !is_playing && timer_active {
        stop_position_timer();
    }
}

fn setting_row(label: &str) -> web_sys::Element {
    let row = dom::create_div();
    dom::set_class(&row, "flex items-center justify-between mb-8");
    let lbl = dom::create_div();
    dom::set_style(&lbl, "font-size", "13px");
    dom::set_text(&lbl, label);
    dom::append(&row, &lbl);
    row
}

/// Inline speaker-icon SVG (currentColor) for the horizontal volume slider,
/// muted / low / loud by level. Pure.
fn vol_icon_svg(val: u8) -> &'static str {
    if val == 0 {
        // Muted: speaker + an X.
        r#"<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><path d="M4 9v6h3l5 4V5L7 9z"/><path d="M16 9.5l5 5M21 9.5l-5 5" fill="none"/></svg>"#
    } else if val < 50 {
        // Low: speaker + one wave.
        r#"<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 9v6h3l5 4V5L7 9z"/><path d="M16 9.5a4 4 0 0 1 0 5" fill="none"/></svg>"#
    } else {
        // Loud: speaker + two waves.
        r#"<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 9v6h3l5 4V5L7 9z"/><path d="M16 9.5a4 4 0 0 1 0 5M18.5 7a8 8 0 0 1 0 10" fill="none"/></svg>"#
    }
}

/// Swap the volume speaker icon for the current level, only when it changed.
fn set_vol_icon(val: u8) {
    let svg = vol_icon_svg(val);
    if let Some(icon) = dom::get_el("sp-vol-icon") {
        if icon.inner_html() != svg {
            icon.set_inner_html(svg);
        }
    }
}

/// Drive the gold fill of the horizontal volume slider by setting the
/// `--sp-vol-fill` custom property (a percentage the track gradient reads).
fn set_vol_fill(val: u8) {
    if let Some(el) = dom::get_el("sp-vol-slider") {
        dom::set_style(&el, "--sp-vol-fill", &format!("{}%", val.min(100)));
    }
}

fn save_spotify_settings() {
    crate::state::with(|s| {
        if let Some(ref config) = s.config {
            let mut cfg = config.clone();
            if let Some(v) = get_input_value("sp-cfg-bitrate") {
                cfg.spotify_bitrate = v;
            }
            cfg.spotify_gapless = get_checkbox_checked("sp-cfg-gapless");
            cfg.spotify_normalisation = get_checkbox_checked("sp-cfg-normalisation");
            if let Some(v) = get_input_value("sp-cfg-norm-type") {
                cfg.spotify_normalisation_type = v;
            }
            if let Some(v) = get_input_value("sp-cfg-pregain") {
                cfg.spotify_normalisation_pregain_db = v.parse().unwrap_or(0.0);
            }
            crate::ws::send_msg(&ClientMsg::SaveConfig(Box::new(cfg)));
            crate::components::toast::success("Spotify settings saved");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_valuetext_formats_position_of_duration() {
        // 1:23 of 3:45
        assert_eq!(seek_valuetext(83_000, 225_000), "1:23 of 3:45");
        assert_eq!(seek_valuetext(0, 0), "0:00 of 0:00");
        assert_eq!(seek_valuetext(5_000, 65_000), "0:05 of 1:05");
    }

    #[test]
    fn seek_step_advances_and_clamps() {
        // +5s from the middle.
        assert_eq!(seek_step_ms(60_000, 200_000, 5), 65_000);
        // -5s from the middle.
        assert_eq!(seek_step_ms(60_000, 200_000, -5), 55_000);
        // Clamps at the end.
        assert_eq!(seek_step_ms(198_000, 200_000, 5), 200_000);
        // Never goes below zero.
        assert_eq!(seek_step_ms(2_000, 200_000, -5), 0);
    }
}
