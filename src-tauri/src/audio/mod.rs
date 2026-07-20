//! Audio pipeline: cpal callback -> rtrb ring -> worker (downmix, resample 16k, Silero VAD,
//! amplitude bars) -> CoordMsg. One persistent worker thread owns the cpal stream (`!Send`)
//! and all DSP. See PLAN.md §3 and CONTRACTS.md.

mod capture;
mod cues;
mod resample;
mod vad;

pub use capture::list_input_devices;
pub use cues::{Cue, Cues};

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::types::{CoordMsg, LevelBar, BAR_SAMPLES};
use vad::Segmenter;

/// Peak >= -1 dBFS marks a clipped bar (oxide, full height, forever).
const CLIP_THRESHOLD: f32 = 0.891;

/// Worker control messages (pipeline handle -> worker thread).
enum Ctrl {
    Start(Option<String>),
    /// Ack fires once the tail has been dispatched, so `stop()` can return after it.
    Stop(Sender<()>),
    Abort,
    Shutdown,
}

/// The public handle. Cheap to hold; all real work is on the worker thread.
pub struct AudioPipeline {
    ctrl: Sender<Ctrl>,
    worker: Option<JoinHandle<()>>,
}

impl AudioPipeline {
    /// Spawns the worker (and loads the VAD model) once.
    pub fn new(coord_tx: Sender<CoordMsg>, vad_model_path: PathBuf) -> Self {
        let (ctrl_tx, ctrl_rx) = mpsc::channel();
        let worker = thread::spawn(move || worker_loop(ctrl_rx, coord_tx, vad_model_path));
        Self { ctrl: ctrl_tx, worker: Some(worker) }
    }

    /// Re-query the device and open the stream immediately (worker opens off the caller's
    /// thread). Never loses first words: capture starts as soon as the worker picks this up.
    pub fn start(&self, device: Option<String>) {
        let _ = self.ctrl.send(Ctrl::Start(device));
    }

    /// Stop capture and dispatch the tail; returns after the tail has been sent.
    pub fn stop(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        if self.ctrl.send(Ctrl::Stop(ack_tx)).is_ok() {
            // ponytail: 2 s ceiling so a wedged flush can't hang the coordinator; the flush
            // is sub-millisecond in practice.
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }

    /// Discard everything (Esc). Fire-and-forget; ordered before any following `start`.
    pub fn abort(&self) {
        let _ = self.ctrl.send(Ctrl::Abort);
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        let _ = self.ctrl.send(Ctrl::Shutdown);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn worker_loop(ctrl_rx: Receiver<Ctrl>, coord_tx: Sender<CoordMsg>, vad_model_path: PathBuf) {
    let mut seg = Segmenter::new(&vad_model_path);
    let mut session: Option<Session> = None;

    loop {
        // Idle: block for the next command (zero-latency wake). Recording: poll so we can
        // keep draining audio.
        let msg = if session.is_some() {
            match ctrl_rx.try_recv() {
                Ok(m) => Some(m),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        } else {
            match ctrl_rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            }
        };

        match msg {
            Some(Ctrl::Start(device)) => {
                session = None; // drop any prior stream first
                seg.reset();
                match Session::open(device) {
                    Ok(s) => session = Some(s),
                    Err(e) => {
                        let _ = coord_tx.send(CoordMsg::CaptureDead(e));
                    }
                }
            }
            Some(Ctrl::Stop(ack)) => {
                if let Some(s) = session.take() {
                    s.finalize(&mut seg, &coord_tx);
                }
                let _ = ack.send(());
            }
            Some(Ctrl::Abort) => {
                session = None;
                seg.reset();
            }
            Some(Ctrl::Shutdown) => break,
            None => {}
        }

        // Poll the death flag and process, without holding a session borrow across take().
        let mut died: Option<String> = None;
        if let Some(s) = session.as_mut() {
            died = s.dead.lock().ok().and_then(|mut g| g.take());
            if died.is_none() {
                s.process(&mut seg, &coord_tx);
                thread::sleep(Duration::from_millis(5));
            }
        }
        if let Some(err) = died {
            // Deliver buffered audio first, then report the death (PLAN §4.5).
            if let Some(s) = session.take() {
                s.finalize(&mut seg, &coord_tx);
            }
            let _ = coord_tx.send(CoordMsg::CaptureDead(err));
        }
    }
}

/// Per-session capture + DSP state. Holds the cpal stream alive; dropping it stops capture.
struct Session {
    _stream: cpal::Stream,
    consumer: rtrb::Consumer<f32>,
    dead: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    channels: usize,
    resampler: resample::Downsampler,
    scratch: Vec<f32>,   // interleaved ring read
    mono: Vec<f32>,      // downmixed native-rate
    out16: Vec<f32>,     // resampled 16 kHz
    level_buf: Vec<f32>, // pending < BAR_SAMPLES for bar computation
    capture_started: bool,
}

impl Session {
    fn open(device: Option<String>) -> Result<Self, String> {
        let h = capture::open(device)?;
        Ok(Session {
            resampler: resample::Downsampler::new(h.native_rate),
            channels: h.channels,
            consumer: h.consumer,
            dead: h.dead,
            _stream: h.stream,
            scratch: Vec::new(),
            mono: Vec::new(),
            out16: Vec::new(),
            level_buf: Vec::new(),
            capture_started: false,
        })
    }

    /// Drain the ring, downmix, resample; leaves 16 kHz mono in `self.out16` and emits
    /// `CaptureStarted` (first frames) + `Levels`.
    fn pull(&mut self, coord_tx: &Sender<CoordMsg>) {
        self.out16.clear();
        let avail = self.consumer.slots();
        let n = whole_frames(avail, self.channels);
        if n > 0 {
            if !self.capture_started {
                self.capture_started = true;
                let _ = coord_tx.send(CoordMsg::CaptureStarted);
            }
            self.scratch.clear();
            if let Ok(chunk) = self.consumer.read_chunk(n) {
                let (a, b) = chunk.as_slices(); // ring may wrap into two slices
                self.scratch.extend_from_slice(a);
                self.scratch.extend_from_slice(b);
                chunk.commit_all();
            }
            self.mono.clear();
            resample::downmix_to_mono(&self.scratch, self.channels, &mut self.mono);
            self.resampler.push(&self.mono, &mut self.out16);
        }
        let bars = compute_bars(&mut self.level_buf, &self.out16);
        if !bars.is_empty() {
            let _ = coord_tx.send(CoordMsg::Levels(bars));
        }
    }

    /// One recording pass: pull audio, feed the VAD (emits mid-hold `SegmentClosed`).
    fn process(&mut self, seg: &mut Segmenter, coord_tx: &Sender<CoordMsg>) {
        self.pull(coord_tx);
        seg.feed(&self.out16, coord_tx);
    }

    /// Stop / death: drain the last audio, flush the resampler tail, and dispatch one
    /// `TailSegment` (remaining VAD segments + open speech, concatenated). Consumes self so
    /// the stream is torn down.
    fn finalize(mut self, seg: &mut Segmenter, coord_tx: &Sender<CoordMsg>) {
        self.pull(coord_tx);
        let mut leftover = std::mem::take(&mut self.out16);

        self.out16.clear();
        self.resampler.finish(&mut self.out16);
        let bars = compute_bars(&mut self.level_buf, &self.out16);
        if !bars.is_empty() {
            let _ = coord_tx.send(CoordMsg::Levels(bars));
        }
        leftover.extend_from_slice(&self.out16);

        // All remaining audio goes into the tail — no SegmentClosed after stop.
        let tail = seg.finish(&leftover);
        let _ = coord_tx.send(CoordMsg::TailSegment(tail));
    }
}

/// Largest multiple of `channels` not exceeding `avail` (keeps ring reads frame-aligned;
/// the partial frame stays in the ring for next time).
fn whole_frames(avail: usize, channels: usize) -> usize {
    let c = channels.max(1);
    avail - (avail % c)
}

/// One amplitude bar from a full window: peak amplitude, clip if peak hit -1 dBFS.
fn bar_from_window(win: &[f32]) -> LevelBar {
    let peak = win.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    LevelBar { amp: peak.min(1.0), clip: peak >= CLIP_THRESHOLD }
}

/// Append 16 kHz samples, emit one bar per `BAR_SAMPLES`, keep the sub-bar remainder.
fn compute_bars(level_buf: &mut Vec<f32>, samples: &[f32]) -> Vec<LevelBar> {
    level_buf.extend_from_slice(samples);
    let mut bars = Vec::new();
    let mut start = 0;
    while level_buf.len() - start >= BAR_SAMPLES {
        bars.push(bar_from_window(&level_buf[start..start + BAR_SAMPLES]));
        start += BAR_SAMPLES;
    }
    level_buf.drain(..start);
    bars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_frames_aligns_to_channels() {
        assert_eq!(whole_frames(10, 2), 10);
        assert_eq!(whole_frames(11, 2), 10);
        assert_eq!(whole_frames(7, 4), 4);
        assert_eq!(whole_frames(3, 1), 3);
        assert_eq!(whole_frames(5, 0), 5); // guard: channels never 0
    }

    #[test]
    fn bar_reports_peak_and_no_clip_below_threshold() {
        let bar = bar_from_window(&[0.1, -0.5, 0.3, -0.2]);
        assert_eq!(bar.amp, 0.5);
        assert!(!bar.clip);
    }

    #[test]
    fn bar_clips_at_minus_1_dbfs_and_clamps_amp() {
        let bar = bar_from_window(&[0.2, -1.5, 0.9]); // peak 1.5 -> amp clamps to 1.0
        assert_eq!(bar.amp, 1.0);
        assert!(bar.clip);

        let edge = bar_from_window(&[CLIP_THRESHOLD]);
        assert!(edge.clip);
    }

    #[test]
    fn compute_bars_emits_full_bars_and_retains_remainder() {
        let mut buf = Vec::new();
        // BAR_SAMPLES + 100 samples -> exactly one bar, 100 retained.
        let bars = compute_bars(&mut buf, &vec![0.4f32; BAR_SAMPLES + 100]);
        assert_eq!(bars.len(), 1);
        assert_eq!(buf.len(), 100);
        assert_eq!(bars[0].amp, 0.4);

        // Next 500 samples -> 600 buffered -> one more bar, 0 retained.
        let bars2 = compute_bars(&mut buf, &vec![0.4f32; 500]);
        assert_eq!(bars2.len(), 1);
        assert_eq!(buf.len(), 0);
    }
}
