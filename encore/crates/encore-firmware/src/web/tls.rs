//! Private CA and server certificate generation for trusted TLS.
//!
//! On first boot, generates a private CA (stable, never changes) and a
//! server certificate signed by that CA. The CA cert is served at `/ca.crt`
//! for users to install in their OS trust store. Once trusted, HTTPS gets
//! a green padlock and PWA install works.
//!
//! Server cert regenerates when the device hostname or local IPs change.

use anyhow::{Context, Result};
use axum::http::header;
use axum::response::IntoResponse;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use std::net::IpAddr;
use tracing::{info, warn};

const TLS_DIR: &str = "/lsync/encore/tls";
const CA_CERT_PATH: &str = "/lsync/encore/tls/ca.pem";
const CA_KEY_PATH: &str = "/lsync/encore/tls/ca.key";
const SERVER_CERT_PATH: &str = "/lsync/encore/tls/server.pem";
const SERVER_KEY_PATH: &str = "/lsync/encore/tls/server.key";
const SERVER_META_PATH: &str = "/lsync/encore/tls/server.meta";

/// Ensure CA and server certs exist and are up-to-date.
/// Returns `(ca_pem, server_cert_pem, server_key_pem)`.
pub fn ensure_certs(device_name: &str) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    std::fs::create_dir_all(TLS_DIR).context("create TLS directory")?;

    let hostname = crate::network::sanitize_hostname(device_name);
    let local_ips = detect_local_ips();

    // 1. Load or generate CA
    let (ca_cert_pem, ca_key_pem) = load_or_generate_ca()?;

    // 2. Check if server cert needs regeneration
    let needs_regen = needs_regeneration(&hostname, &local_ips);

    let (server_cert_pem, server_key_pem) = if needs_regen {
        info!(
            "TLS: generating server cert for {} with {} IPs",
            hostname,
            local_ips.len()
        );
        let (cert, key) = generate_server_cert(&ca_cert_pem, &ca_key_pem, &hostname, &local_ips)?;
        std::fs::write(SERVER_CERT_PATH, &cert).context("write server cert")?;
        std::fs::write(SERVER_KEY_PATH, &key).context("write server key")?;
        write_server_meta(&hostname, &local_ips);
        (cert, key)
    } else {
        let cert = std::fs::read_to_string(SERVER_CERT_PATH).context("read server cert")?;
        let key = std::fs::read_to_string(SERVER_KEY_PATH).context("read server key")?;
        (cert, key)
    };

    Ok((
        ca_cert_pem.into_bytes(),
        server_cert_pem.into_bytes(),
        server_key_pem.into_bytes(),
    ))
}

/// Load existing CA from disk, or generate a new one.
fn load_or_generate_ca() -> Result<(String, String)> {
    if let (Ok(cert), Ok(key)) = (
        std::fs::read_to_string(CA_CERT_PATH),
        std::fs::read_to_string(CA_KEY_PATH),
    ) {
        if !cert.is_empty() && !key.is_empty() {
            return Ok((cert, key));
        }
    }

    info!("TLS: generating new private CA");

    let key_pair = KeyPair::generate().context("generate CA key pair")?;

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Invoke Local CA");
    dn.push(DnType::OrganizationName, "Invoke");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    // Valid for 10 years
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(3650);

    let cert = params.self_signed(&key_pair).context("self-sign CA cert")?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    std::fs::write(CA_CERT_PATH, &cert_pem).context("write CA cert")?;
    std::fs::write(CA_KEY_PATH, &key_pem).context("write CA key")?;

    info!("TLS: CA saved to {}", CA_CERT_PATH);

    Ok((cert_pem, key_pem))
}

/// Generate a server certificate signed by the CA.
fn generate_server_cert(
    ca_cert_pem: &str,
    ca_key_pem: &str,
    hostname: &str,
    local_ips: &[IpAddr],
) -> Result<(String, String)> {
    // Reconstruct CA issuer from saved PEM
    let ca_key = KeyPair::from_pem(ca_key_pem).context("parse CA key")?;
    let issuer =
        Issuer::from_ca_cert_pem(ca_cert_pem, &ca_key).context("parse CA cert for issuer")?;

    // Generate server key pair
    let server_key = KeyPair::generate().context("generate server key pair")?;

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname);
    params.distinguished_name = dn;
    params.is_ca = IsCa::ExplicitNoCa;

    // Build SANs
    let mut sans: Vec<SanType> = Vec::new();

    // DNS SAN: the mDNS hostname (e.g., "encore.local")
    if let Ok(ia5) = hostname.try_into() {
        sans.push(SanType::DnsName(ia5));
    }

    // Also add hostname without .local suffix
    if let Some(short) = hostname.strip_suffix(".local") {
        if let Ok(ia5) = short.try_into() {
            sans.push(SanType::DnsName(ia5));
        }
    }

    // IP SANs: all detected local IPs
    for ip in local_ips {
        sans.push(SanType::IpAddress(*ip));
    }

    // Always include AP mode IP
    let ap_ip: IpAddr = "192.168.43.1".parse().unwrap();
    if !local_ips.contains(&ap_ip) {
        sans.push(SanType::IpAddress(ap_ip));
    }

    params.subject_alt_names = sans;

    // Valid for 825 days (industry max for private certs)
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(825);

    let cert = params
        .signed_by(&server_key, &issuer)
        .context("sign server cert")?;

    Ok((cert.pem(), server_key.serialize_pem()))
}

/// Detect all non-loopback IPv4 addresses on the system.
fn detect_local_ips() -> Vec<IpAddr> {
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

/// Check if the server cert needs to be regenerated.
fn needs_regeneration(hostname: &str, current_ips: &[IpAddr]) -> bool {
    let meta = match std::fs::read_to_string(SERVER_META_PATH) {
        Ok(m) => m,
        Err(_) => return true,
    };

    if !std::path::Path::new(SERVER_CERT_PATH).exists()
        || !std::path::Path::new(SERVER_KEY_PATH).exists()
    {
        return true;
    }

    let mut lines = meta.lines();
    let stored_hostname = match lines.next() {
        Some(h) => h,
        None => return true,
    };

    if stored_hostname != hostname {
        info!(
            "TLS: hostname changed ({} -> {}), regenerating",
            stored_hostname, hostname
        );
        return true;
    }

    // Compare IPs (order-independent)
    let mut stored_ips: Vec<IpAddr> = lines.filter_map(|l| l.parse().ok()).collect();
    let mut current_sorted: Vec<IpAddr> = current_ips.to_vec();
    stored_ips.sort();
    current_sorted.sort();

    if stored_ips != current_sorted {
        info!("TLS: IPs changed, regenerating server cert");
        return true;
    }

    false
}

/// Write server cert metadata (hostname + IPs) for later comparison.
fn write_server_meta(hostname: &str, ips: &[IpAddr]) {
    let mut content = hostname.to_string();
    for ip in ips {
        content.push('\n');
        content.push_str(&ip.to_string());
    }
    if let Err(e) = std::fs::write(SERVER_META_PATH, &content) {
        warn!("TLS: failed to write server metadata: {}", e);
    }
}

/// HTTP handler: serve CA certificate for download.
pub async fn ca_cert_handler() -> impl IntoResponse {
    match std::fs::read(CA_CERT_PATH) {
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

    #[test]
    fn needs_regen_when_no_meta() {
        assert!(needs_regeneration("encore.local", &[]));
    }

    #[test]
    fn server_meta_roundtrip() {
        let hostname = "encore.local";
        let ips: Vec<IpAddr> = vec![
            "127.0.0.1".parse().unwrap(),
            "192.168.43.1".parse().unwrap(),
        ];

        let mut content = hostname.to_string();
        for ip in &ips {
            content.push('\n');
            content.push_str(&ip.to_string());
        }

        let mut lines = content.lines();
        assert_eq!(lines.next().unwrap(), "encore.local");
        let parsed_ips: Vec<IpAddr> = lines.filter_map(|l| l.parse().ok()).collect();
        assert_eq!(parsed_ips, ips);
    }
}
