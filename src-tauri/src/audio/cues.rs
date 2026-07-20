//! Earcons on a fully isolated output path (Handy #1712: a wedged chime device once hung
//! the whole pipeline). WAVs are pre-decoded at construction; the sink is opened on a
//! spawned thread guarded by a timeout so a stuck device never blocks callers; `play` is a
//! non-blocking fire-and-forget; every failure is a silent no-op.

use std::num::NonZero;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rodio::buffer::SamplesBuffer;
use rodio::source::Buffered;
use rodio::{DeviceSinkBuilder, Player, Source};

/// File name order matches `Cue as usize`.
const CUE_FILES: [&str; 4] = [
    "cue_start.wav",
    "cue_stop.wav",
    "cue_discard.wav",
    "cue_error.wav",
];

#[derive(Clone, Copy, Debug)]
pub enum Cue {
    Start = 0,
    Stop = 1,
    Discard = 2,
    Error = 3,
}

/// Plain, `Send` decoded audio — carried to the player thread which rebuilds the sink-bound
/// (`!Send`) source there.
struct CueData {
    channels: u16,
    rate: u32,
    samples: Vec<f32>,
}

pub struct Cues {
    tx: SyncSender<Cue>,
    enabled: Arc<AtomicBool>,
}

impl Cues {
    /// Decode the earcon WAVs from `resources_dir` and spawn the isolated player thread.
    /// Returns immediately — the audio device is never touched on the calling thread.
    pub fn new(resources_dir: &Path, enabled: bool) -> Self {
        let data: Vec<Option<CueData>> = CUE_FILES
            .iter()
            .map(|name| decode(&resources_dir.join(name)))
            .collect();
        let enabled = Arc::new(AtomicBool::new(enabled));
        let (tx, rx) = mpsc::sync_channel::<Cue>(8);
        {
            let enabled = enabled.clone();
            thread::spawn(move || run_player(rx, data, enabled));
        }
        Self { tx, enabled }
    }

    /// Fire-and-forget. Never blocks, never surfaces an error (drops if the channel is full
    /// or the player is gone).
    pub fn play(&self, cue: Cue) {
        if self.enabled.load(Ordering::Relaxed) {
            let _ = self.tx.try_send(cue);
        }
    }

    /// Toggle from config (audio_cues).
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }
}

fn decode(path: &Path) -> Option<CueData> {
    let mut reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .filter_map(Result::ok)
                .map(|s| s as f32 * scale)
                .collect()
        }
    };
    if samples.is_empty() {
        return None;
    }
    Some(CueData { channels: spec.channels, rate: spec.sample_rate, samples })
}

/// Player supervisor: spawns the sink-owning thread and waits for it to come up with a
/// timeout, so a genuinely wedged output device disables cues instead of hanging.
fn run_player(rx: Receiver<Cue>, data: Vec<Option<CueData>>, enabled: Arc<AtomicBool>) {
    let (ready_tx, ready_rx) = mpsc::channel::<bool>();
    thread::spawn(move || {
        // This thread owns the `!Send` sink + player + play loop for its whole life.
        let sink = match DeviceSinkBuilder::open_default_sink() {
            Ok(s) => s,
            Err(_) => {
                let _ = ready_tx.send(false);
                return;
            }
        };
        let player = Player::connect_new(sink.mixer());
        let buffers: Vec<Option<Buffered<SamplesBuffer>>> =
            data.iter().map(|d| d.as_ref().and_then(build_buffer)).collect();
        let _ = ready_tx.send(true);
        let _keep_alive = sink; // dropping the sink would tear down the stream
        for cue in rx.iter() {
            if enabled.load(Ordering::Relaxed) {
                if let Some(Some(buf)) = buffers.get(cue as usize) {
                    player.append(buf.clone());
                }
            }
        }
    });

    // "spawn a thread joined with timeout" — never wait forever on a stuck device.
    if ready_rx.recv_timeout(Duration::from_secs(3)) != Ok(true) {
        eprintln!("audio cues: output device unavailable — cues disabled");
    }
}

fn build_buffer(d: &CueData) -> Option<Buffered<SamplesBuffer>> {
    let channels = NonZero::new(d.channels)?;
    let rate = NonZero::new(d.rate)?;
    Some(SamplesBuffer::new(channels, rate, d.samples.clone()).buffered())
}
