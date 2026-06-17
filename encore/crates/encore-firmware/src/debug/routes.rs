//! REST API routes for hardware debug tools.
//!
//! All endpoints are under `/api/debug/` and operate on DebugState.
//! Each operation runs with a timeout — if hardware hangs, the HTTP
//! request times out and you get an error response, not a hung connection.

use super::DebugState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;

/// Build the debug API router. Nest this under `/api/debug` in the main app.
pub fn router() -> Router<Arc<DebugState>> {
    Router::new()
        // GPIO
        .route("/gpio/init", post(gpio_init))
        .route("/gpio/deinit", post(gpio_deinit))
        .route("/gpio/state", get(gpio_state))
        .route("/gpio/pin/{pin}", get(gpio_pin_read))
        .route("/gpio/pin/{pin}/export", post(gpio_pin_export))
        .route("/gpio/pin/{pin}/unexport", post(gpio_pin_unexport))
        // UART
        .route("/uart/probe", post(uart_probe))
        .route("/uart/send", post(uart_send))
        .route("/uart/close", post(uart_close))
        .route("/uart/status", get(uart_status))
}

// ── GPIO handlers ───────────────────────────────────────────────────────

async fn gpio_init(State(state): State<Arc<DebugState>>) -> impl IntoResponse {
    match super::gpio::init_all(state.gpio.clone()).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => error_response(e),
    }
}

async fn gpio_deinit(State(state): State<Arc<DebugState>>) -> impl IntoResponse {
    match super::gpio::deinit_all(state.gpio.clone()).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => error_response(e),
    }
}

async fn gpio_state(State(state): State<Arc<DebugState>>) -> impl IntoResponse {
    match super::gpio::read_state(state.gpio.clone()).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => error_response(e),
    }
}

async fn gpio_pin_read(Path(pin): Path<u32>) -> impl IntoResponse {
    match super::gpio::read_pin(pin).await {
        Ok(info) => (StatusCode::OK, Json(info)).into_response(),
        Err(e) => error_response(e),
    }
}

async fn gpio_pin_export(Path(pin): Path<u32>) -> impl IntoResponse {
    match super::gpio::export_pin(pin).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"ok": true, "pin": pin})),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

async fn gpio_pin_unexport(Path(pin): Path<u32>) -> impl IntoResponse {
    match super::gpio::unexport_pin(pin).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"ok": true, "pin": pin})),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

// ── UART handlers ───────────────────────────────────────────────────────

async fn uart_probe(State(state): State<Arc<DebugState>>) -> impl IntoResponse {
    match super::uart::probe(state.uart.clone()).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct UartSendRequest {
    command: String,
}

async fn uart_send(
    State(state): State<Arc<DebugState>>,
    Json(body): Json<UartSendRequest>,
) -> impl IntoResponse {
    match super::uart::send(state.uart.clone(), body.command).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => error_response(e),
    }
}

async fn uart_close(State(state): State<Arc<DebugState>>) -> impl IntoResponse {
    match super::uart::close(state.uart.clone()).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => error_response(e),
    }
}

async fn uart_status(State(state): State<Arc<DebugState>>) -> impl IntoResponse {
    let status = super::uart::status(state.uart.clone()).await;
    (StatusCode::OK, Json(status)).into_response()
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn error_response(e: anyhow::Error) -> axum::response::Response {
    let msg = format!("{:#}", e);
    tracing::warn!("Debug API error: {}", msg);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"ok": false, "error": msg})),
    )
        .into_response()
}
