//! DOM builder helpers — ergonomic wrappers around web-sys.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, HtmlCanvasElement, HtmlElement, HtmlInputElement};

pub fn window() -> web_sys::Window {
    web_sys::window().expect("no window")
}

pub fn document() -> Document {
    window().document().expect("no document")
}

pub fn body() -> HtmlElement {
    document().body().expect("no body")
}

pub fn get_el(id: &str) -> Option<Element> {
    document().get_element_by_id(id)
}

pub fn create_el(tag: &str) -> Element {
    document().create_element(tag).unwrap()
}

pub fn create_div() -> Element {
    create_el("div")
}

pub fn set_text(el: &Element, text: &str) {
    el.set_text_content(Some(text));
}

pub fn set_attr(el: &Element, name: &str, val: &str) {
    el.set_attribute(name, val).unwrap();
}

pub fn add_class(el: &Element, cls: &str) {
    let cur = el.class_name();
    if cur.is_empty() {
        el.set_class_name(cls);
    } else if !cur.split_whitespace().any(|c| c == cls) {
        el.set_class_name(&format!("{} {}", cur, cls));
    }
}

pub fn remove_class(el: &Element, cls: &str) {
    let cur = el.class_name();
    let new: Vec<&str> = cur.split_whitespace().filter(|c| *c != cls).collect();
    el.set_class_name(&new.join(" "));
}

pub fn set_class(el: &Element, cls: &str) {
    el.set_class_name(cls);
}

pub fn append(parent: &Element, child: &Element) {
    parent.append_child(child).unwrap();
}

pub fn clear(el: &Element) {
    el.set_inner_html("");
}

pub fn on_click<F>(el: &Element, f: F)
where
    F: FnMut() + 'static,
{
    let mut f = f;
    let cb = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| {
        f();
    }) as Box<dyn FnMut(_)>);
    el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
        .unwrap();
    cb.forget();
}

pub fn on_input<F>(el: &Element, f: F)
where
    F: FnMut(String) + 'static,
{
    let mut f = f;
    let cb = Closure::wrap(Box::new(move |e: web_sys::InputEvent| {
        if let Some(target) = e.target() {
            if let Some(input) = target.dyn_ref::<HtmlInputElement>() {
                f(input.value());
            }
        }
    }) as Box<dyn FnMut(_)>);
    el.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())
        .unwrap();
    cb.forget();
}

// Toggle helpers
pub fn toggle_set(el: &Element, is_on: bool) {
    if let Some(input) = el.dyn_ref::<HtmlInputElement>() {
        input.set_checked(is_on);
    }
}

pub fn canvas(width: u32, height: u32) -> (HtmlCanvasElement, web_sys::CanvasRenderingContext2d) {
    let el = document().create_element("canvas").unwrap();
    let canvas: HtmlCanvasElement = el.unchecked_into();
    canvas.set_width(width);
    canvas.set_height(height);
    let ctx = canvas
        .get_context("2d")
        .unwrap()
        .unwrap()
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    (canvas, ctx)
}

/// Create a styled element with class and optional text
pub fn el(tag: &str, class: &str, text: Option<&str>) -> Element {
    let e = create_el(tag);
    if !class.is_empty() {
        e.set_class_name(class);
    }
    if let Some(t) = text {
        e.set_text_content(Some(t));
    }
    e
}

/// Set a CSS style property on an element
pub fn set_style(el: &Element, prop: &str, val: &str) {
    if let Some(html_el) = el.dyn_ref::<HtmlElement>() {
        html_el.style().set_property(prop, val).ok();
    }
}

/// Set interval helper, returns interval ID
pub fn set_interval<F>(f: F, ms: i32) -> i32
where
    F: FnMut() + 'static,
{
    let cb = Closure::wrap(Box::new(f) as Box<dyn FnMut()>);
    let id = window()
        .set_interval_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), ms)
        .unwrap();
    cb.forget();
    id
}

/// Set timeout helper
pub fn set_timeout<F>(f: F, ms: i32)
where
    F: FnOnce() + 'static,
{
    let cb = Closure::once(f);
    window()
        .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), ms)
        .unwrap();
    cb.forget();
}

// ── Standalone app support ──

/// Get the API origin for the speaker.
///
/// In standalone mode (Tauri app), reads the saved speaker host from localStorage.
/// When served directly from the device, returns `window.location.origin`.
pub fn api_origin() -> String {
    if let Some(host) = get_local("encore_speaker_host") {
        if host.starts_with("http://") || host.starts_with("https://") {
            return host;
        }
        return format!("https://{}", host);
    }
    window().location().origin().unwrap_or_default()
}

/// Check if running as a standalone app (not served from the speaker).
pub fn is_standalone() -> bool {
    has_tauri() || get_local("encore_speaker_host").is_some()
}

/// Check if the Tauri runtime is available.
pub fn has_tauri() -> bool {
    let w = window();
    let w_ref: &wasm_bindgen::JsValue = w.as_ref();
    if let Ok(tauri) = js_sys::Reflect::get(w_ref, &"__TAURI__".into()) {
        !tauri.is_undefined() && !tauri.is_null()
    } else {
        false
    }
}

/// Invoke a Tauri command by name. Returns the result, or None on error.
pub async fn tauri_invoke(cmd: &str) -> Option<wasm_bindgen::JsValue> {
    let code = format!("window.__TAURI__.core.invoke('{}')", cmd);
    let promise: js_sys::Promise = js_sys::eval(&code).ok()?.dyn_into().ok()?;
    wasm_bindgen_futures::JsFuture::from(promise).await.ok()
}

/// Discover speakers via Tauri mDNS backend. Returns (name, host_ip) pairs.
pub async fn tauri_discover_speakers() -> Vec<(String, String)> {
    let Some(result) = tauri_invoke("discover_speakers").await else {
        return Vec::new();
    };
    let Some(arr) = result.dyn_ref::<js_sys::Array>() else {
        return Vec::new();
    };
    let mut speakers = Vec::new();
    for i in 0..arr.length() {
        let item = arr.get(i);
        let name = js_sys::Reflect::get(&item, &"name".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let host = js_sys::Reflect::get(&item, &"host".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if !host.is_empty() {
            speakers.push((name, host));
        }
    }
    speakers
}

/// Check if running as a Tauri desktop app (not mobile, not browser).
/// Returns true when `window.__TAURI__` exists and the platform is not Android/iOS.
pub fn is_desktop_app() -> bool {
    // On mobile Tauri, __TAURI_INTERNALS__ exists but the user agent reveals the platform.
    // On desktop Tauri with withGlobalTauri:true, __TAURI__ is injected.
    is_standalone() && {
        let ua = window()
            .navigator()
            .user_agent()
            .unwrap_or_default()
            .to_lowercase();
        !ua.contains("android") && !ua.contains("iphone") && !ua.contains("ipad")
    }
}

/// Read a value from localStorage.
pub fn get_local(key: &str) -> Option<String> {
    let storage = window().local_storage().ok()??;
    storage.get_item(key).ok()?.filter(|v| !v.is_empty())
}

/// Write a value to localStorage.
pub fn set_local(key: &str, value: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        storage.set_item(key, value).ok();
    }
}

/// Remove a value from localStorage.
pub fn remove_local(key: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        storage.remove_item(key).ok();
    }
}
