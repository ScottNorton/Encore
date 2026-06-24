//! Theme selection (Dark / Light / Auto).
//!
//! Persisted in localStorage under `encore_theme`. `dark`/`light` write an
//! explicit `<html data-theme>` (which wins over the OS query in both
//! `brand::css` and `style.rs`); `auto` (or absent) removes the attribute so the
//! OS `prefers-color-scheme` decides. `graphics::theme::is_dark()` reads the same
//! attribute, so canvas widgets agree with the DOM.

use crate::dom;

const KEY: &str = "encore_theme";

/// The three theme modes. `as_str` is the localStorage / `data-theme` token.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    Auto,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Auto => "auto",
        }
    }

    /// Parse a stored token. Unknown/empty/absent -> Auto.
    pub fn from_str(s: Option<&str>) -> Theme {
        match s {
            Some("dark") => Theme::Dark,
            Some("light") => Theme::Light,
            _ => Theme::Auto,
        }
    }
}

/// The currently-selected mode (from localStorage). Auto if unset.
pub fn current_theme() -> Theme {
    Theme::from_str(dom::get_local(KEY).as_deref())
}

/// Apply a mode: persist it and set/clear `<html data-theme>`. Canvas widgets
/// read the attribute live, so no canvas restart is needed for CSS-driven UI;
/// callers that own a canvas should repaint after calling this.
pub fn set_theme(theme: Theme) {
    dom::set_local(KEY, theme.as_str());
    apply_attr(theme);
}

/// Read the persisted mode and reflect it onto `<html>`. Call this once on boot,
/// before `style::inject()`, so the first paint matches (no theme flash).
pub fn init() {
    apply_attr(current_theme());
}

fn apply_attr(theme: Theme) {
    let Some(root) = dom::document().document_element() else {
        return;
    };
    match theme {
        Theme::Dark => {
            root.set_attribute("data-theme", "dark").ok();
        }
        Theme::Light => {
            root.set_attribute("data-theme", "light").ok();
        }
        // Auto: no attribute -> :root:not([data-theme]) + OS query take over.
        Theme::Auto => {
            root.remove_attribute("data-theme").ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_tokens() {
        assert!(matches!(Theme::from_str(Some("dark")), Theme::Dark));
        assert!(matches!(Theme::from_str(Some("light")), Theme::Light));
        assert!(matches!(Theme::from_str(Some("auto")), Theme::Auto));
    }

    #[test]
    fn parse_unknown_defaults_to_auto() {
        assert!(matches!(Theme::from_str(None), Theme::Auto));
        assert!(matches!(Theme::from_str(Some("")), Theme::Auto));
        assert!(matches!(Theme::from_str(Some("sepia")), Theme::Auto));
    }

    #[test]
    fn as_str_roundtrips() {
        for t in [Theme::Dark, Theme::Light, Theme::Auto] {
            assert!(matches!(Theme::from_str(Some(t.as_str())), x if x == t));
        }
    }
}
