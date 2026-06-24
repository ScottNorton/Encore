//! Section header — the gold letter-spaced small-caps "eyebrow" used to label a
//! group of controls within a page. Renders `.section-label`.

use crate::dom;

/// Build a `.section-label` element (gold uppercase eyebrow). Append it above a
/// group of cards or rows.
pub fn section_header(text: &str) -> web_sys::Element {
    dom::el("div", "section-label", Some(text))
}
