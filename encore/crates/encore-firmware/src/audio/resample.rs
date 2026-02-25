//! Linear interpolation resampler for speech audio.
//!
//! TTS output is typically 16kHz or 22050Hz mono; the mixer runs at 48kHz.
//! Linear interpolation is sufficient quality for speech.

/// Resample i16 PCM using linear interpolation.
///
/// Passthrough (clone) when `src_rate == dst_rate`.
pub fn resample_linear(input: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((input.len() as f64) / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        let sample = if idx + 1 < input.len() {
            let a = input[idx] as f64;
            let b = input[idx + 1] as f64;
            (a + (b - a) * frac) as i16
        } else {
            input[input.len() - 1]
        };

        output.push(sample);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_same_rate() {
        let input = vec![100, 200, 300];
        let out = resample_linear(&input, 48000, 48000);
        assert_eq!(out, input);
    }

    #[test]
    fn upsample_16k_to_48k() {
        let input = vec![0, 3000, 6000];
        let out = resample_linear(&input, 16000, 48000);
        // 3x upsample: 3 input → 9 output
        assert_eq!(out.len(), 9);
        // First and last should match input boundaries
        assert_eq!(out[0], 0);
        assert_eq!(out[out.len() - 1], 6000);
    }

    #[test]
    fn upsample_22050_to_48000() {
        let input = vec![0, 1000];
        let out = resample_linear(&input, 22050, 48000);
        // Ratio ~2.177x, so 2 samples → ~4 output
        assert!(out.len() >= 4);
        assert_eq!(out[0], 0);
    }

    #[test]
    fn empty_input() {
        let out = resample_linear(&[], 16000, 48000);
        assert!(out.is_empty());
    }
}
