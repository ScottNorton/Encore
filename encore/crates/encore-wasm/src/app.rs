//! App shell — header, primary nav, hash routing.

use crate::dom;
use std::cell::Cell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Primary nav destinations: (route_id, display_label). Settings is the gateway
/// to every configure/diagnose surface.
pub const NAV: &[(&str, &str)] = &[
    ("home", "Stage"),
    ("sound", "Sound"),
    ("lights", "Lights"),
    ("speakers", "Groups"),
    ("settings", "Settings"),
];

/// Control destinations that horizontal swipe cycles through (Settings excluded).
const SWIPE_DESTS: &[&str] = &["home", "sound", "lights", "speakers"];

thread_local! {
    /// Guards against overlapping page transitions.
    static TRANSITIONING: Cell<bool> = const { Cell::new(false) };
}

/// Initialize the app shell after boot sequence completes.
///
/// `landing` is the page to show first: "home", "connect", or "setup".
/// Called by `boot::run()` after the smart boot sequence finishes.
pub fn init_after_boot(landing: &str) {
    let body = dom::body();

    // Build app container (hidden for crossfade from loading screen)
    let app = dom::create_div();
    app.set_id("app");
    dom::set_style(&app, "opacity", "0");

    // Reconnecting bar — first child so it sits above the header when the link
    // drops. Hidden until a live connection is actually lost.
    let recon = dom::create_div();
    recon.set_id("reconnect-bar");
    dom::set_class(&recon, "reconnect-bar");
    dom::set_text(&recon, "Reconnecting to your speaker\u{2026}");
    dom::set_style(&recon, "display", "none");
    dom::append(&app, &recon);

    // Header
    let header = build_header();
    dom::append(&app, &header);

    // Unified logo (fixed position, animates between states)
    let logo = crate::brand::build_logo();
    dom::append(&app, &logo);

    // Primary navigation (bottom bar on mobile, left rail on desktop)
    let nav = build_nav();
    dom::append(&app, &nav);

    // Persistent mini now-playing bar (app-shell scope: built once, survives
    // route() content swaps, updated surgically from the WS dispatcher).
    let mini = build_mini_bar();
    dom::append(&app, &mini);

    // Content area
    let content = dom::create_div();
    content.set_id("content");
    dom::set_class(&content, "content");
    dom::append(&app, &content);

    body.append_child(&app).unwrap();

    // Animated morph: loading screen → logo in correct state
    if let Some(loading) = dom::get_el("loading") {
        dom::add_class(&loading, "exit");
        let landing_page = landing.to_string();
        dom::set_timeout(
            move || {
                if let Some(app) = dom::get_el("app") {
                    dom::set_style(&app, "transition", "opacity 0.5s ease");
                    dom::set_style(&app, "opacity", "1");
                }
                let lp = landing_page.clone();
                dom::set_timeout(
                    move || {
                        if lp == "connect" {
                            crate::brand::set_state(crate::brand::LogoState::Hero);
                        } else {
                            crate::brand::set_state(crate::brand::LogoState::Header);
                        }
                    },
                    100,
                );
                dom::set_timeout(
                    move || {
                        if let Some(loading) = dom::get_el("loading") {
                            loading.remove();
                        }
                    },
                    1200,
                );
            },
            50,
        );
    } else {
        // No loading screen (hot reload) — show immediately
        dom::set_style(&app, "opacity", "1");
    }

    // Set up hash routing
    setup_routing();

    // Set up touch swipe between control destinations
    setup_swipe_navigation();

    // Desktop Tauri: add body class for drag region
    apply_desktop_mode();
    dom::set_timeout(apply_desktop_mode, 150);

    // Sync localStorage speaker host into state (boot.rs may have set it already)
    if let Some(host) = dom::get_local("encore_speaker_host") {
        crate::state::with_mut(|s| {
            if s.speaker_host.is_none() {
                s.speaker_host = Some(host);
            }
        });
    }

    // Route to the landing page
    if landing == "connect" {
        if let Some(nav) = dom::get_el("primary-nav") {
            dom::set_style(&nav, "display", "none");
        }
        dom::window().location().set_hash("connect").ok();
    } else {
        check_setup_status();
        let hash = dom::window().location().hash().unwrap_or_default();
        if hash.is_empty() || hash == "#" {
            dom::window().location().set_hash(landing).ok();
        }
    }
    route();
}

fn build_header() -> web_sys::Element {
    let header = dom::create_el("header");
    dom::set_class(&header, "header");

    let left = dom::create_div();
    dom::set_class(&left, "header-left");
    dom::append(&header, &left);

    // Window control buttons — placed in a header-right container
    let right = dom::create_div();
    dom::set_style(&right, "display", "flex");
    dom::set_style(&right, "align-items", "center");
    dom::set_style(&right, "gap", "0px");
    dom::set_style(&right, "flex-shrink", "0");

    // Device name (populated when config arrives via WebSocket)
    let name = dom::create_el("span");
    name.set_id("device-name");
    dom::set_class(&name, "header-device-name");
    dom::append(&right, &name);

    // Connection status dot (to the left of the hamburger)
    let dot = dom::create_el("span");
    dot.set_id("conn-dot");
    dom::set_class(&dot, "conn-dot");
    dom::append(&right, &dot);

    // Gear button — stopPropagation prevents the document-level
    // close handler from racing with the toggle.
    let gear = dom::create_el("button");
    gear.set_id("gear-btn");
    dom::set_class(&gear, "gear-btn");
    gear.set_inner_html(r#"<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M12 2.5l1.4 2.6 2.9-.5.6 2.9 2.6 1.4-1.3 2.6 1.3 2.6-2.6 1.4-.6 2.9-2.9-.5L12 21.5l-1.4-2.6-2.9.5-.6-2.9L4.5 15l1.3-2.6L4.5 9.8l2.6-1.4.6-2.9 2.9.5z"/></svg>"#);
    dom::set_attr(&gear, "aria-label", "Settings menu");
    dom::set_attr(&gear, "title", "Settings");
    {
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            e.stop_propagation();
            dom::window().location().set_hash("settings").ok();
        }) as Box<dyn FnMut(_)>);
        gear.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }
    dom::append(&right, &gear);

    // Window control buttons — desktop only (Android has its own chrome)
    if dom::is_desktop_app() {
        // Vertical divider between hamburger and window controls
        let divider = dom::create_el("span");
        dom::set_class(&divider, "header-divider");
        dom::append(&right, &divider);

        let btn_min = dom::create_el("button");
        dom::set_class(&btn_min, "wc-btn wc-minimize");
        btn_min.set_inner_html(r#"<svg width="12" height="12" viewBox="0 0 12 12"><rect x="1" y="5.5" width="10" height="1" fill="currentColor"/></svg>"#);
        dom::set_attr(&btn_min, "aria-label", "Minimize");
        dom::set_attr(&btn_min, "title", "Minimize");
        {
            let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
                wnd_eval("minimize()");
            }) as Box<dyn FnMut(_)>);
            btn_min
                .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
                .unwrap();
            cb.forget();
        }
        dom::append(&right, &btn_min);

        let btn_max = dom::create_el("button");
        btn_max.set_id("wc-maximize-btn");
        dom::set_class(&btn_max, "wc-btn wc-maximize");
        // Two SVGs: maximize (single rect) shown by default, restore (overlapping rects) hidden
        // Two SVGs: maximize icon (single rect) and restore icon (overlapping rects).
        // CSS toggles visibility via .maximized class on the button.
        btn_max.set_inner_html(r#"<svg class="wc-icon-maximize" width="12" height="12" viewBox="0 0 12 12"><rect x="1.5" y="1.5" width="9" height="9" rx="1" fill="none" stroke="currentColor" stroke-width="1.2"/></svg><svg class="wc-icon-restore" width="12" height="12" viewBox="0 0 12 12"><rect x="3" y="0.5" width="8" height="8" rx="1" fill="none" stroke="currentColor" stroke-width="1.2"/><rect x="0.5" y="3" width="8" height="8" rx="1" fill="var(--bg-header, #1a1a2e)" stroke="currentColor" stroke-width="1.2"/></svg>"#);
        dom::set_attr(&btn_max, "aria-label", "Maximize");
        dom::set_attr(&btn_max, "title", "Maximize");
        {
            let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
                wnd_eval("toggleMaximize()");
                // Swap icon after a short delay to let the state change
                dom::set_timeout(update_maximize_icon, 50);
            }) as Box<dyn FnMut(_)>);
            btn_max
                .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
                .unwrap();
            cb.forget();
        }
        dom::append(&right, &btn_max);

        let btn_close = dom::create_el("button");
        dom::set_class(&btn_close, "wc-btn wc-close");
        btn_close.set_inner_html(r#"<svg width="12" height="12" viewBox="0 0 12 12"><path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>"#);
        dom::set_attr(&btn_close, "aria-label", "Close");
        dom::set_attr(&btn_close, "title", "Close");
        {
            let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
                wnd_eval("close()");
            }) as Box<dyn FnMut(_)>);
            btn_close
                .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
                .unwrap();
            cb.forget();
        }
        dom::append(&right, &btn_close);
    }

    dom::append(&header, &right);

    // Drag the window when mousedown on the header (not on buttons/interactive elements).
    // Uses Tauri's startDragging() API — CSS -webkit-app-region doesn't work in WebView2.
    if dom::is_desktop_app() {
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            // Don't drag if the click target is a button or inside one
            if let Some(target) = e.target() {
                let el: web_sys::Element = target.dyn_into().unwrap();
                if el.closest("button").ok().flatten().is_some() {
                    return;
                }
            }
            wnd_eval("startDragging()");
        }) as Box<dyn FnMut(_)>);
        header
            .add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }

    header
}

/// Build the persistent mini now-playing bar (app-shell scope).
///
/// Mounted once between the nav and the content area; never rebuilt. Starts
/// hidden and is shown/populated by `mini_update` from the WS dispatcher.
fn build_mini_bar() -> web_sys::Element {
    let bar = dom::create_el("button");
    bar.set_id("mini-nowplaying");
    dom::set_class(&bar, "mini-nowplaying");
    dom::set_attr(&bar, "aria-hidden", "true");
    dom::set_attr(&bar, "aria-label", "Now playing, open Stage");
    dom::set_style(&bar, "display", "none");

    // Tapping the bar (outside the play button) routes to Stage.
    dom::on_click(&bar, || {
        dom::window().location().set_hash("home").ok();
    });

    // Album thumbnail (static <img>, never the ray canvas).
    let art = dom::create_el("img");
    art.set_id("mini-art");
    dom::set_class(&art, "mini-art");
    dom::set_attr(&art, "alt", "");
    dom::append(&bar, &art);

    // Source glyph + track meta.
    let meta = dom::create_div();
    dom::set_class(&meta, "mini-meta");

    let src = dom::create_el("span");
    src.set_id("mini-source");
    dom::set_class(&src, "mini-source");
    dom::append(&meta, &src);

    let text = dom::create_div();
    dom::set_class(&text, "mini-text");
    let title = dom::el("div", "mini-title", None);
    title.set_id("mini-title");
    let artist = dom::el("div", "mini-artist", None);
    artist.set_id("mini-artist");
    dom::append(&text, &title);
    dom::append(&text, &artist);
    dom::append(&meta, &text);
    dom::append(&bar, &meta);

    // Compact play/pause. stop_propagation so it doesn't also route to Stage.
    let play = dom::create_el("button");
    play.set_id("mini-playpause");
    dom::set_class(&play, "mini-playpause");
    dom::set_attr(&play, "aria-label", "Play or pause");
    play.set_text_content(Some("\u{25B6}"));
    {
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            e.stop_propagation();
            let is_playing = crate::state::with(|s| {
                s.spotify_status
                    .as_ref()
                    .map(|st| st.is_playing)
                    .unwrap_or(false)
            });
            let action = if is_playing {
                encore_common::protocol::SpotifyAction::Pause
            } else {
                encore_common::protocol::SpotifyAction::Play
            };
            crate::ws::send_msg(&encore_common::protocol::ClientMsg::SpotifyControl(action));
        }) as Box<dyn FnMut(_)>);
        play.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }
    dom::append(&bar, &play);

    bar
}

thread_local! {
    /// Last cover URL written to the mini-bar thumb (avoids re-decoding the image
    /// on every surgical update).
    static MINI_COVER_URL: Cell<Option<String>> = const { Cell::new(None) };
}

/// Set text content only when it differs (avoids needless DOM writes).
fn mini_set_text(id: &str, text: &str) {
    if let Some(el) = dom::get_el(id) {
        if el.text_content().as_deref() != Some(text) {
            dom::set_text(&el, text);
        }
    }
}

/// Surgically update the mini now-playing bar from current state.
///
/// Hidden (`aria-hidden` + `display:none`) when there is no track, Spotify is
/// disabled, or the WS link is down. Otherwise shows the source glyph, album
/// thumb, and title/artist, and reflects the play/pause icon. Called directly
/// from the WS dispatcher on SpotifyStatus / TrackChanged; never via a heavy
/// page `update()`.
pub fn mini_update() {
    let Some(bar) = dom::get_el("mini-nowplaying") else {
        return;
    };

    let (connected, spotify_enabled, track, is_playing, source) = crate::state::with(|s| {
        (
            s.connected,
            s.config.as_ref().map(|c| c.spotify_enabled).unwrap_or(true),
            s.track.clone(),
            s.spotify_status
                .as_ref()
                .map(|st| st.is_playing)
                .unwrap_or(false),
            crate::state::active_source(s),
        )
    });

    let show = connected && spotify_enabled && track.is_some();
    if !show {
        dom::set_attr(&bar, "aria-hidden", "true");
        dom::set_style(&bar, "display", "none");
        return;
    }

    dom::set_attr(&bar, "aria-hidden", "false");
    dom::set_style(&bar, "display", "flex");

    let track = track.unwrap();
    mini_set_text("mini-title", &track.title);
    mini_set_text("mini-artist", &track.artist);

    // Source glyph: distinct per derived active source (text, never color-only).
    let glyph = match source {
        "bluetooth" => "\u{1F4F6}", // antenna bars
        "voice" => "\u{1F3A4}",     // microphone
        _ => "\u{266B}",            // notes (Spotify/audio)
    };
    mini_set_text("mini-source", glyph);

    // Album thumb: dedupe on URL so we don't re-fetch the image each update.
    let changed = MINI_COVER_URL.with(|c| {
        let prev = c.take();
        let same = prev.as_deref() == Some(track.cover_url.as_str());
        c.set(Some(track.cover_url.clone()));
        !same
    });
    if changed {
        if let Some(art) = dom::get_el("mini-art") {
            if track.cover_url.is_empty() {
                dom::set_style(&art, "display", "none");
            } else {
                dom::set_attr(&art, "src", &track.cover_url);
                dom::set_style(&art, "display", "block");
            }
        }
    }

    mini_set_text(
        "mini-playpause",
        if is_playing { "\u{23F8}" } else { "\u{25B6}" },
    );
}

fn build_nav() -> web_sys::Element {
    let nav = dom::create_el("nav");
    nav.set_id("primary-nav");
    dom::set_class(&nav, "primary-nav");

    for (id, label) in NAV {
        let item = dom::create_el("button");
        item.set_id(&format!("nav-{}", id));
        dom::set_class(&item, "nav-item");
        item.set_inner_html(&format!(
            "<span class=\"nav-icon\">{}</span><span class=\"nav-label\">{}</span>",
            nav_icon(id),
            label
        ));

        let route_id = id.to_string();
        dom::on_click(&item, move || {
            dom::window().location().set_hash(&route_id).ok();
        });

        dom::append(&nav, &item);
    }

    nav
}

/// Inline SVG glyph for a nav destination (avoids an icon-font dependency).
fn nav_icon(id: &str) -> &'static str {
    match id {
        "home" => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/></svg>"#
        }
        "sound" => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 9v6h4l5 4V5L8 9z"/><path d="M16.5 8.5a5 5 0 0 1 0 7"/><path d="M18.5 6a8 8 0 0 1 0 12"/></svg>"#
        }
        "lights" => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18h6"/><path d="M10 21h4"/><path d="M12 3a6 6 0 0 0-4 10.5c.7.7 1 1.3 1 2.5h6c0-1.2.3-1.8 1-2.5A6 6 0 0 0 12 3z"/></svg>"#
        }
        "speakers" => {
            // Three speakers in a close trident (center taller, two flanking,
            // bottoms aligned) — reads as a group, not one box.
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="7.5" width="4.5" height="13" rx="1.3"/><circle cx="4.75" cy="15" r="1.35"/><rect x="17" y="7.5" width="4.5" height="13" rx="1.3"/><circle cx="19.25" cy="15" r="1.35"/><rect x="9" y="3.5" width="6" height="17" rx="1.6"/><circle cx="12" cy="14" r="2.1"/><circle cx="12" cy="8" r="0.85"/></svg>"#
        }
        "settings" => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M12 2.5l1.4 2.6 2.9-.5.6 2.9 2.6 1.4-1.3 2.6 1.3 2.6-2.6 1.4-.6 2.9-2.9-.5L12 21.5l-1.4-2.6-2.9.5-.6-2.9L4.5 15l1.3-2.6L4.5 9.8l2.6-1.4.6-2.9 2.9.5z"/></svg>"#
        }
        _ => "",
    }
}

fn setup_routing() {
    let cb = Closure::wrap(Box::new(move |_: web_sys::HashChangeEvent| {
        route();
    }) as Box<dyn FnMut(_)>);
    dom::window().set_onhashchange(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
}

/// Check if setup is required and redirect to wizard if so.
pub fn check_setup_status() {
    wasm_bindgen_futures::spawn_local(async {
        let window = dom::window();
        let origin = dom::api_origin();
        let url = format!("{}/api/setup", origin);

        let resp_val = match wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&url)).await
        {
            Ok(v) => v,
            Err(_) => return,
        };
        let resp: web_sys::Response = resp_val.unchecked_into();
        let text_val = match resp.text() {
            Ok(p) => match wasm_bindgen_futures::JsFuture::from(p).await {
                Ok(v) => v,
                Err(_) => return,
            },
            Err(_) => return,
        };
        let text = match text_val.as_string() {
            Some(s) => s,
            None => return,
        };

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
            if val
                .get("setup_required")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                // Enter setup mode: hide tab bar, navigate to setup
                crate::state::with_mut(|s| {
                    s.setup_complete = false;
                    s.setup_step = 0;
                });
                if let Some(nav) = dom::get_el("primary-nav") {
                    dom::set_style(&nav, "display", "none");
                }
                window.location().set_hash("setup").ok();
            } else {
                crate::state::with_mut(|s| s.setup_complete = true);
            }
        }
    });
}

/// Route to the current hash path: highlight the active nav destination and
/// render the surface into #content with a fade transition.
pub fn route() {
    let hash = dom::window().location().hash().unwrap_or_default();
    let segments = crate::router::parse_path(&hash);
    let top = segments
        .first()
        .map(|s| s.as_str())
        .unwrap_or("home")
        .to_string();

    // Hero logo only on the connect host-picker; header logo everywhere else.
    let is_connect = top == "connect";
    crate::brand::set_state(if is_connect {
        crate::brand::LogoState::Hero
    } else {
        crate::brand::LogoState::Header
    });
    set_connect_mode(is_connect);

    // Tear down every Stage canvas loop on any route change so none survive
    // off-Stage (epoch-cancel pattern; one loop per module).
    crate::graphics::led_ring::stop_animation();
    crate::graphics::tuner::stop();
    crate::graphics::ovation::park();
    crate::graphics::sound_viz::stop();

    // Stop an in-progress mic test when leaving the Sound tab. Sound's route id
    // is "sound" and it is dispatched directly here (bypassing pages::render), so
    // this is the single chokepoint where the teardown can run. Read the previous
    // page BEFORE active_page is overwritten below.
    let leaving_mic_test =
        crate::state::with(|s| s.active_page == "sound" && top != "sound" && s.mic_testing);
    if leaving_mic_test {
        crate::ws::send_msg(&encore_common::protocol::ClientMsg::StopMicTest);
        crate::state::with_mut(|s| s.mic_testing = false);
    }

    // Highlight the active primary destination (Settings stays active for any
    // #settings/* sub-route).
    for (id, _) in NAV {
        if let Some(item) = dom::get_el(&format!("nav-{}", id)) {
            if *id == top {
                dom::set_class(&item, "nav-item active");
                dom::set_attr(&item, "aria-current", "page");
            } else {
                dom::set_class(&item, "nav-item");
                item.remove_attribute("aria-current").ok();
            }
        }
    }

    crate::state::with_mut(|s| s.active_page = top.clone());

    let already = TRANSITIONING.with(|t| t.get());
    if already {
        if let Some(content) = dom::get_el("content") {
            dom::clear(&content);
            render_surface(&segments, &content);
        }
        return;
    }

    if let Some(content) = dom::get_el("content") {
        TRANSITIONING.with(|t| t.set(true));
        dom::add_class(&content, "page-exit");

        let segments_owned = segments.clone();
        dom::set_timeout(
            move || {
                if let Some(content) = dom::get_el("content") {
                    dom::clear(&content);
                    render_surface(&segments_owned, &content);
                    dom::remove_class(&content, "page-exit");
                    // Force reflow so browser doesn't coalesce
                    let _ = content.client_width();
                    dom::add_class(&content, "page-enter");

                    dom::set_timeout(
                        move || {
                            if let Some(content) = dom::get_el("content") {
                                dom::remove_class(&content, "page-enter");
                                dom::add_class(&content, "page-enter-active");
                                dom::set_timeout(
                                    move || {
                                        if let Some(content) = dom::get_el("content") {
                                            dom::remove_class(&content, "page-enter-active");
                                        }
                                        TRANSITIONING.with(|t| t.set(false));
                                    },
                                    120,
                                );
                            }
                        },
                        10,
                    );
                }
            },
            120,
        );
    }
}

/// Render the surface for a parsed route path into `content`.
fn render_surface(segments: &[String], content: &web_sys::Element) {
    let top = segments.first().map(|s| s.as_str()).unwrap_or("home");
    match top {
        "settings" => crate::pages::settings::render(&segments[1..], content),
        "home" => crate::pages::home::render(content),
        "sound" => crate::pages::audio::render(content),
        other => crate::pages::render(other, content),
    }
}

/// Update whatever surface is currently active (called by the WS dispatcher).
pub fn update_active_page() {
    let hash = dom::window().location().hash().unwrap_or_default();
    let segments = crate::router::parse_path(&hash);
    let top = segments.first().map(|s| s.as_str()).unwrap_or("home");
    match top {
        "settings" => crate::pages::settings::update(&segments[1..]),
        "home" => crate::pages::home::update(),
        "sound" => crate::pages::audio::update(),
        other => crate::pages::update(other),
    }
}

/// Set up touch swipe navigation between tabs.
fn setup_swipe_navigation() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let touch_start: Rc<RefCell<Option<(f64, f64)>>> = Rc::new(RefCell::new(None));

    // touchstart — record start position (skip interactive elements)
    {
        let ts = touch_start.clone();
        let cb = Closure::wrap(Box::new(move |e: web_sys::TouchEvent| {
            // Don't capture swipe on sliders, buttons, inputs, or canvases
            let dominated = e.target().and_then(|t| {
                let el: web_sys::Element = t.dyn_into().ok()?;
                let tag = el.tag_name().to_uppercase();
                match tag.as_str() {
                    "INPUT" | "BUTTON" | "CANVAS" | "SELECT" | "TEXTAREA" => Some(true),
                    _ => None,
                }
            });
            if dominated.is_some() {
                *ts.borrow_mut() = None;
                return;
            }
            if let Some(touch) = e.touches().get(0) {
                *ts.borrow_mut() = Some((touch.client_x() as f64, touch.client_y() as f64));
            }
        }) as Box<dyn FnMut(_)>);
        if let Some(content) = dom::get_el("content") {
            content
                .add_event_listener_with_callback("touchstart", cb.as_ref().unchecked_ref())
                .ok();
        }
        cb.forget();
    }

    // touchend — check for horizontal swipe
    {
        let ts = touch_start;
        let cb = Closure::wrap(Box::new(move |e: web_sys::TouchEvent| {
            let start = ts.borrow_mut().take();
            if let Some((sx, sy)) = start {
                if let Some(touch) = e.changed_touches().get(0) {
                    let dx = touch.client_x() as f64 - sx;
                    let dy = (touch.client_y() as f64 - sy).abs();

                    // Horizontal swipe: > 80px horizontal, < 100px vertical
                    if dx.abs() > 80.0 && dy < 100.0 {
                        let current = crate::state::with(|s| s.active_page.clone());
                        // Swipe only cycles the control destinations (not Settings).
                        let idx = match SWIPE_DESTS.iter().position(|id| *id == current) {
                            Some(i) => i,
                            None => return,
                        };

                        let next_id = if dx < 0.0 {
                            SWIPE_DESTS.get(idx + 1).copied()
                        } else if idx > 0 {
                            SWIPE_DESTS.get(idx - 1).copied()
                        } else {
                            None
                        };

                        if let Some(id) = next_id {
                            dom::window().location().set_hash(id).ok();
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        if let Some(content) = dom::get_el("content") {
            content
                .add_event_listener_with_callback("touchend", cb.as_ref().unchecked_ref())
                .ok();
        }
        cb.forget();
    }
}

/// Update the connection status indicator.
pub fn set_connection_status(connected: bool) {
    let was = crate::state::with(|s| s.connected);
    if let Some(dot) = dom::get_el("conn-dot") {
        if connected {
            dom::set_class(&dot, "conn-dot connected");
        } else {
            dom::set_class(&dot, "conn-dot");
        }
    }
    crate::state::with_mut(|s| s.connected = connected);
    if let Some(bar) = dom::get_el("reconnect-bar") {
        dom::set_style(&bar, "display", if connected { "none" } else { "block" });
    }
    // Surface only a real drop (a previously-live link going away), not the
    // initial boot connect or the pre-connection reconnect attempts.
    if was && !connected {
        crate::components::toast::error("Connection lost");
    }
    // The mini bar hides when the link drops and re-shows on reconnect.
    mini_update();
}

/// Update the device name displayed in the header.
///
/// Shows the device name next to the "ENCORE" brand when it differs from the
/// default ("Encore"). This lets users identify which device they're controlling
/// when they have multiple Encore speakers on their network.
/// Apply desktop-app mode: add body class for drag and show window controls.
/// Called immediately and again after a short delay to handle late __TAURI__ injection.
fn apply_desktop_mode() {
    if dom::is_desktop_app() {
        dom::add_class(&dom::body(), "desktop-app");
        if let Some(wc) = dom::get_el("window-controls") {
            dom::set_style(&wc, "display", "flex");
        }
    }
}

/// Call a method on the Tauri window object (e.g. "minimize()", "close()").
fn wnd_eval(method: &str) {
    let code = format!("window.__TAURI__.window.getCurrentWindow().{}", method);
    let _ = js_sys::eval(&code);
}

/// Swap the maximize/restore icon based on the current window state.
fn update_maximize_icon() {
    // Run entirely in JS to avoid SVG className issues in WASM
    let _ = js_sys::eval(
        "window.__TAURI__.window.getCurrentWindow().isMaximized().then(function(m){\
            var b=document.getElementById('wc-maximize-btn');\
            if(b){if(m){b.classList.add('maximized')}else{b.classList.remove('maximized')}\
            b.title=m?'Restore Down':'Maximize';b.setAttribute('aria-label',b.title)}\
        })",
    );
}

/// Transition the unified logo to a new state.
///
/// States: "loading" (centered 60px), "header" (top-left 32px), "hero" (centered 80px).
/// Toggle connect mode — hides system menu items and conn-dot.
///
/// On the connect screen (and during connecting), the user has no active
/// speaker connection. System items (Config, Health, Crashes, Update,
/// Reboot, Change Speaker) are irrelevant. Only About stays visible.
pub fn set_connect_mode(active: bool) {
    // Hide/show connection status dot
    if let Some(dot) = dom::get_el("conn-dot") {
        dom::set_style(&dot, "display", if active { "none" } else { "" });
    }

    // Hide the primary nav on the connect host-picker (no speaker to control yet).
    if let Some(nav) = dom::get_el("primary-nav") {
        dom::set_style(&nav, "display", if active { "none" } else { "" });
    }
}

pub fn update_device_name(name: &str) {
    if let Some(el) = dom::get_el("device-name") {
        // Don't show redundant name if it matches the brand
        if name.eq_ignore_ascii_case("encore") || name.is_empty() {
            dom::set_text(&el, "");
            dom::remove_class(&el, "visible");
        } else {
            dom::set_text(&el, name);
            dom::add_class(&el, "visible");
        }
    }
}
