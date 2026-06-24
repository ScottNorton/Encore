//! CSS theme system — injected at startup.
//!
//! Auto light/dark via prefers-color-scheme. CSS custom properties
//! for all colors. Responsive breakpoints at 640px and 960px.

use crate::dom;

/// Inject the complete stylesheet into <head>
pub fn inject() {
    let style = dom::create_el("style");
    let combined = format!(
        "{}\n{}\n{}\n{}",
        crate::brand::css::css(),
        CSS,
        crate::components::toast::css(),
        crate::components::async_state::css()
    );
    style.set_text_content(Some(&combined));
    let doc = dom::document();
    if let Ok(Some(head)) = doc.query_selector("head") {
        head.append_child(&style).unwrap();
    }
}

// Gold palette vars (--accent, --accent-r/g/b, --accent-hover, --ring-cream, --dome)
// and logo/wordmark CSS live in brand::css::css() and are prepended by inject().
const CSS: &str = r#"
@font-face {
    font-family: 'Inter';
    font-style: normal;
    font-weight: 400;
    font-display: swap;
    src: url('/fonts/Inter-Regular.woff2') format('woff2');
}
@font-face {
    font-family: 'Inter';
    font-style: normal;
    font-weight: 500;
    font-display: swap;
    src: url('/fonts/Inter-Medium.woff2') format('woff2');
}
:root {
    /* Center Stage dark (default). Gold vars (--accent*, --dome, --ring-cream)
       are emitted by brand::css and are not redefined here. */
    --stage: #14110d;
    --surface: #1c1610;
    --surface-2: #1d1810;
    --surface-hover: #241d15;
    --surface-input: #15110b;
    --sunken: #0e0b07;
    --border: rgba(242,233,218,0.07);
    --border-strong: rgba(242,233,218,0.14);
    --text: #f2e9da;
    --text-secondary: #cdbd92;
    --text-tertiary: #9a8e72;       /* 5.8:1 on stage — lowest tier for small text */
    --text-muted: #8f8266;          /* AA on all four dark surfaces (>=4.66:1) */
    --text-on-accent: #14110d;
    --accent-active: #c99a3a;
    --accent-glow: #ffce6a;
    --accent-soft: rgba(230,179,77,0.14);
    --warning-soft: rgba(230,162,60,0.10);
    --source-active: #ffce6a;
    --live: #ff5c4d;
    --success: #7fb86a;
    --warning: #e6a23c;
    --danger: #f0584d;
    --spotify: #1ed760;
    --focus-ring-inner: #14110d;
    --focus-ring-outer: var(--accent);
    /* Legacy aliases — let call sites migrate incrementally. */
    --bg: var(--stage);
    --bg-card: var(--surface);
    --bg-card-hover: var(--surface-hover);
    --bg-input: var(--surface-input);
    --green: var(--success);
    --orange: var(--warning);
    --red: var(--danger);
    --purple: #bc8cff;
    --cyan: #39d2c0;
    --yellow: var(--warning);
    /* Spacing scale (8px grid). */
    --space-1: 4px;
    --space-2: 8px;
    --space-3: 12px;
    --space-4: 16px;
    --space-5: 24px;
    --space-6: 32px;
    --radius: 16px;
    --radius-sm: 10px;
    --radius-pill: 22px;
    --radius-chip: 999px;
    --shadow: 0 4px 20px rgba(0,0,0,0.5);
    --shadow-hover: 0 6px 24px rgba(0,0,0,0.6);
    --glow-sm: 0 0 6px rgba(230,179,77,0.45);
    --glow-md: 0 0 14px rgba(230,179,77,0.35);
    --nav-height: 56px;
    --tab-height: 44px;
    --header-height: 56px;
    /* Type families. Sans for chrome; mono for the signature numeric readouts. */
    --font-sans: 'Inter', system-ui, -apple-system, 'Segoe UI', sans-serif;
    --font-mono: ui-monospace, 'SF Mono', 'Cascadia Code', 'Consolas', monospace;
}
/* Matinee (light) — explicit attribute. */
[data-theme='light'] {
    --stage: #f4ecdd;
    --surface: #fffaf0;
    --surface-2: #f3ead9;
    --surface-hover: #efe6d2;
    --surface-input: #fffaf0;
    --sunken: #e7dcc6;
    --border: rgba(60,42,16,0.10);
    --border-strong: rgba(60,42,16,0.18);
    --text: #241d10;
    --text-secondary: #5c5036;
    --text-tertiary: #7a6c4c;
    --text-muted: #7a6c4c;
    --text-on-accent: #1c1509;
    --accent-active: #7d5d1e;
    --accent-glow: #c79a3a;
    --accent-soft: rgba(168,134,47,0.12);
    --warning-soft: rgba(176,125,28,0.12);
    --source-active: #a8862f;
    --live: #c2342b;
    --success: #33682f;
    --warning: #b07d1c;
    --danger: #b32d24;
    --spotify: #1db954;
    --focus-ring-inner: #fffaf0;
    --focus-ring-outer: var(--accent);
    --bg: var(--stage);
    --bg-card: var(--surface);
    --bg-card-hover: var(--surface-hover);
    --bg-input: var(--surface-input);
    --green: var(--success);
    --orange: var(--warning);
    --red: var(--danger);
    --purple: #8250df;
    --cyan: #1b7c83;
    --yellow: var(--warning);
    --shadow: 0 4px 16px rgba(0,0,0,0.06);
    --glow-sm: 0 0 6px rgba(168,134,47,0.45);
    --glow-md: 0 0 14px rgba(168,134,47,0.35);
}
/* Matinee (light) — OS default, only when no explicit data-theme is set. */
@media (prefers-color-scheme: light) {
    :root:not([data-theme]) {
        --stage: #f4ecdd;
        --surface: #fffaf0;
        --surface-2: #f3ead9;
        --surface-hover: #efe6d2;
        --surface-input: #fffaf0;
        --sunken: #e7dcc6;
        --border: rgba(60,42,16,0.10);
        --border-strong: rgba(60,42,16,0.18);
        --text: #241d10;
        --text-secondary: #5c5036;
        --text-tertiary: #7a6c4c;
        --text-muted: #7a6c4c;
        --text-on-accent: #1c1509;
        --accent-active: #7d5d1e;
        --accent-glow: #c79a3a;
        --accent-soft: rgba(168,134,47,0.12);
        --warning-soft: rgba(176,125,28,0.12);
        --source-active: #a8862f;
        --live: #c2342b;
        --success: #33682f;
        --warning: #b07d1c;
        --danger: #b32d24;
        --spotify: #1db954;
        --focus-ring-inner: #fffaf0;
        --focus-ring-outer: var(--accent);
        --bg: var(--stage);
        --bg-card: var(--surface);
        --bg-card-hover: var(--surface-hover);
        --bg-input: var(--surface-input);
        --green: var(--success);
        --orange: var(--warning);
        --red: var(--danger);
        --purple: #8250df;
        --cyan: #1b7c83;
        --yellow: var(--warning);
        --shadow: 0 4px 16px rgba(0,0,0,0.06);
        --glow-sm: 0 0 6px rgba(168,134,47,0.45);
        --glow-md: 0 0 14px rgba(168,134,47,0.35);
    }
}
* { margin: 0; padding: 0; box-sizing: border-box; }
html, body {
    height: 100%; width: 100%;
    background: var(--bg);
    color: var(--text);
    font-family: 'Inter', system-ui, -apple-system, 'Segoe UI', sans-serif;
    font-size: 14px;
    line-height: 1.5;
    letter-spacing: 0.01em;
    overflow-x: hidden;
    -webkit-font-smoothing: antialiased;
    touch-action: manipulation;
    -webkit-tap-highlight-color: transparent;
    -webkit-user-select: none;
    user-select: none;
}
/* Allow text selection only in logs console and about info */
.log-output, .about-value, .about-copy, pre, code, .crash-log {
    -webkit-user-select: text;
    user-select: text;
}
/* Desktop app: drag is handled via Tauri startDragging() API on the header.
   CSS -webkit-app-region doesn't work reliably in WebView2. */
@supports (height: 100dvh) {
    html, body { height: 100dvh; }
}
#app {
    position: relative;
    display: flex; flex-direction: column;
    height: 100%; width: 100%;
    max-width: 1200px;
    margin: 0 auto;
}
/* Stage spotlight — one fixed radial-gradient, painted once, effectively free. */
#app::before {
    content: "";
    position: fixed;
    inset: 0;
    pointer-events: none;
    z-index: 0;
    background: radial-gradient(120% 80% at 50% -10%,
        rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.10), transparent 60%);
}
/* Stage spotlight stacking: keep in-flow children above the ::before glow, but
   exclude the fixed shell elements (nav rail, mini-bar, header logo) so their
   own position:fixed wins — an unscoped `#app > *` (id specificity) would
   otherwise force them back to position:relative and break the desktop rail,
   mini-bar, and the top-left logo (which would then flow inline, mis-placed). */
#app > *:not(.primary-nav):not(#mini-nowplaying):not(#app-logo) { position: relative; z-index: 1; }
@supports (height: 100dvh) {
    #app { height: 100dvh; }
}

/* ── Header ── */
.header {
    position: relative;
    padding: 12px 16px;
    padding-top: calc(12px + env(safe-area-inset-top, 0px));
    display: flex;
    align-items: center;
    justify-content: space-between;
    z-index: 10;
}
.header-left {
    display: flex; align-items: center; gap: 10px;
    flex: 1; min-width: 0;
}

.header-device-name {
    font-size: 13px; font-weight: 400;
    color: var(--text-secondary);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    max-width: 200px;
    margin-right: 8px;
    opacity: 0; transition: opacity 0.4s ease;
}
.header-device-name.visible { opacity: 1; }
.conn-dot {
    width: 8px; height: 8px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: background .3s;
    flex-shrink: 0;
    margin-right: 6px;
}
.conn-dot.connected { background: var(--green); box-shadow: 0 0 6px var(--green); }
.conn-dot.error { background: var(--red); }
.gear-btn {
    background: none; border: none; color: var(--text-muted);
    cursor: pointer;
    width: 44px; height: 44px;
    display: inline-flex; align-items: center; justify-content: center;
    border-radius: 50%;
    transition: color .25s ease;
    flex-shrink: 0;
    -webkit-tap-highlight-color: transparent;
}
.gear-btn:hover { color: var(--accent); }
.gear-btn.open { color: var(--text-secondary); }
.gear-btn.open:hover { color: var(--accent); }
.ham-top, .ham-mid, .ham-bot {
    transition: transform .3s cubic-bezier(.4,0,.2,1);
}
.gear-btn.open .ham-top { transform: translateY(-2px); }
.gear-btn.open .ham-bot { transform: translateY(2px); }

/* ── Window Controls (desktop Tauri only) ── */
.window-controls {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-left: 4px;
    -webkit-app-region: no-drag;
    flex-shrink: 0;
}
.header-divider {
    width: 1px;
    height: 20px;
    background: var(--border);
    margin: 0 4px;
    flex-shrink: 0;
}
.wc-btn {
    background: none;
    border: none;
    color: var(--text-secondary);
    width: 36px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background .15s, color .15s;
    -webkit-tap-highlight-color: transparent;
    -webkit-app-region: no-drag;
}
.wc-btn svg {
    pointer-events: none;
}
/* Maximize/restore icon toggle */
.wc-maximize .wc-icon-restore { display: none; }
.wc-maximize.maximized .wc-icon-maximize { display: none; }
.wc-maximize.maximized .wc-icon-restore { display: block; }
.wc-btn:hover {
    background: rgba(255,255,255,0.08);
    color: var(--text);
}
.wc-close:hover {
    background: #e81123;
    color: #fff;
}
@media (prefers-color-scheme: light) {
    .wc-btn:hover {
        background: rgba(0,0,0,0.06);
    }
    .wc-close:hover {
        background: #e81123;
        color: #fff;
    }
}

/* ── Tab Navigation ── */
.tab-nav-wrap {
    position: relative;
    margin: 0 16px 12px;
}
.tab-nav-wrap::before,
.tab-nav-wrap::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    width: 28px;
    z-index: 2;
    pointer-events: none;
    opacity: 0;
    transition: opacity .3s;
}
.tab-nav-wrap::before {
    left: 0;
    background: linear-gradient(to right, var(--bg-card), transparent);
    border-radius: 22px 0 0 22px;
}
.tab-nav-wrap::after {
    right: 0;
    background: linear-gradient(to left, var(--bg-card), transparent);
    border-radius: 0 22px 22px 0;
}
.tab-nav-wrap.fade-left::before { opacity: 1; }
.tab-nav-wrap.fade-right::after { opacity: 1; }
.tab-nav {
    display: flex;
    gap: 2px;
    padding: 3px;
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
    scrollbar-width: none;
    background: var(--bg-card);
    border: none;
    border-radius: 22px;
    cursor: grab;
    user-select: none;
    -webkit-user-select: none;
}
.tab-nav::-webkit-scrollbar { display: none; }
.tab-nav.dragging { cursor: grabbing; }
.tab-btn {
    flex-shrink: 0;
    padding: 9px 16px;
    border-radius: 18px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: color .15s, background .15s, box-shadow .15s;
    white-space: nowrap;
    outline: none;
    -webkit-tap-highlight-color: transparent;
}
.tab-btn:hover { color: var(--text); background: rgba(255,255,255,0.06); }
.tab-btn.active {
    color: var(--text);
    background: linear-gradient(135deg, var(--bg) 0%, var(--bg-card) 100%);
    box-shadow: 0 1px 4px rgba(0,0,0,0.15), inset 0 -2px 0 var(--accent);
}
@media (prefers-color-scheme: light) {
    .tab-btn:hover { background: rgba(0,0,0,0.04); }
    .tab-btn.active { box-shadow: 0 1px 4px rgba(0,0,0,0.08), inset 0 -2px 0 var(--accent); }
}

/* ── Content Area ── */
#main {
    transition: opacity 0.15s ease;
}
.content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 16px 24px;
    padding-bottom: calc(24px + env(safe-area-inset-bottom, 0px));
    -webkit-overflow-scrolling: touch;
}

/* ── Cards ── */
@keyframes fadeInUp { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
.card {
    background: var(--bg-card);
    border: none;
    border-radius: var(--radius);
    padding: 16px;
    margin-bottom: 12px;
    transition: background .2s, box-shadow .2s;
    animation: fadeInUp 0.2s ease;
    box-shadow: var(--shadow);
}
.card:hover { background: var(--bg-card-hover); box-shadow: 0 6px 24px rgba(0,0,0,0.6); }
@media (prefers-color-scheme: light) {
    .card:hover { box-shadow: 0 6px 24px rgba(0,0,0,0.08); }
}
.card-title {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-secondary);
    margin-bottom: 12px;
}
.section-label {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--accent);
    margin: var(--space-4) 0 var(--space-2);
}
.section-label:first-child { margin-top: 0; }
.card-grid {
    display: grid;
    gap: 12px;
    grid-template-columns: 1fr;
}
@media (min-width: 640px) {
    .card-grid { grid-template-columns: repeat(2, 1fr); }
}
@media (min-width: 960px) {
    .card-grid { grid-template-columns: repeat(3, 1fr); }
    .card-grid .card-wide { grid-column: span 2; }
}

/* ── Volume Row (master knob + source sliders side by side) ── */
.vol-row {
    display: grid;
    gap: 12px;
    grid-template-columns: auto 1fr;
    align-items: start;
}
@media (max-width: 479px) {
    .vol-row { grid-template-columns: 1fr; }
}

/* ── Badge Grid ── */
.badge-grid {
    display: flex; flex-wrap: wrap;
    gap: 6px; margin-bottom: 12px;
}
.badge-pill {
    display: inline-flex; align-items: center; gap: 4px;
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 10px;
    font-weight: 500;
    background: var(--bg-card);
    border: 1px solid var(--border);
    white-space: nowrap;
}
.badge-icon {
    font-size: 10px; line-height: 1;
}
.badge-running .badge-icon { color: var(--green); }
.badge-stopped .badge-icon { color: var(--text-muted); }
.badge-crashed .badge-icon { color: var(--red); }
.badge-degraded .badge-icon { color: var(--orange); }
/* Legacy badge (used by other pages) */
.badge {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 4px 10px;
    border-radius: 12px;
    font-size: 11px;
    font-weight: 500;
    background: var(--bg-card);
    border: 1px solid var(--border);
}
.badge-dot {
    width: 6px; height: 6px;
    border-radius: 50%;
}
.badge-running .badge-dot { background: var(--green); }
.badge-stopped .badge-dot { background: var(--text-muted); }
.badge-crashed .badge-dot { background: var(--red); }
.badge-degraded .badge-dot { background: var(--orange); }
.badge-blue { background: rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.15); border-color: rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.3); color: var(--accent); }
.badge-green { background: rgba(63,185,80,0.15); border-color: rgba(63,185,80,0.3); color: var(--green); }
.badge-muted { background: var(--bg); color: var(--text-muted); }
.badge-sm { padding: 2px 8px; font-size: 10px; border-radius: 10px; }
.text-bold { font-weight: 600; }

/* ── Health Indicator Dots (speaker peers) ── */
.health-dot {
    display: inline-block;
    width: 8px; height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
}
.health-good { background: var(--green); box-shadow: 0 0 4px var(--green); }
.health-warn { background: var(--orange); box-shadow: 0 0 4px var(--orange); }
.health-bad { background: var(--red); box-shadow: 0 0 4px var(--red); }

/* ── Skeleton Loading ── */
@keyframes shimmer {
    0% { background-position: -200% 0; }
    100% { background-position: 200% 0; }
}
.skeleton {
    background: linear-gradient(90deg, var(--bg-card) 25%, var(--bg-card-hover) 50%, var(--bg-card) 75%);
    background-size: 200% 100%;
    animation: shimmer 1.5s ease infinite;
    border-radius: var(--radius-sm);
}
.skeleton-line {
    height: 14px;
    border-radius: 4px;
    margin-bottom: 8px;
    background: linear-gradient(90deg, var(--bg-card) 25%, var(--bg-card-hover) 50%, var(--bg-card) 75%);
    background-size: 200% 100%;
    animation: shimmer 1.5s ease infinite;
}

/* ── Progress Bars ── */
.bar-track {
    height: 8px;
    background: var(--bg);
    border-radius: 4px;
    overflow: hidden;
}
.bar-fill {
    height: 100%;
    border-radius: 4px;
    transition: width .3s;
}

/* ── Stat Row ── */
.stat-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 0;
}
.stat-label { color: var(--text-secondary); font-size: 13px; }
.stat-value { font-weight: 600; font-variant-numeric: tabular-nums; }

/* ── Form Controls ── */
input[type="text"], input[type="password"], input[type="number"], select, textarea {
    width: 100%;
    padding: 8px 12px;
    background: var(--bg-input);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 14px;
    font-family: inherit;
    outline: none;
    transition: border-color .2s;
}
input:focus, select:focus, textarea:focus {
    border-color: var(--accent);
}
input[type="range"] {
    -webkit-appearance: none;
    width: 100%;
    height: 6px;
    background: var(--border);
    border-radius: 3px;
    outline: none;
    touch-action: none;
}
input[type="range"]::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 22px; height: 22px;
    border-radius: 50%;
    background: var(--accent);
    cursor: pointer;
    box-shadow: 0 1px 4px rgba(0,0,0,0.25);
    transition: transform .1s ease, box-shadow .1s ease;
}
input[type="range"]:active::-webkit-slider-thumb {
    transform: scale(1.15);
    box-shadow: 0 2px 8px rgba(0,0,0,0.3);
}
label {
    display: block;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    margin-bottom: 4px;
}

/* ── Buttons ── */
.btn {
    display: inline-flex; align-items: center; justify-content: center; gap: 6px;
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-card);
    color: var(--text);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
}
.btn:hover { background: var(--bg-card-hover); }
.btn:active { transform: scale(0.97); }
.btn-primary {
    background: var(--accent);
    color: var(--text-on-accent);
    border-color: var(--accent);
}
.btn-primary:hover { background: var(--accent-hover); }
.btn-danger {
    color: var(--red);
    border-color: var(--red);
}
.btn-danger:hover { background: rgba(248,81,73,0.1); }

/* ── Toggle Switch ── */
.toggle-wrap {
    display: flex; align-items: center; justify-content: space-between;
    padding: 8px 0;
}
.toggle {
    position: relative;
    width: 44px; height: 24px;
    cursor: pointer;
}
.toggle input { display: none; }
.toggle-track {
    position: absolute; inset: 0;
    background: var(--border);
    border-radius: 12px;
    transition: background .2s;
}
.toggle input:checked + .toggle-track {
    background: var(--accent);
}
.toggle-thumb {
    position: absolute;
    top: 2px; left: 2px;
    width: 20px; height: 20px;
    background: #fff;
    border-radius: 50%;
    transition: transform .2s;
    box-shadow: 0 1px 3px rgba(0,0,0,0.3);
}
.toggle input:checked ~ .toggle-thumb {
    transform: translateX(20px);
}

/* ── Slide-over Panel ── */
.panel-overlay {
    position: fixed; inset: 0;
    background: rgba(0,0,0,0.5);
    z-index: 100;
    opacity: 0;
    transition: opacity .3s;
    pointer-events: none;
}
.panel-overlay.open {
    opacity: 1;
    pointer-events: auto;
}
.panel {
    position: fixed;
    top: 0; right: 0; bottom: 0;
    width: 100%;
    max-width: 480px;
    background: var(--bg);
    z-index: 101;
    transform: translateX(100%);
    transition: transform .3s ease;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
}
.panel.open { transform: translateX(0); }
.panel-header {
    display: flex; align-items: center; gap: 12px;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    background: var(--bg);
    z-index: 1;
}
.panel-back {
    background: none; border: none;
    color: var(--text-secondary);
    font-size: 20px; cursor: pointer;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    transition: color .15s, background .15s;
    -webkit-tap-highlight-color: transparent;
}
.panel-back:hover { color: var(--text); background: var(--bg-card); }
.panel-back:active { transform: scale(0.92); }
.panel-title {
    font-size: 16px; font-weight: 600;
}
.panel-body {
    flex: 1;
    padding: 16px 20px;
}

/* ── Gear Menu Dropdown ── */
.gear-menu {
    position: absolute;
    top: var(--header-height);
    right: 16px;
    z-index: 200;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    min-width: 200px;
    display: none;
    overflow: hidden;
}
.gear-menu.connect-mode .system-item { display: none !important; }
.gear-menu.open { display: block; }
.gear-menu-item {
    display: block; width: 100%;
    padding: 10px 16px;
    background: none; border: none;
    color: var(--text);
    font-size: 14px;
    text-align: left;
    cursor: pointer;
    transition: background .15s;
}
.gear-menu-item:hover { background: var(--bg-card-hover); }
.gear-menu-sep {
    height: 1px;
    background: var(--border);
    margin: 4px 0;
}

/* ── Spotify Glass Card ── */
.glass-card {
    /* Flat scrim by default — cheap on the device, and blur near the bloom would
       force a compositor reblur every paint. Opt into blur only where supported
       and cheap. Center Stage: a warm surface, not the old Spotify-green wash. */
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 20px;
}
@supports (backdrop-filter: blur(8px)) {
    .glass-card {
        background: var(--surface);
        backdrop-filter: blur(8px);
        -webkit-backdrop-filter: blur(8px);
    }
}

/* ── Spotify Transport Controls ── */
.sp-transport {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 16px;
    padding: 8px 0;
}
.sp-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--text-secondary);
    font-size: 20px;
    cursor: pointer;
    padding: 8px;
    border-radius: 50%;
    transition: opacity .15s, transform .1s, background .15s;
    min-width: 40px;
    min-height: 40px;
    opacity: 0.5;
    outline: none;
    -webkit-tap-highlight-color: transparent;
}
.sp-btn:hover { opacity: 0.8; color: var(--accent); }
.sp-btn:active { transform: scale(0.92); }
/* Active ghost button (shuffle/repeat) goes gold, not Spotify green. */
.sp-btn.active {
    opacity: 1.0;
    color: var(--accent);
    background: var(--accent-soft);
}
/* Play/pause: the gold spotlight button with a small glow. */
.sp-btn-play {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 52px;
    height: 52px;
    border-radius: 50%;
    border: none;
    background: var(--accent);
    color: var(--text-on-accent);
    font-size: 24px;
    cursor: pointer;
    transition: transform .1s, background .15s, box-shadow .15s;
    box-shadow: var(--glow-sm);
    outline: none;
    -webkit-tap-highlight-color: transparent;
}
.sp-btn-play:hover {
    background: var(--accent-hover);
    transform: scale(1.05);
    box-shadow: var(--glow-md);
}
.sp-btn-play:active { transform: scale(0.95); }
.sp-btn-skip {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--text);
    font-size: 26px;
    cursor: pointer;
    padding: 8px;
    border-radius: 50%;
    transition: color .15s, transform .1s;
    min-width: 44px;
    min-height: 44px;
    outline: none;
    -webkit-tap-highlight-color: transparent;
}
.sp-btn-skip:hover { color: var(--accent); }
.sp-btn-skip:active { transform: scale(0.92); }
/* Spotify settings */
.sp-settings-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    width: 100%;
    padding: 8px 16px;
    margin-top: 16px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-secondary);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all .15s;
}
.sp-settings-btn:hover { color: var(--accent); background: var(--surface-hover); }
.sp-save-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    padding: 10px 16px;
    border-radius: var(--radius-sm);
    border: none;
    background: var(--accent);
    color: var(--text-on-accent);
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    transition: background .15s, transform .1s;
}
.sp-save-btn:hover { background: var(--accent-hover); }
.sp-save-btn:active { transform: scale(0.97); }
/* Stage player volume — the single horizontal slider (the now-playing volume,
   matching the preview): speaker icon + gold-filled track + % readout. The gold
   fill is a left→right gradient driven inline by --sp-vol-fill (0..100). */
.sp-vol-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 0;
    max-width: 320px;
    margin-left: auto;
    margin-right: auto;
}
.sp-vol-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    min-width: 20px;
    height: 20px;
    color: var(--text-secondary);
}
.sp-vol-icon svg { width: 20px; height: 20px; }
.sp-vol-row input[type="range"] {
    -webkit-appearance: none;
    flex: 1;
    height: 5px;
    /* Gold fill up to the current value, sunken track after it. */
    background: linear-gradient(
        to right,
        var(--accent) 0%,
        var(--accent) var(--sp-vol-fill, 50%),
        var(--sunken) var(--sp-vol-fill, 50%),
        var(--sunken) 100%
    );
    border-radius: 3px;
    outline: none;
}
.sp-vol-row input[type="range"]::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 16px; height: 16px;
    border-radius: 50%;
    background: var(--accent);
    border: 2px solid var(--text-on-accent);
    cursor: pointer;
    box-shadow: 0 1px 4px rgba(0,0,0,0.3);
    transition: transform .1s ease, box-shadow .15s;
}
.sp-vol-row input[type="range"]:hover::-webkit-slider-thumb,
.sp-vol-row input[type="range"]:active::-webkit-slider-thumb {
    transform: scale(1.18);
    box-shadow: var(--glow-sm);
}

/* ── Canvas ── */
canvas {
    display: block;
    max-width: 100%;
}

/* ── Utilities ── */
.flex { display: flex; }
.flex-col { flex-direction: column; }
.items-center { align-items: center; }
.justify-between { justify-content: space-between; }
.gap-4 { gap: 4px; }
.gap-8 { gap: 8px; }
.gap-12 { gap: 12px; }
.gap-16 { gap: 16px; }
.mt-4 { margin-top: 4px; }
.mt-8 { margin-top: 8px; }
.mt-12 { margin-top: 12px; }
.mt-16 { margin-top: 16px; }
.mb-4 { margin-bottom: 4px; }
.mb-8 { margin-bottom: 8px; }
.mb-12 { margin-bottom: 12px; }
.text-sm { font-size: 12px; }
.text-muted { color: var(--text-secondary); }
.text-mono { font-family: 'SF Mono', 'Cascadia Code', monospace; }
.text-center { text-align: center; }
.w-full { width: 100%; }
.hidden { display: none !important; }
.truncate { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

/* ── Scrollbar ── */
::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--text-muted); }

/* ── Audio Page ── */
.eq-band-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
}
.eq-band-row .eq-freq {
    min-width: 42px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-align: right;
    flex-shrink: 0;
}
.eq-band-row input[type="range"] {
    flex: 1;
    margin: 0;
}
.eq-band-row .eq-gain {
    min-width: 42px;
    font-size: 11px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    text-align: right;
    flex-shrink: 0;
}
.drc-param-row {
    display: flex;
    align-items: center;
    gap: 6px;
}
.drc-param-row label {
    min-width: 48px;
    font-size: 10px;
    margin-bottom: 0;
    flex-shrink: 0;
}
.drc-param-row input[type="range"] {
    flex: 1;
    margin: 0;
}
.drc-param-row .drc-val {
    min-width: 36px;
    font-size: 10px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    text-align: right;
    flex-shrink: 0;
}
.eq-detail {
    border-top: 1px solid var(--border);
    padding-top: 12px;
    margin-top: 8px;
}
.eq-detail-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
}
.eq-detail-band-label {
    font-size: 14px;
    font-weight: 600;
    color: var(--accent);
}
.eq-param-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 0;
}
.eq-param-row label {
    min-width: 36px;
    font-size: 11px;
    margin-bottom: 0;
    flex-shrink: 0;
}
.eq-param-row input[type="range"] {
    flex: 1;
    margin: 0;
}
.eq-param-val {
    min-width: 52px;
    font-size: 11px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    text-align: right;
    flex-shrink: 0;
    color: var(--text-secondary);
}

/* ── Animations ── */
@keyframes fadeIn { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
.fade-in { animation: fadeIn .3s ease forwards; }

/* ── Spinner ── */
@keyframes spin { to { transform: rotate(360deg); } }
.spinner {
    display: inline-block;
    width: 16px; height: 16px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin .8s linear infinite;
    vertical-align: middle;
}
.spinner-lg { width: 24px; height: 24px; border-width: 3px; }

/* ── Result Banner ── */
.result-banner {
    padding: 10px 16px;
    border-radius: var(--radius-sm);
    font-size: 13px;
    font-weight: 500;
    margin-bottom: 12px;
    animation: fadeIn .2s ease;
}
.result-banner.success {
    background: rgba(63,185,80,0.12);
    border: 1px solid rgba(63,185,80,0.3);
    color: var(--green);
}
.result-banner.error {
    background: rgba(248,81,73,0.12);
    border: 1px solid rgba(248,81,73,0.3);
    color: var(--red);
}

/* ── Signal Bars ── */
.signal-bars {
    display: inline-flex; align-items: flex-end; gap: 2px; height: 16px;
}
.signal-bar {
    width: 3px; border-radius: 1px;
    background: var(--border);
    transition: background .2s;
}
.signal-bar.active { background: var(--green); }

/* ── Small Select (developer tools) ── */
.select-sm {
    padding: 4px 8px;
    font-size: 12px;
    border-radius: 6px;
    background: var(--bg-input);
    color: var(--text);
    border: 1px solid var(--border);
    outline: none;
}
.select-sm:focus { border-color: var(--accent); }

/* ── Backdrop Blur on Panel Overlay ── */
@supports (backdrop-filter: blur(12px)) {
    .panel-overlay.open {
        backdrop-filter: blur(12px);
        -webkit-backdrop-filter: blur(12px);
    }
}

/* ── Button Press Micro-Animation ── */
.btn:active, .tab-btn:active { transform: scale(0.95); transition: transform 100ms ease; }

/* ── Card Stagger Animation ── */
@keyframes fadeInCard {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
}
.card:nth-child(1) { animation: fadeInCard 0.25s ease both; }
.card:nth-child(2) { animation: fadeInCard 0.25s ease 0.04s both; }
.card:nth-child(3) { animation: fadeInCard 0.25s ease 0.08s both; }
.card:nth-child(4) { animation: fadeInCard 0.25s ease 0.12s both; }
.card:nth-child(5) { animation: fadeInCard 0.25s ease 0.16s both; }
.card:nth-child(6) { animation: fadeInCard 0.25s ease 0.20s both; }

/* ── Page Transitions ── */
.page-exit {
    opacity: 0;
    transition: opacity 120ms ease-out;
}
.page-enter {
    opacity: 0;
    transition: opacity 120ms ease-in;
}
.page-enter-active {
    opacity: 1;
}

/* ── Color Wheel ── */
.color-wheel-wrap {
    display: flex;
    justify-content: center;
    padding: 8px 0;
}

/* ── Animation Designer ── */
.designer-timeline {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    padding: 8px 0;
    scrollbar-width: none;
    -webkit-overflow-scrolling: touch;
}
.designer-timeline::-webkit-scrollbar { display: none; }
.designer-frame {
    flex-shrink: 0;
    width: 48px;
    height: 48px;
    border-radius: var(--radius-sm);
    border: 2px solid var(--border);
    cursor: pointer;
    transition: border-color .15s;
    overflow: hidden;
}
.designer-frame.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.3);
}
.designer-frame:hover { border-color: var(--text-muted); }

/* ── Segmented Control ── */
.seg-control {
    display: inline-flex;
    padding: 2px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 16px;
}
.seg-btn {
    flex: 1; padding: 5px 12px; border: none;
    background: transparent; color: var(--text-secondary);
    font-size: 12px; font-weight: 500; cursor: pointer;
    transition: color .15s, background .15s;
    white-space: nowrap; outline: none; border-radius: 14px;
    -webkit-tap-highlight-color: transparent;
}
.seg-btn:hover { color: var(--text); }
.seg-btn.active { background: var(--accent); color: var(--text-on-accent); }
@media (max-width: 640px) {
    .seg-btn { min-height: 44px; }
}

/* ── Button Variants ── */
.btn-sm { padding: 4px 10px; font-size: 11px; }
.btn-ghost { border-color: transparent; background: transparent; }
.btn-ghost:hover { background: var(--bg-card); border-color: var(--border); }

/* ── Log Output ── */
.log-output {
    font-family: 'SF Mono','Cascadia Code','Consolas', monospace;
    font-size: 11px; line-height: 1.6;
    max-height: 60vh; overflow-y: auto;
    padding: 12px; white-space: pre-wrap; word-break: break-all;
}
.log-line-error { color: var(--red); }
.log-line-warn  { color: var(--orange); }
.log-line-info  { color: var(--green); }
.log-line-debug { color: var(--text-muted); }
.log-line-trace { color: var(--text-tertiary); }

/* ── Form Field Wrapper ── */
.field { margin-bottom: 12px; }
.field > label { display: block; font-size: 12px; font-weight: 500;
    color: var(--text-secondary); margin-bottom: 4px; }

/* ── Branding ── */
.about-logo {
    text-align: center;
    padding: 20px 0 24px;
}
.about-logo img {
    max-width: 280px;
    width: 100%;
    height: auto;
    object-fit: contain;
}

/* ── Connect Page (standalone app) ── */
.connect-page {
    display: flex; flex-direction: column; align-items: center;
    justify-content: center; min-height: 70vh; padding: 32px 20px;
    text-align: center;
}
.connect-logo-spacer { height: 160px; }
.connect-title { font-size: 22px; font-weight: 500; margin-bottom: 8px; }
.connect-subtitle { font-size: 14px; color: var(--text-secondary); margin-bottom: 28px; max-width: 320px; }
.connect-input-row { display: flex; gap: 8px; width: 100%; max-width: 380px; margin-bottom: 12px; }
.connect-input { flex: 1; font-size: 16px; }
.connect-status { font-size: 13px; color: var(--text-secondary); min-height: 20px; margin-bottom: 8px; }
.connect-status.error { color: var(--red); }
.connect-discovery { width: 100%; max-width: 380px; margin-top: 8px; }
.connect-divider {
    display: flex; align-items: center; gap: 12px; margin: 16px 0;
    font-size: 13px; color: var(--text-muted);
}
.connect-divider::before, .connect-divider::after {
    content: ''; flex: 1; height: 1px; background: var(--border);
}
.connect-scan-btn { width: 100%; }
.connect-results { margin-top: 12px; }
.connect-result-item {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 12px; background: var(--bg-card); border-radius: var(--radius-sm);
    margin-bottom: 6px; border: 1px solid var(--border);
}
.connect-result-name { font-size: 14px; text-align: left; }
.connect-no-scan { font-size: 13px; line-height: 1.5; padding: 8px 0; }
.btn-small { font-size: 12px; padding: 4px 12px; }

/* ── Settings hub ── */
/* Pinned so the back button / title stays reachable while a long settings
   sub-page scrolls. Opaque background masks content sliding underneath. */
.settings-header {
    position: sticky; top: 0; z-index: 5;
    background: var(--bg);
    /* Mask the content scrollport's top padding so nothing peeks above the
       pinned bar (the band is clipped by the scroll container's overflow). */
    box-shadow: 0 -16px 0 var(--bg);
    max-width: 640px; margin: 0 auto; padding-bottom: 16px;
}
.settings-title { font-size: 22px; font-weight: 500; line-height: 1.2; }
.settings-back {
    background: none; border: none; color: var(--text); cursor: pointer;
    font-size: 20px; font-weight: 500;
    /* 44px tap target (WCAG 2.5.5); the negative margin keeps the text aligned
       with the content column while the hit area extends left and down. */
    display: inline-flex; align-items: center; min-height: 44px;
    padding: 8px 12px; margin-left: -12px;
}
.settings-back:hover { color: var(--accent); }
.settings-list { display: flex; flex-direction: column; gap: 8px; max-width: 640px; margin: 0 auto; }
.settings-row {
    display: flex; align-items: center; gap: 12px;
    width: 100%; padding: 16px; border-radius: var(--radius-sm);
    background: var(--bg-card); color: var(--text); border: 1px solid var(--border);
    font-size: 15px; text-align: left; cursor: pointer;
}
.settings-row:hover { background: var(--bg-card-hover); }
.settings-row-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.settings-row-topline { display: flex; align-items: center; gap: 8px; min-width: 0; }
.settings-row-label { font-size: 15px; }
.settings-row-desc { font-size: 12px; color: var(--text-secondary); }
.settings-row-chev { color: var(--text-muted); font-size: 20px; }
.badge-experimental {
    font-size: 10px; text-transform: uppercase; letter-spacing: 1px;
    padding: 3px 8px; border-radius: 999px;
    background: var(--bg-input); color: var(--text-secondary); border: 1px solid var(--border);
}
.settings-body { max-width: 640px; margin: 0 auto; }

/* ── Primary navigation ── */
.primary-nav {
    display: flex;
    justify-content: center;
    gap: 6px;
    margin: 0 16px 14px;
}
.nav-item {
    display: flex; align-items: center; gap: 8px;
    background: none; border: none; cursor: pointer;
    color: var(--text-secondary);
    font-size: 14px; font-weight: 500;
    padding: 8px 18px; border-radius: 22px;
    transition: background .2s ease, color .2s ease;
}
.nav-item:hover { color: var(--text); background: var(--bg-card); }
.nav-item.active { color: var(--accent); background: var(--bg-card); }
.nav-icon { display: inline-flex; }
.nav-icon svg { width: 20px; height: 20px; display: block; }
.nav-label { white-space: nowrap; }

@media (max-width: 640px) {
    .primary-nav {
        position: fixed; left: 0; right: 0; bottom: 0; z-index: 50;
        margin: 0; gap: 0;
        justify-content: space-around;
        background: var(--bg-card);
        border-top: 1px solid var(--border);
        padding: 6px 4px calc(6px + env(safe-area-inset-bottom, 0px));
    }
    .nav-item {
        flex: 1; flex-direction: column; gap: 3px;
        font-size: 10px; padding: 4px 2px; border-radius: 10px;
    }
    .nav-item:hover, .nav-item.active { background: none; }
    .nav-icon svg { width: 23px; height: 23px; }
    .content { padding-bottom: 66px; }
}

/* Desktop left rail: above the mobile bottom-bar breakpoint, the primary nav
   leaves the #app flex column entirely and becomes a fixed, full-height vertical
   rail pinned to the left edge. Because it is out of flow it occupies no vertical
   space in the column, so the header/logo/content fill from the top instead of
   being pushed down by the rail's height. The in-flow shell (header, logo,
   reconnect bar, content, mini bar) is shifted right by the rail width so nothing
   sits underneath the fixed rail. */
@media (min-width: 641px) {
    .primary-nav {
        position: fixed; top: 0; left: 0; bottom: 0; height: 100dvh; z-index: 40;
        flex-direction: column; justify-content: center; align-items: center;
        gap: 4px; margin: 0; width: 96px; padding: 16px 10px;
        background: var(--surface);
        border-right: 1px solid var(--border);
        overflow-y: auto;
    }
    .primary-nav .nav-item {
        flex-direction: column; gap: 4px; width: 100%;
        padding: 10px 4px; font-size: 11px; border-radius: var(--radius-sm);
        text-align: center;
    }
    .primary-nav .nav-icon svg { width: 22px; height: 22px; }
    /* Clear the fixed rail: in-flow shell children start to the right of it. */
    .header { padding-left: calc(16px + 96px); }
    /* The fixed header logo sits top-left over the content band; give every page
       a little top clearance so the leading element (eyebrow / settings title /
       Stage SOURCE label) is never tucked under it. Applied on the common
       container so all surfaces clear the logo, not just Stage. Small enough that
       content still starts near the top. */
    #content { margin-left: 96px; padding-top: var(--space-3); }
    #reconnect-bar { margin-left: 96px; }
}

/* ── Mini now-playing bar (app-shell scope) ── */
.mini-nowplaying {
    display: none; align-items: center; gap: 10px;
    width: 100%; max-width: 560px; margin: 0 auto 10px;
    padding: 8px 12px; cursor: pointer; text-align: left;
    background: var(--surface); color: var(--text);
    border: 1px solid var(--border); border-radius: var(--radius-pill);
    box-shadow: var(--shadow);
}
.mini-art {
    width: 40px; height: 40px; border-radius: 8px;
    object-fit: cover; flex-shrink: 0; background: var(--surface-2);
}
.mini-meta { display: flex; align-items: center; gap: 10px; min-width: 0; flex: 1; }
.mini-source { color: var(--accent); font-size: 16px; flex-shrink: 0; }
.mini-text { min-width: 0; }
.mini-title {
    font-size: 13px; font-weight: 500; color: var(--text);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.mini-artist {
    font-size: 11px; color: var(--text-secondary);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.mini-playpause {
    flex-shrink: 0; width: 40px; height: 40px; min-width: 40px;
    border-radius: 50%; border: none; cursor: pointer;
    background: var(--accent); color: var(--text-on-accent);
    font-size: 15px; display: flex; align-items: center; justify-content: center;
}
.mini-playpause:hover { background: var(--accent-hover); }
@media (max-width: 640px) {
    .mini-nowplaying {
        position: fixed; left: 8px; right: 8px; z-index: 49;
        width: auto; margin: 0;
        bottom: calc(62px + env(safe-area-inset-bottom, 0px));
    }
}
@media (min-width: 641px) {
    .mini-nowplaying { margin-left: calc(96px + 16px); margin-right: 16px; }
}

/* ── Home ── */
.home-glance {
    display: flex; align-items: center; justify-content: space-between;
    gap: 12px; max-width: 480px; margin: 0 auto 14px;
    padding: 12px 16px; background: var(--bg-card);
    border: 1px solid var(--border); border-radius: var(--radius-sm);
}
.home-glance-chips { display: flex; gap: 16px; flex-wrap: wrap; }
.home-chip { font-size: 12px; color: var(--text-secondary); white-space: nowrap; }
.home-glance-link {
    background: none; border: none; color: var(--accent);
    font-size: 12px; font-weight: 500; cursor: pointer; flex-shrink: 0;
}

/* ── Stage (the theatre) ──
   A single centered vertical column on all widths: gold-on-warm-black. Top to
   bottom: Source Tuner dial, the Ovation bloom hero, centered track meta, time
   row, transport, the player-volume slider, then the slim status/quick strip.
   Capped to a comfortable column on desktop so it reads as a stage, not a grid. */
.stage {
    width: 100%;
    max-width: 480px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
}
@media (min-width: 641px) {
    /* Top clearance for the fixed header logo now lives on #content (applies to
       every page), so Stage only needs its wider desktop column here. */
    .stage { max-width: 720px; }
}
/* The Tuner host owns the source-icon row + the backlit dial bar. */
.stage-tuner {
    position: relative;
    width: 100%;
    max-width: 100%;
    margin: 0 auto var(--space-3);
}

/* ── Source Tuner: a horizontal backlit dial bar ── */
/* A source-icon row stacked above a wide rounded-rect dial: a dark warm inset
   panel with a faint vertical tick texture, three evenly-spaced station labels,
   and a glowing gold needle that glides over the active station. */
.tuner {
    position: relative;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: var(--space-2);
}

/* Source-icon row: three small icons mirroring the stations, active one gold. */
.tuner-icons {
    display: flex;
    justify-content: space-around;
    padding: 0 12px;
}
.tuner-icon {
    font-size: 15px;
    line-height: 1;
    color: var(--text-muted);
    opacity: 0.55;
    transition: color .2s, opacity .2s, text-shadow .2s;
}
.tuner-icon.active {
    color: var(--accent);
    opacity: 1;
    text-shadow: var(--glow-sm);
}

/* The dial bar itself: a backlit rounded-rect with an inset warm gradient. */
.tuner-bar {
    position: relative;
    width: 100%;
    height: 44px;
    border-radius: var(--radius-pill);
    background: linear-gradient(180deg, var(--sunken) 0%, var(--surface-2) 100%);
    border: 1px solid var(--border-strong);
    box-shadow: inset 0 1px 4px rgba(0,0,0,0.55), inset 0 0 0 1px rgba(0,0,0,0.25);
    overflow: hidden;
}
/* Faint vertical tick lines across the dial face (decorative texture). */
.tuner-ticks {
    position: absolute;
    inset: 0;
    pointer-events: none;
    background-image: repeating-linear-gradient(
        90deg,
        rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.10) 0px,
        rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.10) 1px,
        transparent 1px,
        transparent 10px
    );
    -webkit-mask-image: linear-gradient(90deg, transparent, #000 12%, #000 88%, transparent);
    mask-image: linear-gradient(90deg, transparent, #000 12%, #000 88%, transparent);
}
/* The glowing gold needle, centered over the active station band. The left
   position is set inline by tuner::render; the transition does the glide. */
.tuner-needle {
    position: absolute;
    top: 4px;
    bottom: 4px;
    left: 50%;
    width: 3px;
    margin-left: -1.5px;
    border-radius: 2px;
    background: var(--accent-glow);
    box-shadow: 0 0 8px 2px rgba(var(--accent-r),var(--accent-g),var(--accent-b),0.65),
                0 0 2px 1px var(--accent-glow);
    pointer-events: none;
    transition: left .36s cubic-bezier(.22,.61,.36,1);
}
/* Station labels laid across the bar; each label owns an even band. */
.tuner-stations {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: space-around;
}
.tuner-station {
    flex: 1;
    background: none;
    border: none;
    height: 100%;
    color: var(--text-muted);
    font-family: var(--font-mono, ui-monospace, 'SF Mono', 'Cascadia Code', Consolas, monospace);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    cursor: pointer;
    transition: color .2s, text-shadow .2s;
    -webkit-tap-highlight-color: transparent;
}
.tuner-station:hover { color: var(--text-secondary); }
/* Active station: gold + brighter + a soft glow, so the active source is
   signalled by more than position (WCAG 1.4.1). */
.tuner-station.active {
    color: var(--accent-glow);
    text-shadow: var(--glow-sm);
}
@media (prefers-reduced-motion: reduce) {
    .tuner-needle { transition: none; }
}

/* ── Ovation bloom (the now-playing hero) ── */
/* ovation::render fixes the stack at 280x280 inline; this centers and lifts it
   and styles the layered children (album img, explicit badge, progress fallback
   ring). The canvas + seek overlay are positioned inline by the renderer. */
.ovation {
    margin: var(--space-2) auto var(--space-4) !important;
}
.ovation-art {
    background: var(--surface-2);
    box-shadow: var(--shadow);
}
/* Circular album disc behind the <img>: shows a music-note fallback glyph when
   there is no cover art (idle hero). The img on top covers it when art loads. */
.ovation-disc {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 150px;
    height: 150px;
    transform: translate(-50%,-50%);
    border-radius: 50%;
    background: radial-gradient(circle at 50% 42%, var(--surface-hover) 0%, var(--sunken) 100%);
    box-shadow: var(--shadow), inset 0 0 0 1px var(--border-strong);
    display: flex;
    align-items: center;
    justify-content: center;
}
.ovation-disc::after {
    content: "\266B"; /* ♫ music-note fallback glyph */
    font-size: 46px;
    line-height: 1;
    color: var(--accent);
    opacity: 0.5;
}
/* Conic-gradient progress fallback ring (only paints when the 2D canvas is
   unavailable; the canvas arc is the primary). Sits behind the canvas. */
#sp-progress {
    position: absolute;
    inset: 0;
    border-radius: 50%;
    pointer-events: none;
}
/* Explicit badge pinned to the album center. */
.sp-explicit {
    position: absolute;
    left: 50%;
    bottom: 18%;
    transform: translateX(-50%);
    z-index: 2;
    padding: 1px 5px;
    border-radius: var(--radius-sm);
    background: var(--text-muted);
    color: var(--text-on-accent);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.04em;
}

/* ── Now-playing card on the Stage (the Ovation column) ── */
/* spotify::render builds a `.card.glass-card.sp-card`; on the Stage it should be
   transparent so the theatre reads as one column, not a boxed card. */
.sp-card {
    background: transparent;
    border: none;
    box-shadow: none;
    padding: 0;
}
.sp-card:hover { background: transparent; box-shadow: none; }
@supports (backdrop-filter: blur(8px)) {
    .sp-card { backdrop-filter: none; -webkit-backdrop-filter: none; }
}
/* Idle hero (no track): the full composition still renders, but the transport
   and player volume read as dimmed/disabled. The Ovation bloom, meta prompt,
   and time row stay at full strength so the Stage matches the preview. */
.sp-card.sp-idle .sp-transport,
.sp-card.sp-idle .sp-vol-row {
    opacity: 0.4;
    pointer-events: none;
}
.sp-card.sp-idle .sp-time-row { opacity: 0.6; }

/* Centered track meta. */
.sp-meta { margin-top: var(--space-3); }
.sp-title {
    font-size: 18px;
    font-weight: 600;
    color: var(--text);
    line-height: 1.3;
}
.sp-artist {
    font-size: 14px;
    color: var(--text-secondary);
    margin-top: 2px;
}
.sp-album {
    font-size: 12px;
    color: var(--text-tertiary);
}

/* Time row — mono tabular readouts flanking the transport. */
.sp-time-row {
    margin-top: var(--space-3);
    font-family: var(--font-mono, ui-monospace, 'SF Mono', 'Cascadia Code', Consolas, monospace);
    font-variant-numeric: tabular-nums;
    color: var(--text-tertiary);
}

/* ── Status / quick strip ── */
.stage-strip {
    margin-top: var(--space-4);
}
/* Master quick-chip — the everyday "make it louder" without leaving the Stage. */
.stage-master-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin: var(--space-1) 0 var(--space-4);
    padding: 8px 14px;
    min-height: 40px;
    border-radius: var(--radius-chip);
    border: 1px solid var(--border-strong);
    background: var(--surface-2);
    color: var(--text);
    font-size: 13px;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
    cursor: pointer;
    transition: color .15s, background .15s, border-color .15s;
    -webkit-tap-highlight-color: transparent;
}
.stage-master-chip:hover {
    color: var(--accent);
    border-color: var(--accent);
    background: var(--surface-hover);
}
.stage-master-chip:active { transform: scale(0.97); }

/* ── Inline help marker ── */
.hint {
    display: inline-flex; align-items: center; justify-content: center;
    width: 15px; height: 15px; margin-left: 6px;
    border-radius: 50%; font-size: 9px; font-weight: 500;
    color: var(--text-muted); border: 1px solid var(--border);
    cursor: help; vertical-align: middle; user-select: none;
}
.hint:hover { color: var(--accent); border-color: var(--accent); }

/* ── Accessibility ── */
/* Keyboard focus ring (mouse clicks don't trigger :focus-visible). The many
   `outline: none` rules above strip focus for everyone; restore it for keyboard
   users with !important so the ring always wins. */
/* Two-tone focus ring: gold outer outline (survives overflow:hidden clipping)
   plus a contrasting inner box-shadow stroke, so one half always contrasts on
   gold-filled controls (play button, active tab, seg-btn.active). */
:focus-visible {
    outline: 2px solid var(--focus-ring-outer) !important;
    outline-offset: 2px !important;
    box-shadow: inset 0 0 0 2px var(--focus-ring-inner) !important;
}
/* Let the ring escape containers that own the active controls. */
.seg-control, .sp-transport { overflow: visible; }
/* Tactile press feedback (disabled under reduced-motion below). */
.btn:active, .seg-btn:active, .settings-row:active, .nav-item:active, .home-glance-link:active {
    transform: scale(0.97);
    transition: transform 0.05s ease;
}
/* Reconnecting bar: shown (display:block, set inline) only while the link is down. */
.reconnect-bar {
    padding: 7px 12px;
    text-align: center;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.02em;
    background: var(--orange);
    color: var(--text-on-accent);
    animation: reconnect-pulse 1.6s ease-in-out infinite;
}
@keyframes reconnect-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.65; }
}
@media (prefers-reduced-motion: reduce) {
    *, *::before, *::after {
        animation-duration: 0.001ms !important;
        animation-iteration-count: 1 !important;
        transition-duration: 0.001ms !important;
        scroll-behavior: auto !important;
    }
}
"#;
