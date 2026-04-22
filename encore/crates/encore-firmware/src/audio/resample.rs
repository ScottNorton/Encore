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

/// Resample stereo interleaved i32 PCM using linear interpolation.
///
/// Input and output are interleaved `[L, R, L, R, ...]`. Passthrough when
/// `src_rate == dst_rate`.
pub fn resample_i32_stereo(input: &[i32], src_rate: u32, dst_rate: u32) -> Vec<i32> {
    if src_rate == dst_rate || input.len() < 2 {
        return input.to_vec();
    }

    let in_frames = input.len() / 2;
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_frames = ((in_frames as f64) / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_frames * 2);

    for i in 0..out_frames {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        if idx + 1 < in_frames {
            let a_l = input[idx * 2] as f64;
            let b_l = input[(idx + 1) * 2] as f64;
            output.push((a_l + (b_l - a_l) * frac) as i32);

            let a_r = input[idx * 2 + 1] as f64;
            let b_r = input[(idx + 1) * 2 + 1] as f64;
            output.push((a_r + (b_r - a_r) * frac) as i32);
        } else if idx < in_frames {
            output.push(input[idx * 2]);
            output.push(input[idx * 2 + 1]);
        }
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

    // ── i32 stereo resampler tests ──

    #[test]
    fn i32_stereo_passthrough_same_rate() {
        let input = vec![100i32, 200, 300, 400]; // 2 stereo frames
        let out = resample_i32_stereo(&input, 48000, 48000);
        assert_eq!(out, input);
    }

    #[test]
    fn i32_stereo_44100_to_48000_length() {
        // 100 stereo frames at 44100 → should produce ~109 frames at 48000
        let input: Vec<i32> = (0..200).map(|i| i * 1000).collect();
        let out = resample_i32_stereo(&input, 44100, 48000);
        let out_frames = out.len() / 2;
        // Expected: ceil(100 * 48000/44100) = ceil(108.84) = 109
        assert_eq!(out_frames, 109);
        // Output length must be even (stereo pairs)
        assert_eq!(out.len() % 2, 0);
    }

    #[test]
    fn i32_stereo_preserves_channel_separation() {
        // L=1000, R=2000 for all frames
        let input = vec![1000i32, 2000, 1000, 2000, 1000, 2000, 1000, 2000]; // 4 frames
        let out = resample_i32_stereo(&input, 44100, 48000);
        // All left samples should be 1000, all right should be 2000
        for (i, &s) in out.iter().enumerate() {
            if i % 2 == 0 {
                assert_eq!(s, 1000, "L sample at {}", i);
            } else {
                assert_eq!(s, 2000, "R sample at {}", i);
            }
        }
    }

    #[test]
    fn i32_stereo_empty_input() {
        let out = resample_i32_stereo(&[], 44100, 48000);
        assert!(out.is_empty());
    }

    #[test]
    fn i32_stereo_single_frame() {
        let input = vec![500i32, -500];
        let out = resample_i32_stereo(&input, 44100, 48000);
        // 1 frame in → at least 1 frame out
        assert!(out.len() >= 2);
        assert_eq!(out.len() % 2, 0);
        assert_eq!(out[0], 500);
        assert_eq!(out[1], -500);
    }
}
