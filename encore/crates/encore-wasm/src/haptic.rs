//! Haptic feedback — navigator.vibrate() wrapper.
//!
//! Feature-detects vibrate support; no-op on desktop/iOS.

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

/// Trigger a short vibration pulse (milliseconds).
pub fn pulse(ms: u32) {
    let window = crate::dom::window();
    let navigator = window.navigator();
    let nav: &JsValue = navigator.as_ref();
    if let Ok(vibrate_fn) = js_sys::Reflect::get(nav, &JsValue::from_str("vibrate")) {
        if let Some(func) = vibrate_fn.dyn_ref::<js_sys::Function>() {
            let arg = JsValue::from_f64(ms as f64);
            let _ = func.call1(nav, &arg);
        }
    }
}
