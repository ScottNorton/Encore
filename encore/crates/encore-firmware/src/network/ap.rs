//! Access Point lifecycle management.
//!
//! Monitors and manages the AP on the p2p0 interface. Default SSID is
//! `Invoke-XXXX` where XXXX is the last 4 hex digits of the WiFi MAC.
//! Primary: hostapd (WPA2). Fallback: Marvell uAP firmware mode (open).
//!
//! The Marvell mlan driver has two critical limitations:
//! 1. Once hostapd is killed, nl80211 can't reinitialize (entropy + state).
//! 2. Single-radio chip can't run AP on 2.4GHz while STA is on 5GHz —
//!    the driver kills hostapd after ~35s when bands differ.
//!
//! When hostapd fails to restart, `uaputl.exe` can start the AP in
//! firmware mode. WPA2 is not supported via uaputl on p2p0, so the
//! fallback AP runs as an open network.

use anyhow::Result;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use tracing::{info, warn};

const AP_SCRIPT: &str = "/sbin/start_ap.sh";
const AP_IFACE: &str = "p2p0";
const AP_IP: &str = "192.168.43.1";
const LEASE_FILE: &str = "/tmp/dnsmasq-ap.leases";
const UAPUTL: &str = "uaputl.exe";
const DNSMASQ_CONF: &str = "/tmp/dnsmasq-ap.conf";
const DNSMASQ_PID: &str = "/tmp/dnsmasq-ap.pid";

/// Track hostapd restart failures. After MAX_HOSTAPD_RETRIES, use uaputl.
static HOSTAPD_FAILURES: AtomicU8 = AtomicU8::new(0);
const MAX_HOSTAPD_RETRIES: u8 = 2;

/// Check if the AP is currently running.
/// Checks hostapd first (primary), then uaputl BSS status (fallback).
pub fn is_ap_running() -> bool {
    // Primary: hostapd alive AND p2p0 has IP
    let hostapd_alive = Command::new("pgrep")
        .arg("hostapd")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if hostapd_alive && interface_has_ip(AP_IFACE, AP_IP) {
        return true;
    }

    // Fallback: check if uAP firmware mode is active
    is_uaputl_bss_started() && interface_has_ip(AP_IFACE, AP_IP)
}

/// Check if the Marvell uAP BSS is started via uaputl.
fn is_uaputl_bss_started() -> bool {
    Command::new(UAPUTL)
        .args(["-i", AP_IFACE, "sys_cfg_bss_status"])
        .output()
        .map(|out| {
            let text = String::from_utf8_lossy(&out.stdout);
            text.contains("started")
        })
        .unwrap_or(false)
}

/// Wait for AP to come up (boot script is starting it).
/// Returns true if AP is running within the timeout.
pub fn wait_for_ap(timeout_secs: u32) -> bool {
    for i in 0..timeout_secs * 2 {
        if is_ap_running() {
            if i > 0 {
                info!("AP: ready after {}ms", i * 500);
            } else {
                info!("AP: already running");
            }
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    warn!("AP: not running after {}s timeout", timeout_secs);
    false
}

/// Ensure AP is running. Tries hostapd first, falls back to uaputl.
/// NOTE: Only call this from the monitoring loop, NOT at startup.
/// At startup, use wait_for_ap() to avoid racing with the boot script.
pub fn ensure_ap() -> Result<()> {
    if is_ap_running() {
        return Ok(());
    }

    let failures = HOSTAPD_FAILURES.load(Ordering::Relaxed);
    if failures < MAX_HOSTAPD_RETRIES {
        // Try hostapd (primary)
        info!("AP: starting via {} (attempt {})", AP_SCRIPT, failures + 1);
        let status = Command::new(AP_SCRIPT)
            .status()
            .map_err(|e| anyhow::anyhow!("failed to run {}: {}", AP_SCRIPT, e))?;

        if !status.success() {
            warn!("AP: {} exited with {}", AP_SCRIPT, status);
        }

        std::thread::sleep(std::time::Duration::from_secs(2));

        if is_ap_running() {
            info!("AP: started successfully via hostapd");
            HOSTAPD_FAILURES.store(0, Ordering::Relaxed);
            return Ok(());
        }

        let new_failures = HOSTAPD_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
        warn!(
            "AP: hostapd failed ({}/{})",
            new_failures, MAX_HOSTAPD_RETRIES
        );
    }

    // Fallback: Marvell uAP firmware mode
    ensure_ap_uaputl()
}

/// Start AP via Marvell uAP firmware mode (uaputl.exe).
/// This bypasses hostapd entirely and uses the WiFi firmware's built-in AP.
/// Limitation: WPA2 not supported on p2p0 via uaputl — runs as open network.
fn ensure_ap_uaputl() -> Result<()> {
    info!("AP: starting via uaputl (firmware mode, open network)");

    // Reset the uAP to clean state
    let _ = Command::new(UAPUTL)
        .args(["-i", AP_IFACE, "sys_reset"])
        .output();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Stop BSS if it auto-started after reset
    let _ = Command::new(UAPUTL)
        .args(["-i", AP_IFACE, "bss_stop"])
        .output();
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Configure SSID (use same default as hostapd path)
    let ssid = default_ap_ssid();
    let _ = Command::new(UAPUTL)
        .args(["-i", AP_IFACE, "sys_cfg_ssid", &ssid])
        .output();

    // Start BSS (auto channel/rates from firmware)
    let start_out = Command::new(UAPUTL)
        .args(["-i", AP_IFACE, "bss_start"])
        .output();

    if let Ok(out) = &start_out {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("successful") || text.contains("already started") {
            info!("AP: uaputl BSS started");
        } else {
            warn!("AP: uaputl bss_start: {}", text.trim());
        }
    }

    std::thread::sleep(std::time::Duration::from_secs(2));

    // Set IP
    let _ = Command::new("ifconfig")
        .args([AP_IFACE, AP_IP, "netmask", "255.255.255.0", "up"])
        .status();

    // Ensure dnsmasq is running for DHCP
    ensure_dnsmasq();

    if interface_has_ip(AP_IFACE, AP_IP) {
        warn!("AP: running in uaputl mode (OPEN network — no WPA2)");
        Ok(())
    } else {
        warn!("AP: uaputl fallback also failed");
        Ok(())
    }
}

/// Ensure dnsmasq is running for AP DHCP.
fn ensure_dnsmasq() {
    // Check if dnsmasq is already running via pidfile
    if let Ok(pid_str) = std::fs::read_to_string(DNSMASQ_PID) {
        let pid = pid_str.trim();
        if !pid.is_empty()
            && Command::new("kill")
                .args(["-0", pid])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        {
            return; // dnsmasq already running
        }
    }

    // Write dnsmasq config if missing
    if !std::path::Path::new(DNSMASQ_CONF).exists() {
        let conf = format!(
            "interface={iface}\n\
             bind-interfaces\n\
             dhcp-range=192.168.43.100,192.168.43.155,255.255.255.0,12h\n\
             dhcp-option=option:router,{ip}\n\
             dhcp-authoritative\n\
             no-resolv\n\
             no-poll\n\
             no-hosts\n\
             no-negcache\n\
             user=root\n\
             pid-file={pid}\n\
             dhcp-leasefile={lease}\n\
             address=/#/{ip}\n\
             dhcp-option=6,{ip}\n\
             dhcp-option=114,http://{ip}/\n",
            iface = AP_IFACE,
            ip = AP_IP,
            pid = DNSMASQ_PID,
            lease = LEASE_FILE,
        );
        let _ = std::fs::write(DNSMASQ_CONF, conf);
    }

    info!("AP: starting dnsmasq");
    let _ = Command::new("/bin/dnsmasq")
        .args(["-C", DNSMASQ_CONF])
        .status();
}

/// Start AP on the same band as the given STA frequency.
/// Writes a custom hostapd config with the appropriate hw_mode and channel,
/// then starts hostapd. Falls back to ensure_ap() on failure.
pub fn start_ap_on_band(sta_freq_mhz: u32) -> Result<()> {
    let (hw_mode, channel) = super::wpa::freq_to_ap_channel(sta_freq_mhz);
    info!(
        "AP: starting on band hw_mode={} channel={} (STA freq={}MHz)",
        hw_mode, channel, sta_freq_mhz
    );

    // Read AP credentials from config.toml or use defaults
    let (ap_ssid, ap_pass) = read_ap_credentials();

    let ieee80211ac = if hw_mode == "a" { "ieee80211ac=1\n" } else { "" };

    let config = format!(
        "interface={iface}\n\
         driver=nl80211\n\
         ssid={ssid}\n\
         hw_mode={hw_mode}\n\
         channel={channel}\n\
         ieee80211n=1\n\
         {ieee80211ac}\
         ctrl_interface=/data/wifi\n\
         ignore_broadcast_ssid=0\n\
         wpa=2\n\
         wpa_passphrase={pass}\n\
         wpa_key_mgmt=WPA-PSK\n\
         wpa_pairwise=CCMP\n\
         rsn_pairwise=CCMP\n",
        iface = AP_IFACE,
        ssid = ap_ssid,
        hw_mode = hw_mode,
        channel = channel,
        ieee80211ac = ieee80211ac,
        pass = ap_pass,
    );

    std::fs::create_dir_all("/data/wifi").ok();
    std::fs::write("/data/wifi/hostapd_ap.conf", &config)
        .map_err(|e| anyhow::anyhow!("failed to write hostapd config: {}", e))?;

    // Kill existing hostapd
    let _ = Command::new("killall").arg("hostapd").output();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Start hostapd in daemon mode
    let status = Command::new("/bin/hostapd")
        .args(["-B", "/data/wifi/hostapd_ap.conf"])
        .status()
        .map_err(|e| anyhow::anyhow!("failed to start hostapd: {}", e))?;

    if !status.success() {
        warn!("AP: hostapd exited with {}", status);
    }

    std::thread::sleep(std::time::Duration::from_secs(3));

    // Set IP
    let _ = Command::new("ifconfig")
        .args([AP_IFACE, AP_IP, "netmask", "255.255.255.0", "up"])
        .status();

    ensure_dnsmasq();

    if interface_has_ip(AP_IFACE, AP_IP) {
        info!("AP: started on hw_mode={} ch{}", hw_mode, channel);
        HOSTAPD_FAILURES.store(0, Ordering::Relaxed);
        Ok(())
    } else {
        warn!("AP: failed to start on hw_mode={} ch{}, falling back to default", hw_mode, channel);
        ensure_ap()
    }
}

/// Generate default AP SSID from WiFi MAC address.
/// Returns `Invoke-XXXX` where XXXX is the last 4 hex digits of the MAC.
pub fn default_ap_ssid() -> String {
    let mac = std::fs::read_to_string("/sys/class/net/wlan0/address")
        .or_else(|_| std::fs::read_to_string("/sys/class/net/p2p0/address"))
        .unwrap_or_default();
    let mac = mac.trim();
    // MAC format: "aa:bb:cc:dd:ee:ff" — take last 4 hex chars (ee:ff → eeff)
    let suffix: String = mac.chars().rev().take(5).filter(|c| *c != ':').collect::<String>().chars().rev().collect();
    if suffix.len() == 4 {
        format!("Invoke-{}", suffix.to_ascii_uppercase())
    } else {
        "Invoke".to_string()
    }
}

/// Read AP SSID and password from `/lsync/encore/config.toml`, with defaults.
fn read_ap_credentials() -> (String, String) {
    let mut ssid = default_ap_ssid();
    let mut pass = "ridiculous".to_string();
    if let Ok(cfg) = encore_common::config::EncoreConfigFile::load(
        std::path::Path::new(super::CONFIG_PATH),
    ) {
        if let Some(s) = &cfg.network.ap_ssid {
            if !s.is_empty() {
                ssid = s.clone();
            }
        }
        if let Some(p) = &cfg.network.ap_password {
            if !p.is_empty() {
                pass = p.clone();
            }
        }
    }
    (ssid, pass)
}

/// Stop AP by killing hostapd and AP's dnsmasq (via pidfile, not killall).
/// Also stops uaputl BSS if running in firmware mode.
pub fn stop_ap() -> Result<()> {
    info!("AP: stopping");
    let _ = Command::new("killall").arg("hostapd").output();
    // Also stop uaputl BSS
    let _ = Command::new(UAPUTL)
        .args(["-i", AP_IFACE, "bss_stop"])
        .output();
    // Kill only the AP's dnsmasq via its pidfile — do NOT killall dnsmasq,
    // as the system dnsmasq (for wlan0 DNS) must stay alive.
    if let Ok(pid) = std::fs::read_to_string(DNSMASQ_PID) {
        let pid = pid.trim();
        if !pid.is_empty() {
            let _ = Command::new("kill").arg(pid).output();
        }
    }
    Ok(())
}

/// Count connected AP clients from the DHCP lease file.
pub fn client_count() -> u8 {
    std::fs::read_to_string(LEASE_FILE)
        .map(|content| {
            content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count() as u8
        })
        .unwrap_or(0)
}

/// Check if a network interface has a specific IP address.
fn interface_has_ip(iface: &str, expected_ip: &str) -> bool {
    Command::new("ifconfig")
        .arg(iface)
        .output()
        .map(|out| {
            let text = String::from_utf8_lossy(&out.stdout);
            text.contains(expected_ip)
        })
        .unwrap_or(false)
}
