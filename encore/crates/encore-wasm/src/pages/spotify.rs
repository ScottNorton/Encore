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
    static POSITION_TIMER: RefCell<Option<i32>> = RefCell::new(None);
    static SETTINGS_LOADED: RefCell<bool> = RefCell::new(false);
    static LAST_COVER_URL: RefCell<String> = RefCell::new(String::new());
    static VOL_DRAGGING: RefCell<bool> = RefCell::new(false);
}

fn format_time(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn start_position_timer() {
    stop_position_timer();
    let id = dom::set_interval(|| {
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
    }, 1000);
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
        }
    });
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

pub fn render(container: &web_sys::Element) {
    stop_position_timer();
    SETTINGS_LOADED.with(|s| *s.borrow_mut() = false);
    LAST_COVER_URL.with(|s| s.borrow_mut().clear());
    VOL_DRAGGING.with(|d| *d.borrow_mut() = false);

    // ── Main player card ──
    let card = dom::create_div();
    dom::set_class(&card, "card glass-card");
    card.set_id("spotify-card");

    // Album art container
    let art_wrap = dom::create_div();
    art_wrap.set_id("sp-art-wrap");
    dom::set_style(&art_wrap, "width", "280px");
    dom::set_style(&art_wrap, "height", "280px");
    dom::set_style(&art_wrap, "max-width", "100%");
    dom::set_style(&art_wrap, "border-radius", "12px");
    dom::set_style(&art_wrap, "margin", "0 auto 12px auto");
    dom::set_style(&art_wrap, "overflow", "hidden");
    dom::set_style(&art_wrap, "position", "relative");
    // Gradient fallback
    dom::set_style(&art_wrap, "background", "linear-gradient(135deg, #1db954 0%, #191414 100%)");

    // <img> element for album art (no crossorigin!)
    let art_img = dom::create_el("img");
    art_img.set_id("sp-art-img");
    dom::set_attr(&art_img, "alt", "");
    dom::set_style(&art_img, "width", "100%");
    dom::set_style(&art_img, "height", "100%");
    dom::set_style(&art_img, "object-fit", "cover");
    dom::set_style(&art_img, "display", "none"); // hidden until loaded

    // onload: show the image
    {
        let onload = Closure::wrap(Box::new(|_: web_sys::Event| {
            web_sys::console::log_1(&"SP: album art loaded OK".into());
            if let Some(el) = dom::get_el("sp-art-img") {
                dom::set_style(&el, "display", "block");
            }
        }) as Box<dyn FnMut(_)>);
        let _ = art_img.add_event_listener_with_callback("load", onload.as_ref().unchecked_ref());
        onload.forget();
    }

    // onerror: log failure
    {
        let onerror = Closure::wrap(Box::new(|_: web_sys::Event| {
            web_sys::console::error_1(&"SP: album art FAILED to load".into());
            if let Some(el) = dom::get_el("sp-art-img") {
                dom::set_style(&el, "display", "none");
            }
        }) as Box<dyn FnMut(_)>);
        let _ = art_img.add_event_listener_with_callback("error", onerror.as_ref().unchecked_ref());
        onerror.forget();
    }

    dom::append(&art_wrap, &art_img);

    // Explicit badge
    let explicit = dom::create_div();
    explicit.set_id("sp-explicit");
    dom::set_style(&explicit, "display", "none");
    dom::set_style(&explicit, "position", "absolute");
    dom::set_style(&explicit, "bottom", "8px");
    dom::set_style(&explicit, "right", "8px");
    dom::set_style(&explicit, "background", "rgba(255,255,255,0.15)");
    dom::set_style(&explicit, "color", "#fff");
    dom::set_style(&explicit, "font-size", "11px");
    dom::set_style(&explicit, "font-weight", "700");
    dom::set_style(&explicit, "padding", "2px 6px");
    dom::set_style(&explicit, "border-radius", "3px");
    dom::set_text(&explicit, "E");
    dom::append(&art_wrap, &explicit);

    dom::append(&card, &art_wrap);

    // ── Track info ──
    let title = dom::create_div();
    title.set_id("sp-title");
    dom::set_class(&title, "text-center");
    dom::set_style(&title, "font-size", "18px");
    dom::set_style(&title, "font-weight", "700");
    dom::set_style(&title, "margin-bottom", "4px");
    dom::set_style(&title, "white-space", "nowrap");
    dom::set_style(&title, "overflow", "hidden");
    dom::set_style(&title, "text-overflow", "ellipsis");
    dom::append(&card, &title);

    let artist = dom::create_div();
    artist.set_id("sp-artist");
    dom::set_class(&artist, "text-center text-muted");
    dom::set_style(&artist, "white-space", "nowrap");
    dom::set_style(&artist, "overflow", "hidden");
    dom::set_style(&artist, "text-overflow", "ellipsis");
    dom::append(&card, &artist);

    let album_el = dom::create_div();
    album_el.set_id("sp-album");
    dom::set_class(&album_el, "text-center text-muted text-sm mt-4");
    dom::set_style(&album_el, "white-space", "nowrap");
    dom::set_style(&album_el, "overflow", "hidden");
    dom::set_style(&album_el, "text-overflow", "ellipsis");
    dom::append(&card, &album_el);

    // ── Progress / Seek bar ──
    let progress_section = dom::create_div();
    dom::set_class(&progress_section, "mt-16");

    let bar_wrap = dom::create_div();
    bar_wrap.set_id("sp-bar-wrap");
    dom::set_class(&bar_wrap, "sp-seek-wrap");
    dom::set_style(&bar_wrap, "cursor", "pointer");
    dom::set_style(&bar_wrap, "padding", "8px 0");
    let track_bar = dom::create_div();
    dom::set_class(&track_bar, "sp-seek-track");
    let fill = dom::create_div();
    fill.set_id("sp-progress");
    dom::set_class(&fill, "sp-seek-fill");
    dom::set_style(&fill, "width", "0%");
    // Seek dot (thumb that appears on hover)
    let seek_dot = dom::create_div();
    dom::set_class(&seek_dot, "sp-seek-dot");
    dom::append(&fill, &seek_dot);
    dom::append(&track_bar, &fill);
    dom::append(&bar_wrap, &track_bar);
    dom::append(&progress_section, &bar_wrap);

    // Seek click handler — only sends command, no state mutation
    {
        let bar_id = "sp-bar-wrap".to_string();
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            if let Some(bar) = dom::get_el(&bar_id) {
                let rect = bar.get_bounding_client_rect();
                let pct = ((e.client_x() as f64 - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let duration = crate::state::with(|s| {
                    s.spotify_status.as_ref().map(|st| st.duration_ms).unwrap_or(0)
                });
                if duration > 0 {
                    let pos = (pct * duration as f64) as u32;
                    crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Seek { position_ms: pos }));
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = bar_wrap.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Time labels
    let time_row = dom::create_div();
    dom::set_class(&time_row, "flex justify-between text-muted text-sm");
    dom::set_style(&time_row, "margin-top", "4px");
    let time_cur = dom::create_div();
    time_cur.set_id("sp-time-cur");
    dom::set_text(&time_cur, "0:00");
    let time_dur = dom::create_div();
    time_dur.set_id("sp-time-dur");
    dom::set_text(&time_dur, "0:00");
    dom::append(&time_row, &time_cur);
    dom::append(&time_row, &time_dur);
    dom::append(&progress_section, &time_row);

    dom::append(&card, &progress_section);

    // ── Transport controls ──
    let controls = dom::create_div();
    dom::set_class(&controls, "sp-transport mt-16");

    // Shuffle — ghost button, green when active
    let shuffle_btn = dom::el("button", "sp-btn", Some("\u{1F500}"));
    shuffle_btn.set_id("sp-shuffle");
    dom::on_click(&shuffle_btn, || {
        let current = crate::state::with(|s| {
            s.spotify_status.as_ref().map(|st| st.shuffle).unwrap_or(false)
        });
        crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Shuffle { enabled: !current }));
    });
    dom::append(&controls, &shuffle_btn);

    // Previous — skip button
    let prev_btn = dom::el("button", "sp-btn-skip", Some("\u{23EE}"));
    dom::on_click(&prev_btn, || {
        crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Previous));
    });
    dom::append(&controls, &prev_btn);

    // Play/Pause — big green circle
    let play_btn = dom::el("button", "sp-btn-play", Some("\u{25B6}"));
    play_btn.set_id("sp-playpause");
    dom::on_click(&play_btn, || {
        let is_playing = crate::state::with(|s| {
            s.spotify_status.as_ref().map(|st| st.is_playing).unwrap_or(false)
        });
        let action = if is_playing { SpotifyAction::Pause } else { SpotifyAction::Play };
        crate::ws::send_msg(&ClientMsg::SpotifyControl(action));
    });
    dom::append(&controls, &play_btn);

    // Next — skip button
    let next_btn = dom::el("button", "sp-btn-skip", Some("\u{23ED}"));
    dom::on_click(&next_btn, || {
        crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Next));
    });
    dom::append(&controls, &next_btn);

    // Repeat — ghost button, green when active, cycles off→context→track→off
    let repeat_btn = dom::el("button", "sp-btn", Some("\u{1F501}"));
    repeat_btn.set_id("sp-repeat");
    dom::on_click(&repeat_btn, || {
        let (rc, rt) = crate::state::with(|s| {
            s.spotify_status.as_ref()
                .map(|st| (st.repeat_context, st.repeat_track))
                .unwrap_or((false, false))
        });
        if !rc && !rt {
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Repeat { enabled: true }));
        } else if rc && !rt {
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Repeat { enabled: false }));
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::RepeatTrack { enabled: true }));
        } else {
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::RepeatTrack { enabled: false }));
            crate::ws::send_msg(&ClientMsg::SpotifyControl(SpotifyAction::Repeat { enabled: false }));
        }
    });
    dom::append(&controls, &repeat_btn);

    dom::append(&card, &controls);

    // ── Volume slider ──
    let vol_section = dom::create_div();
    vol_section.set_id("sp-vol-section");
    dom::set_class(&vol_section, "sp-vol-row mt-16");

    let vol_icon = dom::create_div();
    vol_icon.set_id("sp-vol-icon");
    dom::set_style(&vol_icon, "font-size", "16px");
    dom::set_style(&vol_icon, "min-width", "20px");
    dom::set_text(&vol_icon, "\u{1F509}"); // 🔉

    let vol_slider = dom::create_el("input");
    vol_slider.set_id("sp-vol-slider");
    dom::set_attr(&vol_slider, "type", "range");
    dom::set_attr(&vol_slider, "min", "0");
    dom::set_attr(&vol_slider, "max", "100");
    dom::set_attr(&vol_slider, "value", "50");

    // Track drag state to prevent server overwrites during thumb drag
    {
        let down = Closure::wrap(Box::new(|_: web_sys::Event| {
            VOL_DRAGGING.with(|d| *d.borrow_mut() = true);
        }) as Box<dyn FnMut(_)>);
        vol_slider.add_event_listener_with_callback("mousedown", down.as_ref().unchecked_ref()).ok();
        vol_slider.add_event_listener_with_callback("touchstart", down.as_ref().unchecked_ref()).ok();
        down.forget();
    }
    {
        let up = Closure::wrap(Box::new(|_: web_sys::Event| {
            VOL_DRAGGING.with(|d| *d.borrow_mut() = false);
        }) as Box<dyn FnMut(_)>);
        vol_slider.add_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref()).ok();
        vol_slider.add_event_listener_with_callback("touchend", up.as_ref().unchecked_ref()).ok();
        up.forget();
    }

    // Input event: update label + send volume command
    {
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                let val = input.value_as_number() as u8;
                if let Some(el) = dom::get_el("sp-vol-label") {
                    dom::set_text(&el, &format!("{}%", val));
                }
                // Update icon based on level
                if let Some(icon) = dom::get_el("sp-vol-icon") {
                    let emoji = if val == 0 { "\u{1F507}" } else if val < 50 { "\u{1F509}" } else { "\u{1F50A}" };
                    dom::set_text(&icon, emoji);
                }
                crate::ws::send_msg(&ClientMsg::SpotifyControl(
                    SpotifyAction::SetVolume { level: val },
                ));
            }
        }) as Box<dyn FnMut(_)>);
        vol_slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref()).ok();
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
    let settings_toggle = dom::el("button", "sp-settings-btn", Some("Settings"));
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

    // ── Empty state ──
    let empty = dom::create_div();
    empty.set_id("sp-empty");
    dom::set_class(&empty, "text-center text-muted mt-16");

    let empty_icon = dom::create_div();
    dom::set_style(&empty_icon, "font-size", "48px");
    dom::set_style(&empty_icon, "margin-bottom", "12px");
    dom::set_style(&empty_icon, "color", "#1db954");
    dom::set_style(&empty_icon, "opacity", "0.6");
    empty_icon.set_inner_html("&#9835;");
    dom::append(&empty, &empty_icon);

    let empty_title = dom::create_div();
    dom::set_style(&empty_title, "font-size", "16px");
    dom::set_style(&empty_title, "font-weight", "600");
    dom::set_style(&empty_title, "margin-bottom", "8px");
    dom::set_text(&empty_title, "Connect with Spotify");
    dom::append(&empty, &empty_title);

    let empty_desc = dom::create_div();
    dom::set_class(&empty_desc, "text-sm");
    dom::set_text(&empty_desc, "Open Spotify and select \"Invoke\" as playback device");
    dom::append(&empty, &empty_desc);

    dom::append(container, &empty);

    update();
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
                    dom::set_style(&el, "display", if track.is_explicit { "block" } else { "none" });
                }
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

            // Play/Pause icon — driven by server state only
            let play_icon = if st.is_playing { "\u{23F8}" } else { "\u{25B6}" };
            set_text_if_changed("sp-playpause", play_icon);

            // Shuffle highlight — toggle .active class for green color
            if let Some(el) = dom::get_el("sp-shuffle") {
                dom::set_class(&el, if st.shuffle { "sp-btn active" } else { "sp-btn" });
            }

            // Repeat highlight + icon — toggle .active class
            if let Some(el) = dom::get_el("sp-repeat") {
                if st.repeat_track {
                    set_text_if_changed("sp-repeat", "\u{1F502}");
                    dom::set_class(&el, "sp-btn active");
                } else if st.repeat_context {
                    set_text_if_changed("sp-repeat", "\u{1F501}");
                    dom::set_class(&el, "sp-btn active");
                } else {
                    set_text_if_changed("sp-repeat", "\u{1F501}");
                    dom::set_class(&el, "sp-btn");
                }
            }

            // Volume slider — skip server updates while user is dragging
            let dragging = VOL_DRAGGING.with(|d| *d.borrow());
            if !dragging {
                let vol_pct = (st.volume as u32 * 100 / 65535u32).min(100) as u8;
                if let Some(el) = dom::get_el("sp-vol-slider") {
                    let _ = js_sys::Reflect::set(
                        &el,
                        &"value".into(),
                        &vol_pct.to_string().into(),
                    );
                }
                set_text_if_changed("sp-vol-label", &format!("{}%", vol_pct));
                let emoji = if vol_pct == 0 { "\u{1F507}" } else if vol_pct < 50 { "\u{1F509}" } else { "\u{1F50A}" };
                set_text_if_changed("sp-vol-icon", emoji);
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
                    dom::set_text(&el, &format!("{} dB", config.spotify_normalisation_pregain_db));
                }
                SETTINGS_LOADED.with(|s| *s.borrow_mut() = true);
            }
        }

        // Visibility
        if let Some(card) = dom::get_el("spotify-card") {
            dom::set_style(&card, "display", if has_track { "block" } else { "none" });
        }
        if let Some(empty) = dom::get_el("sp-empty") {
            dom::set_style(&empty, "display", if has_track { "none" } else { "block" });
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
        }
    });
}
