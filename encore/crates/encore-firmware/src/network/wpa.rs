//! wpa_supplicant control interface client.
//!
//! Uses `wpa_cli` command-line tool to communicate with wpa_supplicant.
//! The raw Unix datagram socket approach doesn't work on this device
//! (permission issues with /data/wifi owned by wifi:wifi), but wpa_cli
//! handles it correctly.

use anyhow::{bail, Context, Result};
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Default control interface directory (matches wpa_supplicant.conf.in)
const WPA_CTRL_DIR: &str = "/data/wifi";
const WPA_CTRL_IFACE: &str = "wlan0";
/// Android init.rc creates the socket here instead of WPA_CTRL_DIR
const WPA_CTRL_ANDROID: &str = "/dev/socket/wpa_wlan0";

const WPA_SUPPLICANT_BIN: &str = "/bin/wpa_supplicant";
const WPA_SUPPLICANT_CONF: &str = "/data/wifi/wpa_supplicant.conf";
const WPA_ENTROPY: &str = "/data/wifi/entropy.bin";

/// WiFi network scan result
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub bssid: String,
    pub frequency: u32,
    pub signal: i32,
    pub ssid: String,
}

/// WiFi connection status
#[derive(Debug, Clone, Default)]
pub struct WpaStatus {
    pub wpa_state: String,
    pub ssid: Option<String>,
    pub ip_address: Option<String>,
}

/// Check if wpa_supplicant process is running.
fn is_process_running() -> bool {
    Command::new("pgrep")
        .arg("wpa_supplicant")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Ensure wpa_supplicant is running. If not, start it in daemon mode.
/// Returns Ok(()) if wpa_supplicant is ready.
pub fn ensure_running() -> Result<()> {
    let ctrl_path = format!("{}/{}", WPA_CTRL_DIR, WPA_CTRL_IFACE);

    // Check if already running by process list (most reliable)
    if is_process_running() {
        // Process exists — wait for socket (cold boot can take 15-20s for wpa_supplicant
        // to create the control socket, especially when WiFi firmware is still loading).
        // Android init.rc puts the socket at /dev/socket/wpa_wlan0 instead of /data/wifi/wlan0.
        for i in 0..60 {
            if std::path::Path::new(&ctrl_path).exists()
                || std::path::Path::new(WPA_CTRL_ANDROID).exists()
            {
                if i > 0 {
                    info!("wpa_supplicant control socket ready after {}ms", i * 250);
                } else {
                    debug!("wpa_supplicant already running");
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        // 15s without socket — interface may not exist yet
        warn!("wpa_supplicant running but control socket not found after 15s");
        return Ok(());
    }

    info!("wpa_supplicant not running, starting it");

    // Ensure config directory exists
    std::fs::create_dir_all(WPA_CTRL_DIR).ok();

    // Ensure config file exists
    if !std::path::Path::new(WPA_SUPPLICANT_CONF).exists() {
        info!("creating {}", WPA_SUPPLICANT_CONF);
        std::fs::write(
            WPA_SUPPLICANT_CONF,
            "ctrl_interface=/data/wifi\nupdate_config=1\ncountry=US\n",
        )
        .context("failed to write wpa_supplicant.conf")?;
    }

    // Start wpa_supplicant in daemon mode
    let status = Command::new(WPA_SUPPLICANT_BIN)
        .args([
            "-B",
            "-Dnl80211",
            "-iwlan0",
            "-c", WPA_SUPPLICANT_CONF,
            &format!("-e{}", WPA_ENTROPY),
        ])
        .status()
        .context("failed to start wpa_supplicant")?;

    if !status.success() {
        bail!("wpa_supplicant exited with {}", status);
    }

    // Wait for control socket to appear (up to 5 seconds)
    for i in 0..50 {
        if std::path::Path::new(&ctrl_path).exists()
            || std::path::Path::new(WPA_CTRL_ANDROID).exists()
        {
            info!("wpa_supplicant ready after {}ms", i * 100);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    bail!("wpa_supplicant started but control socket did not appear within 5s")
}

/// Client for wpa_supplicant control interface.
/// Uses `wpa_cli` subprocess calls (reliable on this device).
pub struct WpaClient;

impl WpaClient {
    /// Create a new client, waiting for wpa_supplicant to become responsive.
    /// At cold boot, the control socket may take time to appear even after
    /// ensure_running() returns — retry for up to 10 seconds.
    pub fn connect() -> Result<Self> {
        let ctrl_path = format!("{}/{}", WPA_CTRL_DIR, WPA_CTRL_IFACE);
        let mut last_err = String::new();

        for attempt in 0..20 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(500));
            }

            // Check if control socket exists before calling wpa_cli.
            // Android init.rc places it at /dev/socket/wpa_wlan0.
            if !std::path::Path::new(&ctrl_path).exists()
                && !std::path::Path::new(WPA_CTRL_ANDROID).exists()
            {
                last_err = format!("control socket not found at {} or {}", ctrl_path, WPA_CTRL_ANDROID);
                if attempt == 0 {
                    info!("wpa_cli: waiting for control socket...");
                }
                continue;
            }

            let output = match Command::new("wpa_cli")
                .args(["-i", WPA_CTRL_IFACE, "-p", WPA_CTRL_DIR, "status"])
                .output()
            {
                Ok(o) => o,
                Err(e) => {
                    last_err = format!("failed to run wpa_cli: {}", e);
                    continue;
                }
            };

            if output.status.success() {
                if attempt > 0 {
                    info!("wpa_cli: connected after {}ms", attempt * 500);
                } else {
                    debug!("wpa_cli: connected");
                }
                return Ok(Self);
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            last_err = stderr.trim().to_string();
        }

        bail!("wpa_cli status failed after 10s: {}", last_err);
    }

    /// Run a wpa_cli command and return stdout.
    fn cli(&self, args: &[&str]) -> Result<String> {
        let mut cmd_args = vec!["-i", WPA_CTRL_IFACE, "-p", WPA_CTRL_DIR];
        cmd_args.extend_from_slice(args);

        let output = Command::new("wpa_cli")
            .args(&cmd_args)
            .output()
            .with_context(|| format!("wpa_cli {:?} failed to run", args))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        debug!("wpa_cli {:?} -> {} bytes", args, stdout.len());
        Ok(stdout)
    }

    /// Run a wpa_cli command and check it returns "OK".
    fn cli_ok(&self, args: &[&str]) -> Result<()> {
        let resp = self.cli(args)?;
        if resp.trim().contains("FAIL") {
            bail!("wpa_cli {:?} returned: {}", args, resp.trim());
        }
        Ok(())
    }

    /// Query wpa_supplicant status.
    pub fn status(&self) -> Result<WpaStatus> {
        let resp = self.cli(&["status"])?;

        let mut wpa_state = String::new();
        let mut ssid = None;
        let mut ip_address = None;

        for line in resp.lines() {
            if let Some((key, val)) = line.split_once('=') {
                match key {
                    "wpa_state" => wpa_state = val.to_string(),
                    "ssid" => ssid = Some(val.to_string()),
                    "ip_address" => ip_address = Some(val.to_string()),
                    _ => {}
                }
            }
        }

        Ok(WpaStatus {
            wpa_state,
            ssid,
            ip_address,
        })
    }

    /// Trigger a WiFi scan. Returns immediately; use scan_results() after ~3s.
    pub fn scan(&self) -> Result<()> {
        let resp = self.cli(&["scan"])?;
        if !resp.contains("OK") && !resp.contains("FAIL-BUSY") {
            bail!("SCAN failed: {}", resp.trim());
        }
        Ok(())
    }

    /// Get scan results (call after scan + delay).
    pub fn scan_results(&self) -> Result<Vec<ScanResult>> {
        let resp = self.cli(&["scan_results"])?;
        let mut results = Vec::new();

        for line in resp.lines().skip(1) {
            // Format: bssid / frequency / signal level / flags / ssid
            let parts: Vec<&str> = line.splitn(5, '\t').collect();
            if parts.len() >= 5 {
                results.push(ScanResult {
                    bssid: parts[0].to_string(),
                    frequency: parts[1].parse().unwrap_or(0),
                    signal: parts[2].parse().unwrap_or(-100),
                    ssid: parts[4].to_string(),
                });
            }
        }

        Ok(results)
    }

    /// Add a network and connect to it.
    pub fn connect_network(&self, ssid: &str, psk: &str) -> Result<()> {
        // Remove all existing networks
        let list = self.cli(&["list_networks"])?;
        for line in list.lines().skip(1) {
            if let Some(id) = line.split('\t').next() {
                if let Err(e) = self.cli_ok(&["remove_network", id]) {
                    warn!("wpa: remove network {} failed: {}", id, e);
                }
            }
        }

        // Add new network
        let resp = self.cli(&["add_network"])?;
        let net_id = resp.trim();
        info!("wpa: added network id={}", net_id);

        // SET_NETWORK via wpa_cli uses positional args: set_network <id> <field> <value>
        self.cli_ok(&["set_network", net_id, "ssid", &format!("\"{}\"", ssid)])
            .context("set_network ssid failed")?;

        if psk.is_empty() {
            self.cli_ok(&["set_network", net_id, "key_mgmt", "NONE"])
                .context("set_network key_mgmt NONE failed")?;
        } else {
            self.cli_ok(&["set_network", net_id, "psk", &format!("\"{}\"", psk)])
                .context("set_network psk failed")?;
        }

        self.cli_ok(&["enable_network", net_id])
            .context("enable_network failed")?;

        // save_config is best-effort: on this device, wpa_supplicant uses the
        // Android socket FD mechanism and may not have a writable config path.
        // WiFi credential persistence is handled via config.toml instead.
        if let Err(e) = self.cli_ok(&["save_config"]) {
            debug!("wpa: save_config failed (expected on Android socket): {}", e);
        }

        info!("wpa: network {} configured and enabled (ssid={})", net_id, ssid);
        Ok(())
    }

    /// Check if connected (wpa_state == COMPLETED).
    pub fn is_connected(&self) -> Result<bool> {
        let status = self.status()?;
        Ok(status.wpa_state == "COMPLETED")
    }

    /// Get the frequency (MHz) of the connected network from wpa_supplicant status.
    /// Returns e.g. 2437 for 2.4GHz ch6, 5180 for 5GHz ch36.
    pub fn connected_frequency(&self) -> Option<u32> {
        let resp = self.cli(&["status"]).ok()?;
        for line in resp.lines() {
            if let Some((key, val)) = line.split_once('=') {
                if key == "freq" {
                    return val.parse().ok();
                }
            }
        }
        None
    }
}

/// Map a STA frequency to a suitable AP channel on the same band.
/// Returns (hw_mode, channel) for hostapd config.
pub fn freq_to_ap_channel(freq_mhz: u32) -> (&'static str, u8) {
    if freq_mhz >= 5000 {
        // 5GHz — pick a common DFS-free channel in the same UNII band
        let channel = match freq_mhz {
            5180..=5240 => 36,
            5260..=5320 => 44,
            5500..=5700 => 100,
            5745..=5825 => 149,
            _ => 36,
        };
        ("a", channel)
    } else {
        ("g", 6)
    }
}
