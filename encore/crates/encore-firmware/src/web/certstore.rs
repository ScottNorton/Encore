//! Certificate dates and the on-disk certificate store.
//!
//! A speaker has no battery-backed clock. It boots at 1 January 1970 and NTP sets
//! the real time a little later, so the web server can need a certificate before it
//! knows what day it is. Dating a certificate straight from that clock gives one that
//! is valid only in 1970 and looks expired to every browser, for good, because
//! nothing ever looked at the dates again.
//!
//! This module keeps the dates honest:
//!
//! - While the clock is unset, certificates are dated from the moment the firmware
//!   was built. The real time can never be earlier than that, so they start in the
//!   past and last their full term.
//! - A certificate on disk that starts before 2025 can only have been made with an
//!   unset clock. It is replaced at once, and so is the CA if the CA is just as bad.
//! - Once the clock is set, a certificate that has expired, will within a month, or is
//!   not valid yet is replaced as well.
//!
//! The server certificate lasts 825 days, the longest Apple accepts from a CA the user
//! installed. The CA lasts ten years. Everything lives in one directory and the clock
//! is passed in, so the whole flow can be tested on a desktop.

use anyhow::{anyhow, Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use tracing::{info, warn};

/// File names inside the store directory.
pub const CA_CERT_FILE: &str = "ca.pem";
const CA_KEY_FILE: &str = "ca.key";
const SERVER_CERT_FILE: &str = "server.pem";
const SERVER_KEY_FILE: &str = "server.key";
const SERVER_META_FILE: &str = "server.meta";

const DAY: i64 = 86_400;

/// 1 January 2025, 00:00 UTC. No real clock reading is earlier than this, and neither
/// is the build time of any firmware that has this logic.
const FIRST_SANE_UNIX: i64 = 1_735_689_600;
/// A certificate that starts before this was made while the clock was unset. Two days
/// of slack, because new certificates start a day before the time they were made.
const PLAUSIBLE_START_UNIX: i64 = FIRST_SANE_UNIX - 2 * DAY;
/// New certificates start this long before "now", so a client whose clock runs a little
/// slow does not see a certificate that is not valid yet.
const BACKDATE: i64 = DAY;

const CA_VALID_DAYS: i64 = 3650;
const SERVER_VALID_DAYS: i64 = 825;
const CA_RENEW_BEFORE_DAYS: i64 = 90;
const SERVER_RENEW_BEFORE_DAYS: i64 = 30;

/// When this firmware was built, from build.rs. Zero if it was built without it.
fn build_unix() -> i64 {
    option_env!("ENCORE_BUILD_UNIX")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// What the speaker knows about the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    /// The system clock, in seconds since the Unix epoch.
    pub now: i64,
    /// True once something (NTP) has set the clock. False while it still reads 1970.
    pub set: bool,
    /// The earliest the real time can be: when the firmware was built.
    pub floor: i64,
}

impl Clock {
    /// The clock as it is right now. `set` comes from the NTP check.
    pub fn system(set: bool) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Clock {
            now,
            set,
            floor: build_unix().max(FIRST_SANE_UNIX),
        }
    }

    /// The time to date new certificates from, and to judge old ones by: the clock when
    /// it can be trusted, otherwise the earliest the real time can be.
    pub fn base(&self) -> i64 {
        if self.set {
            self.now
        } else {
            self.floor
        }
    }
}

/// The first and last second a certificate is valid, as Unix times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Validity {
    pub not_before: i64,
    pub not_after: i64,
}

/// Read the validity dates out of the first certificate in a PEM string.
pub fn validity_of_pem(pem: &str) -> Result<Validity> {
    let mut reader = pem.as_bytes();
    let der = rustls_pemfile::certs(&mut reader)
        .next()
        .ok_or_else(|| anyhow!("no certificate in the PEM"))?
        .context("read certificate PEM")?;
    let (_, cert) = x509_parser::parse_x509_certificate(der.as_ref())
        .map_err(|e| anyhow!("parse certificate: {e}"))?;
    let validity = cert.validity();
    Ok(Validity {
        not_before: validity.not_before.timestamp(),
        not_after: validity.not_after.timestamp(),
    })
}

/// What is wrong with a certificate's dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    MadeWithUnsetClock,
    Expired,
    ExpiresSoon,
    NotYetValid,
}

impl Problem {
    fn describe(self) -> &'static str {
        match self {
            Problem::MadeWithUnsetClock => "was made while the clock was unset",
            Problem::Expired => "has expired",
            Problem::ExpiresSoon => "expires soon",
            Problem::NotYetValid => "is not valid yet",
        }
    }
}

/// Decide whether a certificate's dates are a reason to replace it.
///
/// The first rule needs no clock at all, which is what lets a speaker fix a certificate
/// from 1970 at startup, before NTP has run. The last needs a clock that has been set.
pub fn judge(v: &Validity, clock: &Clock, renew_before_days: i64) -> Option<Problem> {
    if v.not_before < PLAUSIBLE_START_UNIX {
        return Some(Problem::MadeWithUnsetClock);
    }
    let base = clock.base();
    if base >= v.not_after {
        return Some(Problem::Expired);
    }
    if base >= v.not_after - renew_before_days * DAY {
        return Some(Problem::ExpiresSoon);
    }
    if clock.set && clock.now < v.not_before {
        return Some(Problem::NotYetValid);
    }
    None
}

/// The dates for a new certificate that should last `valid_days`.
fn window(clock: &Clock, valid_days: i64) -> (i64, i64) {
    let start = clock.base() - BACKDATE;
    (start, start + valid_days * DAY)
}

fn to_datetime(unix: i64) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(unix).context("certificate date out of range")
}

/// Which checks to run on a certificate that is already on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Dates, and whether the hostname or the local IP addresses changed. Used at startup.
    Everything,
    /// Dates only. Used by the later renewal check, so a DHCP change alone does not cause
    /// a rewrite of the certificate files.
    DatesOnly,
}

/// The PEM text the web server needs.
pub struct Material {
    pub ca_pem: Vec<u8>,
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// Make sure `dir` holds a usable CA and server certificate, replacing whatever is
/// missing, unreadable or dated wrongly. Returns the PEM text to serve.
pub fn ensure(
    dir: &Path,
    hostname: &str,
    ips: &[IpAddr],
    clock: &Clock,
    check: Check,
) -> Result<Material> {
    fs::create_dir_all(dir).context("create TLS directory")?;

    let (ca_cert, ca_key, ca_replaced) = load_or_make_ca(dir, clock)?;

    let reason = if ca_replaced {
        Some("the CA was replaced".to_string())
    } else {
        server_problem(dir, hostname, ips, clock, check)
    };

    let (cert, key) = match reason {
        Some(why) => {
            info!(
                "TLS: generating server cert for {} with {} IPs ({})",
                hostname,
                ips.len(),
                why
            );
            let (cert, key) = generate_server(
                &ca_cert,
                &ca_key,
                hostname,
                ips,
                window(clock, SERVER_VALID_DAYS),
            )?;
            // The metadata file goes first and comes back last. If power fails in the
            // middle, there is none, and the next start makes a fresh pair.
            let _ = fs::remove_file(dir.join(SERVER_META_FILE));
            write_file(&dir.join(SERVER_KEY_FILE), &key, true)?;
            write_file(&dir.join(SERVER_CERT_FILE), &cert, false)?;
            write_server_meta(dir, hostname, ips);
            (cert, key)
        }
        None => (
            fs::read_to_string(dir.join(SERVER_CERT_FILE)).context("read server cert")?,
            fs::read_to_string(dir.join(SERVER_KEY_FILE)).context("read server key")?,
        ),
    };

    Ok(Material {
        ca_pem: ca_cert.into_bytes(),
        cert_pem: cert.into_bytes(),
        key_pem: key.into_bytes(),
    })
}

/// Load the CA from disk if it is present and its dates are fine, otherwise make a new
/// one. The last value says whether a new one was made.
fn load_or_make_ca(dir: &Path, clock: &Clock) -> Result<(String, String, bool)> {
    let cert_path = dir.join(CA_CERT_FILE);
    let key_path = dir.join(CA_KEY_FILE);

    if let (Ok(cert), Ok(key)) = (
        fs::read_to_string(&cert_path),
        fs::read_to_string(&key_path),
    ) {
        if !cert.is_empty() && !key.is_empty() {
            match validity_of_pem(&cert) {
                Ok(v) => match judge(&v, clock, CA_RENEW_BEFORE_DAYS) {
                    None => return Ok((cert, key, false)),
                    Some(problem) => warn!(
                        "TLS: the CA certificate {}, replacing it. Install the new /ca.crt on your devices.",
                        problem.describe()
                    ),
                },
                Err(e) => warn!("TLS: the CA certificate is unreadable ({e:#}), replacing it"),
            }
        }
    }

    info!("TLS: generating new private CA");
    let (cert, key) = generate_ca(window(clock, CA_VALID_DAYS))?;

    // The server certificate is signed by the CA being replaced. Drop its metadata first
    // so that an interruption below leads to a new server certificate on the next start.
    let _ = fs::remove_file(dir.join(SERVER_META_FILE));
    // Key before certificate: if power fails between the two, the old certificate is
    // still the bad one, so the next start replaces the CA again.
    write_file(&key_path, &key, true)?;
    write_file(&cert_path, &cert, false)?;
    info!("TLS: CA saved to {}", cert_path.display());

    Ok((cert, key, true))
}

/// Why the server certificate on disk should be replaced, or None if it is fine.
fn server_problem(
    dir: &Path,
    hostname: &str,
    ips: &[IpAddr],
    clock: &Clock,
    check: Check,
) -> Option<String> {
    let cert_path = dir.join(SERVER_CERT_FILE);
    let key_path = dir.join(SERVER_KEY_FILE);

    let meta = match fs::read_to_string(dir.join(SERVER_META_FILE)) {
        Ok(m) => m,
        Err(_) => return Some("no metadata".to_string()),
    };
    if !cert_path.exists() || !key_path.exists() {
        return Some("files missing".to_string());
    }

    if check == Check::Everything {
        let mut lines = meta.lines();
        let stored_hostname = match lines.next() {
            Some(h) => h,
            None => return Some("empty metadata".to_string()),
        };
        if stored_hostname != hostname {
            return Some(format!(
                "hostname changed ({stored_hostname} -> {hostname})"
            ));
        }

        // Compare IPs (order-independent)
        let mut stored_ips: Vec<IpAddr> = lines.filter_map(|l| l.parse().ok()).collect();
        let mut current: Vec<IpAddr> = ips.to_vec();
        stored_ips.sort();
        current.sort();
        if stored_ips != current {
            return Some("IPs changed".to_string());
        }
    }

    let validity = fs::read_to_string(&cert_path)
        .map_err(anyhow::Error::from)
        .and_then(|pem| validity_of_pem(&pem));
    match validity {
        Err(e) => Some(format!("certificate unreadable: {e:#}")),
        Ok(v) => judge(&v, clock, SERVER_RENEW_BEFORE_DAYS)
            .map(|problem| format!("certificate {}", problem.describe())),
    }
}

/// Write server cert metadata (hostname + IPs) for later comparison.
fn write_server_meta(dir: &Path, hostname: &str, ips: &[IpAddr]) {
    let mut content = hostname.to_string();
    for ip in ips {
        content.push('\n');
        content.push_str(&ip.to_string());
    }
    if let Err(e) = write_file(&dir.join(SERVER_META_FILE), &content, false) {
        warn!("TLS: failed to write server metadata: {:#}", e);
    }
}

/// Write a file by way of a temporary name, so a power cut cannot leave half a file.
fn write_file(path: &Path, data: &str, private: bool) -> Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = private;

    let mut file = options
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(data.as_bytes())
        .with_context(|| format!("write {}", tmp.display()))?;
    file.sync_all().ok();
    drop(file);
    fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// Generate a private CA. Returns the certificate and key as PEM.
fn generate_ca(valid: (i64, i64)) -> Result<(String, String)> {
    let key_pair = KeyPair::generate().context("generate CA key pair")?;

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Invoke Local CA");
    dn.push(DnType::OrganizationName, "Invoke");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = to_datetime(valid.0)?;
    params.not_after = to_datetime(valid.1)?;

    let cert = params.self_signed(&key_pair).context("self-sign CA cert")?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

/// Generate a server certificate signed by the CA. Returns the certificate and key as PEM.
fn generate_server(
    ca_cert_pem: &str,
    ca_key_pem: &str,
    hostname: &str,
    local_ips: &[IpAddr],
    valid: (i64, i64),
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
    params.not_before = to_datetime(valid.0)?;
    params.not_after = to_datetime(valid.1)?;

    let cert = params
        .signed_by(&server_key, &issuer)
        .context("sign server cert")?;

    Ok((cert.pem(), server_key.serialize_pem()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 2026-10-03 or so: pretend this is when the firmware was built.
    const BUILT: i64 = 1_791_000_000;

    /// A speaker that has just booted: clock reads 1970, NTP has not run.
    fn unset() -> Clock {
        Clock {
            now: 9,
            set: false,
            floor: BUILT,
        }
    }

    /// A speaker whose clock has been set to `now`.
    fn set_at(now: i64) -> Clock {
        Clock {
            now,
            set: true,
            floor: BUILT,
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "encore-certstore-{}-{}-{}",
                std::process::id(),
                tag,
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn read(&self, file: &str) -> String {
            fs::read_to_string(self.0.join(file)).unwrap()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ips() -> Vec<IpAddr> {
        vec!["192.168.1.42".parse().unwrap()]
    }

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    fn validity(pem: &str) -> Validity {
        validity_of_pem(pem).unwrap()
    }

    /// Write what the old code wrote on a speaker whose clock read 1970-01-01 00:00:09.
    fn write_old_style_material(dir: &Path, hostname: &str, ips: &[IpAddr]) {
        let (ca_cert, ca_key) = generate_ca((9, 9 + CA_VALID_DAYS * DAY)).unwrap();
        let (cert, key) = generate_server(
            &ca_cert,
            &ca_key,
            hostname,
            ips,
            (9, 9 + SERVER_VALID_DAYS * DAY),
        )
        .unwrap();
        write_file(&dir.join(CA_KEY_FILE), &ca_key, true).unwrap();
        write_file(&dir.join(CA_CERT_FILE), &ca_cert, false).unwrap();
        write_file(&dir.join(SERVER_KEY_FILE), &key, true).unwrap();
        write_file(&dir.join(SERVER_CERT_FILE), &cert, false).unwrap();
        write_server_meta(dir, hostname, ips);
    }

    // ---- the date rules -------------------------------------------------

    #[test]
    fn unset_clock_dates_certificates_from_the_build_time() {
        let (start, end) = window(&unset(), SERVER_VALID_DAYS);
        assert_eq!(start, BUILT - DAY);
        assert_eq!(end - start, 825 * DAY);
    }

    #[test]
    fn set_clock_dates_certificates_from_now() {
        let now = BUILT + 40 * DAY;
        let (start, end) = window(&set_at(now), CA_VALID_DAYS);
        assert_eq!(start, now - DAY);
        assert_eq!(end - start, 3650 * DAY);
    }

    #[test]
    fn a_server_certificate_never_spans_more_than_825_days() {
        for clock in [unset(), set_at(BUILT), set_at(BUILT + 1000 * DAY)] {
            let (start, end) = window(&clock, SERVER_VALID_DAYS);
            assert!(end - start <= 825 * DAY);
        }
    }

    #[test]
    fn a_1970_certificate_is_flagged_without_needing_a_clock() {
        let v = Validity {
            not_before: 9,
            not_after: 9 + 825 * DAY,
        };
        for clock in [unset(), set_at(BUILT)] {
            assert_eq!(
                judge(&v, &clock, SERVER_RENEW_BEFORE_DAYS),
                Some(Problem::MadeWithUnsetClock)
            );
        }
    }

    #[test]
    fn a_certificate_dated_from_the_build_time_is_fine_before_and_after_ntp() {
        let (start, end) = window(&unset(), SERVER_VALID_DAYS);
        let v = Validity {
            not_before: start,
            not_after: end,
        };
        assert_eq!(judge(&v, &unset(), SERVER_RENEW_BEFORE_DAYS), None);
        assert_eq!(
            judge(&v, &set_at(BUILT + 3600), SERVER_RENEW_BEFORE_DAYS),
            None
        );
    }

    #[test]
    fn expiry_is_noticed_once_the_clock_is_set_and_a_month_ahead() {
        let (start, end) = window(&unset(), SERVER_VALID_DAYS);
        let v = Validity {
            not_before: start,
            not_after: end,
        };
        let fine = set_at(end - 31 * DAY);
        let soon = set_at(end - 29 * DAY);
        let gone = set_at(end + DAY);
        assert_eq!(judge(&v, &fine, SERVER_RENEW_BEFORE_DAYS), None);
        assert_eq!(
            judge(&v, &soon, SERVER_RENEW_BEFORE_DAYS),
            Some(Problem::ExpiresSoon)
        );
        assert_eq!(
            judge(&v, &gone, SERVER_RENEW_BEFORE_DAYS),
            Some(Problem::Expired)
        );
    }

    #[test]
    fn not_valid_yet_needs_a_clock_that_has_been_set() {
        // Minted by a build machine whose clock ran a month ahead.
        let v = Validity {
            not_before: BUILT + 29 * DAY,
            not_after: BUILT + 29 * DAY + 825 * DAY,
        };
        assert_eq!(judge(&v, &unset(), SERVER_RENEW_BEFORE_DAYS), None);
        assert_eq!(
            judge(&v, &set_at(BUILT), SERVER_RENEW_BEFORE_DAYS),
            Some(Problem::NotYetValid)
        );
    }

    #[test]
    fn validity_reads_back_exactly_what_was_written() {
        let (cert, _key) = generate_ca((BUILT - DAY, BUILT + 100 * DAY)).unwrap();
        assert_eq!(
            validity(&cert),
            Validity {
                not_before: BUILT - DAY,
                not_after: BUILT + 100 * DAY
            }
        );
        assert!(validity_of_pem("not a certificate").is_err());
    }

    // ---- the store ------------------------------------------------------

    #[test]
    fn first_start_with_an_unset_clock_makes_certificates_that_are_valid_today() {
        let dir = TempDir::new("first");
        let m = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        let ca = validity(text(&m.ca_pem));
        let server = validity(text(&m.cert_pem));
        assert_eq!(ca.not_before, BUILT - DAY);
        assert_eq!(ca.not_after - ca.not_before, 3650 * DAY);
        assert_eq!(server.not_before, BUILT - DAY);
        assert_eq!(server.not_after - server.not_before, 825 * DAY);
        // Valid on the real day the speaker is first started, whatever it is.
        let today = BUILT + 5 * DAY;
        assert!(server.not_before <= today && today < server.not_after);
    }

    #[test]
    fn certificates_from_a_1970_clock_are_replaced_at_startup() {
        let dir = TempDir::new("old");
        write_old_style_material(dir.path(), "living-room.local", &ips());
        let old_ca = dir.read(CA_CERT_FILE);
        let old_server = dir.read(SERVER_CERT_FILE);
        assert_eq!(validity(&old_server).not_before, 9);

        let m = ensure(
            dir.path(),
            "living-room.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        assert_ne!(text(&m.ca_pem), old_ca);
        assert_ne!(text(&m.cert_pem), old_server);
        assert_eq!(validity(text(&m.ca_pem)).not_before, BUILT - DAY);
        assert_eq!(validity(text(&m.cert_pem)).not_before, BUILT - DAY);
        // And what is on disk is what was returned.
        assert_eq!(dir.read(CA_CERT_FILE), text(&m.ca_pem));
        assert_eq!(dir.read(SERVER_CERT_FILE), text(&m.cert_pem));
    }

    #[test]
    fn replaced_certificates_stay_put_on_later_starts_and_after_ntp() {
        let dir = TempDir::new("stable");
        write_old_style_material(dir.path(), "living-room.local", &ips());
        let first = ensure(
            dir.path(),
            "living-room.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        for (clock, check) in [
            (unset(), Check::Everything),
            (set_at(BUILT + 3600), Check::DatesOnly),
            (set_at(BUILT + 100 * DAY), Check::Everything),
        ] {
            let again = ensure(dir.path(), "living-room.local", &ips(), &clock, check).unwrap();
            assert_eq!(again.ca_pem, first.ca_pem);
            assert_eq!(again.cert_pem, first.cert_pem);
            assert_eq!(again.key_pem, first.key_pem);
        }
    }

    #[test]
    fn the_new_key_belongs_to_the_new_certificate() {
        let dir = TempDir::new("pair");
        let m = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        let mut reader = m.cert_pem.as_slice();
        let der = rustls_pemfile::certs(&mut reader).next().unwrap().unwrap();
        let key = KeyPair::from_pem(text(&m.key_pem)).unwrap();
        let public = key.public_key_raw();
        assert!(der.windows(public.len()).any(|w| w == public));
    }

    #[test]
    fn a_server_certificate_that_expires_is_renewed_and_the_ca_is_kept() {
        let dir = TempDir::new("renew");
        let first = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        // 800 days later: inside the last month of the 825-day server certificate.
        let later = set_at(BUILT + 800 * DAY);
        let second = ensure(dir.path(), "encore.local", &ips(), &later, Check::DatesOnly).unwrap();

        assert_eq!(second.ca_pem, first.ca_pem);
        assert_ne!(second.cert_pem, first.cert_pem);
        let v = validity(text(&second.cert_pem));
        assert_eq!(v.not_before, later.now - DAY);
        assert_eq!(v.not_after - v.not_before, 825 * DAY);
    }

    #[test]
    fn a_ca_close_to_expiry_is_replaced_together_with_the_server_certificate() {
        let dir = TempDir::new("ca-expiry");
        let first = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        // Ten years less 60 days.
        let later = set_at(BUILT + 3590 * DAY);
        let second = ensure(dir.path(), "encore.local", &ips(), &later, Check::DatesOnly).unwrap();

        assert_ne!(second.ca_pem, first.ca_pem);
        assert_ne!(second.cert_pem, first.cert_pem);
        assert_eq!(validity(text(&second.ca_pem)).not_before, later.now - DAY);
    }

    #[test]
    fn certificates_dated_in_the_future_are_renewed_when_the_clock_proves_it() {
        // The build machine's clock ran a month ahead, so this speaker made certificates
        // that start in the future.
        let ahead = Clock {
            now: 9,
            set: false,
            floor: BUILT + 30 * DAY,
        };
        let dir = TempDir::new("future");
        let first = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &ahead,
            Check::Everything,
        )
        .unwrap();
        assert_eq!(validity(text(&first.cert_pem)).not_before, BUILT + 29 * DAY);

        let real = Clock {
            now: BUILT,
            set: true,
            floor: BUILT + 30 * DAY,
        };
        let second = ensure(dir.path(), "encore.local", &ips(), &real, Check::DatesOnly).unwrap();
        assert_ne!(second.cert_pem, first.cert_pem);
        assert_eq!(validity(text(&second.cert_pem)).not_before, BUILT - DAY);
    }

    #[test]
    fn a_hostname_or_ip_change_replaces_only_the_server_certificate_at_startup() {
        let dir = TempDir::new("identity");
        let first = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        let moved: Vec<IpAddr> = vec!["192.168.1.99".parse().unwrap()];
        let second = ensure(
            dir.path(),
            "encore.local",
            &moved,
            &unset(),
            Check::Everything,
        )
        .unwrap();
        assert_eq!(second.ca_pem, first.ca_pem);
        assert_ne!(second.cert_pem, first.cert_pem);

        let third = ensure(
            dir.path(),
            "kitchen.local",
            &moved,
            &unset(),
            Check::Everything,
        )
        .unwrap();
        assert_eq!(third.ca_pem, first.ca_pem);
        assert_ne!(third.cert_pem, second.cert_pem);
    }

    #[test]
    fn the_dates_only_check_does_not_rewrite_files_for_a_dhcp_change() {
        let dir = TempDir::new("dates-only");
        let first = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        let moved: Vec<IpAddr> = vec!["10.0.0.7".parse().unwrap()];
        let second = ensure(
            dir.path(),
            "other.local",
            &moved,
            &set_at(BUILT + 3600),
            Check::DatesOnly,
        )
        .unwrap();
        assert_eq!(second.cert_pem, first.cert_pem);
    }

    #[test]
    fn damaged_files_are_replaced_instead_of_failing_every_start() {
        let dir = TempDir::new("damaged");
        let first = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        // A server certificate cut short.
        fs::write(dir.path().join(SERVER_CERT_FILE), "-----BEGIN CERT").unwrap();
        let second = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();
        assert_eq!(second.ca_pem, first.ca_pem);
        assert!(validity_of_pem(text(&second.cert_pem)).is_ok());

        // A CA that is not a certificate at all.
        fs::write(dir.path().join(CA_CERT_FILE), "garbage").unwrap();
        let third = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();
        assert_ne!(third.ca_pem, first.ca_pem);
        assert_ne!(third.cert_pem, second.cert_pem);
    }

    #[test]
    fn an_interrupted_ca_swap_is_repaired_on_the_next_start() {
        let dir = TempDir::new("interrupted");
        let first = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();

        // Power failed after the new CA was written and before the new server certificate
        // was: the metadata is gone and the server certificate still has the old issuer.
        let (ca_cert, ca_key) = generate_ca(window(&unset(), CA_VALID_DAYS)).unwrap();
        fs::remove_file(dir.path().join(SERVER_META_FILE)).unwrap();
        write_file(&dir.path().join(CA_KEY_FILE), &ca_key, true).unwrap();
        write_file(&dir.path().join(CA_CERT_FILE), &ca_cert, false).unwrap();

        let second = ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::DatesOnly,
        )
        .unwrap();
        assert_eq!(text(&second.ca_pem), ca_cert);
        assert_ne!(second.cert_pem, first.cert_pem);
    }

    #[cfg(unix)]
    #[test]
    fn private_keys_are_not_readable_by_other_users() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("modes");
        ensure(
            dir.path(),
            "encore.local",
            &ips(),
            &unset(),
            Check::Everything,
        )
        .unwrap();
        for file in [CA_KEY_FILE, SERVER_KEY_FILE] {
            let mode = fs::metadata(dir.path().join(file))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "{file} is group or world accessible");
        }
    }
}
