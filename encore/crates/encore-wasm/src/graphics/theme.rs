//! Canvas theme tokens.
//!
//! Canvas 2D needs literal color strings and cannot read CSS variables, so the
//! graphics widgets pick their colors here. `level_color` is the shared
//! green/yellow/orange/red ramp (deduped from the per-widget copies); `is_dark`
//! reads the OS color scheme so visualizations work in light mode too.

/// True when the dashboard should paint dark. Honors an explicit
/// `<html data-theme>` override first (so canvas agrees with the DOM), then
/// falls back to the OS media query. Defaults to dark.
pub fn is_dark() -> bool {
    if let Some(theme) = crate::dom::document()
        .document_element()
        .and_then(|el| el.get_attribute("data-theme"))
    {
        if let Some(forced) = is_dark_from_attr(Some(&theme)) {
            return forced;
        }
    }
    crate::dom::window()
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(true)
}

/// Pure core of the `data-theme` branch: "dark" -> Some(true), "light" ->
/// Some(false), anything else (incl. "auto"/empty/absent) -> None (defer to OS).
fn is_dark_from_attr(attr: Option<&str>) -> Option<bool> {
    match attr {
        Some("dark") => Some(true),
        Some("light") => Some(false),
        _ => None,
    }
}

/// True when the user asks for reduced motion. Drives the hard gate on every
/// canvas rAF loop (the CSS reduced-motion block cannot stop a JS animation
/// loop). Defaults to false (animate) if the media query is unavailable.
pub fn reduce_motion() -> bool {
    crate::dom::window()
        .match_media("(prefers-reduced-motion: reduce)")
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(false)
}

/// Brightest gold (glow/highlight only — ray peak, applause flash) as an RGB
/// triplet, per theme. Mirrors `brand::palette::DOME`.
///
// `#[allow(dead_code)]`: the canvas tuner needle that consumed this was replaced
// by the DOM dial bar (it glows via the `--accent-glow` CSS token); the token is
// kept here for the ovation peak tier and is covered by tests.
#[allow(dead_code)]
pub fn glow_rgb(is_dark: bool) -> (u8, u8, u8) {
    if is_dark {
        (255, 206, 106)
    } else {
        (199, 154, 58)
    }
}

/// The brand accent (gold) as an RGB triplet for the current theme. Mirrors
/// `brand::palette::ACCENT` (dark 230,179,77) / `ACCENT_LIGHT` (116,86,27).
pub fn accent_rgb(is_dark: bool) -> (u8, u8, u8) {
    if is_dark {
        (230, 179, 77)
    } else {
        (116, 86, 27)
    }
}

/// Warm translucent backdrop tint a canvas paints over its area, per theme.
pub fn canvas_bg(is_dark: bool) -> &'static str {
    if is_dark {
        "rgba(20,17,13,0.6)"
    } else {
        "rgba(60,42,16,0.04)"
    }
}

/// Opaque app-background ("--stage") as an RGB triplet, per theme. For canvas
/// elements that need to paint a solid disc/hole matching the page behind them.
pub fn stage_rgb(is_dark: bool) -> (u8, u8, u8) {
    if is_dark {
        (20, 17, 13)
    } else {
        (244, 236, 221)
    }
}

/// High-contrast foreground (text, peak markers) as an RGB triplet, per theme.
/// Cream on dark, near-black ink on light.
pub fn ink_rgb(is_dark: bool) -> (u8, u8, u8) {
    if is_dark {
        (242, 233, 218)
    } else {
        (36, 29, 16)
    }
}

/// Faint hairline color for canvas grid lines and unlit dot wells, per theme.
/// Warm-tinted so it agrees with the `--border` family instead of the old
/// off-palette GitHub gray (`48,54,61`). Callers wrap it with `rgba_str` and the
/// per-line alpha they want.
pub fn grid_rgb(is_dark: bool) -> (u8, u8, u8) {
    if is_dark {
        (242, 233, 218)
    } else {
        (60, 42, 16)
    }
}

/// Muted label color for canvas axis text, ticks, and inactive markers, per
/// theme. Mirrors `--text-muted`/`--text-tertiary` so canvas captions match the
/// DOM instead of the old off-palette gray (`139,148,158`). Callers wrap it with
/// `rgba_str` and the per-use alpha they want.
pub fn muted_rgb(is_dark: bool) -> (u8, u8, u8) {
    if is_dark {
        (154, 142, 114)
    } else {
        (122, 108, 76)
    }
}

/// Map a 0.0-1.0 level to a ramp bucket: 0 green, 1 yellow, 2 orange, 3 red.
fn bucket(t: f64) -> usize {
    if t < 0.6 {
        0
    } else if t < 0.8 {
        1
    } else if t < 0.92 {
        2
    } else {
        3
    }
}

/// The level ramp color for a meter/visualization segment.
///
/// `t` is the 0.0-1.0 position along the ramp, `lit` is whether the segment is
/// active (a dim ghost outline when false), `is_dark` selects the theme
/// variant. The dark, lit values are identical to the meter's original
/// hardcoded ramp, so dark mode is pixel-for-pixel unchanged.
pub fn level_color(t: f64, lit: bool, is_dark: bool) -> &'static str {
    let b = bucket(t);
    if !lit {
        return [
            "rgba(63,185,80,0.06)",
            "rgba(227,179,65,0.06)",
            "rgba(210,153,34,0.08)",
            "rgba(248,81,73,0.10)",
        ][b];
    }
    if is_dark {
        [
            "rgb(63,185,80)",
            "rgb(227,179,65)",
            "rgb(210,153,34)",
            "rgb(248,81,73)",
        ][b]
    } else {
        [
            "rgb(26,127,55)",
            "rgb(154,103,0)",
            "rgb(191,135,0)",
            "rgb(207,34,46)",
        ][b]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_boundaries() {
        assert_eq!(bucket(0.0), 0);
        assert_eq!(bucket(0.59), 0);
        assert_eq!(bucket(0.6), 1);
        assert_eq!(bucket(0.79), 1);
        assert_eq!(bucket(0.8), 2);
        assert_eq!(bucket(0.91), 2);
        assert_eq!(bucket(0.92), 3);
        assert_eq!(bucket(1.0), 3);
    }

    #[test]
    fn dark_lit_ramp_matches_original_meter_colors() {
        assert_eq!(level_color(0.0, true, true), "rgb(63,185,80)");
        assert_eq!(level_color(0.7, true, true), "rgb(227,179,65)");
        assert_eq!(level_color(0.85, true, true), "rgb(210,153,34)");
        assert_eq!(level_color(1.0, true, true), "rgb(248,81,73)");
    }

    #[test]
    fn light_lit_ramp_differs_from_dark() {
        assert_ne!(level_color(0.0, true, true), level_color(0.0, true, false));
    }

    #[test]
    fn unlit_segments_are_translucent() {
        assert!(level_color(0.0, false, true).starts_with("rgba"));
        assert!(level_color(1.0, false, true).starts_with("rgba"));
    }

    #[test]
    fn glow_is_brightest_gold_in_dark() {
        assert_eq!(glow_rgb(true), (255, 206, 106));
    }

    #[test]
    fn glow_dims_to_light_gold_in_light() {
        assert_eq!(glow_rgb(false), (199, 154, 58));
    }

    #[test]
    fn accent_dark_matches_palette_split() {
        assert_eq!(accent_rgb(true), (230, 179, 77));
    }

    #[test]
    fn accent_light_matches_palette_split() {
        assert_eq!(accent_rgb(false), (116, 86, 27));
    }

    #[test]
    fn ink_dark_is_cream() {
        assert_eq!(ink_rgb(true), (242, 233, 218));
    }

    #[test]
    fn grid_and_muted_differ_between_themes() {
        assert_ne!(grid_rgb(true), grid_rgb(false));
        assert_ne!(muted_rgb(true), muted_rgb(false));
    }

    #[test]
    fn data_theme_parse_dark_wins() {
        // is_dark_from_attr is the pure, testable core of is_dark()'s data-theme
        // branch (the DOM read is the only impure part).
        assert_eq!(is_dark_from_attr(Some("dark")), Some(true));
        assert_eq!(is_dark_from_attr(Some("light")), Some(false));
        assert_eq!(is_dark_from_attr(Some("auto")), None);
        assert_eq!(is_dark_from_attr(Some("")), None);
        assert_eq!(is_dark_from_attr(None), None);
    }
}
