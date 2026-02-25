//! CSS theme system — injected at startup.
//!
//! Auto light/dark via prefers-color-scheme. CSS custom properties
//! for all colors. Responsive breakpoints at 640px and 960px.

use crate::dom;

/// Inject the complete stylesheet into <head>
pub fn inject() {
    let style = dom::create_el("style");
    style.set_text_content(Some(CSS));
    let doc = dom::document();
    if let Ok(Some(head)) = doc.query_selector("head") {
        head.append_child(&style).unwrap();
    }
}

const CSS: &str = r#"
:root {
    --bg: #0d1117;
    --bg-card: #161b22;
    --bg-card-hover: #1c2128;
    --bg-input: #0d1117;
    --border: #30363d;
    --text: #e6edf3;
    --text-secondary: #8b949e;
    --text-muted: #484f58;
    --accent: #58a6ff;
    --accent-hover: #79c0ff;
    --green: #3fb950;
    --orange: #d29922;
    --red: #f85149;
    --purple: #bc8cff;
    --cyan: #39d2c0;
    --yellow: #e3b341;
    --radius: 12px;
    --radius-sm: 8px;
    --shadow: 0 2px 8px rgba(0,0,0,0.3);
    --nav-height: 56px;
    --tab-height: 44px;
    --header-height: 56px;
}
@media (prefers-color-scheme: light) {
    :root {
        --bg: #ffffff;
        --bg-card: #f6f8fa;
        --bg-card-hover: #eef1f5;
        --bg-input: #ffffff;
        --border: #d0d7de;
        --text: #1f2328;
        --text-secondary: #656d76;
        --text-muted: #8c959f;
        --accent: #0969da;
        --accent-hover: #0550ae;
        --green: #1a7f37;
        --orange: #bf8700;
        --red: #cf222e;
        --purple: #8250df;
        --cyan: #1b7c83;
        --yellow: #9a6700;
        --shadow: 0 2px 8px rgba(0,0,0,0.08);
    }
}
* { margin: 0; padding: 0; box-sizing: border-box; }
html, body {
    height: 100%; width: 100%;
    background: var(--bg);
    color: var(--text);
    font-family: system-ui, -apple-system, 'Segoe UI', sans-serif;
    font-size: 14px;
    line-height: 1.5;
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
/* ── Unified Logo ── */
.app-logo {
    position: fixed;
    z-index: 100;
    display: flex;
    align-items: center;
    pointer-events: none;
    transition: top 0.5s cubic-bezier(.4,0,.2,1),
                left 0.5s cubic-bezier(.4,0,.2,1),
                opacity 0.4s ease;
}
.logo-ring-wrap {
    position: relative;
    flex-shrink: 0;
    transition: width 0.5s cubic-bezier(.4,0,.2,1),
                height 0.5s cubic-bezier(.4,0,.2,1);
}
.logo-ring-container {
    position: absolute;
    inset: 0;
}
.logo-ring-container svg {
    width: 100%; height: 100%;
    display: block;
    animation: logo-spin 25s linear infinite;
}
.logo-ring-container .logo-track {
    stroke: var(--border);
}
.logo-glow {
    position: absolute;
    inset: -5px;
    border-radius: 50%;
    background: radial-gradient(circle, rgba(88,166,255,0.12) 0%, transparent 70%);
    animation: logo-glow 4s ease-in-out infinite;
    pointer-events: none;
}
.logo-speaker-container {
    position: absolute;
    inset: 15%;
}
.logo-speaker-container svg {
    width: 100%; height: 100%;
    display: block;
}
.speaker-dome {
    transition: fill 1s ease;
}
.logo-text {
    font-weight: 300;
    letter-spacing: 4px;
    text-transform: uppercase;
    white-space: nowrap;
    transition: opacity 0.3s ease, font-size 0.5s cubic-bezier(.4,0,.2,1);
}
.logo-status {
    font-size: 12px;
    color: var(--text-muted);
    letter-spacing: 2px;
    opacity: 0;
    transition: opacity 0.3s ease;
}
.logo-connecting .logo-status { opacity: 1; }
@keyframes logo-spin { to { transform: rotate(360deg); } }
@keyframes logo-glow {
    0%, 100% { opacity: 0.3; transform: scale(0.92); }
    50% { opacity: 0.9; transform: scale(1.08); }
}
@media (prefers-color-scheme: light) {
    .logo-glow {
        background: radial-gradient(circle, rgba(9,105,218,0.1) 0%, transparent 70%);
    }
}

/* Loading state: centered, 60px */
.logo-loading {
    top: 50%; left: 50%;
    margin-top: -30px; margin-left: -30px;
    flex-direction: column; gap: 12px;
}
.logo-loading .logo-ring-wrap { width: 60px; height: 60px; }
.logo-loading .logo-text { font-size: 16px; opacity: 0; }

/* Header state: top-left, 32px, inline */
.logo-header {
    top: calc(12px + env(safe-area-inset-top, 0px));
    left: 16px;
    margin: 0;
    flex-direction: row; gap: 10px;
}
.logo-header .logo-ring-wrap { width: 32px; height: 32px; }
.logo-header .logo-text { font-size: 16px; opacity: 1; }

/* Hero state: above connect form, 100px, text below */
.logo-hero {
    top: 80px; left: 50%;
    transform: translateX(-50%);
    margin: 0;
    flex-direction: column; align-items: center; gap: 14px;
}
.logo-hero .logo-ring-wrap { width: 100px; height: 100px; }
.logo-hero .logo-text { font-size: 20px; opacity: 1; }

/* Connecting state: centered like loading, with ring + text + status */
.logo-connecting {
    top: 50%; left: 50%;
    transform: translate(-50%, -50%);
    margin: 0;
    flex-direction: column; align-items: center; gap: 14px;
}
.logo-connecting .logo-ring-wrap { width: 80px; height: 80px; }
.logo-connecting .logo-text { font-size: 20px; opacity: 1; }
.logo-connecting .logo-ring-container svg { animation-duration: 3s; }

.header-device-name {
    font-size: 13px; font-weight: 400;
    color: var(--text-secondary);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    max-width: 200px;
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
    width: 36px; height: 36px;
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
    border: 1px solid var(--border);
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
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 16px;
    margin-bottom: 12px;
    transition: background .2s;
    animation: fadeInUp 0.2s ease;
}
.card:hover { background: var(--bg-card-hover); }
.card-title {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-secondary);
    margin-bottom: 12px;
}
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
.badge-blue { background: rgba(88,166,255,0.15); border-color: rgba(88,166,255,0.3); color: var(--accent); }
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
    color: #fff;
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
    background: rgba(30, 215, 96, 0.08);
    border: 1px solid rgba(30, 215, 96, 0.2);
    border-radius: var(--radius);
    padding: 20px;
    backdrop-filter: blur(8px);
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
.sp-btn:hover { opacity: 0.8; }
.sp-btn:active { transform: scale(0.92); }
.sp-btn.active {
    opacity: 1.0;
    background: rgba(29, 185, 84, 0.15);
}
.sp-btn-play {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 52px;
    height: 52px;
    border-radius: 50%;
    border: none;
    background: #1db954;
    color: #000;
    font-size: 24px;
    cursor: pointer;
    transition: transform .1s, background .15s, box-shadow .15s;
    box-shadow: 0 4px 12px rgba(29, 185, 84, 0.3);
    outline: none;
    -webkit-tap-highlight-color: transparent;
}
.sp-btn-play:hover {
    background: #1ed760;
    transform: scale(1.05);
    box-shadow: 0 6px 16px rgba(29, 185, 84, 0.4);
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
.sp-btn-skip:hover { color: #fff; }
.sp-btn-skip:active { transform: scale(0.92); }
/* Spotify seek bar — touch-friendly */
.sp-seek-track {
    height: 6px;
    background: var(--border);
    border-radius: 3px;
    overflow: visible;
    position: relative;
    transition: height .15s ease;
}
.sp-seek-wrap:hover .sp-seek-track,
.sp-seek-wrap:active .sp-seek-track { height: 8px; }
.sp-seek-fill {
    height: 100%;
    background: var(--text-secondary);
    border-radius: 3px;
    transition: width .3s linear, background .15s;
    position: relative;
}
.sp-seek-wrap:hover .sp-seek-fill,
.sp-seek-wrap:active .sp-seek-fill { background: #1db954; }
.sp-seek-dot {
    position: absolute;
    right: -7px;
    top: 50%;
    transform: translateY(-50%) scale(0);
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--text);
    box-shadow: 0 1px 4px rgba(0,0,0,0.3);
    transition: transform .15s ease, opacity .15s;
    opacity: 0;
}
.sp-seek-wrap:hover .sp-seek-dot,
.sp-seek-wrap:active .sp-seek-dot { opacity: 1; transform: translateY(-50%) scale(1); }
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
.sp-settings-btn:hover { color: var(--text); background: var(--bg-card); }
.sp-save-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    padding: 10px 16px;
    border-radius: var(--radius-sm);
    border: none;
    background: #1db954;
    color: #000;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    transition: background .15s, transform .1s;
}
.sp-save-btn:hover { background: #1ed760; }
.sp-save-btn:active { transform: scale(0.97); }
/* Spotify volume slider — green track fill + white thumb */
.sp-vol-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 0;
}
.sp-vol-row input[type="range"] {
    -webkit-appearance: none;
    flex: 1;
    height: 4px;
    background: var(--border);
    border-radius: 2px;
    outline: none;
}
.sp-vol-row input[type="range"]::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 18px; height: 18px;
    border-radius: 50%;
    background: #fff;
    cursor: pointer;
    box-shadow: 0 1px 4px rgba(0,0,0,0.3);
    transition: background .15s, transform .1s ease;
}
.sp-vol-row input[type="range"]:hover::-webkit-slider-thumb,
.sp-vol-row input[type="range"]:active::-webkit-slider-thumb {
    background: #1db954;
    transform: scale(1.15);
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
.drc-band-col {
    display: flex;
    flex-direction: column;
    gap: 2px;
}
.drc-band-col .drc-band-title {
    font-size: 12px;
    font-weight: 600;
    text-align: center;
    color: var(--text);
    margin-bottom: 4px;
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

/* ── DRC Crossover ── */
.drc-crossover {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 0;
    font-size: 12px;
    color: var(--text-secondary);
}
.drc-crossover input[type="range"] { flex: 1; }

/* ── Backdrop Blur on Panel Overlay ── */
.panel-overlay.open {
    backdrop-filter: blur(12px);
    -webkit-backdrop-filter: blur(12px);
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
    box-shadow: 0 0 0 2px rgba(88,166,255,0.3);
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
.seg-btn.active { background: var(--accent); color: #fff; }

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
.log-line-trace { color: rgba(139,148,158,0.5); }

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
"#;
