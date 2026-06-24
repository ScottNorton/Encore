//! Collapsible disclosure — a clickable header with a caret that shows/hides a
//! body. Extracted from the inline Developer-card pattern in `network.rs` and
//! reused for advanced/disclosure sections (Spotify settings, Animation
//! Designer, subsystem holds).

use std::cell::Cell;
use std::rc::Rc;

use crate::dom;

/// Build a collapsible section. Returns `(header, body)`: append the header and
/// body into a card in source order, then fill the body with content. The
/// header click toggles `body.display` and swaps the caret glyph.
///
/// `title` is the plain label (the caret is prepended automatically).
/// `start_open` sets the initial expanded state.
pub fn collapsible(title: &str, start_open: bool) -> (web_sys::Element, web_sys::Element) {
    let caret_open = '\u{25BC}'; // ▼
    let caret_closed = '\u{25B6}'; // ▶

    let header = dom::el("div", "card-title", None);
    dom::set_style(&header, "cursor", "pointer");
    dom::set_style(&header, "user-select", "none");
    dom::set_attr(&header, "role", "button");
    dom::set_attr(&header, "tabindex", "0");
    dom::set_attr(
        &header,
        "aria-expanded",
        if start_open { "true" } else { "false" },
    );

    let body = dom::create_div();
    dom::set_style(&body, "display", if start_open { "block" } else { "none" });

    let title = title.to_string();
    let label =
        move |open: bool| format!("{} {}", if open { caret_open } else { caret_closed }, title);
    dom::set_text(&header, &label(start_open));

    let open = Rc::new(Cell::new(start_open));
    let toggle = {
        let open = open.clone();
        let header = header.clone();
        let body = body.clone();
        move || {
            let next = !open.get();
            open.set(next);
            dom::set_text(&header, &label(next));
            dom::set_attr(
                &header,
                "aria-expanded",
                if next { "true" } else { "false" },
            );
            dom::set_style(&body, "display", if next { "block" } else { "none" });
        }
    };

    dom::on_click(&header, toggle.clone());
    // Space/Enter activates the role=button header for keyboard users.
    dom::on_keydown(&header, move |e: web_sys::KeyboardEvent| {
        let k = e.key();
        if k == "Enter" || k == " " || k == "Spacebar" {
            e.prevent_default();
            toggle();
        }
    });

    (header, body)
}
