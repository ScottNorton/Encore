// encore/crates/encore-wasm/src/brand/mod.rs
//! Brand module — palette, CSS, SVG templates, and logo/wordmark DOM builders.
//!
//! Single source of truth for every brand-related string/asset used at runtime.
//! The loading screen in `encore/web/index.html` also has inline SVG and gradient
//! stops — those are intentionally duplicated (must render before WASM loads)
//! and marked `KEEP IN SYNC` with this module.

pub mod palette;
pub mod css;

use crate::dom;
use encore_common::protocol::LedAnimation;

/// Positioning / size state applied to `#app-logo` via CSS class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoState {
    Loading,
    Header,
    Hero,
    Connecting,
    Wordmark,
}

impl LogoState {
    fn class(self) -> &'static str {
        match self {
            Self::Loading    => "logo-loading",
            Self::Header     => "logo-header",
            Self::Hero       => "logo-hero",
            Self::Connecting => "logo-connecting",
            Self::Wordmark   => "logo-wordmark",
        }
    }

    /// Every CSS class that `class()` may apply. `set_state` iterates this to
    /// clear prior state before applying the new one — keep in sync with the
    /// arms of `class()` above.
    const ALL: &'static [&'static str] = &[
        "logo-loading", "logo-header", "logo-hero", "logo-connecting", "logo-wordmark",
    ];
}

/// Speaker silhouette SVG. viewBox 0 0 100 100; dome fill uses `palette::DOME`.
pub(crate) fn speaker_svg() -> String {
    format!(
        r##"<svg class="logo-speaker" viewBox="0 0 100 100">
  <path class="speaker-body" d="M36,26 Q34.5,54 33,82 A17,5 0 0,0 67,82 Q65.5,54 64,26 A14,3.8 0 0,1 36,26 Z" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.5"/>
  <ellipse class="speaker-cap" cx="50" cy="33" rx="15" ry="3.2" fill="none" stroke="currentColor" stroke-width="0.8" opacity="0.4"/>
  <ellipse class="speaker-base" cx="50" cy="82" rx="17" ry="5" fill="none" stroke="currentColor" stroke-width="0.6" opacity="0.3"/>
  <ellipse id="logo-dome" class="speaker-dome" cx="50" cy="26" rx="12" ry="2.8" fill="{dome}" opacity="0.85"/>
</svg>"##,
        dome = palette::DOME,
    )
}

/// Arc ring SVG — 270° gradient arc with gap facing down. Gradient stops use
/// `palette::ACCENT` and `palette::RING_CREAM`.
pub(crate) fn arc_ring_svg() -> String {
    format!(
        r##"<div class="logo-glow"></div>
<svg class="logo-ring" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="logo-grad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="{a}"/>
      <stop offset="50%" stop-color="{b}"/>
      <stop offset="100%" stop-color="{a}"/>
    </linearGradient>
  </defs>
  <circle class="logo-track" cx="50" cy="50" r="42" fill="none" stroke-width="3"/>
  <circle class="logo-arc" cx="50" cy="50" r="42" fill="none" stroke="url(#logo-grad)" stroke-width="2.5" stroke-linecap="round" stroke-dasharray="198 66" transform="rotate(135 50 50)"/>
</svg>"##,
        a = palette::ACCENT,
        b = palette::RING_CREAM,
    )
}

/// Extract a dominant CSS color for the logo dome from the current LED
/// animation state. Fixes prior off-brand fallbacks (VolumeArc/BootSurge were
/// GitHub blue `#58a6ff`; now gold and cream).
pub fn led_dominant_color(anim: &LedAnimation) -> String {
    match anim {
        LedAnimation::Off => palette::DOME.to_string(),
        LedAnimation::Solid { r, g, b }
        | LedAnimation::Breathe { r, g, b, .. }
        | LedAnimation::Spin { r, g, b, .. }
        | LedAnimation::Pulse { r, g, b } => format!("#{:02x}{:02x}{:02x}", r, g, b),
        LedAnimation::VolumeArc { .. } => palette::LED_VOLUME.to_string(),
        LedAnimation::BootSurge        => palette::LED_BOOT.to_string(),
        LedAnimation::SafeMode         => palette::LED_SAFE.to_string(),
        LedAnimation::Custom { frames } => {
            if let Some(frame) = frames.first() {
                let (r, g, b) = frame.colors[0];
                format!("#{:02x}{:02x}{:02x}", r, g, b)
            } else {
                palette::DOME.to_string()
            }
        }
    }
}

/// Build the unified logo DOM element — a fixed-position div containing the
/// arc ring, speaker silhouette, and "ENCORE" text. Starts in the `Loading` state.
///
/// The returned element has `id="app-logo"`. Runtime can mutate its state class
/// via `set_state()`.
pub fn build_logo() -> web_sys::Element {
    let logo = dom::create_div();
    logo.set_id("app-logo");
    dom::set_class(&logo, "app-logo logo-loading");

    // Ring container (arc ring + speaker silhouette stacked)
    let ring_wrap = dom::create_div();
    dom::set_class(&ring_wrap, "logo-ring-wrap");

    let ring = dom::create_div();
    dom::set_class(&ring, "logo-ring-container");
    ring.set_inner_html(&arc_ring_svg());
    dom::append(&ring_wrap, &ring);

    let speaker = dom::create_div();
    dom::set_class(&speaker, "logo-speaker-container");
    speaker.set_inner_html(&speaker_svg());
    dom::append(&ring_wrap, &speaker);

    dom::append(&logo, &ring_wrap);

    // "ENCORE" text
    let text = dom::create_el("span");
    text.set_id("logo-text");
    dom::set_class(&text, "logo-text");
    dom::set_text(&text, "ENCORE");
    dom::append(&logo, &text);

    // Status text (shown during connecting state)
    let status = dom::create_el("span");
    status.set_id("logo-status");
    dom::set_class(&status, "logo-status");
    dom::append(&logo, &status);

    logo
}

/// Build the about-panel wordmark element — Classical Badge layout.
///
/// Structure:
/// - Animated ring + speaker silhouette (64px)
/// - `<Encore>` text between two fading gold rules
/// - "Community Firmware" subtitle
///
/// Uses the same SVG assets as `build_logo`, positioned differently via CSS.
pub fn build_wordmark() -> web_sys::Element {
    let mark = dom::create_div();
    mark.set_id("app-wordmark");
    dom::set_class(&mark, "app-logo logo-wordmark");

    // Ring + speaker (reuses hero markup)
    let ring_wrap = dom::create_div();
    dom::set_class(&ring_wrap, "logo-ring-wrap");

    let ring = dom::create_div();
    dom::set_class(&ring, "logo-ring-container");
    ring.set_inner_html(&arc_ring_svg());
    dom::append(&ring_wrap, &ring);

    let speaker = dom::create_div();
    dom::set_class(&speaker, "logo-speaker-container");
    speaker.set_inner_html(&speaker_svg());
    dom::append(&ring_wrap, &speaker);

    dom::append(&mark, &ring_wrap);

    // Text row: rule — "Encore" — rule
    let text_row = dom::create_div();
    dom::set_class(&text_row, "wordmark-text-row");

    let rule_left = dom::create_div();
    dom::set_class(&rule_left, "wordmark-rule");
    dom::append(&text_row, &rule_left);

    let brand = dom::create_el("span");
    dom::set_class(&brand, "wordmark-brand");
    dom::set_text(&brand, "Encore");
    dom::append(&text_row, &brand);

    let rule_right = dom::create_div();
    dom::set_class(&rule_right, "wordmark-rule");
    dom::append(&text_row, &rule_right);

    dom::append(&mark, &text_row);

    // Subtitle
    let sub = dom::create_el("span");
    dom::set_class(&sub, "wordmark-sub");
    dom::set_text(&sub, "Community Firmware");
    dom::append(&mark, &sub);

    mark
}

/// Apply a state class to the `#app-logo` element.
///
/// Replaces the untyped string-based `app::set_logo_state`.
pub fn set_state(state: LogoState) {
    if let Some(logo) = dom::get_el("app-logo") {
        for cls in LogoState::ALL {
            dom::remove_class(&logo, cls);
        }
        dom::add_class(&logo, state.class());
    }
}

/// Update the speaker dome fill to reflect live LED state.
pub fn update_dome_color(color: &str) {
    if let Some(dome) = dom::get_el("logo-dome") {
        dom::set_attr(&dome, "fill", color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_svg_contains_dome_color() {
        let out = speaker_svg();
        assert!(out.contains(&format!("fill=\"{}\"", palette::DOME)), "dome fill missing: {out}");
        assert!(out.contains("id=\"logo-dome\""), "dome id missing");
    }

    #[test]
    fn arc_ring_svg_uses_palette_gradient() {
        let out = arc_ring_svg();
        assert!(out.contains(&format!("stop-color=\"{}\"", palette::ACCENT)), "ACCENT stop missing");
        assert!(out.contains(&format!("stop-color=\"{}\"", palette::RING_CREAM)), "RING_CREAM stop missing");
        assert!(out.contains("url(#logo-grad)"), "gradient reference missing");
    }

    #[test]
    fn led_fallbacks_are_on_brand() {
        // VolumeArc uses `level: u8` (not `value`/`max` — verified against protocol.rs)
        assert_eq!(led_dominant_color(&LedAnimation::VolumeArc { level: 80 }), palette::LED_VOLUME);
        assert_eq!(led_dominant_color(&LedAnimation::BootSurge), palette::LED_BOOT);
        assert_eq!(led_dominant_color(&LedAnimation::SafeMode), palette::LED_SAFE);
        assert_eq!(led_dominant_color(&LedAnimation::Off), palette::DOME);
    }

    #[test]
    fn led_explicit_rgb_pass_through() {
        let c = led_dominant_color(&LedAnimation::Solid { r: 0x12, g: 0x34, b: 0x56 });
        assert_eq!(c, "#123456");
    }
}
