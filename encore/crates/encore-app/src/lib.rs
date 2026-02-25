use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct Speaker {
    pub name: String,
    pub host: String,
    pub port: u16,
}

#[tauri::command]
async fn discover_speakers() -> Result<Vec<Speaker>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mdns = ServiceDaemon::new().map_err(|e| format!("mDNS init failed: {e}"))?;
        let receiver = mdns
            .browse("_encore._tcp.local.")
            .map_err(|e| format!("mDNS browse failed: {e}"))?;

        let mut speakers = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);

        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            match receiver.recv_timeout(remaining) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let host = info.get_hostname().trim_end_matches('.').to_string();
                    speakers.push(Speaker {
                        name: info.get_fullname().to_string(),
                        host,
                        port: info.get_port(),
                    });
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        let _ = mdns.shutdown();
        Ok(speakers)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

fn build_app() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![discover_speakers])
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
