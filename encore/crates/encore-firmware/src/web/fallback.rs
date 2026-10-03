//! Fallback TLS identities for the dashboard.
//!
//! The web server normally presents a certificate signed by the device's own
//! CA (see `tls.rs`). When that cannot be produced, for example because
//! `/lsync` is not writable, [`choose`] falls back to two other identities, in
//! order:
//!
//! 1. The `cert.pem` and `key.pem` pair the firmware build installed in
//!    [`BUILD_PAIR_DIR`]. The build creates that pair in `build/tls/` on the
//!    builder's own machine (`scripts/build/gen_tls_fallback.sh`), so the
//!    private key is never part of the source tree or of a published binary.
//! 2. A throwaway self-signed certificate generated in memory at start-up. Its
//!    key is never written to disk and is not shared with any other device.
//!
//! Browsers do not trust either one. They exist so the dashboard stays
//! reachable when the per-device certificate is unavailable.

use anyhow::{bail, Context, Result};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, SanType, PKCS_RSA_SHA256,
};
use std::io::BufReader;
use std::net::IpAddr;
use std::path::Path;
use tracing::warn;

/// Where the firmware build installs the fallback pair (read-only rootfs).
pub const BUILD_PAIR_DIR: &str = "/usr/share/encore/tls";

const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";

/// Addresses a speaker answers on whatever the local network looks like: the
/// setup access point and the USB network gadget.
const FIXED_IPS: [[u8; 4]; 2] = [[192, 168, 43, 1], [10, 55, 55, 1]];

/// Validity window of the throwaway certificate, as Unix timestamps. It is
/// fixed instead of being relative to "now" because the speaker's clock may
/// not be set yet when this runs, and a certificate that is already expired
/// once the clock is corrected would be worse than one with a wide window.
const NOT_BEFORE_UNIX: i64 = 1_577_836_800; // 2020-01-01T00:00:00Z
const NOT_AFTER_UNIX: i64 = 2_524_608_000; // 2050-01-01T00:00:00Z

/// Which fallback identity [`choose`] ended up using.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackSource {
    /// The cert/key pair the firmware build installed.
    BuildPair,
    /// A throwaway certificate generated in memory.
    Throwaway,
}

/// Outcome of comparing a private key with a certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairCheck {
    /// The key belongs to the certificate.
    Match,
    /// The key does not belong to the certificate.
    Mismatch,
    /// One of them is in a form that cannot be read here, so there is no verdict.
    Unverified,
}

/// Pick a fallback identity. `build` turns a PEM certificate and key into
/// whatever the caller needs (a TLS acceptor); a pair that `build` rejects is
/// skipped the same way as a missing one.
pub fn choose<T>(
    dir: &Path,
    hostname: &str,
    local_ips: &[IpAddr],
    build: impl Fn(&[u8], &[u8]) -> Result<T>,
) -> Result<(T, FallbackSource)> {
    let from_build = load_build_pair(dir).and_then(|(cert, key)| {
        match check_pair(&cert, &key) {
            PairCheck::Match => {}
            PairCheck::Mismatch => bail!("{KEY_FILE} is not the private key for {CERT_FILE}"),
            PairCheck::Unverified => warn!(
                "TLS: cannot cross-check {} against {}, using them as they are",
                KEY_FILE, CERT_FILE
            ),
        }
        build(&cert, &key)
    });

    match from_build {
        Ok(built) => Ok((built, FallbackSource::BuildPair)),
        Err(e) => {
            warn!(
                "TLS: no usable build-provided pair in {} ({:#}), using a throwaway in-memory certificate",
                dir.display(),
                e
            );
            let (cert, key) = ephemeral_self_signed(hostname, local_ips)
                .context("generate throwaway TLS certificate")?;
            let built = build(cert.as_bytes(), key.as_bytes())
                .context("use the throwaway TLS certificate")?;
            Ok((built, FallbackSource::Throwaway))
        }
    }
}

/// Read the build-provided `cert.pem` and `key.pem` from `dir`.
pub fn load_build_pair(dir: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let cert_path = dir.join(CERT_FILE);
    let key_path = dir.join(KEY_FILE);
    let cert =
        std::fs::read(&cert_path).with_context(|| format!("read {}", cert_path.display()))?;
    let key = std::fs::read(&key_path).with_context(|| format!("read {}", key_path.display()))?;
    Ok((cert, key))
}

/// Check that `key_pem` is the private key for the first certificate in
/// `cert_pem`. rustls does not make this check when a certificate is
/// installed, so without it a mismatched pair would start up fine and then
/// fail every handshake.
///
/// The key's public bytes must appear in the certificate. A key rcgen cannot
/// read (for example the older SEC1 or PKCS#1 layouts) gets no verdict instead
/// of being rejected.
pub fn check_pair(cert_pem: &[u8], key_pem: &[u8]) -> PairCheck {
    let Some(cert_der) = rustls_pemfile::certs(&mut BufReader::new(cert_pem))
        .next()
        .and_then(|cert| cert.ok())
    else {
        return PairCheck::Unverified;
    };
    let Ok(key_text) = std::str::from_utf8(key_pem) else {
        return PairCheck::Unverified;
    };
    let key = KeyPair::from_pem(key_text)
        .or_else(|_| KeyPair::from_pem_and_sign_algo(key_text, &PKCS_RSA_SHA256));
    let Ok(key) = key else {
        return PairCheck::Unverified;
    };

    let public = key.public_key_raw();
    if public.is_empty() {
        return PairCheck::Unverified;
    }
    if cert_der
        .windows(public.len())
        .any(|window| window == public)
    {
        PairCheck::Match
    } else {
        PairCheck::Mismatch
    }
}

/// Generate a throwaway self-signed server certificate and its private key,
/// both PEM-encoded. A new key is made on every call.
pub fn ephemeral_self_signed(hostname: &str, local_ips: &[IpAddr]) -> Result<(String, String)> {
    let key = KeyPair::generate().context("generate throwaway key pair")?;

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname);

    let mut params = CertificateParams::default();
    params.distinguished_name = dn;
    params.is_ca = IsCa::ExplicitNoCa;
    params.subject_alt_names = subject_alt_names(hostname, local_ips);
    params.not_before = time::OffsetDateTime::from_unix_timestamp(NOT_BEFORE_UNIX)
        .context("certificate start date")?;
    params.not_after = time::OffsetDateTime::from_unix_timestamp(NOT_AFTER_UNIX)
        .context("certificate end date")?;

    let cert = params
        .self_signed(&key)
        .context("self-sign throwaway certificate")?;

    Ok((cert.pem(), key.serialize_pem()))
}

/// Names and addresses the certificate should cover: the mDNS hostname with
/// and without `.local`, the detected local addresses, and the fixed ones.
fn subject_alt_names(hostname: &str, local_ips: &[IpAddr]) -> Vec<SanType> {
    let mut sans = Vec::new();

    if let Ok(name) = hostname.try_into() {
        sans.push(SanType::DnsName(name));
    }
    if let Some(short) = hostname.strip_suffix(".local") {
        if let Ok(name) = short.try_into() {
            sans.push(SanType::DnsName(name));
        }
    }

    let mut ips = local_ips.to_vec();
    for fixed in FIXED_IPS {
        let ip = IpAddr::from(fixed);
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    sans.extend(ips.into_iter().map(SanType::IpAddress));

    sans
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tokio_rustls::rustls::ServerConfig;

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "encore-fallback-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cert_der(cert_pem: &str) -> Vec<u8> {
        let mut reader = BufReader::new(cert_pem.as_bytes());
        let first = rustls_pemfile::certs(&mut reader).next().unwrap().unwrap();
        first.as_ref().to_vec()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn write_pair(dir: &Path, cert: &str, key: &str) {
        std::fs::write(dir.join(CERT_FILE), cert).unwrap();
        std::fs::write(dir.join(KEY_FILE), key).unwrap();
    }

    /// Stand-in for the real acceptor builder: succeeds only if rustls
    /// accepts the certificate and key, as `build_acceptor_from_pem` does.
    fn rustls_builder(cert_pem: &[u8], key_pem: &[u8]) -> Result<ServerConfig> {
        let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(cert_pem))
            .collect::<std::result::Result<_, _>>()
            .context("parse certificate")?;
        let key = rustls_pemfile::private_key(&mut BufReader::new(key_pem))
            .context("parse key")?
            .context("no private key in PEM")?;
        Ok(ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?)
    }

    /// DER encoding of a dNSName entry in subjectAltName: context tag 2, length, name.
    fn dns_entry(name: &str) -> Vec<u8> {
        let mut entry = vec![0x82, name.len() as u8];
        entry.extend_from_slice(name.as_bytes());
        entry
    }

    /// DER encoding of an iPAddress entry in subjectAltName: context tag 7, length 4, octets.
    fn ipv4_entry(octets: [u8; 4]) -> Vec<u8> {
        let mut entry = vec![0x87, 4];
        entry.extend_from_slice(&octets);
        entry
    }

    #[test]
    fn throwaway_pair_is_usable_and_consistent() {
        let (cert, key) = ephemeral_self_signed("encore.local", &[]).unwrap();
        assert!(rustls_builder(cert.as_bytes(), key.as_bytes()).is_ok());
        assert_eq!(
            check_pair(cert.as_bytes(), key.as_bytes()),
            PairCheck::Match
        );
    }

    #[test]
    fn throwaway_pairs_differ_between_calls() {
        let (cert_a, key_a) = ephemeral_self_signed("encore.local", &[]).unwrap();
        let (cert_b, key_b) = ephemeral_self_signed("encore.local", &[]).unwrap();
        assert_ne!(key_a, key_b);
        assert_ne!(cert_a, cert_b);
    }

    #[test]
    fn throwaway_cert_names_the_device() {
        let lan: IpAddr = "192.168.1.77".parse().unwrap();
        let (cert, _key) = ephemeral_self_signed("kitchen.local", &[lan]).unwrap();
        let der = cert_der(&cert);
        // Hostname, and the short name without the .local suffix.
        assert!(contains(&der, &dns_entry("kitchen.local")));
        assert!(contains(&der, &dns_entry("kitchen")));
        // Detected LAN address plus the AP and USB gadget addresses.
        assert!(contains(&der, &ipv4_entry([192, 168, 1, 77])));
        assert!(contains(&der, &ipv4_entry([192, 168, 43, 1])));
        assert!(contains(&der, &ipv4_entry([10, 55, 55, 1])));
    }

    #[test]
    fn throwaway_cert_does_not_duplicate_fixed_addresses() {
        let ap: IpAddr = "192.168.43.1".parse().unwrap();
        let sans = subject_alt_names("encore.local", &[ap]);
        let ap_count = sans
            .iter()
            .filter(|s| matches!(s, SanType::IpAddress(ip) if *ip == ap))
            .count();
        assert_eq!(ap_count, 1);
    }

    #[test]
    fn build_pair_loads_both_files() {
        let dir = scratch_dir("load");
        let (cert, key) = ephemeral_self_signed("encore.local", &[]).unwrap();
        write_pair(&dir, &cert, &key);

        let (loaded_cert, loaded_key) = load_build_pair(&dir).unwrap();
        assert_eq!(loaded_cert, cert.as_bytes());
        assert_eq!(loaded_key, key.as_bytes());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_pair_missing_directory_is_an_error() {
        let dir = scratch_dir("missing").join("does-not-exist");
        let err = load_build_pair(&dir).unwrap_err();
        assert!(format!("{err:#}").contains("cert.pem"));
    }

    #[test]
    fn build_pair_needs_both_files() {
        let dir = scratch_dir("half");
        let (cert, _key) = ephemeral_self_signed("encore.local", &[]).unwrap();
        std::fs::write(dir.join(CERT_FILE), &cert).unwrap();

        let err = load_build_pair(&dir).unwrap_err();
        assert!(format!("{err:#}").contains("key.pem"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_pair_flags_a_key_from_another_certificate() {
        let (cert_a, key_a) = ephemeral_self_signed("encore.local", &[]).unwrap();
        let (_cert_b, key_b) = ephemeral_self_signed("encore.local", &[]).unwrap();
        assert_eq!(
            check_pair(cert_a.as_bytes(), key_a.as_bytes()),
            PairCheck::Match
        );
        assert_eq!(
            check_pair(cert_a.as_bytes(), key_b.as_bytes()),
            PairCheck::Mismatch
        );
    }

    #[test]
    fn check_pair_gives_no_verdict_for_unreadable_input() {
        let (cert, key) = ephemeral_self_signed("encore.local", &[]).unwrap();
        assert_eq!(
            check_pair(b"not a certificate", key.as_bytes()),
            PairCheck::Unverified
        );
        assert_eq!(
            check_pair(cert.as_bytes(), b"not a key"),
            PairCheck::Unverified
        );
        assert_eq!(
            check_pair(cert.as_bytes(), &[0xff, 0xfe, 0x00]),
            PairCheck::Unverified
        );
        assert_eq!(check_pair(b"", b""), PairCheck::Unverified);
    }

    #[test]
    fn rustls_alone_accepts_a_mismatched_pair() {
        // Documents why check_pair exists: this rustls version installs a key
        // that does not belong to the certificate without complaint.
        let (cert_a, _) = ephemeral_self_signed("encore.local", &[]).unwrap();
        let (_, key_b) = ephemeral_self_signed("encore.local", &[]).unwrap();
        assert!(rustls_builder(cert_a.as_bytes(), key_b.as_bytes()).is_ok());
    }

    #[test]
    fn choose_uses_a_valid_build_pair() {
        let dir = scratch_dir("valid");
        let (cert, key) = ephemeral_self_signed("encore.local", &[]).unwrap();
        write_pair(&dir, &cert, &key);

        let (_config, source) = choose(&dir, "encore.local", &[], rustls_builder).unwrap();
        assert_eq!(source, FallbackSource::BuildPair);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choose_throws_away_when_no_pair_is_installed() {
        let dir = scratch_dir("none").join("not-installed");
        let (_config, source) = choose(&dir, "encore.local", &[], rustls_builder).unwrap();
        assert_eq!(source, FallbackSource::Throwaway);
    }

    #[test]
    fn choose_rejects_a_mismatched_build_pair() {
        let dir = scratch_dir("mismatch");
        let (cert, _) = ephemeral_self_signed("encore.local", &[]).unwrap();
        let (_, other_key) = ephemeral_self_signed("encore.local", &[]).unwrap();
        write_pair(&dir, &cert, &other_key);

        let (_config, source) = choose(&dir, "encore.local", &[], rustls_builder).unwrap();
        assert_eq!(source, FallbackSource::Throwaway);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choose_rejects_garbage_in_the_build_pair() {
        let dir = scratch_dir("garbage");
        write_pair(&dir, "not a certificate", "not a key");

        let (_config, source) = choose(&dir, "encore.local", &[], rustls_builder).unwrap();
        assert_eq!(source, FallbackSource::Throwaway);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choose_skips_a_pair_the_builder_refuses() {
        let dir = scratch_dir("refused");
        let (cert, key) = ephemeral_self_signed("encore.local", &[]).unwrap();
        write_pair(&dir, &cert, &key);

        // Refuse the first pair offered (the build pair), accept the next.
        let calls = Cell::new(0);
        let (_, source) = choose(&dir, "encore.local", &[], |c, k| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                bail!("refused")
            }
            rustls_builder(c, k)
        })
        .unwrap();
        assert_eq!(source, FallbackSource::Throwaway);
        assert_eq!(calls.get(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choose_fails_when_even_the_throwaway_is_refused() {
        let dir = scratch_dir("nothing").join("not-installed");
        let result = choose(&dir, "encore.local", &[], |_, _| -> Result<()> {
            bail!("no")
        });
        assert!(result.is_err());
    }
}
