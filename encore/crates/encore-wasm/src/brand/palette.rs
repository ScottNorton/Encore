//! Brand palette — single source of truth for every gold value.
//!
//! Every other place that needs a gold color references a constant from this
//! module, either directly in Rust (e.g. SVG template strings) or indirectly
//! via CSS custom properties emitted by `brand::css::css()`.

// ── Accent (primary gold) ──
// Center Stage gold. Dark accent #e6b34d; light (matinee) accent #74561b is the
// AA-safe dark gold (5.0:1 even on the darkest light surface), used for any gold
// text/icon/focus/thin-stroke in light mode.
pub const ACCENT: &str = "#e6b34d";
pub const ACCENT_LIGHT: &str = "#74561b";
pub const ACCENT_HOVER: &str = "#f0c267";
pub const ACCENT_HOVER_LIGHT: &str = "#946f25";

// ── Ring gradient endpoint ──
pub const RING_CREAM: &str = "#F0E6D0";
pub const RING_CREAM_LIGHT: &str = "#D4C4A0";

// ── Speaker dome (LED stand-in) — brightest gold, glow/highlight only ──
pub const DOME: &str = "#ffce6a";
// Light (matinee) brightest gold — glow-only, used for the light data-theme override.
pub const DOME_LIGHT: &str = "#c79a3a";

// ── RGB triplets for rgba() composition via CSS vars ──
pub const ACCENT_R: &str = "230";
pub const ACCENT_G: &str = "179";
pub const ACCENT_B: &str = "77";
pub const ACCENT_LIGHT_R: &str = "116";
pub const ACCENT_LIGHT_G: &str = "86";
pub const ACCENT_LIGHT_B: &str = "27";

// ── LED-reactive dome fallbacks (fixes off-brand GitHub-blue fallbacks) ──
pub const LED_VOLUME: &str = "#e8b040"; // same as DOME — volume arc uses dome gold (was "#58a6ff")
pub const LED_BOOT: &str = "#F0E6D0"; // was "#58a6ff"
pub const LED_SAFE: &str = "#f85149"; // unchanged (matches --red in style.rs)
