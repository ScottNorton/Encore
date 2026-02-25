//! Safe Rust wrapper around libfreeaptx for aptX / aptX HD decoding.
//!
//! Decodes aptX Bluetooth audio frames into interleaved stereo PCM i32 samples.
//! aptX HD outputs 24-bit samples natively; standard aptX outputs 16-bit.

/// Maximum stereo samples per aptX decode call.
/// aptX decodes 4 stereo samples per codeword (4 bytes aptX, 6 bytes aptX HD).
/// We process up to ~1024 bytes of payload at once → generous buffer.
pub const APTX_MAX_SAMPLES: usize = 4096;

mod ffi {
    use std::os::raw::c_int;

    /// Opaque aptX codec context (allocated by C library).
    #[allow(non_camel_case_types)]
    pub enum aptx_context {}

    unsafe extern "C" {
        /// Initialize aptX context. hd=0 for aptX, hd=1 for aptX HD.
        pub fn aptx_init(hd: c_int) -> *mut aptx_context;

        /// Decode aptX audio samples.
        /// Returns number of bytes consumed from input.
        /// `written` receives number of bytes written to output.
        pub fn aptx_decode(
            ctx: *mut aptx_context,
            input: *const u8,
            input_size: usize,
            output: *mut u8,
            output_size: usize,
            written: *mut usize,
        ) -> usize;

        /// Free aptX context.
        pub fn aptx_finish(ctx: *mut aptx_context);
    }
}

/// aptX / aptX HD decoder.
pub struct AptxDecoder {
    ctx: *mut ffi::aptx_context,
    hd: bool,
}

// The aptx_context is only accessed from a single thread (bt-a2dp-reader).
unsafe impl Send for AptxDecoder {}

impl AptxDecoder {
    /// Create a new decoder. `hd=true` for aptX HD, `hd=false` for standard aptX.
    pub fn new(hd: bool) -> Result<Self, AptxError> {
        let ctx = unsafe { ffi::aptx_init(if hd { 1 } else { 0 }) };
        if ctx.is_null() {
            return Err(AptxError::InitFailed);
        }
        Ok(Self { ctx, hd })
    }

    /// Decode aptX data into interleaved stereo i32 PCM samples.
    ///
    /// The libfreeaptx output format is raw 24-bit signed samples:
    /// 3 bytes per sample, interleaved as LLLRRRLLLRRRLLLRRR (groups of 4 stereo samples).
    /// Each group = 24 bytes output (4 samples × 2 channels × 3 bytes).
    ///
    /// Returns `(bytes_consumed_from_input, stereo_i32_samples_written)`.
    pub fn decode(&mut self, input: &[u8], pcm_out: &mut [i32]) -> Result<(usize, usize), AptxError> {
        if input.is_empty() {
            return Ok((0, 0));
        }

        // Output buffer: 24 bytes per 4 stereo samples.
        // Max output = (input_size / codeword_size) * 24
        let codeword_size: usize = if self.hd { 6 } else { 4 };
        let max_codewords = input.len() / codeword_size + 1;
        let max_raw_bytes = max_codewords * 24;
        let mut raw_output = vec![0u8; max_raw_bytes];

        let mut written: usize = 0;
        let consumed = unsafe {
            ffi::aptx_decode(
                self.ctx,
                input.as_ptr(),
                input.len(),
                raw_output.as_mut_ptr(),
                raw_output.len(),
                &mut written,
            )
        };

        if written == 0 {
            // aptX has 90-sample latency; first calls produce no output
            return Ok((consumed, 0));
        }

        // Convert raw 24-bit samples to i32.
        // Output format: L0L1L2 R0R1R2 L0L1L2 R0R1R2 ... (3 bytes per sample, little-endian signed)
        // Each group of 24 bytes = 4 stereo frames (L R L R L R L R)
        let num_samples = written / 3; // total mono samples (L and R interleaved)
        if pcm_out.len() < num_samples {
            return Err(AptxError::BufferTooSmall);
        }

        for i in 0..num_samples {
            let b0 = raw_output[i * 3] as i32;
            let b1 = raw_output[i * 3 + 1] as i32;
            let b2 = raw_output[i * 3 + 2] as i32;
            // Sign-extend 24-bit to 32-bit, then shift left 8 to fill i32 range
            let sample_24 = b0 | (b1 << 8) | (b2 << 16);
            // Sign extend from 24 bits
            let sample_24 = if sample_24 & 0x800000 != 0 {
                sample_24 | !0xFFFFFF_u32 as i32
            } else {
                sample_24
            };
            // Shift to fill i32 range (24-bit → 32-bit)
            pcm_out[i] = sample_24 << 8;
        }

        Ok((consumed, num_samples))
    }

    /// Whether this is an aptX HD decoder.
    pub fn is_hd(&self) -> bool {
        self.hd
    }
}

impl Drop for AptxDecoder {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { ffi::aptx_finish(self.ctx) };
        }
    }
}

#[derive(Debug)]
pub enum AptxError {
    InitFailed,
    BufferTooSmall,
}

impl std::fmt::Display for AptxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AptxError::InitFailed => write!(f, "aptX context initialization failed"),
            AptxError::BufferTooSmall => write!(f, "PCM output buffer too small"),
        }
    }
}

impl std::error::Error for AptxError {}
