//! WiFi TSF sampling — the group's shared hardware clock.
//!
//! Every 802.11 station hardware-adopts the AP's TSF from beacons: all
//! speakers on the same BSS carry the same microsecond counter, disciplined
//! by the AP's crystal, with no network exchange required. Sampling
//! `(tsf, CLOCK_MONOTONIC)` pairs on each device and sharing the leader's
//! mapping turns clock sync from an estimation problem (NTP over a radio
//! whose RTT wobbles by tens of ms) into a read: the only error left is the
//! read-latency bracket, which is small enough to certify sub-millisecond
//! offsets through the existing ClockSync machinery.
//!
//! The read path is the stock `mlanutl` host-command interface — the SD8887
//! firmware answers GET_TSF (0x0080) even though the kernel driver exposes no
//! TSF API. Forking mlanutl per sample is deliberate laziness: at one sample
//! every few seconds the exec cost is nothing, and the bracket timestamps
//! honestly capture whatever jitter it adds.
// ponytail: exec-per-sample via mlanutl; switch to a direct SIOCDEVPRIVATE
// ioctl if bracket widths ever stop certifying sub-ms.

#[cfg(target_os = "linux")]
use super::clock;
use std::io::Write;
#[cfg(target_os = "linux")]
use tracing::debug;
use tracing::info;

/// Where the host-command config lives; written once at startup.
const CONF_PATH: &str = "/run/encore_tsf.conf";
const CONF_BODY: &str = "gettsf={\n    CmdCode=0x0080\n}\n";

/// One TSF sample: the shared counter value and the local monotonic
/// microsecond it was read at, plus the read-latency bracket as its
/// uncertainty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TsfSample {
    pub tsf_us: u64,
    pub mono_us: u64,
    /// Half the read bracket: the true pairing instant is within
    /// `mono_us ± err_us`.
    pub err_us: u32,
}

/// Write the host-command config file (idempotent).
pub fn ensure_conf() -> std::io::Result<()> {
    let mut f = std::fs::File::create(CONF_PATH)?;
    f.write_all(CONF_BODY.as_bytes())
}

/// Parse the payload hex line out of mlanutl hostcmd output, e.g.
/// `de 50 8f a1 64 00 00 00` → little-endian u64. Returns None on any
/// unexpected shape (failed command, short payload, non-hex).
pub fn parse_tsf_output(out: &str) -> Option<u64> {
    // Success is Result=0000 in the HOSTCMD_RESP line; anything else is a
    // firmware rejection and the payload (if any) is garbage.
    if !out.contains("Result=0000") {
        return None;
    }
    let hex_line = out
        .lines()
        .map(str::trim)
        .find(|l| l.len() >= 23 && l.split(' ').count() >= 8 && l.split(' ').all(is_hex_byte))?;
    let mut bytes = [0u8; 8];
    for (i, tok) in hex_line.split(' ').take(8).enumerate() {
        bytes[i] = u8::from_str_radix(tok, 16).ok()?;
    }
    Some(u64::from_le_bytes(bytes))
}

fn is_hex_byte(tok: &str) -> bool {
    tok.len() == 2 && tok.chars().all(|c| c.is_ascii_hexdigit())
}

/// Read the TSF once. Prefers the direct MLAN_ETH_PRIV ioctl (bracket ≈ tens
/// of µs → sub-ms certified offsets); falls back to exec-ing mlanutl (bracket
/// ≈ ms — still asymmetry-free and immune to network congestion). The first
/// successful direct read is cross-validated against an exec read; a mismatch
/// pins the exec path permanently.
#[cfg(target_os = "linux")]
pub fn sample() -> Option<TsfSample> {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0 = untested, 1 = direct trusted, 2 = exec only.
    static MODE: AtomicU8 = AtomicU8::new(0);

    match MODE.load(Ordering::Relaxed) {
        1 => direct::sample().or_else(sample_exec),
        2 => sample_exec(),
        _ => {
            if let Some(d) = direct::sample() {
                // Cross-validate the layout guess once against mlanutl.
                if let Some(e) = sample_exec() {
                    let gap = (d.tsf_us as i64 - e.tsf_us as i64).unsigned_abs();
                    let elapsed = e.mono_us.saturating_sub(d.mono_us) + 100_000;
                    if gap <= elapsed {
                        info!("TSF: direct ioctl validated against mlanutl (gap {gap}us)");
                        MODE.store(1, Ordering::Relaxed);
                        return Some(d);
                    }
                    debug!("TSF: direct ioctl disagreed with mlanutl ({gap}us), pinning exec");
                }
                MODE.store(2, Ordering::Relaxed);
                return sample_exec();
            }
            debug!("TSF: direct ioctl unavailable, pinning exec path");
            MODE.store(2, Ordering::Relaxed);
            sample_exec()
        }
    }
}

/// Exec-based read: fork mlanutl and bracket the whole exec.
#[cfg(target_os = "linux")]
fn sample_exec() -> Option<TsfSample> {
    /// A read this slow says nothing about when the counter was latched.
    const MAX_BRACKET_US: u64 = 30_000;

    let before = clock::now_us();
    let out = std::process::Command::new("mlanutl")
        .args(["wlan0", "hostcmd", CONF_PATH, "gettsf"])
        .output()
        .ok()?;
    let after = clock::now_us();

    let bracket = after.saturating_sub(before);
    if bracket > MAX_BRACKET_US {
        debug!("TSF: read bracket too wide ({bracket}us), dropping sample");
        return None;
    }
    let tsf_us = parse_tsf_output(&String::from_utf8_lossy(&out.stdout))?;
    Some(TsfSample {
        tsf_us,
        // The firmware latches the counter somewhere inside the exec; the
        // midpoint with ±bracket/2 uncertainty is the honest pairing.
        mono_us: before + bracket / 2,
        err_us: (bracket / 2) as u32,
    })
}

/// Direct MLAN_ETH_PRIV (0x89FE) host-command read — the same bytes mlanutl
/// sends (marker "MRVL_CMD" + verb "hostcmd" + le32 length + HostCmd), minus
/// the fork/exec. Layout reverse-engineered from the on-device tool; any
/// mismatch fails closed (ioctl error or no header match → None → exec path).
#[cfg(target_os = "linux")]
mod direct {
    use super::{clock, TsfSample};

    // musl's ioctl takes c_int for the request (glibc takes c_ulong).
    const MLAN_ETH_PRIV: libc::c_int = 0x89FE;
    const HOSTCMD_GET_TSF: u16 = 0x0080;

    #[repr(C)]
    struct EthPrivCmd {
        buf: *mut u8,
        used_len: u32,
        total_len: u32,
    }

    #[repr(C)]
    struct IfReq {
        name: [u8; 16],
        data: *mut EthPrivCmd,
        _pad: [u8; 12], // ifreq union is 16 bytes; pointer fills 4 on armv7
    }

    pub fn sample() -> Option<TsfSample> {
        let mut buf = [0u8; 256];
        let marker = b"MRVL_CMDhostcmd";
        buf[..marker.len()].copy_from_slice(marker);
        let mut off = marker.len();
        // le32 command length, then the HostCmd: code, size, seq, result + 8B payload.
        buf[off..off + 4].copy_from_slice(&16u32.to_le_bytes());
        off += 4;
        buf[off..off + 2].copy_from_slice(&HOSTCMD_GET_TSF.to_le_bytes());
        buf[off + 2..off + 4].copy_from_slice(&16u16.to_le_bytes());
        // seq/result/payload stay zero.

        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            return None;
        }
        let mut cmd = EthPrivCmd {
            buf: buf.as_mut_ptr(),
            used_len: (off + 16) as u32,
            total_len: buf.len() as u32,
        };
        let mut req = IfReq {
            name: [0; 16],
            data: &mut cmd,
            _pad: [0; 12],
        };
        req.name[..5].copy_from_slice(b"wlan0");

        let before = clock::now_us();
        let ret = unsafe { libc::ioctl(fd, MLAN_ETH_PRIV, &mut req) };
        let after = clock::now_us();
        unsafe { libc::close(fd) };
        if ret != 0 {
            return None;
        }

        // Find the response HostCmd header (code 0x0080, size 0x0010,
        // result 0) wherever the driver placed it, and take the 8 bytes after.
        let pat = [0x80u8, 0x00, 0x10, 0x00];
        for i in 0..buf.len().saturating_sub(16) {
            if buf[i..i + 4] == pat && buf[i + 6] == 0 && buf[i + 7] == 0 {
                let tsf_us = u64::from_le_bytes(buf[i + 8..i + 16].try_into().ok()?);
                // An idle counter or implausibly small value means we parsed
                // padding, not the clock.
                if tsf_us < 1_000_000 {
                    return None;
                }
                let bracket = after.saturating_sub(before);
                return Some(TsfSample {
                    tsf_us,
                    mono_us: before + bracket / 2,
                    err_us: (bracket / 2).max(20) as u32,
                });
            }
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
pub fn sample() -> Option<TsfSample> {
    None
}

/// Synthesize an NTP-style exchange from the leader's TSF mapping and our
/// own, so the sample flows through the existing ClockSync estimator (min-RTT
/// anchoring, certified bound, stable mapping) unchanged.
///
/// Both mappings anchor the SAME hardware counter, so
/// `offset = leader_mono_at(tsf) − our_mono_at(tsf)`; evaluated at a common
/// tsf instant the network path drops out entirely. The synthetic exchange
/// encodes that offset with an "RTT" equal to the combined read brackets —
/// the honest uncertainty — as `(t1, t2, t3, t4)` with
/// `t2 = t3 = our_anchor + offset ± 0` and `t1/t4` straddling by the bracket.
///
/// Extrapolation: the leader's mapping is carried to OUR sample's tsf instant
/// assuming 1 µs/µs (both counters are the same oscillator — the AP's — so
/// the only error is each device's crystal drift over the extrapolation gap;
/// keep the gap short by pairing fresh samples).
pub fn synthesize_exchange(leader: &TsfSample, ours: &TsfSample) -> (u64, u64, u64, u64) {
    // Leader's monotonic clock at OUR tsf instant (1 µs of tsf = 1 µs of time).
    let leader_mono_at_our_tsf = leader
        .mono_us
        .wrapping_add(ours.tsf_us.wrapping_sub(leader.tsf_us));
    let uncertainty = (leader.err_us as u64) + (ours.err_us as u64);
    // NTP arithmetic recovers offset = t2 - t1 - rtt/2 ... with
    // t2 = t3 = remote time at the midpoint and t1/t4 straddling ours.mono_us
    // by the combined bracket: offset = ((t2-t1)+(t3-t4))/2 = remote - local,
    // rtt = (t4-t1) = 2*uncertainty.
    let t1 = ours.mono_us.saturating_sub(uncertainty);
    let t4 = ours.mono_us + uncertainty;
    (t1, leader_mono_at_our_tsf, leader_mono_at_our_tsf, t4)
}

pub fn log_startup(available: bool) {
    if available {
        info!("TSF: shared WiFi clock available (GET_TSF answered)");
    } else {
        info!("TSF: unavailable, staying on UDP clock sync");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_OUTPUT: &str = "HOSTCMD_RESP: CmdCode=0x80, Size=0x10, SeqNum=0x67, Result=0000\npayload: len=8\nde 50 8f a1 64 00 00 00\n";

    #[test]
    fn parses_real_mlanutl_output() {
        // Bytes are little-endian: 0x64A18F50DE.
        assert_eq!(parse_tsf_output(REAL_OUTPUT), Some(0x64A18F50DE));
    }

    #[test]
    fn rejects_failed_command_and_garbage() {
        assert_eq!(
            parse_tsf_output("HOSTCMD failed: CmdCode=0x80, Size=0x10, SeqNum=0x1, Result=0002\n"),
            None
        );
        assert_eq!(parse_tsf_output(""), None);
        assert_eq!(
            parse_tsf_output("HOSTCMD_RESP: Result=0000\npayload: len=8\nzz 50 8f a1 64 00 00 00\n"),
            None
        );
        // Success line but payload missing entirely.
        assert_eq!(parse_tsf_output("HOSTCMD_RESP: Result=0000\n"), None);
    }

    #[test]
    fn synthetic_exchange_recovers_offset_with_bracket_rtt() {
        // Leader read tsf=1_000_000 at its mono 5_000_000 (±100µs);
        // we read tsf=1_400_000 at our mono 9_100_000 (±150µs).
        // Leader's mono at our tsf instant = 5_000_000 + 400_000 = 5_400_000
        // → true offset (leader − ours) = 5_400_000 − 9_100_000 = −3_700_000.
        let leader = TsfSample { tsf_us: 1_000_000, mono_us: 5_000_000, err_us: 100 };
        let ours = TsfSample { tsf_us: 1_400_000, mono_us: 9_100_000, err_us: 150 };
        let (t1, t2, t3, t4) = synthesize_exchange(&leader, &ours);
        let offset = ((t2 as i128 - t1 as i128) + (t3 as i128 - t4 as i128)) / 2;
        let rtt = (t4 as i128 - t1 as i128) - (t3 as i128 - t2 as i128);
        assert_eq!(offset, -3_700_000);
        assert_eq!(rtt, 500, "rtt must equal the combined bracket (2×250)");
    }

    #[test]
    fn synthetic_exchange_feeds_clocksync_to_a_certified_anchor() {
        use super::super::clock::ClockSync;
        let mut c = ClockSync::new();
        for i in 0..4u64 {
            let leader = TsfSample {
                tsf_us: 1_000_000 + i * 2_000_000,
                mono_us: 5_000_000 + i * 2_000_000,
                err_us: 120,
            };
            let ours = TsfSample {
                tsf_us: 1_000_500 + i * 2_000_000,
                mono_us: 9_000_500 + i * 2_000_000,
                err_us: 130,
            };
            let (t1, t2, t3, t4) = synthesize_exchange(&leader, &ours);
            c.process_response(t1, t2, t3, t4);
        }
        assert!(c.converged(), "clean TSF samples must converge");
        assert!(
            c.bound_us() <= 300,
            "bound must be sub-ms (bracket-scale), got {}",
            c.bound_us()
        );
        assert_eq!(c.offset_us(), -4_000_000);
    }
}
