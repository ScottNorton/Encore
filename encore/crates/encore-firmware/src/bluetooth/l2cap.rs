//! L2CAP and HCI socket helpers for direct Bluetooth kernel access.
//!
//! Thin wrappers around raw AF_BLUETOOTH sockets. Used by the SDP server
//! (PSM 1), AVDTP (PSM 25), and Management API (HCI channel 3).
//! The kernel's bt8xxx.ko provides these interfaces — no BlueZ daemon needed.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

// ── Bluetooth socket constants ──

/// AF_BLUETOOTH address family.
pub const AF_BLUETOOTH: i32 = 31;
/// L2CAP protocol.
pub const BTPROTO_L2CAP: i32 = 0;
/// HCI protocol (for management socket).
pub const BTPROTO_HCI: i32 = 1;
/// L2CAP socket option level.
pub const SOL_L2CAP: i32 = 6;
/// L2CAP link mode option.
pub const L2CAP_LM: i32 = 3;
/// Require authentication.
pub const L2CAP_LM_AUTH: i32 = 0x0002;
/// Require encryption.
pub const L2CAP_LM_ENCRYPT: i32 = 0x0004;
/// L2CAP options (MTU etc).
pub const L2CAP_OPTIONS: i32 = 1;
/// HCI management channel.
pub const HCI_CHANNEL_CONTROL: u16 = 3;
/// Non-specific device index (for management socket).
pub const HCI_DEV_NONE: u16 = 0xFFFF;

// ── Socket address structures ──

/// sockaddr_l2 — L2CAP socket address.
///
/// Matches kernel `struct sockaddr_l2` layout (14 bytes with repr(C) padding).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockaddrL2 {
    pub l2_family: u16,
    pub l2_psm: u16,
    pub l2_bdaddr: [u8; 6],
    pub l2_cid: u16,
    pub l2_bdaddr_type: u8,
}

impl SockaddrL2 {
    /// New address bound to BDADDR_ANY with the given PSM.
    pub fn new(psm: u16) -> Self {
        Self {
            l2_family: AF_BLUETOOTH as u16,
            l2_psm: psm.to_le(),
            l2_bdaddr: [0u8; 6],
            l2_cid: 0,
            l2_bdaddr_type: 0,
        }
    }
}

/// sockaddr_hci — HCI socket address (for management API).
#[repr(C)]
pub struct SockaddrHci {
    pub hci_family: u16,
    pub hci_dev: u16,
    pub hci_channel: u16,
}

/// L2CAP connection options (for getsockopt).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct L2capOptions {
    pub omtu: u16,
    pub imtu: u16,
    pub flush_to: u16,
    pub mode: u8,
    pub fcs: u8,
    pub max_tx: u8,
    _pad: u8,
    pub txwin_size: u16,
}

// ── Socket operations ──

/// Create an L2CAP SEQPACKET socket.
pub fn l2cap_socket() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(AF_BLUETOOTH, libc::SOCK_SEQPACKET, BTPROTO_L2CAP) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Create a raw HCI socket for the management API.
pub fn mgmt_socket() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(AF_BLUETOOTH, libc::SOCK_RAW, BTPROTO_HCI) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Set a file descriptor to non-blocking mode.
pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Bind an L2CAP socket to a PSM on BDADDR_ANY.
pub fn l2cap_bind(fd: RawFd, psm: u16) -> io::Result<()> {
    let addr = SockaddrL2::new(psm);
    let ret = unsafe {
        libc::bind(
            fd,
            &addr as *const SockaddrL2 as *const libc::sockaddr,
            std::mem::size_of::<SockaddrL2>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Connect an L2CAP SEQPACKET socket to `bdaddr` (kernel byte order) on `psm`.
///
/// Blocks through paging + link setup — on a bonded peer the kernel brings up
/// the ACL with the stored key, so this is how the speaker initiates a session
/// (e.g. sink-initiated AVDTP) rather than waiting to be connected to. Run it
/// from a dedicated thread; paging an absent device can take ~5–10 s.
pub fn l2cap_connect(bdaddr: &[u8; 6], psm: u16) -> io::Result<OwnedFd> {
    let fd = l2cap_socket()?;
    let mut addr = SockaddrL2::new(psm);
    addr.l2_bdaddr = *bdaddr;
    let ret = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            &addr as *const SockaddrL2 as *const libc::sockaddr,
            std::mem::size_of::<SockaddrL2>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Bind the management socket to HCI_CHANNEL_CONTROL.
pub fn mgmt_bind(fd: RawFd) -> io::Result<()> {
    let addr = SockaddrHci {
        hci_family: AF_BLUETOOTH as u16,
        hci_dev: HCI_DEV_NONE,
        hci_channel: HCI_CHANNEL_CONTROL,
    };
    let ret = unsafe {
        libc::bind(
            fd,
            &addr as *const SockaddrHci as *const libc::sockaddr,
            std::mem::size_of::<SockaddrHci>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Listen on a socket.
pub fn l2cap_listen(fd: RawFd, backlog: i32) -> io::Result<()> {
    let ret = unsafe { libc::listen(fd, backlog) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Accept a connection. Returns (new_fd, remote_bdaddr).
pub fn l2cap_accept(fd: RawFd) -> io::Result<(OwnedFd, [u8; 6])> {
    let mut addr = SockaddrL2::new(0);
    let mut addrlen = std::mem::size_of::<SockaddrL2>() as libc::socklen_t;
    let new_fd = unsafe {
        libc::accept(
            fd,
            &mut addr as *mut SockaddrL2 as *mut libc::sockaddr,
            &mut addrlen,
        )
    };
    if new_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((unsafe { OwnedFd::from_raw_fd(new_fd) }, addr.l2_bdaddr))
}

/// Set L2CAP link mode (e.g. AUTH | ENCRYPT for A2DP).
pub fn l2cap_set_link_mode(fd: RawFd, mode: i32) -> io::Result<()> {
    let ret = unsafe {
        libc::setsockopt(
            fd,
            SOL_L2CAP,
            L2CAP_LM,
            &mode as *const i32 as *const libc::c_void,
            std::mem::size_of::<i32>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Get L2CAP options (includes imtu = incoming MTU).
pub fn l2cap_get_options(fd: RawFd) -> io::Result<L2capOptions> {
    let mut opts = L2capOptions::default();
    let mut optlen = std::mem::size_of::<L2capOptions>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            SOL_L2CAP,
            L2CAP_OPTIONS,
            &mut opts as *mut L2capOptions as *mut libc::c_void,
            &mut optlen,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(opts)
}

/// Non-blocking read from a raw fd.
pub fn raw_read(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// Non-blocking write to a raw fd.
pub fn raw_write(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

// ── Bluetooth address helpers ──

/// Format a bdaddr as "XX:XX:XX:XX:XX:XX" (BlueZ convention: reversed byte order).
pub fn bdaddr_to_string(addr: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[5], addr[4], addr[3], addr[2], addr[1], addr[0]
    )
}

/// Parse "XX:XX:XX:XX:XX:XX" into bdaddr bytes (reversed for kernel).
pub fn string_to_bdaddr(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut addr = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        addr[5 - i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bdaddr_round_trip() {
        let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let s = bdaddr_to_string(&addr);
        assert_eq!(s, "66:55:44:33:22:11");
        assert_eq!(string_to_bdaddr(&s), Some(addr));
    }

    #[test]
    fn bdaddr_parse_invalid() {
        assert_eq!(string_to_bdaddr("not-an-address"), None);
        assert_eq!(string_to_bdaddr("AA:BB:CC"), None);
        assert_eq!(string_to_bdaddr("GG:HH:II:JJ:KK:LL"), None);
    }

    #[test]
    fn sockaddr_l2_size() {
        // Kernel expects 14 bytes (13 fields + 1 padding from repr(C))
        let size = std::mem::size_of::<SockaddrL2>();
        assert!(
            (13..=16).contains(&size),
            "unexpected SockaddrL2 size: {}",
            size
        );
    }
}
