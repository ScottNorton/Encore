//! Loading / empty / error placeholders for data-backed sections.
//!
//! These replace the bare "--" and "—" strings that pages render before async
//! data arrives, which read as broken fields. A section in flight shows
//! `loading()`, a section with no data shows `empty()`, and a failed fetch
//! shows `error()` with an optional Retry button.

use crate::dom;
use web_sys::Element;

/// Centered spinner with a message, for a section whose data is in flight.
///
/// No section fetches through a spinner yet (current pages render `empty()` /
/// `error()` placeholders), so this completes the in-flight/empty/error trio for
/// when one does.
#[allow(dead_code)]
pub fn loading(message: &str) -> Element {
    let wrap = dom::el("div", "async-state", None);
    let spinner = dom::el("div", "async-spinner", None);
    dom::append(&wrap, &spinner);
    let msg = dom::el("div", "async-msg", Some(message));
    dom::append(&wrap, &msg);
    wrap
}

/// Centered message for a section that has loaded but has nothing to show.
pub fn empty(message: &str) -> Element {
    let wrap = dom::el("div", "async-state", None);
    let msg = dom::el("div", "async-msg", Some(message));
    dom::append(&wrap, &msg);
    wrap
}

/// Centered error message with an optional Retry button.
///
/// Pass `None` for no button, or `Some(Box::new(closure))` to show "Retry".
pub fn error(message: &str, retry: Option<Box<dyn Fn() + 'static>>) -> Element {
    let wrap = dom::el("div", "async-state async-error", None);
    let msg = dom::el("div", "async-msg", Some(message));
    dom::append(&wrap, &msg);
    if let Some(cb) = retry {
        let btn = dom::el("button", "btn", Some("Retry"));
        dom::on_click(&btn, cb);
        dom::append(&wrap, &btn);
    }
    wrap
}

/// Async-state CSS, concatenated into the stylesheet by `style::inject()`.
pub fn css() -> &'static str {
    r#"
.async-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 32px 16px;
    color: var(--text-secondary);
    text-align: center;
}
.async-msg { font-size: 14px; }
.async-error .async-msg { color: var(--red); }
.async-spinner {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    animation: async-spin 0.8s linear infinite;
}
@keyframes async-spin { to { transform: rotate(360deg); } }
"#
}
