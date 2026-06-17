//! Safe Rust wrapper around Google's vendored libsbc C library.
//!
//! Decodes SBC Bluetooth audio frames into PCM S16_LE samples.

use std::mem::MaybeUninit;

/// Maximum PCM samples per SBC frame (16 blocks * 8 subbands = 128 per channel).
pub const SBC_MAX_SAMPLES: usize = 128;

// FFI bindings to the vendored C library
#[allow(non_camel_case_types)]
mod ffi {
    use std::os::raw::c_int;

    /// Opaque SBC codec state (sbc_t from C).
    ///
    /// Sized to hold the largest variant (sbc_dstate[2] union):
    /// 3 ints (12B) + union of dstates[2] (2 * (4 + 320) = 648B) = ~660B.
    /// We use 1024 bytes for safety margin across compilers/targets.
    #[repr(C)]
    pub struct sbc_t {
        _data: [u8; 1024],
    }

    /// SBC frame description matching the C struct layout.
    ///
    /// C definition:
    ///   bool msbc;           // _Bool (1 byte) + 3 bytes padding
    ///   enum sbc_freq freq;  // int (4 bytes)
    ///   enum sbc_mode mode;  // int
    ///   enum sbc_bam bam;    // int
    ///   int nblocks, nsubbands, bitpool;
    ///
    /// On ARM GCC, _Bool is 1 byte followed by 3 padding bytes before the
    /// next int-aligned field. Using c_int for msbc produces identical layout
    /// since both representations occupy 4 bytes at offset 0.
    #[repr(C)]
    pub struct sbc_frame {
        pub msbc: c_int,
        pub freq: c_int,
        pub mode: c_int,
        pub bam: c_int,
        pub nblocks: c_int,
        pub nsubbands: c_int,
        pub bitpool: c_int,
    }

    // Mode enum values
    pub const SBC_MODE_MONO: c_int = 0;

    unsafe extern "C" {
        pub fn sbc_reset(sbc: *mut sbc_t);
        pub fn sbc_decode(
            sbc: *mut sbc_t,
            data: *const u8,
            size: u32,
            frame: *mut sbc_frame,
            pcml: *mut i16,
            pitchl: c_int,
            pcmr: *mut i16,
            pitchr: c_int,
        ) -> c_int;
        pub fn sbc_get_frame_size(frame: *const sbc_frame) -> u32;
    }
}

/// SBC decoder state.
pub struct SbcDecoder {
    state: Box<ffi::sbc_t>,
}

impl SbcDecoder {
    /// Create a new SBC decoder.
    pub fn new() -> Self {
        let mut state = Box::new(unsafe { MaybeUninit::<ffi::sbc_t>::zeroed().assume_init() });
        unsafe { ffi::sbc_reset(&mut *state) };
        Self { state }
    }

    /// Decode one SBC frame into interleaved stereo PCM S16_LE samples.
    ///
    /// Returns `Ok((bytes_consumed, samples_written))` where samples_written
    /// is the number of i16 samples written to `pcm_out` (stereo interleaved,
    /// so frames = samples / 2).
    ///
    /// `pcm_out` must be at least `SBC_MAX_SAMPLES * 2` elements (stereo).
    pub fn decode(
        &mut self,
        sbc_data: &[u8],
        pcm_out: &mut [i16],
    ) -> Result<(usize, usize), SbcError> {
        if sbc_data.len() < 4 {
            return Err(SbcError::TooShort);
        }

        let mut frame = unsafe { MaybeUninit::<ffi::sbc_frame>::zeroed().assume_init() };

        // Decode into separate L/R buffers, then interleave
        let mut pcm_l = [0i16; SBC_MAX_SAMPLES];
        let mut pcm_r = [0i16; SBC_MAX_SAMPLES];

        let ret = unsafe {
            ffi::sbc_decode(
                &mut *self.state,
                sbc_data.as_ptr(),
                sbc_data.len() as u32,
                &mut frame,
                pcm_l.as_mut_ptr(),
                1, // pitch = 1 (contiguous samples)
                pcm_r.as_mut_ptr(),
                1,
            )
        };

        if ret != 0 {
            return Err(SbcError::DecodeFailed);
        }

        let frame_size = unsafe { ffi::sbc_get_frame_size(&frame) } as usize;
        let nsamples = (frame.nblocks * frame.nsubbands) as usize;
        let is_mono = frame.mode == ffi::SBC_MODE_MONO;

        // Interleave L/R into stereo output
        let stereo_samples = nsamples * 2;
        if pcm_out.len() < stereo_samples {
            return Err(SbcError::BufferTooSmall);
        }

        for i in 0..nsamples {
            pcm_out[i * 2] = pcm_l[i];
            pcm_out[i * 2 + 1] = if is_mono { pcm_l[i] } else { pcm_r[i] };
        }

        Ok((frame_size, stereo_samples))
    }
}

impl Drop for SbcDecoder {
    fn drop(&mut self) {
        // sbc_reset serves as cleanup — no separate finish function in Google's libsbc
        unsafe { ffi::sbc_reset(&mut *self.state) };
    }
}

#[derive(Debug)]
pub enum SbcError {
    TooShort,
    DecodeFailed,
    BufferTooSmall,
}

impl std::fmt::Display for SbcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SbcError::TooShort => write!(f, "SBC data too short"),
            SbcError::DecodeFailed => write!(f, "SBC decode failed"),
            SbcError::BufferTooSmall => write!(f, "PCM output buffer too small"),
        }
    }
}

impl std::error::Error for SbcError {}
