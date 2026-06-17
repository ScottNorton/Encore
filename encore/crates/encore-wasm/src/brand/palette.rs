//! Brand palette — single source of truth for every gold value.
//!
//! Every other place that needs a gold color references a constant from this
//! module, either directly in Rust (e.g. SVG template strings) or indirectly
//! via CSS custom properties emitted by `brand::css::css()`.

// ── Accent (primary gold) ──
pub const ACCENT: &str = "#C8A55C";
pub const ACCENT_LIGHT: &str = "#A0864A";
pub const ACCENT_HOVER: &str = "#D4B56E";
pub const ACCENT_HOVER_LIGHT: &str = "#8A7340";

// ── Ring gradient endpoint ──
pub const RING_CREAM: &str = "#F0E6D0";
pub const RING_CREAM_LIGHT: &str = "#D4C4A0";

// ── Speaker dome (LED stand-in) ──
pub const DOME: &str = "#e8b040";

// ── RGB triplets for rgba() composition via CSS vars ──
pub const ACCENT_R: &str = "200";
pub const ACCENT_G: &str = "165";
pub const ACCENT_B: &str = "92";
pub const ACCENT_LIGHT_R: &str = "160";
pub const ACCENT_LIGHT_G: &str = "134";
pub const ACCENT_LIGHT_B: &str = "74";

// ── LED-reactive dome fallbacks (fixes off-brand GitHub-blue fallbacks) ──
pub const LED_VOLUME: &str = "#e8b040"; // same as DOME — volume arc uses dome gold (was "#58a6ff")
pub const LED_BOOT: &str = "#F0E6D0"; // was "#58a6ff"
pub const LED_SAFE: &str = "#f85149"; // unchanged (matches --red in style.rs)
