//! Unified app logo — arc ring + speaker silhouette + "ENCORE" text.
//!
//! One DOM element that animates between three states:
//! - Loading: centered, 60px, fades in from index.html loading screen
//! - Header: top-left, 32px, inline with header bar
//! - Hero: centered, 80px, on the connect screen

use crate::dom;
use encore_common::protocol::LedAnimation;

/// Simplified speaker silhouette SVG.
///
/// viewBox 0 0 100 100. Body spans y=26..82, tapers from rx=14 (top) to
/// rx=17 (bottom). Dome at top is the LED region (fill set dynamically).
pub const SPEAKER_SVG: &str = r##"<svg class="logo-speaker" viewBox="0 0 100 100">
  <path class="speaker-body" d="M36,26 Q34.5,54 33,82 A17,5 0 0,0 67,82 Q65.5,54 64,26 A14,3.8 0 0,1 36,26 Z" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.5"/>
  <ellipse class="speaker-cap" cx="50" cy="33" rx="15" ry="3.2" fill="none" stroke="currentColor" stroke-width="0.8" opacity="0.4"/>
  <ellipse class="speaker-base" cx="50" cy="82" rx="17" ry="5" fill="none" stroke="currentColor" stroke-width="0.6" opacity="0.3"/>
  <ellipse id="logo-dome" class="speaker-dome" cx="50" cy="26" rx="12" ry="2.8" fill="#e8b040" opacity="0.85"/>
</svg>"##;

/// Arc ring SVG — 270° gradient arc with gap facing down.
///
/// stroke-dasharray="198 66" = 75% visible = 270°.
/// rotate(135 50 50) positions the gap at the bottom.
pub const ARC_RING_SVG: &str = r##"<div class="logo-glow"></div>
<svg class="logo-ring" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="logo-grad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#58a6ff"/>
      <stop offset="50%" stop-color="#a371f7"/>
      <stop offset="100%" stop-color="#58a6ff"/>
    </linearGradient>
  </defs>
  <circle class="logo-track" cx="50" cy="50" r="42" fill="none" stroke-width="3"/>
  <circle class="logo-arc" cx="50" cy="50" r="42" fill="none" stroke="url(#logo-grad)" stroke-width="2.5" stroke-linecap="round" stroke-dasharray="198 66" transform="rotate(135 50 50)"/>
</svg>"##;

/// Build the unified logo DOM element.
///
/// Returns a fixed-position div containing the arc ring, speaker
/// silhouette, and "ENCORE" text. Starts in `.logo-loading` state.
pub fn build_logo() -> web_sys::Element {
    let logo = dom::create_div();
    logo.set_id("app-logo");
    dom::set_class(&logo, "app-logo logo-loading");

    // Ring container (arc ring + speaker silhouette stacked)
    let ring_wrap = dom::create_div();
    dom::set_class(&ring_wrap, "logo-ring-wrap");

    // Arc ring with glow
    let ring = dom::create_div();
    dom::set_class(&ring, "logo-ring-container");
    ring.set_inner_html(ARC_RING_SVG);
    dom::append(&ring_wrap, &ring);

    // Speaker silhouette (overlaid on ring)
    let speaker = dom::create_div();
    dom::set_class(&speaker, "logo-speaker-container");
    speaker.set_inner_html(SPEAKER_SVG);
    dom::append(&ring_wrap, &speaker);

    dom::append(&logo, &ring_wrap);

    // "ENCORE" text
    let text = dom::create_el("span");
    text.set_id("logo-text");
    dom::set_class(&text, "logo-text");
    dom::set_text(&text, "ENCORE");
    dom::append(&logo, &text);

    // Status text (shown during connecting/syncing states)
    let status = dom::create_el("span");
    status.set_id("logo-status");
    dom::set_class(&status, "logo-status");
    dom::append(&logo, &status);

    logo
}

/// Extract a dominant CSS color from the current LED animation state.
pub fn led_dominant_color(anim: &LedAnimation) -> String {
    match anim {
        LedAnimation::Off => "#e8b040".to_string(),
        LedAnimation::Solid { r, g, b }
        | LedAnimation::Breathe { r, g, b, .. }
        | LedAnimation::Spin { r, g, b, .. }
        | LedAnimation::Pulse { r, g, b } => format!("#{:02x}{:02x}{:02x}", r, g, b),
        LedAnimation::VolumeArc { .. } => "#58a6ff".to_string(),
        LedAnimation::BootSurge => "#58a6ff".to_string(),
        LedAnimation::SafeMode => "#ff6b6b".to_string(),
        LedAnimation::Custom { frames } => {
            if let Some(frame) = frames.first() {
                let (r, g, b) = frame.colors[0];
                format!("#{:02x}{:02x}{:02x}", r, g, b)
            } else {
                "#e8b040".to_string()
            }
        }
    }
}

/// Update the speaker dome fill color to reflect live LED state.
pub fn update_dome_color(color: &str) {
    if let Some(dome) = dom::get_el("logo-dome") {
        dom::set_attr(&dome, "fill", color);
    }
}
