//! ALSA control interface — mixer element get/set via kernel ioctls.
//!
//! Replaces `amixer` shell commands with direct ioctl calls to
//! /dev/snd/controlCn. Struct layouts are for 32-bit ARM (kernel 3.8).

use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use tracing::{debug, info};

// ── Compile-time struct size assertions ─────────────────────────────
// These use only fixed-width types, so sizes are identical on all platforms.
const _: () = {
    assert!(std::mem::size_of::<SndCtlElemId>() == 64);
    assert!(std::mem::size_of::<SndCtlElemInfo>() == 272);
    assert!(std::mem::size_of::<SndCtlElemValue>() == 712);
};

// ── Constants ───────────────────────────────────────────────────────

const CTL_MAGIC: u8 = b'U';

/// ALSA element interface: SNDRV_CTL_ELEM_IFACE_MIXER = 2
const IFACE_MIXER: u32 = 2;

/// ALSA element value types (snd_ctl_elem_type_t)
const TYPE_BOOLEAN: u32 = 1;
const TYPE_INTEGER: u32 = 2;

// ── Kernel structs (repr(C), 32-bit ARM ABI) ────────────────────────

/// Element identifier — 64 bytes.
/// Kernel matches by (iface, name) when numid == 0.
#[repr(C)]
#[derive(Clone)]
struct SndCtlElemId {
    numid: u32,
    iface: u32,
    device: u32,
    subdevice: u32,
    name: [u8; 44], // SNDRV_CTL_ELEM_ID_NAME_MAXLEN
    index: u32,
}

impl SndCtlElemId {
    fn mixer(name: &str) -> Self {
        let mut id: Self = unsafe { std::mem::zeroed() };
        id.iface = IFACE_MIXER;
        let bytes = name.as_bytes();
        let len = bytes.len().min(43); // leave null terminator
        id.name[..len].copy_from_slice(&bytes[..len]);
        id
    }
}

/// Info value union — 128 bytes, 8-byte aligned.
/// Matches C union containing `struct { long long min, max, step; }`.
#[repr(C, align(8))]
struct ElemInfoData {
    bytes: [u8; 128],
}

impl ElemInfoData {
    fn zeroed() -> Self {
        Self { bytes: [0u8; 128] }
    }

    /// INTEGER type: min value (first `long` in the `integer` variant).
    fn integer_min(&self) -> i32 {
        i32::from_ne_bytes(self.bytes[0..4].try_into().unwrap())
    }

    /// INTEGER type: max value (second `long` in the `integer` variant).
    fn integer_max(&self) -> i32 {
        i32::from_ne_bytes(self.bytes[4..8].try_into().unwrap())
    }
}

/// Element value data union — 512 bytes, 8-byte aligned.
/// Matches C union containing `long long value[64]`.
#[repr(C, align(8))]
struct ElemValueData {
    bytes: [u8; 512],
}

impl ElemValueData {
    fn zeroed() -> Self {
        Self { bytes: [0u8; 512] }
    }

    /// Write i32 at channel index (maps to `value.integer.value[idx]`
    /// where `long` = i32 on 32-bit ARM).
    fn set_i32(&mut self, idx: usize, val: i32) {
        let off = idx * 4;
        self.bytes[off..off + 4].copy_from_slice(&val.to_ne_bytes());
    }
}

/// Element info — 272 bytes.
#[repr(C)]
struct SndCtlElemInfo {
    id: SndCtlElemId,     // 64
    type_: u32,           // 4
    access: u32,          // 4
    count: u32,           // 4
    owner: i32,           // 4  (pid_t)
    value: ElemInfoData,  // 128 (at offset 80, already 8-aligned)
    reserved: [u8; 64],   // 64
}

impl SndCtlElemInfo {
    fn new(id: SndCtlElemId) -> Self {
        Self {
            id,
            type_: 0,
            access: 0,
            count: 0,
            owner: 0,
            value: ElemInfoData::zeroed(),
            reserved: [0u8; 64],
        }
    }
}

/// Element value — 712 bytes.
///
/// C struct layout:
///   snd_ctl_elem_id id;           // 64 bytes
///   unsigned int indirect: 1;     // 4 bytes (bitfield storage)
///   /* 4 bytes padding */         // align union to 8
///   union { ... } value;          // 512 bytes
///   struct timespec tstamp;       // 8 bytes (32-bit)
///   unsigned char reserved[120];  // 128 - sizeof(timespec)
///
/// The padding is implicit — repr(C) inserts it because ElemValueData
/// has align(8), matching the C union's alignment from `long long`.
#[repr(C)]
struct SndCtlElemValue {
    id: SndCtlElemId,       // 64
    indirect: u32,          // 4  (+ 4 implicit padding)
    value: ElemValueData,   // 512 at offset 72
    tstamp: [u32; 2],       // 8  (struct timespec on 32-bit)
    reserved: [u8; 120],    // 128 - sizeof(timespec)
}

impl SndCtlElemValue {
    fn new(id: SndCtlElemId) -> Self {
        Self {
            id,
            indirect: 0,
            value: ElemValueData::zeroed(),
            tstamp: [0; 2],
            reserved: [0u8; 120],
        }
    }
}

// ── nix ioctl declarations ──────────────────────────────────────────

nix::ioctl_readwrite!(ctl_elem_info, CTL_MAGIC, 0x11, SndCtlElemInfo);
nix::ioctl_readwrite!(ctl_elem_write, CTL_MAGIC, 0x13, SndCtlElemValue);

// ── Public API ──────────────────────────────────────────────────────

/// ALSA control device for reading/writing mixer elements.
pub struct AlsaCtl {
    file: File,
}

impl AlsaCtl {
    /// Open the ALSA control device for card N.
    pub fn open(card: u32) -> Result<Self> {
        let path = format!("/dev/snd/controlC{}", card);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path))?;
        info!("Opened ALSA control: {}", path);
        Ok(Self { file })
    }

    /// Query element info (type, channel count, min/max for integers).
    fn elem_info(&self, name: &str) -> Result<SndCtlElemInfo> {
        let mut info = SndCtlElemInfo::new(SndCtlElemId::mixer(name));
        let fd = self.file.as_raw_fd();
        unsafe {
            ctl_elem_info(fd, &mut info)
                .with_context(|| format!("ELEM_INFO '{}' failed", name))?;
        }
        Ok(info)
    }

    /// Write element value.
    fn elem_write(&self, ev: &mut SndCtlElemValue, name: &str) -> Result<()> {
        let fd = self.file.as_raw_fd();
        unsafe {
            ctl_elem_write(fd, ev)
                .with_context(|| format!("ELEM_WRITE '{}' failed", name))?;
        }
        Ok(())
    }

    /// Set an integer mixer control (e.g., "Master", "Headphone").
    /// Values are clamped to the element's hardware range.
    /// If fewer values than channels, the last value is replicated.
    pub fn set_integer(&self, name: &str, values: &[i32]) -> Result<()> {
        let info = self.elem_info(name)?;
        if info.type_ != TYPE_INTEGER {
            bail!("'{}': type {} != INTEGER({})", name, info.type_, TYPE_INTEGER);
        }

        let min = info.value.integer_min();
        let max = info.value.integer_max();
        let count = info.count as usize;

        let mut ev = SndCtlElemValue::new(info.id.clone());
        for i in 0..count {
            let v = values.get(i).copied().unwrap_or(values[values.len() - 1]);
            ev.value.set_i32(i, v.clamp(min, max));
        }
        self.elem_write(&mut ev, name)?;

        debug!("{}: {} (range {}..{})", name,
            values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
            min, max);
        Ok(())
    }

    /// Set a boolean mixer switch (e.g., "DAC OSRx2").
    pub fn set_bool(&self, name: &str, on: bool) -> Result<()> {
        let info = self.elem_info(name)?;
        if info.type_ != TYPE_BOOLEAN {
            bail!("'{}': type {} != BOOLEAN({})", name, info.type_, TYPE_BOOLEAN);
        }

        let count = info.count as usize;
        let mut ev = SndCtlElemValue::new(info.id.clone());
        for i in 0..count {
            ev.value.set_i32(i, on as i32);
        }
        self.elem_write(&mut ev, name)?;

        debug!("{}: {}", name, if on { "on" } else { "off" });
        Ok(())
    }
}
