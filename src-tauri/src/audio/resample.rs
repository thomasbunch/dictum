//! Native-rate -> 16 kHz mono resampling (rubato SincFixedIn, chunked) + channel downmix.
//! Runs on the audio worker thread only — never in the cpal callback.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// ASR / VAD input rate.
pub const TARGET_RATE: u32 = 16_000;

/// Fixed input frames per rubato `process` call. Output ≈ chunk * ratio.
const CHUNK: usize = 1024;

/// Output/input ratio for the resampler. Pure — testable without a device.
pub fn resample_ratio(native_rate: u32) -> f64 {
    TARGET_RATE as f64 / native_rate as f64
}

/// Average interleaved channels down to mono, appending to `out`.
/// A trailing partial frame (fewer than `channels` samples) is ignored — the caller
/// only ever hands whole frames (ring reads are aligned to the channel count).
pub fn downmix_to_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let inv = 1.0 / channels as f32;
    for frame in interleaved.chunks_exact(channels) {
        let sum: f32 = frame.iter().sum();
        out.push(sum * inv);
    }
}

/// Chunked downsampler with an internal input accumulator. Feed native-rate mono via
/// [`push`]; drain the final partial chunk with [`finish`] at end of stream.
pub struct Downsampler {
    inner: SincFixedIn<f32>,
    in_buf: Vec<f32>,
}

impl Downsampler {
    pub fn new(native_rate: u32) -> Self {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        // Ratio > 0 always (native_rate > 0), so construction never fails here.
        let inner = SincFixedIn::<f32>::new(resample_ratio(native_rate), 1.0, params, CHUNK, 1)
            .expect("resampler construction");
        Self { inner, in_buf: Vec::with_capacity(CHUNK * 2) }
    }

    /// Append native-rate mono samples; push any full-chunk 16 kHz output into `out`.
    pub fn push(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        self.in_buf.extend_from_slice(mono);
        while self.in_buf.len() >= CHUNK {
            // `process` returns an owned Vec, so the &in_buf borrow ends before drain.
            let res = self.inner.process(&[&self.in_buf[..CHUNK]], None);
            self.in_buf.drain(..CHUNK);
            if let Ok(v) = res {
                out.extend_from_slice(&v[0]);
            }
        }
    }

    /// Flush the sub-chunk tail through `process_partial` at end of stream.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.in_buf.is_empty() {
            return;
        }
        let tail = std::mem::take(&mut self.in_buf);
        if let Ok(v) = self.inner.process_partial(Some(&[tail.as_slice()]), None) {
            out.extend_from_slice(&v[0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_48k_to_16k_is_one_third() {
        assert!((resample_ratio(48_000) - 1.0 / 3.0).abs() < 1e-12);
        assert!((resample_ratio(16_000) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn downmix_stereo_averages_channels() {
        let mut out = Vec::new();
        // frames: (1,-1)->0, (0.5,0.5)->0.5, (0.2,0.8)->0.5
        downmix_to_mono(&[1.0, -1.0, 0.5, 0.5, 0.2, 0.8], 2, &mut out);
        assert_eq!(out, vec![0.0, 0.5, 0.5]);
    }

    #[test]
    fn downmix_mono_is_passthrough() {
        let mut out = Vec::new();
        downmix_to_mono(&[0.1, 0.2, 0.3], 1, &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn downmix_ignores_trailing_partial_frame() {
        let mut out = Vec::new();
        // 5 samples, 2 channels -> 2 whole frames, last sample dropped
        downmix_to_mono(&[1.0, 1.0, 0.0, 0.0, 9.9], 2, &mut out);
        assert_eq!(out, vec![1.0, 0.0]);
    }

    #[test]
    fn downsampler_retains_remainder_between_pushes() {
        // Feed less than one chunk twice; first push yields nothing, buffer accumulates.
        let mut ds = Downsampler::new(48_000);
        let mut out = Vec::new();
        ds.push(&vec![0.0f32; 500], &mut out);
        assert!(out.is_empty(), "no full chunk yet");
        ds.push(&vec![0.0f32; 600], &mut out); // total 1100 >= 1024 -> one chunk processed
        assert!(!out.is_empty(), "one full chunk produced ~341 samples");
    }
}
