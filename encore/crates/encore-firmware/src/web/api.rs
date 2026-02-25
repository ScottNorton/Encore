//! REST API endpoints.
//!
//! Read-only system metrics from /proc. Config management via TOML.
//! These endpoints work independently of inter-subsystem channels.

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::Json;
use encore_common::config::EncoreConfigFile;
use encore_common::protocol::{CpuCoreSnapshot, DiskUsage, NetInterfaceSnapshot};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

const CONFIG_PATH: &str = "/lsync/encore/config.toml";
const OTA_NEXT_PATH: &str = "/lsync/encore/encore_next";
pub const MAX_UPDATE_SIZE: usize = 16 * 1024 * 1024; // 16 MB
const FIRMWARE_STAGING_PATH: &str = "/run/firmware_upload.squashfs";
const ROOTFS_MAX: usize = 84_824_064; // 84.8 MB — NAND partition limit

/// GET /api/setup — returns setup mode status.
/// Dashboard uses this to redirect to the setup wizard on first boot.
pub async fn setup_handler() -> Json<Value> {
    let needs_setup = !std::path::Path::new(CONFIG_PATH).exists();
    Json(json!({ "setup_required": needs_setup }))
}

/// POST /api/setup/complete — finalize first-boot setup.
/// Creates the config file with device name and AP keep-alive preference.
pub async fn setup_complete_handler(
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let device_name = body.get("device_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Encore")
        .to_string();
    let ap_keep_alive = body.get("ap_keep_alive")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    info!("Setup: completing (name={}, ap_keep_alive={})", device_name, ap_keep_alive);

    // Load existing config (may have WiFi creds from SetWifi during setup)
    let path = std::path::Path::new(CONFIG_PATH);
    let mut cfg = EncoreConfigFile::load(path).unwrap_or_default();
    cfg.device.name = device_name;
    cfg.network.ap_keep_alive = ap_keep_alive;

    match cfg.save(path) {
        Ok(()) => {
            info!("Setup: config saved to {}", CONFIG_PATH);
            (StatusCode::OK, Json(json!({ "status": "complete" })))
        }
        Err(e) => {
            warn!("Setup: config save failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save failed: {}", e) })),
            )
        }
    }
}

/// GET /api/system — system metrics from procfs.
pub async fn system_handler() -> Json<Value> {
    debug!("API: /api/system");
    Json(json!({
        "uptime": read_uptime(),
        "cpu": read_cpu(),
        "memory": read_memory(),
        "load": read_loadavg(),
        "firmware": encore_common::VERSION,
    }))
}

/// GET /api/config — read config file.
pub async fn config_handler() -> Json<Value> {
    debug!("API: /api/config");
    match std::fs::read_to_string(CONFIG_PATH) {
        Ok(content) => {
            match toml::from_str::<toml::Value>(&content) {
                Ok(val) => Json(json!({ "config": val })),
                Err(e) => Json(json!({ "error": format!("parse error: {}", e) })),
            }
        }
        Err(_) => {
            // No config file yet — return defaults
            Json(json!({ "config": {} }))
        }
    }
}

/// POST /api/config — save config file.
pub async fn config_save_handler(
    Json(config): Json<EncoreConfigFile>,
) -> (StatusCode, Json<Value>) {
    let path = std::path::Path::new(CONFIG_PATH);
    match config.save(path) {
        Ok(()) => {
            info!("API: config saved to {}", CONFIG_PATH);
            (StatusCode::OK, Json(json!({ "status": "saved" })))
        }
        Err(e) => {
            warn!("API: config save failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save failed: {}", e) })),
            )
        }
    }
}

/// GET /api/logs — return log history as JSON array of LogEntry.
pub async fn logs_handler() -> Json<Value> {
    let entries = crate::web::log_layer::log_history();
    Json(json!(entries))
}

/// GET /api/crashes — return crash log as JSON array.
pub async fn crashes_handler() -> Json<Value> {
    use crate::debugger::CRASH_LOG;
    let crashes = CRASH_LOG.crashes();
    Json(json!(crashes))
}

/// GET /api/crashes/:subsystem — return last crash for a specific subsystem.
pub async fn crashes_subsystem_handler(
    axum::extract::Path(subsystem): axum::extract::Path<String>,
) -> Json<Value> {
    use crate::debugger::CRASH_LOG;
    let crash = CRASH_LOG.last_crash(&subsystem);
    Json(json!(crash))
}

// ── procfs readers ──

pub fn read_uptime() -> f64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

pub fn read_cpu() -> Value {
    let stat = match std::fs::read_to_string("/proc/stat") {
        Ok(s) => s,
        Err(_) => return json!({}),
    };

    let mut cpus = Vec::new();
    for line in stat.lines() {
        if !line.starts_with("cpu") {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }
        let vals: Vec<u64> = parts[1..8].iter().filter_map(|s| s.parse().ok()).collect();
        if vals.len() >= 7 {
            cpus.push(json!({
                "name": parts[0],
                "user": vals[0], "nice": vals[1], "system": vals[2],
                "idle": vals[3], "iowait": vals[4],
                "irq": vals[5], "softirq": vals[6],
            }));
        }
    }

    json!(cpus)
}

pub fn read_memory() -> Value {
    let meminfo = match std::fs::read_to_string("/proc/meminfo") {
        Ok(s) => s,
        Err(_) => return json!({}),
    };

    let mut mem = std::collections::HashMap::new();
    for line in meminfo.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let key = parts[0].trim_end_matches(':');
            if let Ok(val) = parts[1].parse::<u64>() {
                mem.insert(key.to_string(), val);
            }
        }
    }

    json!({
        "total": mem.get("MemTotal").copied().unwrap_or(0),
        "free": mem.get("MemFree").copied().unwrap_or(0),
        "available": mem.get("MemAvailable").or(mem.get("MemFree")).copied().unwrap_or(0),
        "buffers": mem.get("Buffers").copied().unwrap_or(0),
        "cached": mem.get("Cached").copied().unwrap_or(0),
    })
}

pub fn read_loadavg() -> Value {
    match std::fs::read_to_string("/proc/loadavg") {
        Ok(s) => {
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.len() >= 3 {
                let load: Vec<f64> = parts[..3].iter().filter_map(|s| s.parse().ok()).collect();
                json!(load)
            } else {
                json!([0.0, 0.0, 0.0])
            }
        }
        Err(_) => json!([0.0, 0.0, 0.0]),
    }
}

/// Compute CPU usage percent from /proc/stat deltas.
pub fn compute_cpu_percent() -> u8 {
    // Read /proc/stat twice with a small gap for delta
    // For 1Hz broadcasts, we use instantaneous jiffies and let the client diff.
    // Here we just return a snapshot ratio of non-idle vs total.
    let stat = match std::fs::read_to_string("/proc/stat") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let line = match stat.lines().next() {
        Some(l) if l.starts_with("cpu ") => l,
        _ => return 0,
    };
    let vals: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(7)
        .filter_map(|s| s.parse().ok())
        .collect();
    if vals.len() < 4 {
        return 0;
    }
    let total: u64 = vals.iter().sum();
    let idle = vals[3];
    if total == 0 {
        return 0;
    }
    (((total - idle) * 100) / total) as u8
}

/// Read memory used in KB.
pub fn read_mem_used_kb() -> (u32, u32) {
    let meminfo = match std::fs::read_to_string("/proc/meminfo") {
        Ok(s) => s,
        Err(_) => return (0, 0),
    };

    let mut total: u32 = 0;
    let mut available: u32 = 0;
    for line in meminfo.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let key = parts[0].trim_end_matches(':');
            if let Ok(val) = parts[1].parse::<u32>() {
                match key {
                    "MemTotal" => total = val,
                    "MemAvailable" => available = val,
                    "MemFree" if available == 0 => available = val,
                    _ => {}
                }
            }
        }
    }
    (total.saturating_sub(available), total)
}

/// Read per-core CPU jiffies from /proc/stat (cpu0, cpu1, ...).
/// Also reads current clock frequency per core from cpufreq sysfs.
pub fn read_cpu_cores() -> Vec<CpuCoreSnapshot> {
    let stat = match std::fs::read_to_string("/proc/stat") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let freqs = read_cpu_frequencies();
    let mut cores = Vec::new();
    let mut core_idx = 0usize;
    for line in stat.lines() {
        // Skip the aggregate "cpu " line, only parse "cpu0", "cpu1", etc.
        if !line.starts_with("cpu") {
            break;
        }
        let name = line.split_whitespace().next().unwrap_or("");
        if name == "cpu" {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }
        let vals: Vec<u64> = parts[1..8].iter().filter_map(|s| s.parse().ok()).collect();
        if vals.len() >= 7 {
            cores.push(CpuCoreSnapshot {
                user: vals[0],
                nice: vals[1],
                system: vals[2],
                idle: vals[3],
                iowait: vals[4],
                irq: vals[5],
                softirq: vals[6],
                freq_khz: freqs.get(core_idx).copied().unwrap_or(0),
            });
            core_idx += 1;
        }
    }
    cores
}

/// Read current CPU frequency per core from sysfs (kHz).
fn read_cpu_frequencies() -> Vec<u32> {
    let mut freqs = Vec::new();
    for i in 0..8 {
        let path = format!("/sys/devices/system/cpu/cpu{}/cpufreq/scaling_cur_freq", i);
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                if let Ok(khz) = s.trim().parse::<u32>() {
                    freqs.push(khz);
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    freqs
}

/// Read CPU temperature in millidegrees C from thermal_zone0.
pub fn read_temperature_mc() -> Option<i32> {
    std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Read disk usage via statvfs for actually-mounted filesystems.
/// Parses /proc/mounts to find real mount points, deduplicates by device.
pub fn read_disk_usage() -> Vec<DiskUsage> {
    let wanted = ["/", "/lsync", "/data"];
    let mut disks = Vec::new();

    #[cfg(unix)]
    {
        // Parse /proc/mounts to find which paths are real mount points
        let mounts_text = match std::fs::read_to_string("/proc/mounts") {
            Ok(s) => s,
            Err(_) => return disks,
        };

        // Collect (device, mount_point) for wanted paths that are actual mounts
        let mut seen_devices = std::collections::HashSet::new();
        for line in mounts_text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let device = parts[0];
            let mount_point = parts[1];
            if !wanted.contains(&mount_point) {
                continue;
            }
            // Skip rootfs pseudo-mount (duplicate of the real block device mount)
            if device == "rootfs" {
                continue;
            }
            // Deduplicate by device — don't report the same partition twice
            if !seen_devices.insert(device.to_string()) {
                continue;
            }

            use std::ffi::CString;
            let c_path = match CString::new(mount_point) {
                Ok(p) => p,
                Err(_) => continue,
            };
            unsafe {
                let mut stat: libc::statvfs = std::mem::zeroed();
                if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
                    let block_size = stat.f_frsize as u64;
                    let total_kb = (stat.f_blocks * block_size) / 1024;
                    let free_kb = (stat.f_bfree * block_size) / 1024;
                    if total_kb > 0 {
                        disks.push(DiskUsage {
                            mount: mount_point.to_string(),
                            total_kb,
                            used_kb: total_kb.saturating_sub(free_kb),
                        });
                    }
                }
            }
        }
    }

    disks
}

/// Read network interface RX/TX bytes from /proc/net/dev.
pub fn read_net_interfaces() -> Vec<NetInterfaceSnapshot> {
    let dev = match std::fs::read_to_string("/proc/net/dev") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let wanted = ["wlan0", "ap0", "eth0"];
    let mut interfaces = Vec::new();
    for line in dev.lines().skip(2) {
        let line = line.trim();
        let (name, rest) = match line.split_once(':') {
            Some((n, r)) => (n.trim(), r),
            None => continue,
        };
        if !wanted.contains(&name) {
            continue;
        }
        let vals: Vec<u64> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        if vals.len() >= 9 {
            interfaces.push(NetInterfaceSnapshot {
                name: name.to_string(),
                rx_bytes: vals[0],
                tx_bytes: vals[8],
            });
        }
    }
    interfaces
}

/// Read process count from /proc/loadavg (4th field: running/total).
pub fn read_process_count() -> u16 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| {
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.len() >= 4 {
                // Format: "1/123" — total is after the slash
                parts[3].split('/').nth(1)?.parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// Read detailed memory info: (total, free, buffers, cached) in KB.
pub fn read_detailed_memory() -> (u32, u32, u32, u32, u32) {
    let meminfo = match std::fs::read_to_string("/proc/meminfo") {
        Ok(s) => s,
        Err(_) => return (0, 0, 0, 0, 0),
    };

    let mut total: u32 = 0;
    let mut free: u32 = 0;
    let mut available: u32 = 0;
    let mut buffers: u32 = 0;
    let mut cached: u32 = 0;
    for line in meminfo.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let key = parts[0].trim_end_matches(':');
            if let Ok(val) = parts[1].parse::<u32>() {
                match key {
                    "MemTotal" => total = val,
                    "MemFree" => free = val,
                    "MemAvailable" => available = val,
                    "Buffers" => buffers = val,
                    "Cached" => cached = val,
                    _ => {}
                }
            }
        }
    }
    if available == 0 {
        available = free;
    }
    (total, free, available, buffers, cached)
}

/// Read load averages as [f32; 3].
pub fn read_load_avg() -> [f32; 3] {
    match std::fs::read_to_string("/proc/loadavg") {
        Ok(s) => {
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.len() >= 3 {
                let vals: Vec<f32> = parts[..3].iter().filter_map(|s| s.parse().ok()).collect();
                if vals.len() == 3 {
                    return [vals[0], vals[1], vals[2]];
                }
            }
            [0.0, 0.0, 0.0]
        }
        Err(_) => [0.0, 0.0, 0.0],
    }
}

/// GET /api/wifi/scan — trigger WiFi scan and return results.
/// Ensures wpa_supplicant is running, then uses wpa_cli to scan.
pub async fn wifi_scan_handler() -> impl axum::response::IntoResponse {
    use std::process::Command;

    // Ensure wpa_supplicant is running (blocking)
    let ensure_result = tokio::task::spawn_blocking(|| {
        crate::network::wpa::ensure_running()
    }).await;
    if let Err(e) = ensure_result.as_ref().map_err(|e| e.to_string()).and_then(|r| r.as_ref().map_err(|e| e.to_string())) {
        tracing::warn!("wifi_scan: failed to ensure wpa_supplicant: {}", e);
    }

    // Trigger scan
    let _ = Command::new("wpa_cli")
        .args(["-i", "wlan0", "-p", "/data/wifi", "scan"])
        .output();

    // Poll for results — wpa_supplicant scan is async, may take 1-6s depending
    // on how many channels the radio needs to probe. Poll every 500ms, return
    // as soon as we have results (or after 6s timeout).
    let mut networks: Vec<encore_common::protocol::WifiNetwork> = Vec::new();
    for _ in 0..12 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        if let Ok(out) = Command::new("wpa_cli")
            .args(["-i", "wlan0", "-p", "/data/wifi", "scan_results"])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            let results: Vec<encore_common::protocol::WifiNetwork> = text
                .lines()
                .skip(1)
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 5 {
                        Some(encore_common::protocol::WifiNetwork {
                            bssid: parts[0].to_string(),
                            signal_dbm: parts[2].parse().unwrap_or(-100),
                            security: parts[3].to_string(),
                            ssid: parts[4].to_string(),
                            frequency_mhz: parts[1].parse().unwrap_or(0),
                        })
                    } else {
                        None
                    }
                })
                .collect();
            if !results.is_empty() {
                networks = results;
                break;
            }
        }
    }

    axum::Json(networks)
}

/// POST /api/reboot — sync filesystems and reboot.
pub async fn reboot_handler() -> impl axum::response::IntoResponse {
    use std::process::Command;

    // Sync filesystems
    let _ = Command::new("sync").output();

    // Schedule reboot after 3s
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let _ = std::process::Command::new("reboot").output();
    });

    axum::Json(serde_json::json!({ "status": "rebooting", "delay_secs": 3 }))
}

// ── OTA update ──

/// POST /api/update — upload a new Encore binary, stage for next boot, reboot.
///
/// Accepts the raw binary as the request body. Stages to
/// /lsync/encore/encore_next (writable yaffs2 partition). The boot script
/// (mount_partition.sh) detects this file on next boot and bind-mounts
/// it over /usr/bin/encore, since the rootfs is read-only SquashFS.
pub async fn update_handler(body: Bytes) -> (StatusCode, Json<Value>) {
    let size = body.len();
    info!("OTA: received {} bytes", size);

    if size < 1024 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "file too small"})),
        );
    }
    if size > MAX_UPDATE_SIZE {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "file too large", "max": MAX_UPDATE_SIZE})),
        );
    }

    // Verify ELF magic
    if body.len() < 4 || &body[..4] != b"\x7fELF" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "not a valid ELF binary"})),
        );
    }

    // Ensure staging directory exists on writable partition
    if let Err(e) = std::fs::create_dir_all("/lsync/encore") {
        warn!("OTA: failed to create staging dir: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "staging dir creation failed"})),
        );
    }

    // Write to staging path on writable /lsync partition
    if let Err(e) = std::fs::write(OTA_NEXT_PATH, &body) {
        warn!("OTA: write failed: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "write failed"})),
        );
    }

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        if let Err(e) = std::fs::set_permissions(OTA_NEXT_PATH, perms) {
            warn!("OTA: chmod failed: {}", e);
        }
    }

    info!("OTA: staged {} bytes to {}, rebooting in 3s", size, OTA_NEXT_PATH);

    // Reboot after a delay so the response can be sent
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        info!("OTA: rebooting now");
        // sync filesystems before reboot
        unsafe { libc::sync() };
        unsafe { libc::reboot(libc::RB_AUTOBOOT) };
    });

    (
        StatusCode::OK,
        Json(json!({"status": "staged", "size": size, "message": "Update staged. Rebooting in 3 seconds..."})),
    )
}

// ── Rootfs flash ──

/// POST /api/firmware/flash — upload a SquashFS rootfs, flash to NAND, reboot.
///
/// Streams the body to /run/firmware_upload.squashfs (tmpfs) to avoid holding
/// 85 MB in RAM on a 512 MB device. Validates SquashFS magic, size-checks
/// against the NAND partition limit, then calls `flash_image rootfs <path>`.
pub async fn firmware_flash_handler(body: axum::body::Body) -> (StatusCode, Json<Value>) {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    info!("Firmware: starting rootfs upload");

    let mut stream = body.into_data_stream();

    // Create staging file on tmpfs (RAM-backed, no NAND wear)
    let file = match tokio::fs::File::create(FIRMWARE_STAGING_PATH).await {
        Ok(f) => f,
        Err(e) => {
            warn!("Firmware: failed to create staging file: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("staging file creation failed: {}", e)})),
            );
        }
    };
    let mut writer = tokio::io::BufWriter::new(file);

    let mut total: usize = 0;
    let mut magic_buf: Vec<u8> = Vec::with_capacity(4);
    let mut magic_checked = false;

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                warn!("Firmware: body read error: {}", e);
                let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("upload read error: {}", e)})),
                );
            }
        };

        // Validate SquashFS magic from the first 4 bytes
        if !magic_checked {
            let needed = 4 - magic_buf.len();
            let take = chunk.len().min(needed);
            magic_buf.extend_from_slice(&chunk[..take]);
            if magic_buf.len() >= 4 {
                if &magic_buf[..4] != b"hsqs" {
                    let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "not a valid SquashFS image (bad magic)"})),
                    );
                }
                magic_checked = true;
            }
        }

        total += chunk.len();
        if total > ROOTFS_MAX {
            let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "file too large", "max": ROOTFS_MAX})),
            );
        }

        if let Err(e) = writer.write_all(&chunk).await {
            warn!("Firmware: write error: {}", e);
            let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("disk write failed: {}", e)})),
            );
        }
    }

    if let Err(e) = writer.flush().await {
        warn!("Firmware: flush failed: {}", e);
        let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("flush failed: {}", e)})),
        );
    }
    drop(writer);

    if total < 1000 {
        let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "file too small"})),
        );
    }

    if !magic_checked {
        let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "file too small for magic check"})),
        );
    }

    info!("Firmware: {} bytes written to staging, flashing rootfs", total);

    // Flash to NAND via stock flash_image utility
    let output = tokio::task::spawn_blocking(|| {
        std::process::Command::new("flash_image")
            .args(["rootfs", FIRMWARE_STAGING_PATH])
            .output()
    })
    .await;

    match output {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{}{}", stdout, stderr);
            if !out.status.success() || combined.to_lowercase().contains("error") {
                warn!("Firmware: flash_image failed: {}", combined.trim());
                let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("flash failed: {}", combined.trim())})),
                );
            }
            info!("Firmware: flash_image succeeded");
        }
        Ok(Err(e)) => {
            warn!("Firmware: flash_image not found: {}", e);
            let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("flash_image not available: {}", e)})),
            );
        }
        Err(e) => {
            warn!("Firmware: flash task panicked: {}", e);
            let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "flash task failed"})),
            );
        }
    }

    // Clean up staging file
    let _ = tokio::fs::remove_file(FIRMWARE_STAGING_PATH).await;

    // Reboot after delay so the HTTP response can be sent
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        info!("Firmware: rebooting now");
        unsafe { libc::sync() };
        unsafe { libc::reboot(libc::RB_AUTOBOOT) };
    });

    (
        StatusCode::OK,
        Json(json!({
            "status": "flashed",
            "size": total,
            "message": "Rootfs flashed to NAND. Rebooting in 3 seconds..."
        })),
    )
}
