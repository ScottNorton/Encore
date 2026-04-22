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
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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

/// Whether we're running in uaputl mode (only set after hostapd exhausts retries).
/// When false, is_ap_running() skips the uaputl check (which can hang the driver).
static USING_UAPUTL: AtomicBool = AtomicBool::new(false);

/// Guard against concurrent AP restarts. The monitor loop skips AP health
/// checks while this is true, preventing races with startup or resets.
static AP_RESTARTING: AtomicBool = AtomicBool::new(false);

/// Check if an AP restart is in progress. Used by the monitor loop to
/// skip AP health checks and avoid spawning duplicate restarts.
pub fn is_restarting() -> bool {
    AP_RESTARTING.load(Ordering::Relaxed)
}

/// Set the restarting flag directly. Used by the TX-error reset path
/// which needs to hold the flag across stop + reset + start.
pub fn set_restarting(v: bool) {
    AP_RESTARTING.store(v, Ordering::SeqCst);
}

/// Start AP without acquiring the restart lock. Caller must hold it.
pub fn ensure_ap_unlocked() -> Result<()> {
    if is_ap_running() {
        return Ok(());
    }
    ensure_ap_inner()
}

/// Check if the AP is currently running.
/// Checks hostapd first (primary). Only checks uaputl if we're in uaputl mode
/// (uaputl.exe can hang the Marvell driver during interface resets).
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

    // Only check uaputl if we've actually entered uaputl mode.
    // Calling uaputl.exe on a freshly-reset interface can hang indefinitely.
    if USING_UAPUTL.load(Ordering::Relaxed) {
        return is_uaputl_bss_started() && interface_has_ip(AP_IFACE, AP_IP);
    }

    false
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
/// Uses AP_RESTARTING flag to prevent concurrent restarts.
pub fn ensure_ap() -> Result<()> {
    if is_ap_running() {
        return Ok(());
    }

    // Acquire restart lock — if another thread is already restarting, bail out.
    if AP_RESTARTING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        info!("AP: restart already in progress, skipping");
        return Ok(());
    }

    // Re-check after acquiring lock (may have come up while we waited)
    if is_ap_running() {
        AP_RESTARTING.store(false, Ordering::SeqCst);
        return Ok(());
    }

    let result = ensure_ap_inner();
    AP_RESTARTING.store(false, Ordering::SeqCst);
    result
}

/// Inner AP startup logic. Caller must hold AP_RESTARTING lock.
fn ensure_ap_inner() -> Result<()> {
    let failures = HOSTAPD_FAILURES.load(Ordering::Relaxed);
    if failures < MAX_HOSTAPD_RETRIES {
        // Start hostapd directly — do NOT delegate to start_ap.sh because
        // it exits early when Encore is running (we ARE Encore).
        info!("AP: starting hostapd (attempt {})", failures + 1);

        let (ap_ssid, ap_pass) = read_ap_credentials();
        let config = format!(
            "interface={iface}\n\
             driver=nl80211\n\
             ssid={ssid}\n\
             hw_mode=g\n\
             channel=6\n\
             ieee80211n=1\n\
             ctrl_interface=/data/wifi\n\
             ignore_broadcast_ssid=0\n\
             wpa=2\n\
             wpa_passphrase={pass}\n\
             wpa_key_mgmt=WPA-PSK\n\
             wpa_pairwise=CCMP\n\
             rsn_pairwise=CCMP\n",
            iface = AP_IFACE,
            ssid = ap_ssid,
            pass = ap_pass,
        );

        std::fs::create_dir_all("/data/wifi").ok();
        let _ = std::fs::write("/data/wifi/hostapd_ap.conf", &config);

        let _ = Command::new("killall").arg("hostapd").output();
        std::thread::sleep(std::time::Duration::from_millis(500));

        let _ = Command::new("/bin/hostapd")
            .args(["-B", "/data/wifi/hostapd_ap.conf"])
            .status();

        // Poll for hostapd to come up instead of blind 3s sleep
        let mut ready = false;
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if interface_has_ip(AP_IFACE, AP_IP) {
                ready = true;
                break;
            }
        }

        if !ready {
            // Set IP ourselves — hostapd may be up but ifconfig not done
            let _ = Command::new("ifconfig")
                .args([AP_IFACE, AP_IP, "netmask", "255.255.255.0", "up"])
                .status();
        }

        ensure_dnsmasq();

        if is_ap_running() {
            info!("AP: started successfully via hostapd");
            HOSTAPD_FAILURES.store(0, Ordering::Relaxed);
            super::AP_FREQ_MHZ.store(2437, std::sync::atomic::Ordering::Relaxed);
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
        USING_UAPUTL.store(true, Ordering::Relaxed);
        // uaputl uses firmware default (2.4GHz auto channel)
        super::AP_FREQ_MHZ.store(2437, std::sync::atomic::Ordering::Relaxed);
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
             listen-address={ip}\n\
             except-interface=lo\n\
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
/// then starts hostapd.
pub fn start_ap_on_band(sta_freq_mhz: u32) -> Result<()> {
    // Acquire restart lock
    if AP_RESTARTING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        info!("AP: restart already in progress, skipping band switch");
        return Ok(());
    }

    let result = start_ap_on_band_inner(sta_freq_mhz);
    AP_RESTARTING.store(false, Ordering::SeqCst);
    result
}

fn start_ap_on_band_inner(sta_freq_mhz: u32) -> Result<()> {
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
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Start hostapd in daemon mode
    let status = Command::new("/bin/hostapd")
        .args(["-B", "/data/wifi/hostapd_ap.conf"])
        .status()
        .map_err(|e| anyhow::anyhow!("failed to start hostapd: {}", e))?;

    if !status.success() {
        warn!("AP: hostapd exited with {}", status);
    }

    // Poll for AP to come up
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if interface_has_ip(AP_IFACE, AP_IP) {
            break;
        }
    }

    if !interface_has_ip(AP_IFACE, AP_IP) {
        let _ = Command::new("ifconfig")
            .args([AP_IFACE, AP_IP, "netmask", "255.255.255.0", "up"])
            .status();
    }

    ensure_dnsmasq();

    if interface_has_ip(AP_IFACE, AP_IP) {
        info!("AP: started on hw_mode={} ch{}", hw_mode, channel);
        HOSTAPD_FAILURES.store(0, Ordering::Relaxed);
        // Cache AP frequency for NetworkState reporting
        let freq_mhz = channel_to_freq(hw_mode, channel);
        super::AP_FREQ_MHZ.store(freq_mhz, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    } else {
        warn!("AP: failed to start on hw_mode={} ch{}", hw_mode, channel);
        Err(anyhow::anyhow!("AP failed to start on hw_mode={} ch{}", hw_mode, channel))
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
    // Kill AP's dnsmasq via its pidfile.
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

/// Convert hostapd hw_mode + channel to frequency in MHz.
fn channel_to_freq(hw_mode: &str, channel: u8) -> u32 {
    if hw_mode == "a" {
        5000 + (channel as u32) * 5
    } else if channel <= 13 {
        2407 + (channel as u32) * 5
    } else {
        2484
    }
}

/// Parse TX error/packet counts and determine if AP is unhealthy.
/// Unhealthy = TX errors > 0 AND TX packets == 0 (interface broken).
fn parse_tx_health(errors: u64, packets: u64) -> bool {
    errors > 0 && packets == 0
}

/// Check if p2p0 has TX errors with no successful packets (broken interface).
pub fn has_tx_errors() -> bool {
    let tx_errors = std::fs::read_to_string("/sys/class/net/p2p0/statistics/tx_errors")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let tx_packets = std::fs::read_to_string("/sys/class/net/p2p0/statistics/tx_packets")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    parse_tx_health(tx_errors, tx_packets)
}

/// Reset p2p0 interface (down/up cycle) to clear stale driver state.
pub fn reset_interface() {
    info!("AP: resetting p2p0 interface to clear TX errors");
    let _ = Command::new("ifconfig").args([AP_IFACE, "down"]).status();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let _ = Command::new("ifconfig").args([AP_IFACE, "up"]).status();
    std::thread::sleep(std::time::Duration::from_secs(1));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_health_no_errors() {
        assert!(!parse_tx_health(0, 100));
    }

    #[test]
    fn tx_health_errors_with_no_packets_is_broken() {
        assert!(parse_tx_health(10, 0));
    }

    #[test]
    fn tx_health_errors_with_packets_is_ok() {
        // Some TX errors but packets are flowing — not broken
        assert!(!parse_tx_health(2, 500));
    }

    #[test]
    fn tx_health_zero_everything_is_ok() {
        // Fresh interface, no traffic yet — not broken
        assert!(!parse_tx_health(0, 0));
    }

    #[test]
    fn channel_to_freq_2g() {
        assert_eq!(channel_to_freq("g", 1), 2412);
        assert_eq!(channel_to_freq("g", 6), 2437);
        assert_eq!(channel_to_freq("g", 11), 2462);
    }

    #[test]
    fn channel_to_freq_5g() {
        assert_eq!(channel_to_freq("a", 36), 5180);
        assert_eq!(channel_to_freq("a", 149), 5745);
    }
}
