//! Bluetooth Management API (kernel Mgmt v1.x).
//!
//! Talks directly to the kernel's Bluetooth management interface via
//! AF_BLUETOOTH SOCK_RAW on HCI_CHANNEL_CONTROL (channel 3).
//! Replaces bluetoothd + D-Bus for adapter configuration, pairing, and link keys.

use super::l2cap;
use anyhow::{Context, Result};
use std::os::fd::{AsRawFd, OwnedFd};
use tokio::io::unix::AsyncFd;
use tracing::{debug, info, warn};

// ── Management opcodes ──

const MGMT_OP_READ_INFO: u16 = 0x0004;
const MGMT_OP_SET_POWERED: u16 = 0x0005;
const MGMT_OP_SET_DISCOVERABLE: u16 = 0x0006;
const MGMT_OP_SET_CONNECTABLE: u16 = 0x0007;
const MGMT_OP_SET_PAIRABLE: u16 = 0x0009;
const MGMT_OP_SET_SSP: u16 = 0x000B;
const MGMT_OP_SET_DEV_CLASS: u16 = 0x000D;
pub const MGMT_OP_SET_LOCAL_NAME: u16 = 0x000F;
const MGMT_OP_LOAD_LINK_KEYS: u16 = 0x0012;
const MGMT_OP_DISCONNECT: u16 = 0x0014;
const MGMT_OP_SET_IO_CAPABILITY: u16 = 0x0018;
const MGMT_OP_PAIR_DEVICE: u16 = 0x0019;
const MGMT_OP_UNPAIR_DEVICE: u16 = 0x001B;
const MGMT_OP_USER_CONFIRM_REPLY: u16 = 0x001C;
const MGMT_OP_START_DISCOVERY: u16 = 0x0023;
const MGMT_OP_STOP_DISCOVERY: u16 = 0x0024;

// ── Management events ──

const MGMT_EV_CMD_COMPLETE: u16 = 0x0001;
const MGMT_EV_CMD_STATUS: u16 = 0x0002;
const MGMT_EV_NEW_LINK_KEY: u16 = 0x0009;
const MGMT_EV_DEVICE_CONNECTED: u16 = 0x000B;
const MGMT_EV_DEVICE_DISCONNECTED: u16 = 0x000C;
const MGMT_EV_USER_CONFIRM_REQUEST: u16 = 0x000F;
const MGMT_EV_DEVICE_FOUND: u16 = 0x0012;

/// Link key file path.
const BT_KEYS_PATH: &str = "/lsync/encore/bt_keys";

/// Size of one link key entry: bdaddr(6) + addr_type(1) + key_type(1) + val(16) + pin_len(1).
const LINK_KEY_SIZE: usize = 25;

/// Events parsed from the management socket.
#[derive(Debug)]
pub enum MgmtEvent {
    /// Adapter command completed.
    CmdComplete {
        opcode: u16,
        status: u8,
        data: Vec<u8>,
    },
    /// Device connected (from mgmt, not AVDTP).
    DeviceConnected {
        addr: [u8; 6],
        addr_type: u8,
        name: Option<String>,
    },
    /// Device disconnected.
    DeviceDisconnected { addr: [u8; 6], addr_type: u8 },
    /// New link key received (store it).
    NewLinkKey {
        store_hint: u8,
        addr: [u8; 6],
        addr_type: u8,
        key_type: u8,
        key_val: [u8; 16],
        pin_len: u8,
    },
    /// User confirmation request (auto-reply for Just Works).
    UserConfirmRequest {
        addr: [u8; 6],
        addr_type: u8,
        value: u32,
    },
    /// Device found during discovery.
    DeviceFound {
        addr: [u8; 6],
        addr_type: u8,
        rssi: i8,
        name: Option<String>,
    },
    /// Unhandled event.
    Other { opcode: u16 },
}

/// Wrapper around the Bluetooth management socket.
pub struct MgmtSocket {
    fd: AsyncFd<OwnedFd>,
    /// In-memory link keys, loaded from file at startup.
    link_keys: Vec<LinkKey>,
}

/// A stored link key.
#[derive(Clone)]
struct LinkKey {
    addr: [u8; 6],
    addr_type: u8,
    key_type: u8,
    key_val: [u8; 16],
    pin_len: u8,
}

impl MgmtSocket {
    /// Open the management socket and bind to HCI_CHANNEL_CONTROL.
    pub fn open() -> Result<Self> {
        let fd = l2cap::mgmt_socket().context("failed to create mgmt socket")?;
        l2cap::mgmt_bind(fd.as_raw_fd()).context("failed to bind mgmt socket")?;
        l2cap::set_nonblocking(fd.as_raw_fd()).context("failed to set mgmt nonblocking")?;
        let async_fd = AsyncFd::new(fd).context("failed to create AsyncFd for mgmt")?;

        let link_keys = load_link_keys_from_file().unwrap_or_default();
        info!(
            "Bluetooth mgmt: loaded {} stored link keys",
            link_keys.len()
        );

        Ok(Self {
            fd: async_fd,
            link_keys,
        })
    }

    /// Run the adapter setup sequence via management commands.
    ///
    /// Returns the adapter's BD_ADDR on success.
    pub async fn setup_adapter(&mut self, name: &str) -> Result<[u8; 6]> {
        // 1. Power on via management API (may already be UP from HCIDEVUP ioctl).
        match self.send_and_wait(MGMT_OP_SET_POWERED, 0, &[0x01]).await {
            Ok(_) => info!("Bluetooth mgmt: adapter powered on"),
            Err(e) => warn!("Bluetooth mgmt: SET_POWERED: {} (may already be up)", e),
        }

        // Brief delay for firmware to finish initialization
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // 2. READ_INFO — get adapter address
        let info_data = self
            .send_and_wait(MGMT_OP_READ_INFO, 0, &[])
            .await
            .context("READ_INFO failed")?;
        let bdaddr = if info_data.len() >= 6 {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&info_data[..6]);
            addr
        } else {
            anyhow::bail!("READ_INFO returned insufficient data");
        };
        info!(
            "Bluetooth mgmt: adapter address {}",
            l2cap::bdaddr_to_string(&bdaddr)
        );

        // 3. SET_IO_CAPABILITY — NoInputNoOutput (0x03)
        self.send_and_wait(MGMT_OP_SET_IO_CAPABILITY, 0, &[0x03])
            .await
            .context("SET_IO_CAPABILITY failed")?;

        // 4. SET_SSP — enable Secure Simple Pairing (non-fatal on old kernels)
        match self.send_and_wait(MGMT_OP_SET_SSP, 0, &[0x01]).await {
            Ok(_) => info!("Bluetooth mgmt: SSP enabled"),
            Err(e) => warn!(
                "Bluetooth mgmt: SSP not available ({}), continuing with legacy pairing",
                e
            ),
        }

        // 5. SET_DEV_CLASS — Audio (0x04), Loudspeaker (0x14)
        // Non-fatal: device class is cosmetic (affects phone icon) not functional.
        match self
            .send_and_wait(MGMT_OP_SET_DEV_CLASS, 0, &[0x14, 0x04])
            .await
        {
            Ok(_) => info!("Bluetooth mgmt: device class set (Audio/Loudspeaker)"),
            Err(e) => warn!("Bluetooth mgmt: SET_DEV_CLASS: {} (continuing)", e),
        }

        // 6. SET_LOCAL_NAME — device name (249 bytes max, zero-padded)
        let mut name_buf = [0u8; 260]; // 249 name + 11 short_name
        let name_bytes = name.as_bytes();
        let len = name_bytes.len().min(248);
        name_buf[..len].copy_from_slice(&name_bytes[..len]);
        self.send_and_wait(MGMT_OP_SET_LOCAL_NAME, 0, &name_buf)
            .await
            .context("SET_LOCAL_NAME failed")?;

        // 7. LOAD_LINK_KEYS — restore paired devices
        self.load_link_keys().await?;

        // 8-10. Enable adapter features
        self.send_and_wait(MGMT_OP_SET_CONNECTABLE, 0, &[0x01])
            .await
            .context("SET_CONNECTABLE failed")?;
        self.send_and_wait(MGMT_OP_SET_PAIRABLE, 0, &[0x01])
            .await
            .context("SET_PAIRABLE failed")?;

        // SET_DISCOVERABLE with timeout=0 (forever): [val: u8, timeout: u16 LE]
        self.send_and_wait(MGMT_OP_SET_DISCOVERABLE, 0, &[0x01, 0x00, 0x00])
            .await
            .context("SET_DISCOVERABLE failed")?;

        info!("Bluetooth mgmt: adapter configured as '{}'", name);
        Ok(bdaddr)
    }

    /// Read and parse the next management event.
    pub async fn read_event(&self) -> Result<MgmtEvent> {
        let mut buf = [0u8; 1024];
        loop {
            let mut guard = self.fd.readable().await.context("mgmt readable failed")?;
            match guard.try_io(|inner| l2cap::raw_read(inner.get_ref().as_raw_fd(), &mut buf)) {
                Ok(Ok(n)) if n >= 6 => {
                    return Ok(parse_mgmt_event(&buf[..n]));
                }
                Ok(Ok(_)) => continue, // runt packet
                Ok(Err(e)) => return Err(e.into()),
                Err(_would_block) => continue,
            }
        }
    }

    /// Send a management command (fire-and-forget).
    pub async fn send_cmd(&self, opcode: u16, index: u16, params: &[u8]) -> Result<()> {
        let mut msg = Vec::with_capacity(6 + params.len());
        msg.extend_from_slice(&opcode.to_le_bytes());
        msg.extend_from_slice(&index.to_le_bytes());
        msg.extend_from_slice(&(params.len() as u16).to_le_bytes());
        msg.extend_from_slice(params);

        loop {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|inner| {
                // Use send() instead of write() — more appropriate for datagram sockets
                let fd = inner.get_ref().as_raw_fd();
                let n = unsafe {
                    libc::send(
                        fd,
                        msg.as_ptr() as *const libc::c_void,
                        msg.len(),
                        libc::MSG_NOSIGNAL,
                    )
                };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(Ok(n)) if n == msg.len() => return Ok(()),
                Ok(Ok(n)) => {
                    warn!(
                        "mgmt: short write for opcode 0x{:04x}: wrote {}/{} bytes",
                        opcode,
                        n,
                        msg.len()
                    );
                    anyhow::bail!("short write on mgmt socket ({}/{})", n, msg.len())
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_would_block) => continue,
            }
        }
    }

    /// Send a command and wait for its CMD_COMPLETE response.
    /// Discards (logs) unrelated events received while waiting.
    async fn send_and_wait(&self, opcode: u16, index: u16, params: &[u8]) -> Result<Vec<u8>> {
        self.send_cmd(opcode, index, params).await?;

        let mut buf = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);

        loop {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "timeout waiting for CMD_COMPLETE for opcode 0x{:04x}",
                    opcode
                );
            }

            let mut guard = self.fd.readable().await?;
            match guard.try_io(|inner| l2cap::raw_read(inner.get_ref().as_raw_fd(), &mut buf)) {
                Ok(Ok(n)) if n >= 6 => {
                    let ev_opcode = u16::from_le_bytes([buf[0], buf[1]]);
                    // CMD_COMPLETE: hdr(6) + cmd_opcode(2) + status(1) + data
                    if ev_opcode == MGMT_EV_CMD_COMPLETE && n >= 9 {
                        let cmd_opcode = u16::from_le_bytes([buf[6], buf[7]]);
                        if cmd_opcode == opcode {
                            let status = buf[8];
                            if status == 0 {
                                return Ok(buf[9..n].to_vec());
                            } else {
                                anyhow::bail!(
                                    "mgmt 0x{:04x} failed, status=0x{:02x}",
                                    opcode,
                                    status
                                );
                            }
                        }
                    }
                    // CMD_STATUS also carries our opcode
                    if ev_opcode == MGMT_EV_CMD_STATUS && n >= 9 {
                        let cmd_opcode = u16::from_le_bytes([buf[6], buf[7]]);
                        if cmd_opcode == opcode {
                            let status = buf[8];
                            if status != 0 {
                                anyhow::bail!(
                                    "mgmt 0x{:04x} status error=0x{:02x}",
                                    opcode,
                                    status
                                );
                            }
                            // status=0 means command in progress, keep waiting for CMD_COMPLETE
                        }
                    }
                    // Discard other events during setup
                    debug!(
                        "mgmt: discarding event 0x{:04x} while waiting for 0x{:04x}",
                        ev_opcode, opcode
                    );
                }
                Ok(Ok(_)) => {} // runt
                Ok(Err(e)) => return Err(e.into()),
                Err(_would_block) => continue,
            }
        }
    }

    /// Load stored link keys from file and send LOAD_LINK_KEYS command.
    async fn load_link_keys(&self) -> Result<()> {
        let key_count = self.link_keys.len() as u16;
        let mut params = Vec::with_capacity(3 + self.link_keys.len() * LINK_KEY_SIZE);
        params.push(0x00); // debug_keys = false
        params.extend_from_slice(&key_count.to_le_bytes());

        for key in &self.link_keys {
            params.extend_from_slice(&key.addr);
            params.push(key.addr_type);
            params.push(key.key_type);
            params.extend_from_slice(&key.key_val);
            params.push(key.pin_len);
        }

        self.send_and_wait(MGMT_OP_LOAD_LINK_KEYS, 0, &params)
            .await
            .context("LOAD_LINK_KEYS failed")?;

        info!("Bluetooth mgmt: loaded {} link keys", key_count);
        Ok(())
    }

    /// Handle a NEW_LINK_KEY event: store the key and persist to file.
    pub fn handle_new_link_key(&mut self, event: &MgmtEvent) {
        if let MgmtEvent::NewLinkKey {
            store_hint,
            addr,
            addr_type,
            key_type,
            key_val,
            pin_len,
        } = event
        {
            if *store_hint == 0 {
                debug!("mgmt: new link key with store_hint=0, not storing");
                return;
            }

            // Replace existing key for this address or add new
            let existing = self
                .link_keys
                .iter_mut()
                .find(|k| k.addr == *addr && k.addr_type == *addr_type);

            if let Some(k) = existing {
                k.key_type = *key_type;
                k.key_val = *key_val;
                k.pin_len = *pin_len;
            } else {
                self.link_keys.push(LinkKey {
                    addr: *addr,
                    addr_type: *addr_type,
                    key_type: *key_type,
                    key_val: *key_val,
                    pin_len: *pin_len,
                });
            }

            if let Err(e) = save_link_keys_to_file(&self.link_keys) {
                warn!("mgmt: failed to save link keys: {}", e);
            } else {
                info!(
                    "mgmt: stored link key for {}",
                    l2cap::bdaddr_to_string(addr)
                );
            }
        }
    }

    /// Remove a stored link key for the given address.
    /// Addresses of all bonded devices, for the dashboard's paired list.
    pub fn paired_addrs(&self) -> Vec<[u8; 6]> {
        self.link_keys.iter().map(|k| k.addr).collect()
    }

    pub fn remove_link_key(&mut self, addr: &[u8; 6]) {
        self.link_keys.retain(|k| k.addr != *addr);
        if let Err(e) = save_link_keys_to_file(&self.link_keys) {
            warn!("mgmt: failed to save link keys after removal: {}", e);
        }
    }

    /// Auto-reply to a USER_CONFIRM_REQUEST (Just Works pairing).
    pub async fn auto_confirm(&self, addr: &[u8; 6], addr_type: u8) -> Result<()> {
        let mut params = [0u8; 7];
        params[..6].copy_from_slice(addr);
        params[6] = addr_type;
        self.send_cmd(MGMT_OP_USER_CONFIRM_REPLY, 0, &params).await
    }

    /// Handle a BtAction from the dashboard.
    /// Change the advertised local name live (the "visible as" name on phones).
    pub async fn set_local_name(&self, name: &str) {
        let mut name_buf = [0u8; 260]; // 249 name + 11 short_name
        let name_bytes = name.as_bytes();
        let len = name_bytes.len().min(248);
        name_buf[..len].copy_from_slice(&name_bytes[..len]);
        if let Err(e) = self
            .send_cmd(MGMT_OP_SET_LOCAL_NAME, 0, &name_buf)
            .await
        {
            warn!("Bluetooth mgmt: SET_LOCAL_NAME failed: {}", e);
        }
    }

    /// Toggle BR/EDR discoverability. Connectable stays on regardless, so
    /// bonded devices and auto-reconnect still work — this only hides the
    /// speaker from *new* device scans while it is in use.
    pub async fn set_discoverable(&self, on: bool) {
        let val = if on { 0x01 } else { 0x00 };
        // [discoverable: u8, timeout: u16 LE] — timeout 0 = no auto-expiry
        if let Err(e) = self
            .send_cmd(MGMT_OP_SET_DISCOVERABLE, 0, &[val, 0x00, 0x00])
            .await
        {
            warn!("Bluetooth mgmt: SET_DISCOVERABLE({}) failed: {}", on, e);
        }
    }

    pub async fn handle_action(&self, action: encore_common::protocol::BtAction) {
        use encore_common::protocol::BtAction;
        match action {
            BtAction::StartDiscovery => {
                info!("Bluetooth mgmt: starting discovery");
                // type=0x01 (BR/EDR)
                if let Err(e) = self.send_cmd(MGMT_OP_START_DISCOVERY, 0, &[0x01]).await {
                    warn!("Bluetooth mgmt: START_DISCOVERY failed: {}", e);
                }
            }
            BtAction::StopDiscovery => {
                info!("Bluetooth mgmt: stopping discovery");
                if let Err(e) = self.send_cmd(MGMT_OP_STOP_DISCOVERY, 0, &[0x01]).await {
                    warn!("Bluetooth mgmt: STOP_DISCOVERY failed: {}", e);
                }
            }
            BtAction::Pair { addr } => {
                if let Some(bdaddr) = l2cap::string_to_bdaddr(&addr) {
                    info!("Bluetooth mgmt: pairing with {}", addr);
                    // params: bdaddr(6) + addr_type(1) + io_cap(1)
                    let mut params = [0u8; 8];
                    params[..6].copy_from_slice(&bdaddr);
                    params[6] = 0x00; // BR/EDR
                    params[7] = 0x03; // NoInputNoOutput
                    if let Err(e) = self.send_cmd(MGMT_OP_PAIR_DEVICE, 0, &params).await {
                        warn!("Bluetooth mgmt: PAIR_DEVICE failed: {}", e);
                    }
                }
            }
            BtAction::Disconnect { addr } => {
                if let Some(bdaddr) = l2cap::string_to_bdaddr(&addr) {
                    info!("Bluetooth mgmt: disconnecting {}", addr);
                    let mut params = [0u8; 7];
                    params[..6].copy_from_slice(&bdaddr);
                    params[6] = 0x00; // BR/EDR
                    if let Err(e) = self.send_cmd(MGMT_OP_DISCONNECT, 0, &params).await {
                        warn!("Bluetooth mgmt: DISCONNECT failed: {}", e);
                    }
                }
            }
            BtAction::Forget { addr } => {
                if let Some(bdaddr) = l2cap::string_to_bdaddr(&addr) {
                    info!("Bluetooth mgmt: removing {}", addr);
                    // UNPAIR_DEVICE: bdaddr(6) + addr_type(1) + disconnect(1)
                    let mut params = [0u8; 8];
                    params[..6].copy_from_slice(&bdaddr);
                    params[6] = 0x00; // BR/EDR
                    params[7] = 0x01; // disconnect=1
                    if let Err(e) = self.send_cmd(MGMT_OP_UNPAIR_DEVICE, 0, &params).await {
                        warn!("Bluetooth mgmt: UNPAIR_DEVICE failed: {}", e);
                    }
                    self.remove_link_key_by_ref(&bdaddr);
                }
            }
            BtAction::SetName { .. } => {
                // Handled in the subsystem loop (needs to persist + broadcast).
            }
            BtAction::Transport { .. } => {
                // Handled in the subsystem loop (needs the AVCTP socket).
            }
            BtAction::RequestStatus => {
                // Handled in the subsystem loop (it owns broadcast_bt_status).
            }
            BtAction::Connect { addr } => {
                // An A2DP sink doesn't open the media stream itself, but it can
                // page a *bonded* source to re-establish the ACL using the
                // stored link key. Sources with audio routed here then re-open
                // A2DP on their own. (Verified live: pages a paused/dropped PC
                // back as a connected device.)
                if let Some(bdaddr) = l2cap::string_to_bdaddr(&addr) {
                    info!("Bluetooth mgmt: connecting (paging bonded) {}", addr);
                    let mut params = [0u8; 8];
                    params[..6].copy_from_slice(&bdaddr);
                    params[6] = 0x00; // BR/EDR
                    params[7] = 0x03; // NoInputNoOutput (Just Works; already bonded)
                    if let Err(e) = self.send_cmd(MGMT_OP_PAIR_DEVICE, 0, &params).await {
                        warn!("Bluetooth mgmt: Connect (PAIR_DEVICE) failed: {}", e);
                    }
                }
            }
        }
    }

    // Interior-mutable version for use in async contexts where we don't have &mut self
    fn remove_link_key_by_ref(&self, _addr: &[u8; 6]) {
        // Link key removal happens via handle_new_link_key / remove_link_key
        // which require &mut self. For the fire-and-forget action handler,
        // the main loop should call remove_link_key() after the command completes.
    }
}

// ── Event parsing ──

fn parse_mgmt_event(buf: &[u8]) -> MgmtEvent {
    if buf.len() < 6 {
        return MgmtEvent::Other { opcode: 0 };
    }
    let opcode = u16::from_le_bytes([buf[0], buf[1]]);
    // index at [2..4], param_len at [4..6]
    let params = &buf[6..];

    match opcode {
        MGMT_EV_CMD_COMPLETE if params.len() >= 3 => MgmtEvent::CmdComplete {
            opcode: u16::from_le_bytes([params[0], params[1]]),
            status: params[2],
            data: params[3..].to_vec(),
        },
        MGMT_EV_DEVICE_CONNECTED if params.len() >= 11 => {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&params[..6]);
            let addr_type = params[6];
            // flags at [7..11], eir_len at [11..13]
            let name = if params.len() >= 13 {
                let eir_len = u16::from_le_bytes([params[11], params[12]]) as usize;
                let eir_end = 13 + eir_len;
                if params.len() >= eir_end {
                    parse_eir_name(&params[13..eir_end])
                } else {
                    None
                }
            } else {
                None
            };
            MgmtEvent::DeviceConnected {
                addr,
                addr_type,
                name,
            }
        }
        MGMT_EV_DEVICE_DISCONNECTED if params.len() >= 7 => {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&params[..6]);
            MgmtEvent::DeviceDisconnected {
                addr,
                addr_type: params[6],
            }
        }
        MGMT_EV_NEW_LINK_KEY if params.len() >= 26 => {
            let store_hint = params[0];
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&params[1..7]);
            let addr_type = params[7];
            let key_type = params[8];
            let mut key_val = [0u8; 16];
            key_val.copy_from_slice(&params[9..25]);
            let pin_len = params[25];
            MgmtEvent::NewLinkKey {
                store_hint,
                addr,
                addr_type,
                key_type,
                key_val,
                pin_len,
            }
        }
        MGMT_EV_USER_CONFIRM_REQUEST if params.len() >= 11 => {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&params[..6]);
            let addr_type = params[6];
            // confirm_hint at [7], value at [8..12]
            let value = if params.len() >= 12 {
                u32::from_le_bytes([params[8], params[9], params[10], params[11]])
            } else {
                0
            };
            MgmtEvent::UserConfirmRequest {
                addr,
                addr_type,
                value,
            }
        }
        MGMT_EV_DEVICE_FOUND if params.len() >= 14 => {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&params[..6]);
            let addr_type = params[6];
            let rssi = params[7] as i8;
            // flags at [8..12], eir_len at [12..14]
            let name = if params.len() >= 14 {
                let eir_len = u16::from_le_bytes([params[12], params[13]]) as usize;
                let eir_end = 14 + eir_len;
                if params.len() >= eir_end {
                    parse_eir_name(&params[14..eir_end])
                } else {
                    None
                }
            } else {
                None
            };
            MgmtEvent::DeviceFound {
                addr,
                addr_type,
                rssi,
                name,
            }
        }
        _ => MgmtEvent::Other { opcode },
    }
}

/// Parse a Complete Local Name (type 0x09) or Shortened (0x08) from EIR data.
fn parse_eir_name(eir: &[u8]) -> Option<String> {
    let mut pos = 0;
    while pos + 1 < eir.len() {
        let field_len = eir[pos] as usize;
        if field_len == 0 || pos + 1 + field_len > eir.len() {
            break;
        }
        let field_type = eir[pos + 1];
        if field_type == 0x09 || field_type == 0x08 {
            let name_bytes = &eir[pos + 2..pos + 1 + field_len];
            return Some(String::from_utf8_lossy(name_bytes).to_string());
        }
        pos += 1 + field_len;
    }
    None
}

// ── Link key file I/O ──

fn load_link_keys_from_file() -> Result<Vec<LinkKey>> {
    let data = std::fs::read(BT_KEYS_PATH)?;
    if data.len() < 3 {
        return Ok(Vec::new());
    }
    let _debug_keys = data[0];
    let key_count = u16::from_le_bytes([data[1], data[2]]) as usize;

    let mut keys = Vec::with_capacity(key_count);
    let mut pos = 3;
    for _ in 0..key_count {
        if pos + LINK_KEY_SIZE > data.len() {
            break;
        }
        let mut addr = [0u8; 6];
        addr.copy_from_slice(&data[pos..pos + 6]);
        let mut key_val = [0u8; 16];
        key_val.copy_from_slice(&data[pos + 8..pos + 24]);

        keys.push(LinkKey {
            addr,
            addr_type: data[pos + 6],
            key_type: data[pos + 7],
            key_val,
            pin_len: data[pos + 24],
        });
        pos += LINK_KEY_SIZE;
    }

    Ok(keys)
}

fn save_link_keys_to_file(keys: &[LinkKey]) -> Result<()> {
    let key_count = keys.len() as u16;
    let mut data = Vec::with_capacity(3 + keys.len() * LINK_KEY_SIZE);
    data.push(0x00); // debug_keys
    data.extend_from_slice(&key_count.to_le_bytes());

    for key in keys {
        data.extend_from_slice(&key.addr);
        data.push(key.addr_type);
        data.push(key.key_type);
        data.extend_from_slice(&key.key_val);
        data.push(key.pin_len);
    }

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(BT_KEYS_PATH).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    // Atomic write: a reboot/crash mid-write must not truncate bt_keys (that
    // would "load 0" next boot and the next pairing would clobber every key).
    let tmp = format!("{}.tmp", BT_KEYS_PATH);
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, BT_KEYS_PATH)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_eir_complete_name() {
        // EIR with Complete Local Name (type 0x09)
        let eir = [
            0x06, 0x09, b'H', b'e', b'l', b'l', b'o', // len=6, type=0x09, "Hello"
            0x02, 0x01, 0x06, // len=2, type=0x01 (flags)
        ];
        assert_eq!(parse_eir_name(&eir), Some("Hello".into()));
    }

    #[test]
    fn parse_eir_shortened_name() {
        let eir = [0x03, 0x08, b'H', b'K']; // Shortened Local Name
        assert_eq!(parse_eir_name(&eir), Some("HK".into()));
    }

    #[test]
    fn parse_eir_no_name() {
        let eir = [0x02, 0x01, 0x06]; // Only flags, no name
        assert_eq!(parse_eir_name(&eir), None);
    }

    #[test]
    fn parse_eir_empty() {
        assert_eq!(parse_eir_name(&[]), None);
    }

    #[test]
    fn mgmt_event_cmd_complete() {
        // Build a fake CMD_COMPLETE event for READ_INFO
        let mut buf = Vec::new();
        buf.extend_from_slice(&MGMT_EV_CMD_COMPLETE.to_le_bytes()); // opcode
        buf.extend_from_slice(&0u16.to_le_bytes()); // index
        buf.extend_from_slice(&6u16.to_le_bytes()); // param_len
        buf.extend_from_slice(&MGMT_OP_READ_INFO.to_le_bytes()); // cmd opcode
        buf.push(0x00); // status = success
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // data

        let event = parse_mgmt_event(&buf);
        match event {
            MgmtEvent::CmdComplete {
                opcode,
                status,
                data,
            } => {
                assert_eq!(opcode, MGMT_OP_READ_INFO);
                assert_eq!(status, 0);
                assert_eq!(data, vec![0xAA, 0xBB, 0xCC]);
            }
            _ => panic!("expected CmdComplete"),
        }
    }

    #[test]
    fn link_key_serialize_roundtrip() {
        let keys = vec![LinkKey {
            addr: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            addr_type: 0x00,
            key_type: 0x04,
            key_val: [1; 16],
            pin_len: 0,
        }];

        // Serialize
        let mut data = Vec::new();
        data.push(0x00);
        data.extend_from_slice(&1u16.to_le_bytes());
        for key in &keys {
            data.extend_from_slice(&key.addr);
            data.push(key.addr_type);
            data.push(key.key_type);
            data.extend_from_slice(&key.key_val);
            data.push(key.pin_len);
        }

        // Deserialize
        assert!(data.len() >= 3);
        let count = u16::from_le_bytes([data[1], data[2]]);
        assert_eq!(count, 1);
        assert!(data.len() >= 3 + LINK_KEY_SIZE);
    }
}
