//! App shell — header, pill tabs, gear menu, hash routing.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use crate::dom;
use js_sys;
use std::cell::Cell;

/// Tab definitions: (route_id, display_label)
pub const TABS: &[(&str, &str)] = &[
    ("dashboard", "Dashboard"),
    ("spotify", "Spotify"),
    ("audio", "Audio"),
    ("lights", "Lights"),
    ("bluetooth", "Bluetooth"),
    ("speakers", "Speakers"),
    ("network", "Network"),
    ("logs", "Logs"),
];

thread_local! {
    /// Guards against overlapping page transitions.
    static TRANSITIONING: Cell<bool> = Cell::new(false);
}

/// Gear menu items: (id, label). Empty string id = separator.
const MENU_ITEMS: &[(&str, &str)] = &[
    ("config", "Config"),
    ("health", "System Health"),
    ("crashes", "Crash Log"),
    ("", ""),
    ("update", "Firmware Update"),
    ("reboot", "Reboot"),
    ("", ""),
    ("about", "About"),
];

/// Initialize the app shell after boot sequence completes.
///
/// `landing` is the page to show first: "dashboard", "connect", or "setup".
/// Called by `boot::run()` after the smart boot sequence finishes.
pub fn init_after_boot(landing: &str) {
    let body = dom::body();

    // Build app container (hidden for crossfade from loading screen)
    let app = dom::create_div();
    app.set_id("app");
    dom::set_style(&app, "opacity", "0");

    // Header
    let header = build_header();
    dom::append(&app, &header);

    // Unified logo (fixed position, animates between states)
    let logo = crate::logo::build_logo();
    dom::append(&app, &logo);

    // Tab navigation
    let nav = build_tabs();
    dom::append(&app, &nav);

    // Content area
    let content = dom::create_div();
    content.set_id("content");
    dom::set_class(&content, "content");
    dom::append(&app, &content);

    // Gear menu (child of #app, not header, to escape header's stacking context)
    let menu = build_gear_menu();
    dom::append(&app, &menu);

    // Panel overlay + panel container for slide-overs
    let overlay = dom::create_div();
    overlay.set_id("panel-overlay");
    dom::set_class(&overlay, "panel-overlay");
    dom::on_click(&overlay, || close_panel());
    dom::append(&app, &overlay);

    let panel = dom::create_div();
    panel.set_id("panel");
    dom::set_class(&panel, "panel");
    dom::append(&app, &panel);

    body.append_child(&app).unwrap();

    // Animated morph: loading screen → logo in correct state
    if let Some(loading) = dom::get_el("loading") {
        dom::add_class(&loading, "exit");
        let landing_page = landing.to_string();
        dom::set_timeout(move || {
            if let Some(app) = dom::get_el("app") {
                dom::set_style(&app, "transition", "opacity 0.5s ease");
                dom::set_style(&app, "opacity", "1");
            }
            let lp = landing_page.clone();
            dom::set_timeout(move || {
                if lp == "connect" {
                    set_logo_state("hero");
                } else {
                    set_logo_state("header");
                }
            }, 100);
            dom::set_timeout(move || {
                if let Some(loading) = dom::get_el("loading") {
                    loading.remove();
                }
            }, 1200);
        }, 50);
    } else {
        // No loading screen (hot reload) — show immediately
        dom::set_style(&app, "opacity", "1");
    }

    // Set up hash routing
    setup_routing();

    // Set up touch swipe between tabs
    setup_swipe_navigation();

    // Listen for PWA install prompt
    setup_install_prompt();

    // Desktop Tauri: add body class for drag region
    apply_desktop_mode();
    dom::set_timeout(|| apply_desktop_mode(), 150);

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
        if let Some(nav) = dom::get_el("tab-nav-wrap") {
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

    // Device name (populated when config arrives via WebSocket)
    let name = dom::create_el("span");
    name.set_id("device-name");
    dom::set_class(&name, "header-device-name");
    dom::append(&left, &name);

    dom::append(&header, &left);

    // Window control buttons — placed in a header-right container
    let right = dom::create_div();
    dom::set_style(&right, "display", "flex");
    dom::set_style(&right, "align-items", "center");
    dom::set_style(&right, "gap", "0px");
    dom::set_style(&right, "flex-shrink", "0");

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
    gear.set_inner_html(r#"<svg class="hamburger-icon" width="18" height="14" viewBox="0 0 18 14"><rect class="ham-top" x="0" y="0" width="18" height="2" rx="1" fill="currentColor"/><rect class="ham-mid" x="0" y="6" width="18" height="2" rx="1" fill="currentColor"/><rect class="ham-bot" x="0" y="12" width="18" height="2" rx="1" fill="currentColor"/></svg>"#);
    dom::set_attr(&gear, "aria-label", "Settings menu");
    dom::set_attr(&gear, "title", "Settings");
    {
        let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            e.stop_propagation();
            toggle_gear_menu();
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
            btn_min.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref()).unwrap();
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
                dom::set_timeout(|| update_maximize_icon(), 50);
            }) as Box<dyn FnMut(_)>);
            btn_max.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref()).unwrap();
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
            btn_close.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref()).unwrap();
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

fn build_tabs() -> web_sys::Element {
    // Wrapper for gradient fade indicators
    let wrap = dom::create_div();
    wrap.set_id("tab-nav-wrap");
    dom::set_class(&wrap, "tab-nav-wrap");

    let nav = dom::create_el("nav");
    nav.set_id("tab-nav");
    dom::set_class(&nav, "tab-nav");

    for (id, label) in TABS {
        let btn = dom::create_el("button");
        btn.set_id(&format!("tab-{}", id));
        dom::set_class(&btn, "tab-btn");
        dom::set_text(&btn, label);

        let route_id = id.to_string();
        dom::on_click(&btn, move || {
            let window = dom::window();
            window.location().set_hash(&route_id).ok();
        });

        dom::append(&nav, &btn);
    }

    dom::append(&wrap, &nav);

    // ── Scroll behavior ──

    // Update fade indicators based on scroll position
    {
        let cb = Closure::wrap(Box::new(|_: web_sys::Event| {
            update_nav_fades();
        }) as Box<dyn FnMut(_)>);
        nav.add_event_listener_with_callback("scroll", cb.as_ref().unchecked_ref()).ok();
        cb.forget();
    }

    // Mouse wheel → horizontal scroll
    {
        let cb = Closure::wrap(Box::new(|e: web_sys::WheelEvent| {
            if let Some(nav) = dom::get_el("tab-nav") {
                let delta = e.delta_y();
                if delta.abs() > 0.0 {
                    e.prevent_default();
                    let cur = nav.scroll_left();
                    nav.set_scroll_left(cur + delta as i32);
                }
            }
        }) as Box<dyn FnMut(_)>);
        #[allow(deprecated)]
        let mut opts = web_sys::AddEventListenerOptions::new();
        #[allow(deprecated)]
        opts.passive(false);
        nav.add_event_listener_with_callback_and_add_event_listener_options(
            "wheel",
            cb.as_ref().unchecked_ref(),
            &opts,
        ).ok();
        cb.forget();
    }

    // Drag-to-scroll (desktop)
    {
        use std::cell::RefCell;
        use std::rc::Rc;
        let drag_state: Rc<RefCell<Option<(i32, i32)>>> = Rc::new(RefCell::new(None));

        // mousedown — start drag
        {
            let ds = drag_state.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
                if let Some(nav) = dom::get_el("tab-nav") {
                    *ds.borrow_mut() = Some((e.page_x(), nav.scroll_left()));
                    dom::add_class(&nav, "dragging");
                }
            }) as Box<dyn FnMut(_)>);
            nav.add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref()).ok();
            cb.forget();
        }

        // mousemove — scroll during drag
        {
            let ds = drag_state.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
                if let Some((start_x, start_scroll)) = *ds.borrow() {
                    if let Some(nav) = dom::get_el("tab-nav") {
                        let dx = start_x - e.page_x();
                        nav.set_scroll_left(start_scroll + dx);
                    }
                }
            }) as Box<dyn FnMut(_)>);
            dom::document().add_event_listener_with_callback("mousemove", cb.as_ref().unchecked_ref()).ok();
            cb.forget();
        }

        // mouseup — end drag
        {
            let ds = drag_state.clone();
            let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
                let was_dragging = ds.borrow().is_some();
                *ds.borrow_mut() = None;
                if was_dragging {
                    if let Some(nav) = dom::get_el("tab-nav") {
                        dom::remove_class(&nav, "dragging");
                    }
                }
            }) as Box<dyn FnMut(_)>);
            dom::document().add_event_listener_with_callback("mouseup", cb.as_ref().unchecked_ref()).ok();
            cb.forget();
        }
    }

    // Initial fade check after first layout
    dom::set_timeout(|| update_nav_fades(), 100);

    wrap
}

/// Update the gradient fade indicators on the tab nav edges.
fn update_nav_fades() {
    if let (Some(nav), Some(wrap)) = (dom::get_el("tab-nav"), dom::get_el("tab-nav-wrap")) {
        let scroll_left = nav.scroll_left();
        let scroll_width = nav.scroll_width();
        let client_width = nav.client_width();
        let max_scroll = scroll_width - client_width;

        if scroll_left > 2 {
            dom::add_class(&wrap, "fade-left");
        } else {
            dom::remove_class(&wrap, "fade-left");
        }

        if scroll_left < max_scroll - 2 {
            dom::add_class(&wrap, "fade-right");
        } else {
            dom::remove_class(&wrap, "fade-right");
        }
    }
}

/// Smoothly scroll the tab bar so the active tab is centered.
fn scroll_tab_into_view(page: &str) {
    if let Some(tab) = dom::get_el(&format!("tab-{}", page)) {
        let opts = js_sys::Object::new();
        js_sys::Reflect::set(&opts, &"behavior".into(), &"smooth".into()).ok();
        js_sys::Reflect::set(&opts, &"inline".into(), &"center".into()).ok();
        js_sys::Reflect::set(&opts, &"block".into(), &"nearest".into()).ok();

        if let Ok(func) = js_sys::Reflect::get(&tab, &"scrollIntoView".into()) {
            if let Some(f) = func.dyn_ref::<js_sys::Function>() {
                let _ = f.call1(&tab, &opts);
            }
        }
    }
    // Update fade indicators after scroll settles
    dom::set_timeout(|| update_nav_fades(), 300);
}

fn build_gear_menu() -> web_sys::Element {
    let menu = dom::create_div();
    menu.set_id("gear-menu");
    dom::set_class(&menu, "gear-menu");

    for (id, label) in MENU_ITEMS {
        if id.is_empty() {
            let sep = dom::create_div();
            dom::set_class(&sep, "gear-menu-sep system-item");
            dom::append(&menu, &sep);
        } else {
            let item = dom::create_el("button");
            if *id == "about" {
                dom::set_class(&item, "gear-menu-item");
            } else {
                dom::set_class(&item, "gear-menu-item system-item");
            }
            dom::set_text(&item, label);
            let panel_id = id.to_string();
            dom::on_click(&item, move || {
                close_gear_menu();
                open_panel(&panel_id);
            });
            dom::append(&menu, &item);
        }
    }

    // PWA install button — hidden by default, shown when beforeinstallprompt fires
    let sep = dom::create_div();
    dom::set_class(&sep, "gear-menu-sep system-item");
    sep.set_id("pwa-install-sep");
    dom::set_style(&sep, "display", "none");
    dom::append(&menu, &sep);

    let install_btn = dom::create_el("button");
    install_btn.set_id("pwa-install-btn");
    dom::set_class(&install_btn, "gear-menu-item system-item");
    dom::set_text(&install_btn, "Install App");
    dom::set_style(&install_btn, "display", "none");
    dom::on_click(&install_btn, || {
        trigger_install();
    });
    dom::append(&menu, &install_btn);

    // "Change Speaker" button — visible only in standalone app mode
    if dom::is_standalone() || dom::get_local("encore_speaker_host").is_some() {
        let sep2 = dom::create_div();
        dom::set_class(&sep2, "gear-menu-sep system-item");
        dom::append(&menu, &sep2);

        let change_btn = dom::create_el("button");
        dom::set_class(&change_btn, "gear-menu-item system-item");
        dom::set_text(&change_btn, "Change Speaker");
        dom::on_click(&change_btn, || {
            close_gear_menu();
            // Clear saved host and navigate to connect screen
            dom::remove_local("encore_speaker_host");
            crate::state::with_mut(|s| s.speaker_host = None);
            if let Some(nav) = dom::get_el("tab-nav-wrap") {
                dom::set_style(&nav, "display", "none");
            }
            dom::window().location().set_hash("connect").ok();
        });
        dom::append(&menu, &change_btn);
    }

    // Close menu when clicking outside
    let cb = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
        if let Some(target) = e.target() {
            let target: web_sys::Element = target.unchecked_into();
            let dominated_by_menu = target.closest("#gear-menu").ok().flatten().is_some();
            let is_gear_btn = target.closest("#gear-btn").ok().flatten().is_some();
            if !dominated_by_menu && !is_gear_btn {
                close_gear_menu();
            }
        }
    }) as Box<dyn FnMut(_)>);
    dom::document()
        .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();

    menu
}

fn toggle_gear_menu() {
    if let Some(menu) = dom::get_el("gear-menu") {
        let is_open = menu.class_name().split_whitespace().any(|c| c == "open");
        if is_open {
            dom::remove_class(&menu, "open");
            if let Some(btn) = dom::get_el("gear-btn") {
                dom::remove_class(&btn, "open");
            }
        } else {
            dom::add_class(&menu, "open");
            if let Some(btn) = dom::get_el("gear-btn") {
                dom::add_class(&btn, "open");
            }
        }
    }
}

fn close_gear_menu() {
    if let Some(menu) = dom::get_el("gear-menu") {
        dom::remove_class(&menu, "open");
    }
    if let Some(btn) = dom::get_el("gear-btn") {
        dom::remove_class(&btn, "open");
    }
}

/// Open a slide-over panel by ID.
pub fn open_panel(id: &str) {
    crate::state::with_mut(|s| s.panel_open = Some(id.to_string()));

    if let Some(overlay) = dom::get_el("panel-overlay") {
        dom::add_class(&overlay, "open");
    }

    if let Some(panel) = dom::get_el("panel") {
        dom::clear(&panel);

        // Panel header with back button
        let header = dom::create_div();
        dom::set_class(&header, "panel-header");

        let back = dom::create_el("button");
        dom::set_class(&back, "panel-back");
        back.set_inner_html("&#8592;"); // ← arrow
        dom::on_click(&back, || close_panel());
        dom::append(&header, &back);

        let title_text = match id {
            "config" => "Configuration",
            "health" => "System Health",
            "crashes" => "Crash Log",
            "update" => "Firmware Update",
            "reboot" => "Reboot",
            "about" => "About",
            _ => id,
        };
        let title = dom::el("span", "panel-title", Some(title_text));
        dom::append(&header, &title);
        dom::append(&panel, &header);

        // Panel body
        let body = dom::create_div();
        body.set_id("panel-body");
        dom::set_class(&body, "panel-body");
        dom::append(&panel, &body);

        // Render panel content
        crate::panels::render(id, &body);

        dom::add_class(&panel, "open");
    }
}

/// Close the current slide-over panel.
pub fn close_panel() {
    crate::state::with_mut(|s| s.panel_open = None);

    if let Some(overlay) = dom::get_el("panel-overlay") {
        dom::remove_class(&overlay, "open");
    }
    if let Some(panel) = dom::get_el("panel") {
        dom::remove_class(&panel, "open");
    }
}

fn setup_routing() {
    let cb = Closure::wrap(Box::new(move |_: web_sys::HashChangeEvent| {
        route();
    }) as Box<dyn FnMut(_)>);
    dom::window().set_onhashchange(Some(cb.as_ref().unchecked_ref()));
    cb.forget();

    // Update nav fades on window resize
    let resize_cb = Closure::wrap(Box::new(move |_: web_sys::Event| {
        update_nav_fades();
    }) as Box<dyn FnMut(_)>);
    dom::window()
        .add_event_listener_with_callback("resize", resize_cb.as_ref().unchecked_ref())
        .ok();
    resize_cb.forget();
}

/// Listen for the `beforeinstallprompt` event and stash it for later use.
fn setup_install_prompt() {
    let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
        // Prevent the mini-infobar from appearing on mobile
        e.prevent_default();
        // Store the event so we can trigger it later
        let event: JsValue = e.into();
        crate::state::with_mut(|s| s.install_prompt = Some(event));
        // Show the install button (and its separator) in the gear menu
        if let Some(sep) = dom::get_el("pwa-install-sep") {
            dom::set_style(&sep, "display", "block");
        }
        if let Some(btn) = dom::get_el("pwa-install-btn") {
            dom::set_style(&btn, "display", "block");
        }
    }) as Box<dyn FnMut(_)>);
    dom::window()
        .add_event_listener_with_callback("beforeinstallprompt", cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();
}

/// Trigger the stored PWA install prompt.
fn trigger_install() {
    close_gear_menu();
    let prompt = crate::state::with_mut(|s| s.install_prompt.take());
    if let Some(event) = prompt {
        // Call event.prompt() via js_sys::Reflect
        if let Ok(prompt_fn) = js_sys::Reflect::get(&event, &JsValue::from_str("prompt")) {
            if let Some(func) = prompt_fn.dyn_ref::<js_sys::Function>() {
                let _ = func.call0(&event);
            }
        }
        // Hide the install button since we've used the prompt
        if let Some(btn) = dom::get_el("pwa-install-btn") {
            dom::set_style(&btn, "display", "none");
        }
        if let Some(sep) = dom::get_el("pwa-install-sep") {
            dom::set_style(&sep, "display", "none");
        }
    }
}

/// Check if setup is required and redirect to wizard if so.
pub fn check_setup_status() {
    wasm_bindgen_futures::spawn_local(async {
        let window = dom::window();
        let origin = dom::api_origin();
        let url = format!("{}/api/setup", origin);

        let resp_val = match wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&url)).await {
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
            if val.get("setup_required").and_then(|v| v.as_bool()).unwrap_or(false) {
                // Enter setup mode: hide tab bar, navigate to setup
                crate::state::with_mut(|s| {
                    s.setup_complete = false;
                    s.setup_step = 0;
                });
                if let Some(nav) = dom::get_el("tab-nav-wrap") {
                    dom::set_style(&nav, "display", "none");
                }
                window.location().set_hash("setup").ok();
            } else {
                crate::state::with_mut(|s| s.setup_complete = true);
            }
        }
    });
}

/// Returns true if a tab is currently visible (not hidden by config).
fn is_tab_visible(id: &str) -> bool {
    dom::get_el(&format!("tab-{}", id))
        .map(|el| {
            el.dyn_ref::<web_sys::HtmlElement>()
                .map(|h| h.style().get_property_value("display").unwrap_or_default() != "none")
                .unwrap_or(true)
        })
        .unwrap_or(false)
}

/// Route to the current hash page with a fade transition.
pub fn route() {
    let hash = dom::window().location().hash().unwrap_or_default();
    let page = hash.trim_start_matches('#').trim_start_matches('/');
    let page = if page.is_empty() { "dashboard" } else { page };

    // Move logo to hero state for connect screen, header for everything else
    let is_connect = page == "connect";
    if is_connect {
        set_logo_state("hero");
    } else {
        set_logo_state("header");
    }

    // Toggle connect mode: hide system menu items and conn-dot on connect screen
    set_connect_mode(is_connect);

    // Redirect to dashboard if navigating to a hidden tab
    if TABS.iter().any(|(id, _)| *id == page) && !is_tab_visible(page) {
        dom::window().location().set_hash("dashboard").ok();
        return;
    }

    // Stop any running LED animation when navigating away from lights
    crate::graphics::led_ring::stop_animation();

    // Update active tab immediately (don't wait for transition)
    for (id, _) in TABS {
        if let Some(tab) = dom::get_el(&format!("tab-{}", id)) {
            if *id == page {
                dom::set_class(&tab, "tab-btn active");
            } else {
                dom::set_class(&tab, "tab-btn");
            }
        }
    }

    // Scroll active tab into view (smooth, centered)
    scroll_tab_into_view(page);

    // Update state
    crate::state::with_mut(|s| s.active_page = page.to_string());

    // If already transitioning, skip animation and render directly
    let already = TRANSITIONING.with(|t| t.get());
    if already {
        if let Some(content) = dom::get_el("content") {
            dom::clear(&content);
            crate::pages::render(page, &content);
        }
        return;
    }

    // Fade-out → swap → fade-in
    if let Some(content) = dom::get_el("content") {
        TRANSITIONING.with(|t| t.set(true));
        dom::add_class(&content, "page-exit");

        let page = page.to_string();
        dom::set_timeout(move || {
            if let Some(content) = dom::get_el("content") {
                dom::clear(&content);
                crate::pages::render(&page, &content);
                dom::remove_class(&content, "page-exit");
                // Force reflow so browser doesn't coalesce
                let _ = content.client_width();
                dom::add_class(&content, "page-enter");

                dom::set_timeout(move || {
                    if let Some(content) = dom::get_el("content") {
                        dom::remove_class(&content, "page-enter");
                        dom::add_class(&content, "page-enter-active");
                        dom::set_timeout(move || {
                            if let Some(content) = dom::get_el("content") {
                                dom::remove_class(&content, "page-enter-active");
                            }
                            TRANSITIONING.with(|t| t.set(false));
                        }, 120);
                    }
                }, 10);
            }
        }, 120);
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
            content.add_event_listener_with_callback("touchstart", cb.as_ref().unchecked_ref()).ok();
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
                        // Don't swipe-navigate on non-tab pages (connect, setup)
                        let idx = match TABS.iter().position(|(id, _)| *id == current) {
                            Some(i) => i,
                            None => return,
                        };

                        let next_id = if dx < 0.0 {
                            // Swipe left → next visible tab
                            TABS.iter().skip(idx + 1)
                                .find(|(id, _)| is_tab_visible(id))
                                .map(|(id, _)| *id)
                        } else {
                            // Swipe right → prev visible tab
                            TABS.iter().take(idx).rev()
                                .find(|(id, _)| is_tab_visible(id))
                                .map(|(id, _)| *id)
                        };

                        if let Some(id) = next_id {
                            let window = dom::window();
                            window.location().set_hash(id).ok();
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        if let Some(content) = dom::get_el("content") {
            content.add_event_listener_with_callback("touchend", cb.as_ref().unchecked_ref()).ok();
        }
        cb.forget();
    }
}

/// Update the connection status indicator.
pub fn set_connection_status(connected: bool) {
    if let Some(dot) = dom::get_el("conn-dot") {
        if connected {
            dom::set_class(&dot, "conn-dot connected");
        } else {
            dom::set_class(&dot, "conn-dot");
        }
    }
    crate::state::with_mut(|s| s.connected = connected);
}

/// Update tab visibility based on config enabled flags.
///
/// Hides tabs for disabled subsystems (spotify, bluetooth, speakers/group).
/// If the user is on a tab that just became hidden, redirects to dashboard.
pub fn update_tab_visibility() {
    let (spotify, bluetooth, group) = crate::state::with(|s| {
        match &s.config {
            Some(cfg) => (cfg.spotify_enabled, cfg.bluetooth_enabled, cfg.group_enabled),
            None => (true, true, false), // defaults before config arrives
        }
    });

    let conditional: &[(&str, bool)] = &[
        ("spotify", spotify),
        ("bluetooth", bluetooth),
        ("speakers", group),
    ];

    for (id, enabled) in conditional {
        if let Some(tab) = dom::get_el(&format!("tab-{}", id)) {
            dom::set_style(&tab, "display", if *enabled { "" } else { "none" });
        }
    }

    // If the active page is now hidden, redirect to dashboard
    let active = crate::state::with(|s| s.active_page.clone());
    let hidden = conditional.iter().any(|(id, enabled)| *id == active && !enabled);
    if hidden {
        dom::window().location().set_hash("dashboard").ok();
    }

    // Update scroll fades since tab widths changed
    update_nav_fades();
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
    let code = format!(
        "window.__TAURI__.window.getCurrentWindow().{}",
        method
    );
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
        })"
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

    // Toggle system items in gear menu via CSS class
    if let Some(menu) = dom::get_el("gear-menu") {
        if active {
            dom::add_class(&menu, "connect-mode");
        } else {
            dom::remove_class(&menu, "connect-mode");
        }
    }
}

pub fn set_logo_state(state: &str) {
    if let Some(logo) = dom::get_el("app-logo") {
        dom::remove_class(&logo, "logo-loading");
        dom::remove_class(&logo, "logo-header");
        dom::remove_class(&logo, "logo-hero");
        dom::remove_class(&logo, "logo-connecting");
        match state {
            "loading" => dom::add_class(&logo, "logo-loading"),
            "header" => dom::add_class(&logo, "logo-header"),
            "hero" => dom::add_class(&logo, "logo-hero"),
            "connecting" => dom::add_class(&logo, "logo-connecting"),
            _ => dom::add_class(&logo, "logo-header"),
        }
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
