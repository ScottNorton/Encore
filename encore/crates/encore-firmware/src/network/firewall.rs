//! Firewall enforcement via iptables.
//!
//! CRITICAL: The device must NEVER communicate outside the local network.
//! This module applies iptables rules to restrict inbound traffic to LAN only.
//!
//! Fail-safe design: INPUT=DROP is only applied AFTER all ACCEPT rules
//! succeed. If any ACCEPT rule fails, INPUT stays ACCEPT to avoid lockout.

use anyhow::Result;
use std::process::Command;
use tracing::{info, warn};

/// Apply firewall rules. Matches rootfs/etc/init.d/S01firewall.
pub fn apply() -> Result<()> {
    info!("Firewall: applying iptables rules");

    // Test if iptables works at all
    if !iptables_ok(&["-L", "-n"]) {
        warn!("Firewall: iptables not functional, skipping firewall");
        return Ok(());
    }

    // Start with safe defaults — INPUT=ACCEPT until all rules are in place
    let _ = iptables_run(&["-P", "INPUT", "ACCEPT"]);
    let _ = iptables_run(&["-P", "FORWARD", "DROP"]);
    let _ = iptables_run(&["-P", "OUTPUT", "ACCEPT"]);

    // Flush existing rules
    let _ = iptables_run(&["-F"]);
    let _ = iptables_run(&["-X"]);

    // Track whether critical ACCEPT rules succeed
    let mut accepts_ok = true;

    // Allow loopback
    accepts_ok &= iptables_ok(&["-A", "INPUT", "-i", "lo", "-j", "ACCEPT"]);

    // Allow established/related — try conntrack first, fall back to state module.
    // The state module (xt_state) is the older interface and available on kernel 3.8.
    // Without this rule, outbound TCP connections (Spotify, NTP, etc.) will break
    // because SYN-ACK responses from non-LAN IPs get DROP'd.
    if !iptables_ok(&["-A", "INPUT", "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED", "-j", "ACCEPT"]) {
        if !iptables_ok(&["-A", "INPUT", "-m", "state", "--state", "ESTABLISHED,RELATED", "-j", "ACCEPT"]) {
            warn!("Firewall: neither conntrack nor state module available — outbound connections will break");
        } else {
            info!("Firewall: using state module (conntrack unavailable)");
        }
    }

    // Allow LAN ranges — these are critical
    accepts_ok &= iptables_ok(&["-A", "INPUT", "-s", "192.168.0.0/16", "-j", "ACCEPT"]);
    accepts_ok &= iptables_ok(&["-A", "INPUT", "-s", "10.0.0.0/8", "-j", "ACCEPT"]);
    accepts_ok &= iptables_ok(&["-A", "INPUT", "-s", "172.16.0.0/12", "-j", "ACCEPT"]);
    accepts_ok &= iptables_ok(&["-A", "INPUT", "-s", "169.254.0.0/16", "-j", "ACCEPT"]);

    // Allow DHCP
    let _ = iptables_ok(&["-A", "INPUT", "-p", "udp", "--dport", "67:68", "-j", "ACCEPT"]);

    // Allow multicast (mDNS, Spotify Connect discovery)
    let _ = iptables_ok(&["-A", "INPUT", "-d", "224.0.0.0/4", "-j", "ACCEPT"]);

    // Only apply INPUT=DROP if all critical ACCEPT rules succeeded
    if accepts_ok {
        iptables_run(&["-P", "INPUT", "DROP"]);
        info!("Firewall: rules applied (INPUT=DROP, LAN=ACCEPT)");
    } else {
        warn!("Firewall: critical ACCEPT rules failed, leaving INPUT=ACCEPT for safety");
    }

    Ok(())
}

/// Run iptables command, return true if it succeeded.
fn iptables_ok(args: &[&str]) -> bool {
    match Command::new("/usr/sbin/iptables").args(args).output() {
        Ok(output) => {
            if output.status.success() {
                true
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("iptables {:?} failed: {}", args, stderr.trim());
                false
            }
        }
        Err(e) => {
            warn!("iptables {:?} failed to run: {}", args, e);
            false
        }
    }
}

/// Run iptables command, log failures but don't return status.
fn iptables_run(args: &[&str]) {
    iptables_ok(args);
}
