//! Appearance panel — the user-facing Dark / Light / Auto control.
//!
//! This is only the control surface. All persistence and `<html data-theme>`
//! logic lives in the canonical `crate::theme` module (key `encore_theme`); this
//! panel just calls `set_theme` on change and reads `current_theme` to show the
//! active choice. `graphics::theme::is_dark()` reads the same attribute live, so
//! the visible canvases re-theme the instant the user switches.

use crate::components::{section_header, SegmentedControl, SegmentedMode};
use crate::dom;
use crate::theme::{current_theme, set_theme, Theme};

pub fn render(container: &web_sys::Element) {
    let card = dom::create_div();
    dom::set_class(&card, "card");
    dom::append(&card, &section_header("Appearance"));

    let desc = dom::el(
        "div",
        "text-muted text-sm mb-12",
        Some(
            "Dark is the default. Light is a higher-contrast matinee theme. \
             Auto follows your device.",
        ),
    );
    dom::append(&card, &desc);

    let current = current_theme().as_str();
    let seg = SegmentedControl::create(
        "theme-choice",
        &[("dark", "Dark"), ("light", "Light"), ("auto", "Auto")],
        &[current],
        SegmentedMode::Single(Box::new(|value| {
            set_theme(Theme::from_str(Some(value)));
        })),
    );
    dom::append(&card, &seg);
    dom::append(container, &card);
}
