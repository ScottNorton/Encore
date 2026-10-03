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
use std::io::Read;
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

// Audio clock PLL enable bit (bit 24). Read-modify-write: set this bit
// to enable MCLK output while preserving the kernel's clock dividers.
const AUDIO_CLK_PLL_BIT: u32 = 0x0100_0000;

// Audio clock upload mode bit (bit 27). Stock sets this DURING firmware
// upload only (0x0918D249), then clears it after (back to 0x0118D249).
// This configures the SPI clock source for the DSP boot kernel reception.
const AUDIO_CLK_UPLOAD_BIT: u32 = 0x0800_0000;

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
    MemoryDump {
        page: u16,
        data: Vec<u8>,
    },
    TriggerFound,
    PayloadBegin,
    PayloadEnd,
    Bootup,
    Unknown {
        category: u16,
        code: u8,
        data: Vec<u8>,
    },
}

/// DSP driver. All post-upload operations use bounded `spi_transfer`.
pub struct Dsp {
    spi_fd: File,
    fw_loaded: bool,
    gpio: Option<DspGpio>,
}

impl Dsp {
    /// Enable the SoC audio PLL/MCLK. Read-modify-write: only sets the PLL
    /// enable bit, preserving the kernel's clock divider configuration.
    pub fn enable_audio_clock() -> Result<()> {
        let cur = devmem_read(AUDIO_CLK_REG)?;
        let val = cur | AUDIO_CLK_PLL_BIT;
        devmem_write(AUDIO_CLK_REG, val)?;
        info!(
            "DSP: audio clock enabled (0xF7EA8008: 0x{:08X} → 0x{:08X})",
            cur, val
        );
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

        info!(
            "SPI: opened {} (mode=3, 8-bit, {} Hz)",
            SPI_DEVICE, UPLOAD_SPEED
        );
        Ok(Self {
            spi_fd,
            fw_loaded: false,
            gpio: None,
        })
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
        File::open(fw_path)?
            .take(MAX_FW_SIZE as u64)
            .read_to_end(&mut fw)?;
        info!("DSP: firmware size: {} bytes", fw.len());

        for byte in fw.iter_mut() {
            *byte = BIT_REVERSE[*byte as usize];
        }

        // Stock dspopen() Step 3-4: Configure GPIO pins via AVIO devmem BEFORE
        // sysfs export. This sets the hardware direction/value at the pin mux level.
        info!("DSP: configuring AVIO GPIO direction...");
        let reg_404 = devmem_read(AVIO_REG_404)?;
        // GPIO 4: output enable (set bit 4), GPIO 12/13/15: input (clear bits)
        let new_404 = (reg_404 | (1 << 4)) & !(1u32 << 12) & !(1u32 << 13) & !(1u32 << 15);
        devmem_write(AVIO_REG_404, new_404)?;
        // GPIO 4: set HIGH (SPI CS idle)
        let reg_400 = devmem_read(AVIO_REG_400)?;
        devmem_write(AVIO_REG_400, reg_400 | (1 << 4))?;
        info!(
            "DSP: AVIO 0x404: 0x{:08X}→0x{:08X}, 0x400: GPIO4 HIGH",
            reg_404, new_404
        );

        // Read GPIO input states (stock step 5)
        let reg_450 = devmem_read(0xF7E8_0450)?;
        info!("DSP: AVIO 0x450=0x{:08X} (GPIO inputs)", reg_450);

        // Stock step 6-8: sysfs GPIO export + directions + values
        info!("DSP: setting up GPIO sysfs...");
        for &pin in &[4u32, 13, 12, 15] {
            // stock export order
            super::gpio::export(pin).ok();
        }
        thread::sleep(Duration::from_millis(50)); // sysfs settle
        super::gpio::set_direction(4, true)?; // output
        super::gpio::set_direction(13, false)?; // input (stock order: 13 before 12)
        super::gpio::set_direction(12, false)?; // input
        super::gpio::set_direction(15, false)?; // input
        super::gpio::write_value(4, true)?; // CS idle HIGH
        info!("DSP: GPIO 4=out(H), 13=in, 12=in, 15=in");

        info!("DSP: resetting DSP...");
        io.reset_dsp()?;

        // Stock dsp-client: FUN_0008df90(1) — set bits 24+27 for upload clock mode.
        // Bit 24 = PLL/MCLK enable, bit 27 = upload clock mode (SPI clock source).
        // This is CRITICAL — without bit 27 the DSP boot kernel can't receive firmware.
        info!("DSP: configuring SoC registers for upload...");
        let clk = devmem_read(AUDIO_CLK_REG)?;
        let clk_upload = clk | AUDIO_CLK_PLL_BIT | AUDIO_CLK_UPLOAD_BIT;
        devmem_write(AUDIO_CLK_REG, clk_upload)?;
        info!(
            "DSP: AUDIO_CLK 0x{:08X} → 0x{:08X} (upload mode)",
            clk, clk_upload
        );

        // Stock: configure GPIO 5 as output via AVIO devmem BEFORE sysfs export.
        // FUN_0008e1f0(5,0) — set bit 5 in 0x404 (output enable)
        let reg_404 = devmem_read(AVIO_REG_404)?;
        devmem_write(AVIO_REG_404, reg_404 | (1 << 5))?;
        // Stock: clear bit 5 in 0x400 (GPIO 5 LOW via AVIO)
        let reg_400 = devmem_read(AVIO_REG_400)?;
        devmem_write(AVIO_REG_400, reg_400 & !(1u32 << 5))?;
        info!("DSP: AVIO GPIO 5 configured (output, LOW)");

        // GPIO 5 = DSP upload chip-select. Stock dsp-client pulses this
        // HIGH→LOW before SPI transfer begins, then sets HIGH after upload.
        super::gpio::export(5)?;
        super::gpio::set_direction(5, true)?;
        super::gpio::write_value(5, true)?;
        super::gpio::write_value(5, false)?;
        info!("DSP: GPIO 5 pulsed (upload CS)");

        info!(
            "DSP: uploading {} bytes over SPI (using SPI_IOC_MESSAGE)...",
            fw.len()
        );
        let mut blocks_sent = 0u32;
        // Stock embeds speed_hz in each SPI_IOC_MESSAGE transfer struct.
        // Boot kernel block (first 1536 bytes) at 1 MHz, rest at 58824 Hz.
        let mut cur_speed = UPLOAD_SPEED;

        for offset in (0..fw.len()).step_by(CHUNK_SIZE) {
            let end = (offset + CHUNK_SIZE).min(fw.len());
            let len = end - offset;
            let mut chunk = [0u8; CHUNK_SIZE];
            chunk[..len].copy_from_slice(&fw[offset..end]);
            self.spi_xfer(&chunk[..len], cur_speed)?;

            let bytes_sent = offset + CHUNK_SIZE;
            if bytes_sent.is_multiple_of(BLOCK_SIZE) {
                blocks_sent += 1;
                // Stock drops from 1 MHz to messaging speed after the first
                // 1536-byte block (the DSP boot kernel).
                if blocks_sent == 1 {
                    cur_speed = MESSAGE_SPEED;
                    info!("DSP: boot kernel sent, speed → {} Hz", MESSAGE_SPEED);
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        info!("DSP: upload complete ({} blocks)", blocks_sent);

        // Signal upload complete and release GPIO 5
        super::gpio::write_value(5, true)?;
        super::gpio::unexport(5);

        // Stock: FUN_0008df90(0) — restore clock to PLL-only (clear bit 27, keep bit 24).
        // Do NOT restore AVIO 0x400/0x404 — stock doesn't, and restoring would undo
        // the GPIO configuration the DSP needs for I2S clock generation.
        let clk_post = devmem_read(AUDIO_CLK_REG)?;
        let clk_normal = (clk_post | AUDIO_CLK_PLL_BIT) & !AUDIO_CLK_UPLOAD_BIT;
        devmem_write(AUDIO_CLK_REG, clk_normal)?;
        info!(
            "DSP: AUDIO_CLK 0x{:08X} → 0x{:08X} (normal mode)",
            clk_post, clk_normal
        );

        self.set_speed(MESSAGE_SPEED)?;
        info!("DSP: SPI speed set to {} Hz for messaging", MESSAGE_SPEED);

        self.fw_loaded = true;
        info!("DSP: firmware loaded successfully");
        Ok(())
    }

    // ── SPI primitives ────────────────────────────────────────────

    fn set_speed(&self, speed: u32) -> Result<()> {
        unsafe {
            spi_wr_speed(self.spi_fd.as_raw_fd(), &speed).context("SPI set speed")?;
        }
        Ok(())
    }

    /// Full-duplex SPI transfer at a specified speed. Used during firmware
    /// upload where speed changes per-block (1 MHz boot kernel, then 58824 Hz).
    /// Stock dsp-client uses SPI_IOC_MESSAGE for every 4-byte chunk — NOT write().
    fn spi_xfer(&self, tx: &[u8], speed: u32) -> Result<()> {
        let mut rx = [0u8; CHUNK_SIZE];
        let len = tx.len().min(rx.len());
        let xfer = SpiIocTransfer {
            tx_buf: tx.as_ptr() as u64,
            rx_buf: rx.as_mut_ptr() as u64,
            len: len as u32,
            speed_hz: speed,
            delay_usecs: 0,
            bits_per_word: 8,
            cs_change: 0,
            tx_nbits: 0,
            rx_nbits: 0,
            _pad: 0,
        };
        unsafe { spi_message(self.spi_fd.as_raw_fd(), &xfer).context("SPI upload transfer")? };
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
            bail!(
                "page 0x{:04X} out of range (max 0x{:04X})",
                page,
                DSP_MEMORY_PAGES - 1
            );
        }
        let page_hi = (page >> 8) as u8;
        let page_lo = (page & 0xFF) as u8;
        self.command_and_read(0x0000, &[0x0C, page_hi, page_lo], 200)
    }

    /// Dump a range of pages. Returns concatenated responses.
    pub fn dump_memory_range(&mut self, start_page: u16, num_pages: u16) -> Result<Vec<u8>> {
        // start_page/num_pages arrive from an unauthenticated /ws message. Clamp
        // start_page against the real page count too: without it an out-of-range
        // start (> DSP_MEMORY_PAGES) made `end` clamp below start_page, so
        // `end - start_page` underflowed u16 into a ~133 MB allocation (OOM/abort).
        let start_page = start_page.min(DSP_MEMORY_PAGES);
        let end = start_page.saturating_add(num_pages).min(DSP_MEMORY_PAGES);
        if start_page >= end {
            return Ok(Vec::new());
        }
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
        let mut file =
            std::fs::File::create(path).with_context(|| format!("cannot create {}", path))?;

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
        let end = payload
            .iter()
            .rposition(|&b| b != 0)
            .map(|p| p + 1)
            .unwrap_or(0);
        let payload = &payload[..end];

        Some(match (category, code) {
            (0, 0x04) => DspEvent::DacGain(payload.first().copied().unwrap_or(0)),
            (0, 0x05) => DspEvent::ExpectSpeech,
            (0, 0x06) => DspEvent::CancelTrigger,
            (0, 0x08) => DspEvent::Version(payload.to_vec()),
            (0, 0x09) => DspEvent::MicMute(payload.first().copied().unwrap_or(0) != 0),
            (0, 0x0C) if payload.len() >= 2 => {
                let page = ((payload[0] as u16) << 8) | payload[1] as u16;
                DspEvent::MemoryDump {
                    page,
                    data: payload[2..].to_vec(),
                }
            }
            (1, 0x00) => DspEvent::TriggerFound,
            (1, 0x01) => DspEvent::PayloadBegin,
            (1, 0x02) => DspEvent::PayloadEnd,
            (1, 0x04) => DspEvent::Bootup,
            _ => DspEvent::Unknown {
                category,
                code,
                data: payload.to_vec(),
            },
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
fn devmem_read(addr: usize) -> Result<u32> {
    use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};
    use std::num::NonZeroUsize;

    let page_size = 4096usize;
    let page_base = addr & !(page_size - 1);
    let offset = addr - page_base;

    let fd = OpenOptions::new()
        .read(true)
        .open("/dev/mem")
        .context("failed to open /dev/mem")?;

    let map_len = NonZeroUsize::new(page_size).unwrap();
    let ptr = unsafe {
        mmap(
            None,
            map_len,
            ProtFlags::PROT_READ,
            MapFlags::MAP_SHARED,
            &fd,
            page_base as i64,
        )
        .context("mmap /dev/mem failed")?
    };

    let value = unsafe {
        let reg = ptr.as_ptr().cast::<u8>().add(offset) as *const u32;
        std::ptr::read_volatile(reg)
    };

    unsafe {
        munmap(ptr, page_size).ok();
    }
    Ok(value)
}

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
            None,
            map_len,
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
            MapFlags::MAP_SHARED,
            &fd,
            page_base as i64,
        )
        .context("mmap /dev/mem failed")?
    };

    unsafe {
        let reg = ptr.as_ptr().cast::<u8>().add(offset) as *mut u32;
        std::ptr::write_volatile(reg, value);
    }

    unsafe {
        munmap(ptr, page_size).ok();
    }
    debug!("devmem: 0x{:08X} = 0x{:08X}", addr, value);
    Ok(())
}
