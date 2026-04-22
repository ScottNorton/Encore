//! WebSocket handler for real-time messaging.
//!
//! Text frames carry JSON-serialized ServerMsg (server->client)
//! and ClientMsg (client->server). Binary frames still accepted
//! via rkyv for backward compatibility.

use super::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use encore_common::protocol::ClientMsg;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// WebSocket upgrade handler at /ws.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Maximum time to wait for a WebSocket write before dropping the client.
/// If WiFi is congested, we drop the slow client rather than let backpressure
/// stall the tokio runtime (which would starve the watchdog and other tasks).
const WS_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    info!("WebSocket: client connected");
    let mut rx = state.ws_tx.subscribe();

    // Send boot mode on connect so the dashboard knows if we're in safe mode
    {
        let msg = encore_common::protocol::ServerMsg::BootMode { safe_mode: state.safe_mode, boot_source: state.boot_source.clone() };
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = socket.send(Message::Text(json.into())).await;
        }
    }

    // Send cached WiFi connect result if available (client may have missed it
    // during the AP/WiFi transition that drops the WebSocket)
    {
        let cached = state.wifi_result_cache.lock().ok().and_then(|g| g.clone());
        if let Some(result) = cached {
            let msg = encore_common::protocol::ServerMsg::WifiConnectResult(result);
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = socket.send(Message::Text(json.into())).await;
            }
        }
    }

    // Send cached network state so dashboard doesn't show "Loading..."
    {
        let cached = state.network_state_cache.lock().ok().and_then(|g| g.clone());
        if let Some(net_state) = cached {
            let msg = encore_common::protocol::ServerMsg::NetworkChanged(net_state);
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = socket.send(Message::Text(json.into())).await;
            }
        }
    }

    loop {
        tokio::select! {
            // Forward ServerMsg broadcasts to this client (JSON text)
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        // Write with timeout: if WiFi is too slow, disconnect
                        // rather than block the runtime indefinitely
                        let result = tokio::time::timeout(
                            WS_WRITE_TIMEOUT,
                            socket.send(Message::Text(text.into()))
                        ).await;
                        match result {
                            Ok(Err(_)) => break, // write error
                            Err(_) => {
                                warn!("WebSocket: write timeout, dropping slow client");
                                break;
                            }
                            Ok(Ok(())) => {}
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!("WebSocket: client lagged, skipped {} messages", n);
                    }
                    Err(_) => break,
                }
            }
            // Receive ClientMsg from this client
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!("WebSocket: received text: {}", text);
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(client_msg) => {
                                debug!("WebSocket: ClientMsg: {:?}", client_msg);
                                if let Err(e) = state.client_tx.try_send(client_msg) {
                                    warn!("WebSocket: ClientMsg route failed: {}", e);
                                }
                            }
                            Err(e) => {
                                warn!("WebSocket: JSON parse failed: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        // Legacy rkyv binary path
                        debug!("WebSocket: received {} binary bytes", data.len());
                        match rkyv::from_bytes::<ClientMsg, rkyv::rancor::Error>(&data) {
                            Ok(client_msg) => {
                                debug!("WebSocket: ClientMsg (rkyv): {:?}", client_msg);
                                if let Err(e) = state.client_tx.try_send(client_msg) {
                                    warn!("WebSocket: ClientMsg route failed: {}", e);
                                }
                            }
                            Err(e) => {
                                warn!("WebSocket: rkyv deserialize failed: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        debug!("WebSocket: error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    info!("WebSocket: client disconnected");
}
