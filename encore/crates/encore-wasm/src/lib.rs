//! Encore web dashboard — WASM single-page application.
//!
//! Renders a real-time dashboard for the Harman Kardon Invoke speaker: system
//! telemetry, Spotify player, audio controls (EQ/DRC), LED ring,
//! network configuration, and Bluetooth management. Communicates
//! with the firmware via JSON WebSocket.

mod app;
mod boot;
pub mod brand;
mod components;
mod dom;
mod graphics;
mod haptic;
mod pages;
mod panels;
mod router;
mod state;
mod style;
mod theme;
mod ws;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    // Reflect the persisted theme onto <html> BEFORE injecting CSS so the first
    // paint matches the chosen theme (no flash).
    theme::init();
    style::inject();
    boot::run();
}
