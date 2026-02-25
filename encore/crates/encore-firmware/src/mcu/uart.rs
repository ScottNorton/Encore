//! MCU UART debug console interface.
//!
//! The TI MSP430FR5739 MCU has a UART (eUSCI_A0) routed to SoC UART-1
//! (`/dev/ttyS1`). Stock firmware never uses it (I2C only), but the MCU
//! has a UART ISR with a 32-byte ring buffer that processes commands on CR.
//!
//! This module provides a debug console for probing baud rate and sending
//! commands to the MCU UART. It is NEVER opened automatically -- only via
//! explicit user trigger from the dashboard.

use anyhow::{bail, Context, Result};
use nix::sys::termios;
use std::io::{Read, Write};
use std::os::unix::io::AsFd;
use std::time::{Duration, Instant};
use tracing::{debug, info};

const UART_DEVICE: &str = "/dev/ttyS1";
const PROBE_BAUDS: &[u32] = &[115200, 57600, 38400, 19200, 9600];
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const RECV_TIMEOUT: Duration = Duration::from_millis(500);

/// MCU UART debug console.
pub struct McuUart {
    file: std::fs::File,
    baud: u32,
}

impl McuUart {
    /// Open the UART at a specific baud rate.
    pub fn open(baud: u32) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(UART_DEVICE)
            .with_context(|| format!("failed to open {}", UART_DEVICE))?;

        Self::configure_termios(&file, baud)?;

        info!("MCU UART: opened {} at {} baud", UART_DEVICE, baud);
        Ok(Self { file, baud })
    }

    /// Configure terminal: raw mode, 8N1, specified baud rate.
    fn configure_termios(file: &std::fs::File, baud: u32) -> Result<()> {
        let mut attrs = termios::tcgetattr(file).context("tcgetattr failed")?;

        termios::cfmakeraw(&mut attrs);

        // 8N1
        attrs.control_flags |= termios::ControlFlags::CS8;
        attrs.control_flags &= !termios::ControlFlags::PARENB;
        attrs.control_flags &= !termios::ControlFlags::CSTOPB;
        attrs.control_flags |= termios::ControlFlags::CLOCAL | termios::ControlFlags::CREAD;

        // Set baud rate
        let nix_baud = baud_to_nix(baud)?;
        termios::cfsetispeed(&mut attrs, nix_baud).context("cfsetispeed")?;
        termios::cfsetospeed(&mut attrs, nix_baud).context("cfsetospeed")?;

        // Non-blocking read with 200ms timeout
        attrs.control_chars[libc::VMIN as usize] = 0;
        attrs.control_chars[libc::VTIME as usize] = 2; // 200ms (tenths of seconds)

        termios::tcsetattr(file, termios::SetArg::TCSANOW, &attrs)
            .context("tcsetattr failed")?;

        // Flush any stale data
        termios::tcflush(file, termios::FlushArg::TCIOFLUSH).ok();

        Ok(())
    }

    /// Probe for the correct baud rate by sending CR at each rate
    /// and checking for any response within 200ms.
    pub fn probe_baud() -> Result<(Self, u32)> {
        info!("MCU UART: probing baud rates {:?}", PROBE_BAUDS);

        for &baud in PROBE_BAUDS {
            match Self::try_baud(baud) {
                Ok(uart) => {
                    info!("MCU UART: detected baud rate {}", baud);
                    return Ok((uart, baud));
                }
                Err(e) => {
                    debug!("MCU UART: {} baud - no response ({})", baud, e);
                }
            }
        }

        bail!("MCU UART: no response at any baud rate");
    }

    fn try_baud(baud: u32) -> Result<Self> {
        let mut uart = Self::open(baud)?;

        // Send CR and wait for response
        uart.file.write_all(b"\r")?;
        uart.file.flush()?;

        let start = Instant::now();
        let mut buf = [0u8; 64];

        while start.elapsed() < PROBE_TIMEOUT {
            match uart.file.read(&mut buf) {
                Ok(0) => continue,
                Ok(n) => {
                    debug!("MCU UART: {} baud got {} bytes: {:02X?}", baud, n, &buf[..n]);
                    return Ok(uart);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e.into()),
            }
        }

        bail!("no response at {} baud", baud);
    }

    /// Send a command string (appends CR) and read response until CR or timeout.
    pub fn send_recv(&mut self, cmd: &str) -> Result<String> {
        // Flush input buffer
        termios::tcflush(self.file.as_fd(), termios::FlushArg::TCIFLUSH).ok();

        // Send command + CR
        let mut data = cmd.as_bytes().to_vec();
        data.push(b'\r');
        self.file.write_all(&data)?;
        self.file.flush()?;

        debug!("MCU UART: sent {:?}", cmd);

        // Read response until CR, LF, or timeout
        let start = Instant::now();
        let mut response = Vec::with_capacity(64);
        let mut buf = [0u8; 1];

        while start.elapsed() < RECV_TIMEOUT {
            match self.file.read(&mut buf) {
                Ok(1) => {
                    if buf[0] == b'\r' || buf[0] == b'\n' {
                        if !response.is_empty() {
                            break;
                        }
                        // Skip leading CR/LF (echo of our command)
                        continue;
                    }
                    response.push(buf[0]);
                    if response.len() >= 128 {
                        break; // Safety limit
                    }
                }
                Ok(_) => {} // timeout or no data
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        }

        let text = String::from_utf8_lossy(&response).to_string();
        debug!("MCU UART: recv {:?} ({} bytes, {}ms)", text, response.len(), start.elapsed().as_millis());
        Ok(text)
    }

    /// Get the current baud rate.
    pub fn baud(&self) -> u32 {
        self.baud
    }
}

fn baud_to_nix(baud: u32) -> Result<termios::BaudRate> {
    match baud {
        9600 => Ok(termios::BaudRate::B9600),
        19200 => Ok(termios::BaudRate::B19200),
        38400 => Ok(termios::BaudRate::B38400),
        57600 => Ok(termios::BaudRate::B57600),
        115200 => Ok(termios::BaudRate::B115200),
        _ => bail!("unsupported baud rate: {}", baud),
    }
}
