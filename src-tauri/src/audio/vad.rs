//! Silero VAD segmentation (via sherpa-onnx) with a graceful no-model fallback.
//! Interior-mutable sherpa API; durations are in SECONDS.

use std::path::Path;
use std::sync::mpsc::Sender;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use crate::types::CoordMsg;

/// Silero v5 window @16 kHz (32 ms). We feed exactly this many samples per accept.
pub const WINDOW: usize = 512;

fn create_vad(model_path: &Path) -> Option<VoiceActivityDetector> {
    let silero = SileroVadModelConfig {
        model: Some(model_path.to_string_lossy().into_owned()),
        threshold: 0.3,             // 0.5 clips soft first words (PLAN §4.3)
        min_silence_duration: 0.5,  // seconds
        min_speech_duration: 0.1,   // seconds
        window_size: WINDOW as i32,
        max_speech_duration: 30.0,  // seconds (5 s default force-splits sentences)
    };
    let cfg = VadModelConfig {
        silero_vad: silero,
        ten_vad: Default::default(),
        sample_rate: 16_000,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
    };
    let vad = VoiceActivityDetector::create(&cfg, 60.0); // 60 s ring
    if vad.is_none() {
        eprintln!(
            "audio: Silero VAD failed to load ({}) — capturing without segmentation",
            model_path.display()
        );
    }
    vad
}

/// Owns the persistent VAD (created once, reset between sessions) plus a pending
/// sub-window accumulator. When the model is absent it degrades to buffering the
/// whole recording and emitting it untrimmed as the tail — dictation still works.
pub struct Segmenter {
    vad: Option<VoiceActivityDetector>,
    buf: Vec<f32>,
}

impl Segmenter {
    pub fn new(model_path: &Path) -> Self {
        Self { vad: create_vad(model_path), buf: Vec::new() }
    }

    /// Live feed during recording. Emits `SegmentClosed` for each VAD-closed segment.
    pub fn feed(&mut self, samples: &[f32], coord_tx: &Sender<CoordMsg>) {
        match &self.vad {
            Some(v) => {
                self.buf.extend_from_slice(samples);
                let mut start = 0;
                while self.buf.len() - start >= WINDOW {
                    v.accept_waveform(&self.buf[start..start + WINDOW]);
                    start += WINDOW;
                    while let Some(seg) = v.front() {
                        let _ = coord_tx.send(CoordMsg::SegmentClosed(seg.samples().to_vec()));
                        v.pop();
                    }
                }
                self.buf.drain(..start);
            }
            // No VAD: accumulate everything for an untrimmed tail.
            None => self.buf.extend_from_slice(samples),
        }
    }

    /// Stop / device-death: feed the final audio, flush trailing speech, and return the
    /// remaining segments + open speech concatenated into one buffer (may be empty).
    /// Resets the detector so the next session starts clean.
    pub fn finish(&mut self, samples: &[f32]) -> Vec<f32> {
        match &self.vad {
            Some(v) => {
                self.buf.extend_from_slice(samples);
                let mut start = 0;
                while self.buf.len() - start >= WINDOW {
                    v.accept_waveform(&self.buf[start..start + WINDOW]);
                    start += WINDOW;
                }
                if self.buf.len() > start {
                    v.accept_waveform(&self.buf[start..]); // sub-window tail
                }
                self.buf.clear();
                v.flush();
                let mut tail = Vec::new();
                while let Some(seg) = v.front() {
                    tail.extend_from_slice(seg.samples());
                    v.pop();
                }
                v.reset();
                tail
            }
            None => {
                self.buf.extend_from_slice(samples);
                std::mem::take(&mut self.buf)
            }
        }
    }

    /// Discard all buffered state (abort, or before a fresh session).
    pub fn reset(&mut self) {
        self.buf.clear();
        if let Some(v) = &self.vad {
            v.reset();
        }
    }
}
