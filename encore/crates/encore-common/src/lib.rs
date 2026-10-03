//! Shared types for the Encore firmware and web dashboard.
//!
//! Defines the WebSocket protocol (ClientMsg/ServerMsg), configuration
//! file schema, and subsystem status tracking. Used by both the firmware
//! binary (encore-firmware) and the WASM dashboard (encore-wasm).

pub mod avdtp;
pub mod avrcp;
pub mod config;
pub mod dsp;
pub mod protocol;
pub mod sdp;
pub mod status;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
