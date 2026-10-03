//! Private CA and server certificate for trusted TLS.
//!
//! On first boot, generates a private CA (stable, never changes) and a
//! server certificate signed by that CA. The CA cert is served at `/ca.crt`
//! for users to install in their OS trust store. Once trusted, HTTPS gets
//! a green padlock and PWA install works.
//!
//! Server cert regenerates when the device hostname or local IPs change. Both
//! certificates are also replaced when their dates are wrong, which matters on a
//! speaker whose clock is not set yet. The date rules and the file handling live
//! in `certstore.rs`.

use super::certstore::{self, Check, Clock};
use anyhow::Result;
use axum::http::header;
use axum::response::IntoResponse;
use std::net::IpAddr;
use std::path::Path;

const TLS_DIR: &str = "/lsync/encore/tls";

/// Ensure CA and server certs exist and are up-to-date. Run at startup.
/// Returns `(ca_pem, server_cert_pem, server_key_pem)`.
pub fn ensure_certs(device_name: &str) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    load(device_name, Check::Everything)
}

/// Look at the certificate dates again, once the clock has been set and then daily.
/// Unlike `ensure_certs` it ignores hostname and IP changes, so a DHCP change alone
/// does not rewrite the certificate files.
pub fn renew_certs(device_name: &str) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    load(device_name, Check::DatesOnly)
}

fn load(device_name: &str, check: Check) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let hostname = crate::network::sanitize_hostname(device_name);
    let local_ips = detect_local_ips();
    let clock = Clock::system(crate::network::ntp::is_time_synced());
    let material = certstore::ensure(Path::new(TLS_DIR), &hostname, &local_ips, &clock, check)?;
    Ok((material.ca_pem, material.cert_pem, material.key_pem))
}

/// Detect all non-loopback IPv4 addresses on the system.
pub(super) fn detect_local_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();

    // Parse /proc/net/fib_trie for local IPs (Linux-specific, most reliable)
    // Format: lines with "|-- x.x.x.x" followed by "/32 host LOCAL"
    if let Ok(content) = std::fs::read_to_string("/proc/net/fib_trie") {
        let lines: Vec<&str> = content.lines().collect();
        for i in 0..lines.len() {
            let trimmed = lines[i].trim();
            if trimmed.contains("/32 host LOCAL") {
                // The IP address is on the previous "|-- " line
                if i > 0 {
                    let prev = lines[i - 1].trim();
                    if let Some(ip_str) = prev.strip_prefix("|-- ") {
                        if let Ok(ip) = ip_str.trim().parse::<IpAddr>() {
                            if !ip.is_loopback() && !ips.contains(&ip) {
                                ips.push(ip);
                            }
                        }
                    }
                }
            }
        }
    }

    ips
}

/// HTTP handler: serve CA certificate for download.
pub async fn ca_cert_handler() -> impl IntoResponse {
    match std::fs::read(Path::new(TLS_DIR).join(certstore::CA_CERT_FILE)) {
        Ok(data) => (
            [
                (header::CONTENT_TYPE, "application/x-pem-file"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"encore-ca.crt\"",
                ),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            data,
        )
            .into_response(),
        Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_ips_does_not_panic() {
        let ips = detect_local_ips();
        let _ = ips;
    }
}
