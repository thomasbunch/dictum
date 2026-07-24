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
    /// Arm/disarm the streaming-preview frame tee (config-gated, boot + toggle).
    SetStreamPreview(bool),
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

    /// Arm/disarm the streaming-preview frame tee. When on, each capture pass
    /// tees its 16 kHz frames as `CoordMsg::StreamFrames`. Off (default) costs a
    /// single bool check per pass.
    pub fn set_stream_preview(&self, on: bool) {
        let _ = self.ctrl.send(Ctrl::SetStreamPreview(on));
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
    // Streaming-preview tee, config-gated at boot/toggle (survives across
    // sessions). Default off — the majority pays one bool check per pass.
    let mut stream_on = false;

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
                    s.finalize(&mut seg, &coord_tx, stream_on);
                }
                let _ = ack.send(());
            }
            Some(Ctrl::Abort) => {
                session = None;
                seg.reset();
            }
            Some(Ctrl::SetStreamPreview(on)) => stream_on = on,
            Some(Ctrl::Shutdown) => break,
            None => {}
        }

        // Poll the death flag and process, without holding a session borrow across take().
        let mut died: Option<String> = None;
        if let Some(s) = session.as_mut() {
            died = s.dead.lock().ok().and_then(|mut g| g.take());
            if died.is_none() {
                s.process(&mut seg, &coord_tx, stream_on);
                thread::sleep(Duration::from_millis(5));
            }
        }
        if let Some(err) = died {
            // Deliver buffered audio first, then report the death (PLAN §4.5).
            if let Some(s) = session.take() {
                s.finalize(&mut seg, &coord_tx, stream_on);
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
    /// `CaptureStarted` (first frames) + `Levels`. When `stream_on`, also tees the
    /// resampled frames as `StreamFrames` for the live preview decoder.
    fn pull(&mut self, coord_tx: &Sender<CoordMsg>, stream_on: bool) {
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
        // Streaming-preview tee: one send per non-empty pull (~200/sec at the 5 ms
        // poll, ~80 samples each) — NOT the Levels/bar rate. `out16` is intact for
        // the VAD path below (finalize/process only borrow it). Cost when off: the
        // bool check. Fires while armed regardless of recording state; the
        // coordinator drops frames outside a live preview session.
        tee_stream_frames(&self.out16, stream_on, coord_tx);
    }

    /// One recording pass: pull audio, feed the VAD (emits mid-hold `SegmentClosed`).
    fn process(&mut self, seg: &mut Segmenter, coord_tx: &Sender<CoordMsg>, stream_on: bool) {
        self.pull(coord_tx, stream_on);
        seg.feed(&self.out16, coord_tx);
    }

    /// Stop / death: drain the last audio, flush the resampler tail, and dispatch one
    /// `TailSegment` (remaining VAD segments + open speech, concatenated). Consumes self so
    /// the stream is torn down.
    fn finalize(mut self, seg: &mut Segmenter, coord_tx: &Sender<CoordMsg>, stream_on: bool) {
        self.pull(coord_tx, stream_on);
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

/// Tee resampled 16 kHz frames to the streaming preview decoder. One send per
/// non-empty pull while armed; a borrow, so `out16` is never mutated. Extracted
/// from `pull` so the gated branch is unit-testable without a live cpal ring.
fn tee_stream_frames(out16: &[f32], stream_on: bool, coord_tx: &Sender<CoordMsg>) {
    if stream_on && !out16.is_empty() {
        let _ = coord_tx.send(CoordMsg::StreamFrames(out16.to_vec()));
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
    fn stream_tee_gated_and_exact() {
        let (tx, rx) = mpsc::channel();
        let frames = vec![0.1f32, 0.2, 0.3];

        // Off: nothing teed.
        tee_stream_frames(&frames, false, &tx);
        assert!(rx.try_recv().is_err());

        // On + empty: nothing teed (tee is per non-empty pull).
        tee_stream_frames(&[], true, &tx);
        assert!(rx.try_recv().is_err());

        // On + non-empty: EXACTLY ONE StreamFrames carrying the same samples.
        tee_stream_frames(&frames, true, &tx);
        match rx.try_recv() {
            Ok(CoordMsg::StreamFrames(s)) => assert_eq!(s, frames),
            other => panic!("expected one StreamFrames, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "more than one StreamFrames per pull");
        // The tee borrows the slice — the VAD path still sees it intact.
        assert_eq!(frames, vec![0.1f32, 0.2, 0.3]);
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
