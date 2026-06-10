//! Encore web dashboard — WASM single-page application.
//!
//! Renders a real-time dashboard for the Harman Kardon Invoke speaker: system
//! telemetry, Spotify player, audio controls (EQ/DRC), LED ring,
//! network configuration, and Bluetooth management. Communicates
//! with the firmware via JSON WebSocket.

mod app;
mod boot;
mod dom;
mod state;
mod style;
mod ws;
mod pages;
mod panels;
mod graphics;
mod components;
mod haptic;
pub mod brand;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    style::inject();
    boot::run();
}
