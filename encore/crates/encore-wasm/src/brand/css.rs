// encore/crates/encore-wasm/src/brand/css.rs
//! Palette-driven CSS: `:root` vars, logo classes, keyframes, wordmark rules.
//!
//! `css()` returns a complete stylesheet fragment that `style::inject()`
//! concatenates with the component CSS.

use super::palette;

pub fn css() -> String {
    format!(
        r#"
:root {{
  --accent: {accent};
  --accent-hover: {accent_hover};
  --accent-r: {ar};
  --accent-g: {ag};
  --accent-b: {ab};
  --ring-cream: {ring_cream};
  --dome: {dome};
}}
/* Matinee gold. data-theme wins over the OS query so the Settings control is
   authoritative; the OS query only applies when no data-theme is set. */
[data-theme='light'] {{
  --accent: {accent_light};
  --accent-hover: {accent_hover_light};
  --accent-r: {alr};
  --accent-g: {alg};
  --accent-b: {alb};
  --ring-cream: {ring_cream_light};
  --dome: {dome_light};
}}
@media (prefers-color-scheme: light) {{
  :root:not([data-theme]) {{
    --accent: {accent_light};
    --accent-hover: {accent_hover_light};
    --accent-r: {alr};
    --accent-g: {alg};
    --accent-b: {alb};
    --ring-cream: {ring_cream_light};
    --dome: {dome_light};
  }}
}}

/* ── Unified Logo ── */
.app-logo {{
    position: fixed;
    z-index: 100;
    display: flex;
    align-items: center;
    pointer-events: none;
    transition: top 0.5s cubic-bezier(.4,0,.2,1),
                left 0.5s cubic-bezier(.4,0,.2,1),
                opacity 0.4s ease;
}}
.logo-ring-wrap {{
    position: relative;
    flex-shrink: 0;
    transition: width 0.5s cubic-bezier(.4,0,.2,1),
                height 0.5s cubic-bezier(.4,0,.2,1);
}}
.logo-ring-container {{ position: absolute; inset: 0; }}
.logo-ring-container svg {{
    width: 100%; height: 100%; display: block;
    animation: logo-spin 25s linear infinite;
}}
.logo-ring-container .logo-track {{ stroke: var(--border); }}
.logo-glow {{
    position: absolute; inset: -5px; border-radius: 50%;
    background: radial-gradient(circle, rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.12) 0%, transparent 70%);
    animation: logo-glow 4s ease-in-out infinite;
    pointer-events: none;
}}
.logo-speaker-container {{ position: absolute; inset: 15%; }}
.logo-speaker-container svg {{ width: 100%; height: 100%; display: block; }}
.speaker-dome {{ transition: fill 1s ease; }}
.logo-text {{
    font-weight: 300; letter-spacing: 4px;
    text-transform: uppercase; white-space: nowrap;
    transition: opacity 0.3s ease, font-size 0.5s cubic-bezier(.4,0,.2,1);
}}
.logo-status {{
    font-size: 12px; color: var(--text-muted);
    letter-spacing: 2px; opacity: 0;
    transition: opacity 0.3s ease;
}}
.logo-connecting .logo-status {{ opacity: 1; }}
@keyframes logo-spin {{ to {{ transform: rotate(360deg); }} }}
@keyframes logo-glow {{
    0%, 100% {{ opacity: 0.3; transform: scale(0.92); }}
    50% {{ opacity: 0.9; transform: scale(1.08); }}
}}
[data-theme='light'] .logo-glow {{
    background: radial-gradient(circle, rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.08) 0%, transparent 70%);
}}
@media (prefers-color-scheme: light) {{
    :root:not([data-theme]) .logo-glow {{
        background: radial-gradient(circle, rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.08) 0%, transparent 70%);
    }}
}}

/* Loading state: centered, 60px */
.logo-loading {{
    top: 50%; left: 50%;
    margin-top: -30px; margin-left: -30px;
    flex-direction: column; gap: 12px;
}}
.logo-loading .logo-ring-wrap {{ width: 60px; height: 60px; }}
.logo-loading .logo-text {{ font-size: 16px; opacity: 0; }}

/* Header state: top-left, 32px, inline */
.logo-header {{
    top: calc(12px + env(safe-area-inset-top, 0px));
    left: max(16px, calc((100vw - 1200px) / 2 + 16px));
    margin: 0;
    flex-direction: row; gap: 10px;
}}
.logo-header .logo-ring-wrap {{ width: 32px; height: 32px; }}
.logo-header .logo-text {{ font-size: 16px; opacity: 1; }}

/* Hero state: above connect form, 100px, text below */
.logo-hero {{
    top: 80px; left: 50%;
    transform: translateX(-50%);
    margin: 0;
    flex-direction: column; align-items: center; gap: 14px;
}}
.logo-hero .logo-ring-wrap {{ width: 100px; height: 100px; }}
.logo-hero .logo-text {{ font-size: 20px; opacity: 1; }}

/* Connecting state: same position as hero, ring shrinks slightly */
.logo-connecting {{
    top: 80px; left: 50%;
    transform: translateX(-50%);
    margin: 0;
    flex-direction: column; align-items: center; gap: 14px;
}}
.logo-connecting .logo-ring-wrap {{ width: 80px; height: 80px; }}
.logo-connecting .logo-text {{ font-size: 20px; opacity: 1; }}
.logo-connecting .logo-ring-container svg {{ animation-duration: 3s; }}

/* Desktop: the fixed left rail owns the leftmost 96px, so the header logo is
   shifted right to clear it and stay aligned with the shifted header content. */
@media (min-width: 641px) {{
    .logo-header {{
        left: max(calc(96px + 16px), calc((100vw - 1200px) / 2 + 96px + 16px));
    }}
}}

/* Wordmark state: about panel — Classical Badge */
.logo-wordmark {{
    position: relative;
    margin: 0 auto;
    padding: 8px 0 24px;
    display: flex; flex-direction: column; align-items: center; gap: 16px;
    pointer-events: none;
}}
.logo-wordmark .logo-ring-wrap {{ width: 64px; height: 64px; }}
.wordmark-text-row {{
    display: flex; align-items: center; gap: 14px;
}}
.wordmark-rule {{
    width: 32px; height: 1px;
    background: linear-gradient(90deg, transparent, rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.5), transparent);
}}
.wordmark-brand {{
    font-size: 26px; font-weight: 200;
    letter-spacing: 9px; text-transform: uppercase;
    color: var(--text); line-height: 1;
}}
.wordmark-sub {{
    font-size: 10px; font-weight: 500;
    letter-spacing: 3px; text-transform: uppercase;
    color: var(--text-secondary);
}}
"#,
        accent = palette::ACCENT,
        accent_hover = palette::ACCENT_HOVER,
        accent_light = palette::ACCENT_LIGHT,
        accent_hover_light = palette::ACCENT_HOVER_LIGHT,
        ar = palette::ACCENT_R,
        ag = palette::ACCENT_G,
        ab = palette::ACCENT_B,
        alr = palette::ACCENT_LIGHT_R,
        alg = palette::ACCENT_LIGHT_G,
        alb = palette::ACCENT_LIGHT_B,
        ring_cream = palette::RING_CREAM,
        ring_cream_light = palette::RING_CREAM_LIGHT,
        dome = palette::DOME,
        dome_light = palette::DOME_LIGHT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_contains_palette_values() {
        let out = css();
        assert!(
            out.contains(&format!("--accent: {}", palette::ACCENT)),
            "dark accent var missing"
        );
        assert!(
            out.contains("[data-theme='light']"),
            "data-theme light override block missing"
        );
        assert!(
            out.contains(":root:not([data-theme])"),
            "OS query should be scoped to :root:not([data-theme])"
        );
        assert!(
            out.contains(&format!("--accent: {}", palette::ACCENT_LIGHT)),
            "light accent var missing"
        );
        assert!(
            out.contains(&format!("--ring-cream: {}", palette::RING_CREAM)),
            "ring cream missing"
        );
        assert!(
            out.contains(&format!("--dome: {}", palette::DOME)),
            "dome var missing"
        );
        assert!(
            out.contains(&format!("--accent-r: {}", palette::ACCENT_R)),
            "accent R component missing"
        );
        assert!(
            out.contains(&format!("--accent-r: {}", palette::ACCENT_LIGHT_R)),
            "light accent R component missing"
        );
    }

    #[test]
    fn css_contains_logo_rules() {
        let out = css();
        assert!(out.contains(".app-logo"), "app-logo selector missing");
        assert!(
            out.contains(".logo-ring-container svg"),
            "ring container missing"
        );
        assert!(out.contains(".logo-wordmark"), "wordmark selector missing");
        assert!(
            out.contains(".wordmark-rule"),
            "wordmark rule selector missing"
        );
        assert!(
            out.contains("@keyframes logo-spin"),
            "logo-spin keyframe missing"
        );
        assert!(
            out.contains("@keyframes logo-glow"),
            "logo-glow keyframe missing"
        );
    }

    #[test]
    fn css_uses_css_vars_for_glow() {
        let out = css();
        assert!(
            out.contains("rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.12)"),
            "glow should use CSS vars, not hardcoded rgba"
        );
        assert!(
            !out.contains("rgba(200,165,92,0.12)"),
            "glow should not have hardcoded rgba (found in css() output)"
        );
    }
}
