use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct Speaker {
    pub name: String,
    pub host: String,
    pub port: u16,
}

/// Discover speakers via mDNS (3s scan).
#[tauri::command]
async fn discover_speakers() -> Result<Vec<Speaker>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut speakers = Vec::new();

        if let Ok(mdns) = ServiceDaemon::new() {
            if let Ok(receiver) = mdns.browse("_encore._tcp.local.") {
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                loop {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match receiver.recv_timeout(deadline - now) {
                        Ok(ServiceEvent::ServiceResolved(info)) => {
                            // Use resolved IP for reliable WebSocket connection
                            // (.local hostnames may not resolve in WebView2)
                            let host = info
                                .get_addresses()
                                .iter()
                                .next()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| {
                                    info.get_hostname().trim_end_matches('.').to_string()
                                });
                            let name = info.get_hostname().trim_end_matches('.').to_string();
                            if !speakers.iter().any(|s: &Speaker| s.host == host) {
                                speakers.push(Speaker {
                                    name,
                                    host,
                                    port: info.get_port(),
                                });
                            }
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            }
            let _ = mdns.shutdown();
        }

        Ok(speakers)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

fn build_app() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default().invoke_handler(tauri::generate_handler![discover_speakers])
}

pub fn run() {
    build_app()
        .run(tauri::generate_context!())
        .expect("error while running Encore app");
}

#[cfg(mobile)]
#[tauri::mobile_entry_point]
fn mobile_main() {
    build_app()
        .run(tauri::generate_context!())
        .expect("error while running Encore app");
}
