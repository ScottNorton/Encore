//! DSP firmware uploader and SPI messaging.
//!
//! Uploads the ADSP-21489 SHARC DSP firmware at boot via /dev/spidev0.0,
//! then switches to messaging mode for runtime commands (volume, mic mute,
//! version query, memory dumps).
//!
//! All runtime SPI operations use the bounded `SPI_IOC_MESSAGE` ioctl
//! (full-duplex transfer). No GPIO, no raw read(), nothing that can block
//! indefinitely. Safe to call from `spawn_blocking`.

use super::dsp_gpio::DspGpio;
use super::io_expander::IoExpander;
use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, info};

const SPI_DEVICE: &str = "/dev/spidev0.0";
const UPLOAD_SPEED: u32 = 1_000_000;
const MESSAGE_SPEED: u32 = 58_824;

const FIRMWARE_PATHS: &[&str] = &[
    "/usr/share/dsp/dsp-img.ldr",
    "/media/usb/dsp-img.ldr",
    "/data/test/dsp-img.ldr",
];
const MAX_FW_SIZE: usize = 614_400;
const CHUNK_SIZE: usize = 4;
const BLOCK_SIZE: usize = 1536;

// SPI ioctl declarations (magic 'k' = 0x6b)
nix::ioctl_write_ptr!(spi_wr_mode, b'k', 1, u8);
nix::ioctl_write_ptr!(spi_wr_bits, b'k', 3, u8);
nix::ioctl_write_ptr!(spi_wr_speed, b'k', 4, u32);

/// Kernel `struct spi_ioc_transfer` — 32 bytes on ARM32.
#[repr(C)]
struct SpiIocTransfer {
    tx_buf: u64,
    rx_buf: u64,
    len: u32,
    speed_hz: u32,
    delay_usecs: u16,
    bits_per_word: u8,
    cs_change: u8,
    tx_nbits: u8,
    rx_nbits: u8,
    _pad: u16,
}

// SPI_IOC_MESSAGE(1) = 0x40206b00 on ARM32
nix::ioctl_write_ptr!(spi_message, b'k', 0, SpiIocTransfer);

// SoC register addresses
const AVIO_REG_404: usize = 0xF7E8_0404;
const AVIO_REG_400: usize = 0xF7E8_0400;
const AUDIO_CLK_REG: usize = 0xF7EA_8008;

// SoC register values
const AVIO_SPI_MODE: u32 = 0x0000_0F28;
const AVIO_CONFIG: u32 = 0x0000_0A08;
const AUDIO_CLK_VALUE: u32 = 0x0118_D249;

/// Pre-computed bit-reversal LUT.
const BIT_REVERSE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut r = 0u8;
        let mut v = i as u8;
        let mut b = 0;
        while b < 8 {
            r = (r << 1) | (v & 1);
            v >>= 1;
            b += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
};

/// Total DSP memory pages (0x0000..0x013F).
pub const DSP_MEMORY_PAGES: u16 = 0x140;

/// SPI response buffer size (matches stock dsp-client).
const SPI_RX_SIZE: usize = 2048;

/// DSP event parsed from SPI response.
#[derive(Debug)]
pub enum DspEvent {
    DacGain(u8),
    ExpectSpeech,
    CancelTrigger,
    Version(Vec<u8>),
    MicMute(bool),
    MemoryDump { page: u16, data: Vec<u8> },
    TriggerFound,
    PayloadBegin,
    PayloadEnd,
    Bootup,
    Unknown { category: u16, code: u8, data: Vec<u8> },
}

/// DSP driver. All post-upload operations use bounded `spi_transfer`.
pub struct Dsp {
    spi_fd: File,
    fw_loaded: bool,
    gpio: Option<DspGpio>,
}

impl Dsp {
    /// Enable the SoC audio PLL/MCLK. Must be called well before DSP upload
    /// so the WM8904 codec has time to lock its PLL and start generating I2S
    /// clocks. The DSP firmware needs active I2S input clocks at boot to
    /// configure its own I2S output (SCLK/LRCK to the DAC).
    pub fn enable_audio_clock() -> Result<()> {
        devmem_write(AUDIO_CLK_REG, AUDIO_CLK_VALUE)?;
        info!("DSP: audio clock enabled (0xF7EA8008 = 0x{:08X})", AUDIO_CLK_VALUE);
        Ok(())
    }
}

impl Dsp {
    pub fn open() -> Result<Self> {
        let spi_fd = OpenOptions::new()
            .read(true)
            .write(true)
            .open(SPI_DEVICE)
            .context("failed to open SPI device")?;

        let fd = spi_fd.as_raw_fd();
        unsafe {
            let mode: u8 = 3;
            let bits: u8 = 8;
            spi_wr_mode(fd, &mode).context("SPI set mode")?;
            spi_wr_bits(fd, &bits).context("SPI set bits")?;
            spi_wr_speed(fd, &UPLOAD_SPEED).context("SPI set speed")?;
        }

        info!("SPI: opened {} (mode=3, 8-bit, {} Hz)", SPI_DEVICE, UPLOAD_SPEED);
        Ok(Self { spi_fd, fw_loaded: false, gpio: None })
    }

    /// Reset firmware loaded state (for re-upload after DSP power cycle).
    pub fn reset_fw_state(&mut self) {
        self.fw_loaded = false;
    }

    // ── Firmware upload (boot-time, blocking is fine) ──────────────

    pub fn upload_firmware(&mut self, io: &mut IoExpander) -> Result<()> {
        let fw_path = FIRMWARE_PATHS
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .context("DSP firmware not found")?;

        info!("DSP: loading firmware from {}", fw_path);

        let mut fw = Vec::new();
        File::open(fw_path)?.take(MAX_FW_SIZE as u64).read_to_end(&mut fw)?;
        info!("DSP: firmware size: {} bytes", fw.len());

        for byte in fw.iter_mut() {
            *byte = BIT_REVERSE[*byte as usize];
        }

        info!("DSP: resetting DSP...");
        io.reset_dsp()?;

        info!("DSP: configuring SoC registers for upload...");
        devmem_write(AUDIO_CLK_REG, AUDIO_CLK_VALUE)?;
        devmem_write(AVIO_REG_404, AVIO_SPI_MODE)?;
        devmem_write(AVIO_REG_400, AVIO_CONFIG)?;

        info!("DSP: uploading {} bytes over SPI...", fw.len());
        let mut blocks_sent = 0u32;

        for offset in (0..fw.len()).step_by(CHUNK_SIZE) {
            let end = (offset + CHUNK_SIZE).min(fw.len());
            let len = end - offset;
            let mut chunk = [0u8; CHUNK_SIZE];
            chunk[..len].copy_from_slice(&fw[offset..end]);
            self.spi_fd.write_all(&chunk)?;

            let bytes_sent = offset + CHUNK_SIZE;
            if bytes_sent % BLOCK_SIZE == 0 {
                blocks_sent += 1;
                thread::sleep(Duration::from_millis(10));
            }
        }

        info!("DSP: upload complete ({} blocks)", blocks_sent);

        devmem_write(AVIO_REG_400, AVIO_CONFIG)?;
        info!("DSP: AVIO in stock DSP mode (SPI+I2S)");

        self.set_speed(MESSAGE_SPEED)?;
        info!("DSP: SPI speed set to {} Hz for messaging", MESSAGE_SPEED);

        self.fw_loaded = true;
        info!("DSP: firmware loaded successfully");
        Ok(())
    }

    // ── SPI primitives ────────────────────────────────────────────

    fn set_speed(&self, speed: u32) -> Result<()> {
        unsafe { spi_wr_speed(self.spi_fd.as_raw_fd(), &speed).context("SPI set speed")?; }
        Ok(())
    }

    /// Bounded full-duplex SPI transfer. Sends `tx`, receives into `rx`.
    /// Always completes — master generates exactly `len` clock cycles.
    fn spi_transfer(&self, tx: &[u8], rx: &mut [u8]) -> Result<usize> {
        let len = tx.len().min(rx.len());
        if len == 0 {
            return Ok(0);
        }
        let xfer = SpiIocTransfer {
            tx_buf: tx.as_ptr() as u64,
            rx_buf: rx.as_mut_ptr() as u64,
            len: len as u32,
            speed_hz: MESSAGE_SPEED,
            delay_usecs: 0,
            bits_per_word: 8,
            cs_change: 0,
            tx_nbits: 0,
            rx_nbits: 0,
            _pad: 0,
        };
        unsafe { spi_message(self.spi_fd.as_raw_fd(), &xfer).context("SPI transfer")? };
        Ok(len)
    }

    /// Send command, wait, then read response via full-duplex zero-transfer.
    /// Returns the raw response buffer (caller parses).
    /// When GPIO is available, uses proper handshaking instead of blind sleep.
    fn command_and_read(&mut self, msg_type: u16, data: &[u8], delay_ms: u64) -> Result<Vec<u8>> {
        if !self.fw_loaded {
            bail!("DSP firmware not loaded");
        }

        if let Some(ref gpio) = self.gpio {
            // GPIO-assisted: proper handshake
            gpio.pre_send()?;
            let tx = build_spi_message(msg_type, data);
            let mut discard = vec![0u8; tx.len()];
            self.spi_transfer(&tx, &mut discard)?;
            self.gpio.as_ref().unwrap().post_send()?;

            // Poll GPIO 12 for response data
            let timeout = Duration::from_millis(delay_ms.max(100));
            let start = Instant::now();
            loop {
                if self.gpio.as_ref().unwrap().has_data()? {
                    break;
                }
                if start.elapsed() > timeout {
                    break; // Read anyway -- DSP may have responded already
                }
                thread::sleep(Duration::from_millis(1));
            }

            // Receive handshake + SPI read
            self.gpio.as_ref().unwrap().pre_receive()?;
            let zeros = vec![0u8; SPI_RX_SIZE];
            let mut rx = vec![0u8; SPI_RX_SIZE];
            self.spi_transfer(&zeros, &mut rx)?;
            self.gpio.as_ref().unwrap().post_receive()?;

            Ok(rx)
        } else {
            // No GPIO: blind sleep (existing behavior)
            let tx = build_spi_message(msg_type, data);
            let mut discard = vec![0u8; tx.len()];
            self.spi_transfer(&tx, &mut discard)?;

            thread::sleep(Duration::from_millis(delay_ms));

            let zeros = vec![0u8; SPI_RX_SIZE];
            let mut rx = vec![0u8; SPI_RX_SIZE];
            self.spi_transfer(&zeros, &mut rx)?;

            Ok(rx)
        }
    }

    // ── GPIO flow control ──────────────────────────────────────────

    /// Initialize GPIO flow control pins (4, 12, 13, 15).
    /// Call only after firmware upload + version query succeed.
    pub fn init_gpio(&mut self) -> Result<()> {
        let gpio = DspGpio::init()?;
        self.gpio = Some(gpio);
        Ok(())
    }

    /// Check if GPIO flow control is active.
    pub fn has_gpio(&self) -> bool {
        self.gpio.is_some()
    }

    /// Read GPIO pin states for diagnostics.
    pub fn gpio_state(&self) -> Option<super::dsp_gpio::GpioPinState> {
        self.gpio.as_ref().map(|g| g.read_state())
    }

    /// Poll for unsolicited DSP events via GPIO 12 data-ready signal.
    /// Returns None if no data available or GPIO not initialized.
    pub fn poll_event(&self) -> Result<Option<DspEvent>> {
        let gpio = match &self.gpio {
            Some(g) => g,
            None => return Ok(None),
        };

        if !gpio.has_data()? {
            return Ok(None);
        }

        debug!("DSP GPIO: data ready, receiving event");

        // Receive handshake + SPI read
        gpio.pre_receive()?;
        let zeros = vec![0u8; SPI_RX_SIZE];
        let mut rx = vec![0u8; SPI_RX_SIZE];
        self.spi_transfer(&zeros, &mut rx)?;
        gpio.post_receive()?;

        Ok(Self::parse_event(&rx))
    }

    // ── Runtime commands (all use bounded spi_transfer) ───────────

    /// Send a fire-and-forget DSP command (volume, mute, etc.)
    pub fn send_message(&mut self, msg_type: u16, data: &[u8]) -> Result<()> {
        if !self.fw_loaded {
            bail!("DSP firmware not loaded");
        }
        let tx = build_spi_message(msg_type, data);
        let mut discard = vec![0u8; tx.len()];
        self.spi_transfer(&tx, &mut discard)?;
        Ok(())
    }

    pub fn set_volume(&mut self, level: u8) -> Result<()> {
        self.send_message(0x0000, &[0x04, level.clamp(1, 100)])
    }

    pub fn set_mic_mute(&mut self, muted: bool) -> Result<()> {
        self.send_message(0x0000, &[0x09, if muted { 1 } else { 0 }])?;
        info!("DSP: mic mute = {}", muted);
        Ok(())
    }

    /// Query firmware version. Returns raw bytes from DSP response.
    pub fn query_version(&mut self) -> Result<Vec<u8>> {
        let rx = self.command_and_read(0x0000, &[0x08], 100)?;
        // Find first non-zero region
        let start = rx.iter().position(|&b| b != 0);
        match start {
            Some(s) => {
                let end = rx[s..].iter().rposition(|&b| b != 0).unwrap_or(0) + s + 1;
                Ok(rx[s..end].to_vec())
            }
            None => Ok(Vec::new()),
        }
    }

    /// Send arbitrary SPI command, return full response buffer.
    pub fn send_raw(&mut self, msg_type: u16, data: &[u8]) -> Result<Vec<u8>> {
        self.command_and_read(msg_type, data, 150)
    }

    // ── Memory dump (no GPIO, uses command_and_read) ──────────────

    /// Dump a single memory page. Returns raw response (~2KB).
    pub fn dump_memory_page(&mut self, page: u16) -> Result<Vec<u8>> {
        if page >= DSP_MEMORY_PAGES {
            bail!("page 0x{:04X} out of range (max 0x{:04X})", page, DSP_MEMORY_PAGES - 1);
        }
        let page_hi = (page >> 8) as u8;
        let page_lo = (page & 0xFF) as u8;
        self.command_and_read(0x0000, &[0x0C, page_hi, page_lo], 200)
    }

    /// Dump a range of pages. Returns concatenated responses.
    pub fn dump_memory_range(&mut self, start_page: u16, num_pages: u16) -> Result<Vec<u8>> {
        let end = start_page.saturating_add(num_pages).min(DSP_MEMORY_PAGES);
        info!("DSP: dumping pages 0x{:04X}..0x{:04X}", start_page, end - 1);

        let mut result = Vec::with_capacity((end - start_page) as usize * SPI_RX_SIZE);
        for page in start_page..end {
            let data = self.dump_memory_page(page)?;
            result.extend_from_slice(&data);
            if page % 32 == 0 && page > start_page {
                info!("DSP: dump {}/{}", page - start_page, end - start_page);
            }
        }
        info!("DSP: dump complete ({} bytes)", result.len());
        Ok(result)
    }

    /// Dump all 320 pages to a file. Non-blocking when called via spawn_blocking.
    pub fn dump_all_to_file(&mut self, path: &str) -> Result<usize> {
        use std::io::Write as _;
        let mut file = std::fs::File::create(path)
            .with_context(|| format!("cannot create {}", path))?;

        info!("DSP: dumping {} pages to {}", DSP_MEMORY_PAGES, path);
        let mut total = 0usize;

        for page in 0..DSP_MEMORY_PAGES {
            let data = self.dump_memory_page(page)?;
            file.write_all(&data)?;
            total += data.len();
            if page % 32 == 0 {
                info!("DSP: {}/{} pages ({} bytes)", page, DSP_MEMORY_PAGES, total);
            }
        }
        file.flush()?;
        info!("DSP: {} bytes written to {}", total, path);
        Ok(total)
    }

    /// Parse a raw SPI response buffer into a DspEvent (if any).
    pub fn parse_event(rx: &[u8]) -> Option<DspEvent> {
        if rx.len() < 3 {
            return None;
        }

        // Find first non-zero region (response may be offset in buffer)
        let start = rx.iter().position(|&b| b != 0)?;
        let data = &rx[start..];
        if data.len() < 3 {
            return None;
        }

        let category = ((data[0] as u16) << 8) | data[1] as u16;
        let code = data[2];
        let payload = if data.len() > 3 { &data[3..] } else { &[] };
        // Trim trailing zeros
        let end = payload.iter().rposition(|&b| b != 0).map(|p| p + 1).unwrap_or(0);
        let payload = &payload[..end];

        Some(match (category, code) {
            (0, 0x04) => DspEvent::DacGain(payload.first().copied().unwrap_or(0)),
            (0, 0x05) => DspEvent::ExpectSpeech,
            (0, 0x06) => DspEvent::CancelTrigger,
            (0, 0x08) => DspEvent::Version(payload.to_vec()),
            (0, 0x09) => DspEvent::MicMute(payload.first().copied().unwrap_or(0) != 0),
            (0, 0x0C) if payload.len() >= 2 => {
                let page = ((payload[0] as u16) << 8) | payload[1] as u16;
                DspEvent::MemoryDump { page, data: payload[2..].to_vec() }
            }
            (1, 0x00) => DspEvent::TriggerFound,
            (1, 0x01) => DspEvent::PayloadBegin,
            (1, 0x02) => DspEvent::PayloadEnd,
            (1, 0x04) => DspEvent::Bootup,
            _ => DspEvent::Unknown { category, code, data: payload.to_vec() },
        })
    }
}

/// Build SPI message frame: [type_hi, type_lo, len_hi, len_lo, checksum, data..., pad to 8].
fn build_spi_message(msg_type: u16, data: &[u8]) -> Vec<u8> {
    let type_hi = (msg_type >> 8) as u8;
    let type_lo = (msg_type & 0xFF) as u8;
    let len_hi = (data.len() >> 8) as u8;
    let len_lo = (data.len() & 0xFF) as u8;

    let checksum = type_hi
        .wrapping_add(type_lo)
        .wrapping_add(len_hi)
        .wrapping_add(len_lo)
        .wrapping_add(data.iter().copied().fold(0u8, |a, b| a.wrapping_add(b)));

    let mut msg = Vec::with_capacity(8 + data.len());
    msg.extend_from_slice(&[type_hi, type_lo, len_hi, len_lo, checksum]);
    msg.extend_from_slice(data);
    while msg.len() % 8 != 0 {
        msg.push(0);
    }
    msg
}

/// Write a 32-bit value to a physical SoC register via /dev/mem mmap.
fn devmem_write(addr: usize, value: u32) -> Result<()> {
    use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};
    use std::num::NonZeroUsize;

    let page_size = 4096usize;
    let page_base = addr & !(page_size - 1);
    let offset = addr - page_base;

    let fd = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/mem")
        .context("failed to open /dev/mem")?;

    let map_len = NonZeroUsize::new(page_size).unwrap();
    let ptr = unsafe {
        mmap(
            None, map_len,
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
            MapFlags::MAP_SHARED,
            &fd, page_base as i64,
        ).context("mmap /dev/mem failed")?
    };

    unsafe {
        let reg = ptr.as_ptr().cast::<u8>().add(offset) as *mut u32;
        std::ptr::write_volatile(reg, value);
    }

    unsafe { munmap(ptr, page_size).ok(); }
    debug!("devmem: 0x{:08X} = 0x{:08X}", addr, value);
    Ok(())
}
