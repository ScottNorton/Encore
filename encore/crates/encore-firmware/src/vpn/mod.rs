//! WireGuard VPN subsystem — userspace tunnel via `boringtun`.
//!
//! Creates a TUN device, establishes a WireGuard tunnel to a remote peer,
//! and routes traffic for the configured AllowedIPs through the tunnel.
//! Uses `boringtun` (Cloudflare's userspace WireGuard) for the Noise
//! protocol crypto, with manual TUN/UDP management.
//!
//! The kernel is 3.8.13 with no WireGuard module, but CONFIG_TUN=y and
//! `/dev/net/tun` is created at boot by mount_partition.sh.

use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{bail, Context, Result};
use base64::Engine;
use encore_common::config::VpnConfig;
use encore_common::protocol::SubsystemState;
use std::net::UdpSocket;
use std::os::unix::io::AsRawFd;
use tracing::{error, info, warn};

const TUN_DEVICE: &str = "/dev/net/tun";
const TUN_NAME: &str = "tun0";
const TIMER_TICK_MS: u64 = 250;
const DNS_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const DNS_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

// Linux TUN ioctls and flags
const TUNSETIFF: libc::c_ulong = 0x400454ca;
const IFF_TUN: u16 = 0x0001;
const IFF_NO_PI: u16 = 0x1000;

pub struct VpnSubsystem {
    config: VpnConfig,
}

impl VpnSubsystem {
    pub fn new(config: VpnConfig) -> Self {
        Self { config }
    }
}

/// Open /dev/net/tun and create a TUN interface via ioctl.
/// Returns the raw fd (caller must manage lifetime).
fn create_tun() -> Result<std::os::unix::io::OwnedFd> {
    use std::os::unix::io::FromRawFd;

    // Ensure /dev/net/tun exists
    std::fs::create_dir_all("/dev/net").ok();
    if !std::path::Path::new(TUN_DEVICE).exists() {
        std::process::Command::new("mknod")
            .args([TUN_DEVICE, "c", "10", "200"])
            .status()
            .context("mknod /dev/net/tun failed")?;
    }

    let fd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(TUN_DEVICE)
        .context("failed to open /dev/net/tun")?;

    // Build struct ifreq (40 bytes on ARM32)
    let mut ifr = [0u8; 40];
    let name_bytes = TUN_NAME.as_bytes();
    ifr[..name_bytes.len()].copy_from_slice(name_bytes);
    let flags = (IFF_TUN | IFF_NO_PI).to_le_bytes();
    ifr[16..18].copy_from_slice(&flags);

    let ret = unsafe { libc::ioctl(fd.as_raw_fd(), TUNSETIFF as _, ifr.as_ptr()) };
    if ret < 0 {
        bail!(
            "TUNSETIFF ioctl failed: {}",
            std::io::Error::last_os_error()
        );
    }

    // Convert to OwnedFd for proper RAII
    let raw = fd.as_raw_fd();
    std::mem::forget(fd); // prevent double-close
    Ok(unsafe { std::os::unix::io::OwnedFd::from_raw_fd(raw) })
}

/// Configure the TUN interface: assign address, bring up, add routes.
fn configure_tun(address: &str, allowed_ips: &str) -> Result<()> {
    // ip addr add <address> dev tun0
    let status = std::process::Command::new("/sbin/ip")
        .args(["addr", "add", address, "dev", TUN_NAME])
        .status()
        .context("ip addr add failed")?;
    if !status.success() {
        bail!("ip addr add {} dev {} failed", address, TUN_NAME);
    }

    // ip link set tun0 up
    let status = std::process::Command::new("/sbin/ip")
        .args(["link", "set", TUN_NAME, "up"])
        .status()
        .context("ip link set up failed")?;
    if !status.success() {
        bail!("ip link set {} up failed", TUN_NAME);
    }

    // Add routes for each CIDR in allowed_ips
    for cidr in allowed_ips.split(',') {
        let cidr = cidr.trim();
        if cidr.is_empty() {
            continue;
        }
        let status = std::process::Command::new("/sbin/ip")
            .args(["route", "add", cidr, "dev", TUN_NAME])
            .status()
            .context(format!("ip route add {} failed", cidr))?;
        if !status.success() {
            warn!(
                "VPN: ip route add {} dev {} failed (may already exist)",
                cidr, TUN_NAME
            );
        }
    }

    // NAT: rewrite source IP of outbound tunnel packets to the VPN address.
    // Without this, packets keep their LAN source IP which the server rejects.
    let vpn_ip = address.split('/').next().unwrap_or(address);
    let status = std::process::Command::new("/sbin/iptables")
        .args([
            "-t",
            "nat",
            "-A",
            "POSTROUTING",
            "-o",
            TUN_NAME,
            "-j",
            "SNAT",
            "--to-source",
            vpn_ip,
        ])
        .status()
        .context("iptables SNAT rule failed")?;
    if !status.success() {
        warn!("VPN: iptables SNAT rule failed (NAT may not work)");
    }

    Ok(())
}

/// Remove the TUN interface and NAT rules (best-effort cleanup).
fn cleanup_tun() {
    let _ = std::process::Command::new("/sbin/iptables")
        .args(["-t", "nat", "-F", "POSTROUTING"])
        .status();
    let _ = std::process::Command::new("/sbin/ip")
        .args(["link", "delete", TUN_NAME])
        .status();
}

/// Decode a base64 WireGuard key into a 32-byte array.
fn decode_key(b64: &str) -> Result<[u8; 32]> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("invalid base64 key")?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("key must be 32 bytes, got {}", v.len()))
}

/// Resolve endpoint hostname with retries, waiting for network to come up.
/// Returns on shutdown signal if network never becomes available.
async fn resolve_endpoint(
    endpoint_str: &str,
    ctx: &mut SubsystemContext,
) -> Result<std::net::SocketAddr> {
    let deadline = tokio::time::Instant::now() + DNS_MAX_WAIT;
    let mut interval = tokio::time::interval(DNS_RETRY_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ctx.shutdown.recv() => {
                bail!("shutdown during DNS resolution");
            }
            _ = interval.tick() => {
                match tokio::net::lookup_host(endpoint_str).await {
                    Ok(mut addrs) => {
                        if let Some(addr) = addrs.next() {
                            return Ok(addr);
                        }
                        warn!("VPN: DNS returned no addresses for {}", endpoint_str);
                    }
                    Err(e) => {
                        if tokio::time::Instant::now() > deadline {
                            bail!("DNS lookup failed for {} after {}s: {}",
                                endpoint_str, DNS_MAX_WAIT.as_secs(), e);
                        }
                        info!("VPN: waiting for DNS ({})...", e);
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Subsystem for VpnSubsystem {
    fn name(&self) -> &'static str {
        "vpn"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        // ── Validate config ──
        let private_key_b64 = self
            .config
            .private_key
            .as_deref()
            .context("VPN: private_key not configured")?;
        let address = self
            .config
            .address
            .as_deref()
            .context("VPN: address not configured")?
            .to_string();
        let peer_public_b64 = self
            .config
            .peer_public_key
            .as_deref()
            .context("VPN: peer_public_key not configured")?;
        let endpoint_str = self
            .config
            .peer_endpoint
            .as_deref()
            .context("VPN: peer_endpoint not configured")?
            .to_string();
        let allowed_ips = self
            .config
            .peer_allowed_ips
            .as_deref()
            .unwrap_or("")
            .to_string();
        let keepalive = if self.config.persistent_keepalive > 0 {
            Some(self.config.persistent_keepalive)
        } else {
            None
        };

        // ── Decode keys ──
        let private_key_bytes = decode_key(private_key_b64)?;
        let peer_public_bytes = decode_key(peer_public_b64)?;
        let preshared_key = self
            .config
            .peer_preshared_key
            .as_deref()
            .map(decode_key)
            .transpose()?;

        // ── Resolve endpoint first (wait for network/DNS to be ready) ──
        info!("VPN: resolving endpoint {}...", endpoint_str);
        let endpoint = resolve_endpoint(&endpoint_str, &mut ctx).await?;
        info!("VPN: endpoint resolved to {}", endpoint);

        // ── Create tunnel ──
        info!("VPN: creating tunnel");

        let private_key = boringtun::x25519::StaticSecret::from(private_key_bytes);
        let peer_public = boringtun::x25519::PublicKey::from(peer_public_bytes);

        let mut tunn = boringtun::noise::Tunn::new(
            private_key,
            peer_public,
            preshared_key,
            keepalive,
            0,    // tunnel index
            None, // rate limiter
        );

        // ── Create TUN device ──
        // Clean up any stale tun0 from a previous crash
        cleanup_tun();
        let tun_fd = match create_tun().context("failed to create TUN device") {
            Ok(fd) => fd,
            Err(e) => {
                return Err(e);
            }
        };
        info!("VPN: TUN device {} created", TUN_NAME);

        // From here on, always clean up TUN on exit
        let result = self
            .run_tunnel(
                &mut tunn,
                tun_fd,
                &address,
                &allowed_ips,
                endpoint,
                &mut ctx,
            )
            .await;

        // ── Cleanup: always remove TUN interface ──
        cleanup_tun();
        info!("VPN: stopped");

        result
    }
}

impl VpnSubsystem {
    /// Inner tunnel loop, split out so cleanup_tun() always runs in run().
    async fn run_tunnel(
        &self,
        tunn: &mut boringtun::noise::Tunn,
        tun_fd: std::os::unix::io::OwnedFd,
        address: &str,
        allowed_ips: &str,
        endpoint: std::net::SocketAddr,
        ctx: &mut SubsystemContext,
    ) -> Result<()> {
        // ── Configure TUN ──
        configure_tun(address, allowed_ips)?;
        info!("VPN: {} configured with address {}", TUN_NAME, address);

        // ── Bind UDP socket ──
        let udp = UdpSocket::bind("0.0.0.0:0").context("UDP bind failed")?;
        udp.set_nonblocking(true)?;
        udp.connect(endpoint)?;
        let udp = tokio::net::UdpSocket::from_std(udp)?;
        info!("VPN: UDP socket bound, endpoint {}", endpoint);

        // ── Wrap TUN fd for async I/O ──
        let tun_async = tokio::io::unix::AsyncFd::new(tun_fd)?;

        ctx.health.set_state(SubsystemState::Running);

        // ── Send initial handshake ──
        let mut handshake_buf = vec![0u8; 148];
        match tunn.format_handshake_initiation(&mut handshake_buf, false) {
            boringtun::noise::TunnResult::WriteToNetwork(data) => {
                udp.send(data).await.context("failed to send handshake")?;
                info!("VPN: handshake initiation sent");
            }
            _ => {
                warn!("VPN: unexpected handshake initiation result");
            }
        }

        // ── Event loop ──
        let mut udp_buf = vec![0u8; 65536];
        let mut tun_buf = vec![0u8; 65536];
        let mut dst_buf = vec![0u8; 65536];

        let mut timer = tokio::time::interval(std::time::Duration::from_millis(TIMER_TICK_MS));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                // ── Shutdown signal ──
                _ = ctx.shutdown.recv() => {
                    info!("VPN: shutdown signal received");
                    break;
                }

                // ── Timer: update_timers + send keepalives/handshakes ──
                _ = timer.tick() => {
                    let mut result = tunn.update_timers(&mut dst_buf);
                    loop {
                        match result {
                            boringtun::noise::TunnResult::WriteToNetwork(data) => {
                                if let Err(e) = udp.send(data).await {
                                    warn!("VPN: timer send failed: {}", e);
                                }
                                result = tunn.decapsulate(None, &[], &mut dst_buf);
                            }
                            boringtun::noise::TunnResult::Done => break,
                            boringtun::noise::TunnResult::Err(e) => {
                                warn!("VPN: timer error: {:?}", e);
                                break;
                            }
                            _ => break,
                        }
                    }
                }

                // ── UDP reader: encrypted packets from peer ──
                result = udp.recv(&mut udp_buf) => {
                    match result {
                        Ok(n) => {
                            ctx.health.inc_msg();
                            let mut result = tunn.decapsulate(None, &udp_buf[..n], &mut dst_buf);
                            loop {
                                match result {
                                    boringtun::noise::TunnResult::WriteToTunnelV4(data, _)
                                    | boringtun::noise::TunnResult::WriteToTunnelV6(data, _) => {
                                        if let Ok(mut guard) = tun_async.writable().await {
                                            match guard.try_io(|inner| {
                                                let fd = inner.as_raw_fd();
                                                let ret = unsafe {
                                                    libc::write(fd, data.as_ptr() as *const _, data.len())
                                                };
                                                if ret < 0 {
                                                    Err(std::io::Error::last_os_error())
                                                } else {
                                                    Ok(ret as usize)
                                                }
                                            }) {
                                                Ok(Ok(_)) => {}
                                                Ok(Err(e)) => warn!("VPN: TUN write failed: {}", e),
                                                Err(_would_block) => {}
                                            }
                                        }
                                        result = tunn.decapsulate(None, &[], &mut dst_buf);
                                    }
                                    boringtun::noise::TunnResult::WriteToNetwork(data) => {
                                        if let Err(e) = udp.send(data).await {
                                            warn!("VPN: send failed: {}", e);
                                        }
                                        result = tunn.decapsulate(None, &[], &mut dst_buf);
                                    }
                                    boringtun::noise::TunnResult::Done => break,
                                    boringtun::noise::TunnResult::Err(e) => {
                                        warn!("VPN: decapsulate error: {:?}", e);
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("VPN: UDP recv error: {}", e);
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                }

                // ── TUN reader: outgoing IP packets to encrypt ──
                result = tun_async.readable() => {
                    match result {
                        Ok(mut guard) => {
                            match guard.try_io(|inner| {
                                let fd = inner.as_raw_fd();
                                let ret = unsafe {
                                    libc::read(fd, tun_buf.as_mut_ptr() as *mut _, tun_buf.len())
                                };
                                if ret < 0 {
                                    Err(std::io::Error::last_os_error())
                                } else {
                                    Ok(ret as usize)
                                }
                            }) {
                                Ok(Ok(n)) if n > 0 => {
                                    ctx.health.inc_msg();
                                    match tunn.encapsulate(&tun_buf[..n], &mut dst_buf) {
                                        boringtun::noise::TunnResult::WriteToNetwork(data) => {
                                            if let Err(e) = udp.send(data).await {
                                                warn!("VPN: encap send failed: {}", e);
                                            }
                                        }
                                        boringtun::noise::TunnResult::Err(e) => {
                                            warn!("VPN: encapsulate error: {:?}", e);
                                        }
                                        boringtun::noise::TunnResult::Done => {
                                            // Tunnel not ready yet, packet dropped
                                        }
                                        _ => {}
                                    }
                                }
                                Ok(Ok(_)) => {} // zero-length read
                                Ok(Err(e)) => {
                                    if e.kind() != std::io::ErrorKind::WouldBlock {
                                        warn!("VPN: TUN read error: {}", e);
                                    }
                                }
                                Err(_would_block) => {} // AsyncFd not ready
                            }
                        }
                        Err(e) => {
                            error!("VPN: TUN readable error: {}", e);
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
