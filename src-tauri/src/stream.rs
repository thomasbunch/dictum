//! Streaming preview worker: owns a warm sherpa-onnx `OnlineRecognizer` on its
//! own thread and paints live partials into the HUD during a hold. A companion
//! to asr.rs — a *different* recognizer type that runs concurrently during the
//! take and must be independently unloadable and unable to stall the offline
//! decode. Same isolation reasoning that made reformat.rs its own worker.
//!
//! HUD-only: partials never become injected text. Parakeet's offline decode
//! stays the sole source of injected/history text. Off by default; on a
//! `create()` failure or an absent model the feature silently stays off.

use crate::model;
use crate::types::*;
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

/// Minimal command set — no Begin/End/Cancel. The generation tag on each `Feed`
/// *is* the utterance boundary: the worker recreates its `OnlineStream` when the
/// generation changes, so cancel / stale / new-session all fall out of the
/// coordinator's existing `gen.wrapping_add` mechanism.
enum StreamCmd {
    EnsureLoaded,
    Feed { generation: u64, samples: Vec<f32> },
    Unload,
}

/// Handle held by the coordinator. Every method is fire-and-forget; replies
/// arrive as `CoordMsg::PartialText`/`StreamModelStatus`.
#[derive(Clone)]
pub struct StreamEngine {
    tx: Sender<StreamCmd>,
}

impl StreamEngine {
    pub fn new(coord_tx: Sender<CoordMsg>) -> Self {
        let (tx, rx) = channel();
        thread::spawn(move || run(rx, coord_tx));
        StreamEngine { tx }
    }

    /// Warm-load the preview model (emits Loading{0/50/100} -> Ready, or
    /// Missing/Error). No-op degradation: on failure the feature stays off.
    pub fn ensure_loaded(&self) {
        let _ = self.tx.send(StreamCmd::EnsureLoaded);
    }

    /// Feed one chunk of live frames tagged with the session generation.
    pub fn feed(&self, generation: u64, samples: Vec<f32>) {
        let _ = self.tx.send(StreamCmd::Feed { generation, samples });
    }

    /// Drop the recognizer to free RAM (idle-unload, same 30 s timer as ASR).
    pub fn unload(&self) {
        let _ = self.tx.send(StreamCmd::Unload);
    }
}

fn run(rx: Receiver<StreamCmd>, tx: Sender<CoordMsg>) {
    let mut rec: Option<OnlineRecognizer> = None;
    let mut stream: Option<OnlineStream> = None;
    let mut cur_gen: u64 = 0;
    let mut last_text = String::new();
    // Set once ensure() fails so Feed doesn't re-attempt the ~632MB create()
    // (and re-emit the status cascade) on every one of the ~200 frames/sec.
    // An explicit EnsureLoaded or Unload clears it (user re-fetched / retried).
    let mut load_failed = false;
    // Loop ends when the handle is dropped (channel closed) -> recognizer freed.
    while let Ok(cmd) = rx.recv() {
        match cmd {
            StreamCmd::EnsureLoaded => {
                if rec.is_some() {
                    let _ = tx.send(CoordMsg::StreamModelStatus { status: ModelStatus::Ready });
                } else {
                    // Explicit (re)load: clear a prior failure and re-attempt once.
                    load_failed = !ensure(&mut rec, &tx);
                }
            }
            StreamCmd::Feed { generation, samples } => {
                // Lazy load once; if it failed before, stay silently off — no
                // per-frame retry storm (create() reloads a ~632MB export and
                // re-emits Loading/Error each call).
                if rec.is_none() {
                    if load_failed || !ensure(&mut rec, &tx) {
                        load_failed = true;
                        continue;
                    }
                }
                let r = rec.as_ref().unwrap();
                // Fresh utterance (gen bumped by session start / cancel) -> new
                // stream, discarding any prior session's cache. The `!has_stream`
                // arm also guards the first feed of a session AND the feed that
                // follows an Unload+reload at an *unchanged* generation.
                if needs_new_stream(generation, cur_gen, stream.is_some()) {
                    cur_gen = generation;
                    stream = Some(r.create_stream());
                    last_text.clear();
                }
                let s = stream.as_ref().unwrap();
                s.accept_waveform(16_000, &samples);
                // Self-throttling drain: consume only what's ready this feed. Do
                // NOT input_finished() — that flushes the tail; Parakeet owns it.
                while r.is_ready(s) {
                    r.decode(s);
                }
                let text = r.get_result(s).map(|res| res.text.trim().to_string()).unwrap_or_default();
                if should_emit(&text, &last_text) {
                    last_text = text.clone();
                    let _ = tx.send(CoordMsg::PartialText { generation, text });
                }
            }
            StreamCmd::Unload => {
                rec = None;
                stream = None;
                last_text.clear();
                load_failed = false; // model may be re-fetched before next use
                let _ = tx.send(CoordMsg::StreamModelStatus { status: ModelStatus::Unloaded });
            }
        }
    }
}

/// Emit a partial only when it's non-empty and changed since the last one — the
/// decoder re-reports the same hypothesis every feed while silence holds.
fn should_emit(text: &str, last: &str) -> bool {
    !text.is_empty() && text != last
}

/// Whether the Feed handler must (re)create the `OnlineStream`. A generation
/// change means a new utterance. `!has_stream` is the load-bearing second arm:
/// after an `Unload` (which drops the stream) the very next Feed can carry the
/// *same* generation as the pre-unload session, and it must still start a fresh
/// stream rather than dereference the dropped one — i.e. the Feed-after-Unload
/// path relies on this, not on the generation differing.
fn needs_new_stream(generation: u64, cur_gen: u64, has_stream: bool) -> bool {
    generation != cur_gen || !has_stream
}

/// Load the recognizer if absent. Emits the coarse status flow and returns
/// whether a recognizer is available afterwards. Mirrors asr.rs::ensure.
fn ensure(rec: &mut Option<OnlineRecognizer>, tx: &Sender<CoordMsg>) -> bool {
    if rec.is_some() {
        return true;
    }
    let status = |s| {
        let _ = tx.send(CoordMsg::StreamModelStatus { status: s });
    };
    status(ModelStatus::Loading { pct: 0 });
    let files = model::model_files(model::stream_spec());
    if !files.all_present() {
        status(ModelStatus::Missing);
        return false;
    }
    status(ModelStatus::Loading { pct: 50 });
    match OnlineRecognizer::create(&build_config(&files)) {
        Some(r) => {
            *rec = Some(r);
            status(ModelStatus::Loading { pct: 100 });
            status(ModelStatus::Ready);
            true
        }
        None => {
            // Files present but the recognizer wouldn't init (bad export / wrong
            // feature_dim / static-lib mismatch) — silent-off: surface only on the
            // SETUP card, never on the HUD, never block dictation.
            eprintln!("stream: preview recognizer failed to initialize");
            status(ModelStatus::Error("THE PREVIEW MODEL WOULD NOT LOAD".into()));
            false
        }
    }
}

/// The exact `OnlineRecognizerConfig` the preview uses. Factored out so the
/// ignored live test builds the recognizer identically to production.
/// `feat_config` default is 16 kHz / feature_dim 80 (NeMo FastConformer default);
/// `model_type` left None to auto-detect from encoder metadata (like asr.rs).
fn build_config(files: &model::ModelFiles) -> OnlineRecognizerConfig {
    let mut cfg = OnlineRecognizerConfig::default();
    cfg.model_config.transducer = OnlineTransducerModelConfig {
        encoder: Some(path_str(&files.encoder)),
        decoder: Some(path_str(&files.decoder)),
        joiner: Some(path_str(&files.joiner)),
    };
    cfg.model_config.tokens = Some(path_str(&files.tokens));
    cfg.model_config.provider = Some("cpu".into());
    cfg.model_config.num_threads = 2; // capped below Parakeet's 4 so it can't starve the offline decode
    cfg.decoding_method = Some("greedy_search".into());
    cfg.enable_endpoint = false; // never let sherpa reset mid-hold; Parakeet finalizes
    cfg
}

// ponytail: to_string_lossy is fine for ASCII %APPDATA% paths; sherpa's C API
// takes a UTF-8 char* and non-ASCII usernames are a known upstream limitation.
fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_only_nonempty_changes() {
        assert!(should_emit("hello", ""));
        assert!(should_emit("hello world", "hello"));
        assert!(!should_emit("hello", "hello")); // unchanged (silence re-reports)
        assert!(!should_emit("", "")); // empty
        assert!(!should_emit("", "hello")); // never emit an empty regression
    }

    /// The load_failed guard: when ensure() can't produce a recognizer, a burst
    /// of Feeds must attempt the load exactly ONCE (one status cascade), not
    /// once per frame. Uses the absent-model path (all_present()==false ->
    /// ensure emits Loading{0}+Missing and returns false) so it needs no model;
    /// skips if the model is actually installed (that path builds a recognizer).
    #[test]
    fn feed_attempts_load_only_once() {
        if model::model_files(model::stream_spec()).all_present() {
            return; // model present -> ensure() succeeds, different branch
        }
        let (coord_tx, coord_rx) = channel();
        let (cmd_tx, cmd_rx) = channel();
        let h = thread::spawn(move || run(cmd_rx, coord_tx));
        for _ in 0..5 {
            cmd_tx.send(StreamCmd::Feed { generation: 1, samples: vec![0.0; 80] }).unwrap();
        }
        drop(cmd_tx); // close channel -> run() returns
        h.join().unwrap();
        let statuses = coord_rx
            .try_iter()
            .filter(|m| matches!(m, CoordMsg::StreamModelStatus { .. }))
            .count();
        // One ensure() cascade only: Loading{0} then Missing. Without the guard
        // this would be 5 * 2 = 10.
        assert_eq!(statuses, 2, "expected a single load attempt, got {statuses} statuses");
    }

    /// Locks the utterance-boundary + Feed-after-Unload invariants without a
    /// model. The `!has_stream` arm is what makes an Unload+reload at the SAME
    /// generation start a fresh stream (else the same-gen Feed would keep the
    /// dropped stream's identity and skip recreation). If a future edit reverts
    /// to a bare `generation != cur_gen`, the third assert fails here.
    #[test]
    fn new_stream_decision() {
        // Live stream, gen unchanged -> keep decoding into it.
        assert!(!needs_new_stream(5, 5, true));
        // Gen bumped (new session / cancel) -> fresh stream.
        assert!(needs_new_stream(6, 5, true));
        // First feed of a session (no stream yet) -> fresh stream.
        assert!(needs_new_stream(5, 5, false));
        // Feed-after-Unload at the SAME gen (Unload dropped the stream) -> MUST
        // recreate; the guarantee that Feed never touches a dropped stream.
        assert!(needs_new_stream(9, 9, false));
        // Wraparound edge: MAX -> 0 are distinct gens -> fresh stream (the only
        // unhandled case is a full 2^64-session collision, unreachable).
        assert!(needs_new_stream(0, u64::MAX, true));
    }

    /// Unload before any load must be a safe no-op that still reports Unloaded —
    /// and must NOT touch the model (this runs in the default suite whether or
    /// not the SKU is installed). Guards the "Unload with no load" lifecycle edge
    /// and proves the worker never blocks on the ~632MB create() for an Unload.
    #[test]
    fn unload_without_load_emits_unloaded() {
        let (coord_tx, coord_rx) = channel();
        let (cmd_tx, cmd_rx) = channel();
        let h = thread::spawn(move || run(cmd_rx, coord_tx));
        cmd_tx.send(StreamCmd::Unload).unwrap();
        cmd_tx.send(StreamCmd::Unload).unwrap(); // idempotent: no load, no panic
        drop(cmd_tx);
        h.join().unwrap();
        let msgs: Vec<_> = coord_rx.try_iter().collect();
        assert!(
            msgs.iter().all(|m| matches!(
                m,
                CoordMsg::StreamModelStatus { status: ModelStatus::Unloaded }
            )),
            "Unload-with-no-load emitted something other than Unloaded: {msgs:?}"
        );
        assert_eq!(msgs.len(), 2, "each Unload should emit exactly one Unloaded");
    }

    /// LIVE feasibility confirmation — the design's single residual risk: whether
    /// this exact 2026-04-25 int8 export runs on the CPU provider in the pinned
    /// v1.13.4 static libs. Mirrors lib.rs e2e::model_download_and_decode; needs
    /// the model installed at %APPDATA%\Dictum\models. Run:
    ///   cargo test --release stream::tests::live_streams_test_wav -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_streams_test_wav() {
        let spec = model::stream_spec();
        let files = model::model_files(spec);
        if !files.all_present() {
            eprintln!("stream model not installed — skipping (fetch it via SETUP or sideload)");
            return;
        }

        // (a) create() must return Some — proves the static libs run this arch on CPU.
        let rec = OnlineRecognizer::create(&build_config(&files))
            .expect("stream recognizer create — v1.13.4 static libs must run this arch on CPU");

        let wav = model::model_dir(spec).join("test_wavs").join("0.wav");
        assert!(wav.exists(), "bundled test wav missing: {}", wav.display());
        let wave = sherpa_onnx::Wave::read(wav.to_string_lossy().as_ref()).expect("read test wav");
        let rate = wave.sample_rate();
        let samples = wave.samples();

        let stream = rec.create_stream();
        let chunk = (rate as usize) / 10; // ~100 ms chunks, like the live capture tee
        let mut partials: Vec<String> = Vec::new();
        for c in samples.chunks(chunk) {
            stream.accept_waveform(rate, c);
            while rec.is_ready(&stream) {
                rec.decode(&stream);
            }
            let text = rec.get_result(&stream).map(|r| r.text.trim().to_string()).unwrap_or_default();
            if partials.last().map(|p| p != &text).unwrap_or(!text.is_empty()) && !text.is_empty() {
                partials.push(text);
            }
        }
        // Flush the tail for a final hypothesis (the live worker never does this —
        // Parakeet owns the tail — but the test wants the complete streamed text).
        stream.input_finished();
        while rec.is_ready(&stream) {
            rec.decode(&stream);
        }
        let final_text = rec.get_result(&stream).map(|r| r.text.trim().to_string()).unwrap_or_default();

        eprintln!("partials ({}): {:?}", partials.len(), partials);
        eprintln!("final: {final_text}");
        // (b) partials are non-empty and grow.
        assert!(!partials.is_empty(), "no partials produced — feature_dim/model_type likely wrong");
        assert!(partials.last().unwrap().len() >= partials.first().unwrap().len(), "partials did not grow");
        // (c) final streamed text roughly matches the reference (loose token overlap
        // — streaming int8 need not match the offline decode exactly).
        assert!(!final_text.is_empty(), "empty final transcript");
        let got = final_text.to_lowercase();
        let hits = ["nightfall", "yellow", "lamps", "quarter"].iter().filter(|w| got.contains(*w)).count();
        assert!(hits >= 2, "streamed text does not resemble the reference: {final_text}");
    }

    // ---- Model-gated lifecycle drives (run the REAL `run()` worker) ---------
    // These load the ~632MB SKU, so they're #[ignore] like the live test. Run:
    //   cargo test --release stream::tests::lifecycle_edges_on_real_recognizer -- --ignored --nocapture
    //   cargo test --release stream::tests::soak_utterances_one_recognizer      -- --ignored --nocapture

    fn is_ready(m: &CoordMsg) -> bool {
        matches!(m, CoordMsg::StreamModelStatus { status: ModelStatus::Ready })
    }
    fn is_unloaded(m: &CoordMsg) -> bool {
        matches!(m, CoordMsg::StreamModelStatus { status: ModelStatus::Unloaded })
    }
    fn is_load_start(m: &CoordMsg) -> bool {
        matches!(m, CoordMsg::StreamModelStatus { status: ModelStatus::Loading { pct: 0 } })
    }
    /// Recv until `stop` matches (inclusive). Generous per-message timeout so the
    /// multi-second `create()` between Loading{50} and Loading{100} never trips it.
    fn recv_until(rx: &Receiver<CoordMsg>, stop: fn(&CoordMsg) -> bool) -> Vec<CoordMsg> {
        let mut out = Vec::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(120)) {
                Ok(m) => {
                    let done = stop(&m);
                    out.push(m);
                    if done {
                        return out;
                    }
                }
                Err(_) => panic!("recv_until timed out after {} msgs: {out:?}", out.len()),
            }
        }
    }
    /// Collect messages until the worker goes quiet for `idle` (no reload gap
    /// expected in this window — partials stream continuously once decoding).
    fn drain_quiet(rx: &Receiver<CoordMsg>, idle: std::time::Duration) -> Vec<CoordMsg> {
        let mut out = Vec::new();
        while let Ok(m) = rx.recv_timeout(idle) {
            out.push(m);
        }
        out
    }
    fn feed_wav(cmd: &Sender<StreamCmd>, generation: u64, samples: &[f32], chunk: usize) {
        for c in samples.chunks(chunk) {
            cmd.send(StreamCmd::Feed { generation, samples: c.to_vec() }).unwrap();
        }
    }
    fn n_partials(msgs: &[CoordMsg]) -> usize {
        msgs.iter().filter(|m| matches!(m, CoordMsg::PartialText { .. })).count()
    }
    fn load_wav() -> Vec<f32> {
        let wav = model::model_dir(model::stream_spec()).join("test_wavs").join("0.wav");
        let wave = sherpa_onnx::Wave::read(wav.to_string_lossy().as_ref()).expect("read wav");
        assert_eq!(wave.sample_rate(), 16_000, "worker feeds accept_waveform(16_000); 0.wav must be 16k");
        wave.samples().to_vec()
    }

    /// Exercises every lifecycle edge on ONE real worker: EnsureLoaded-twice
    /// single-load, a real utterance's partials, Unload, Feed-after-Unload at the
    /// SAME generation (must reload + start a fresh stream), 100 rapid gen churns,
    /// and the u64::MAX->0 wraparound — all without a panic or a hang.
    #[test]
    #[ignore]
    fn lifecycle_edges_on_real_recognizer() {
        if !model::model_files(model::stream_spec()).all_present() {
            eprintln!("stream model not installed — skipping");
            return;
        }
        let (coord_tx, rx) = channel();
        let (cmd, cmd_rx) = channel();
        let h = thread::spawn(move || run(cmd_rx, coord_tx));
        let chunk = 1600; // ~100 ms at 16 kHz, like the capture tee

        // (1) EnsureLoaded twice -> exactly one load cascade; the second is a bare Ready.
        cmd.send(StreamCmd::EnsureLoaded).unwrap();
        let first = recv_until(&rx, is_ready);
        cmd.send(StreamCmd::EnsureLoaded).unwrap();
        let second = recv_until(&rx, is_ready);
        let loads = first.iter().chain(&second).filter(|m| is_load_start(m)).count();
        assert_eq!(loads, 1, "second EnsureLoaded reloaded (Loading{{0}} count = {loads})");
        assert_eq!(second.len(), 1, "already-loaded EnsureLoaded must emit a lone Ready, got {second:?}");

        let samples = load_wav();

        // (2) A real utterance at gen=1 -> partials arrive.
        feed_wav(&cmd, 1, &samples, chunk);
        let live = drain_quiet(&rx, std::time::Duration::from_secs(3));
        assert!(n_partials(&live) > 0, "no partials from a real utterance");

        // (3) Unload, then Feed at the SAME gen=1 -> must reload and start fresh.
        cmd.send(StreamCmd::Unload).unwrap();
        let unl = recv_until(&rx, is_unloaded);
        assert!(unl.iter().any(is_unloaded), "Unload did not report Unloaded");
        feed_wav(&cmd, 1, &samples, chunk); // same gen the pre-unload session used
        let reload = recv_until(&rx, is_ready); // reload cascade must reappear
        assert!(reload.iter().any(is_load_start), "Feed-after-Unload did not reload the recognizer");
        let after = drain_quiet(&rx, std::time::Duration::from_secs(3));
        assert!(n_partials(&after) > 0, "no partials after the post-Unload reload (stale/dropped stream?)");

        // (4) Rapid gen churn: 100 start/cancel cycles, one short chunk each.
        let short = samples[..chunk.min(samples.len())].to_vec();
        for g in 2..=101u64 {
            cmd.send(StreamCmd::Feed { generation: g, samples: short.clone() }).unwrap();
        }
        // (5) Generation wraparound edge: u64::MAX then 0 (distinct gens).
        cmd.send(StreamCmd::Feed { generation: u64::MAX, samples: short.clone() }).unwrap();
        cmd.send(StreamCmd::Feed { generation: 0, samples: short.clone() }).unwrap();
        let _ = drain_quiet(&rx, std::time::Duration::from_secs(2)); // flush churn output

        // Responsiveness probe: the worker survived and still answers.
        cmd.send(StreamCmd::EnsureLoaded).unwrap();
        let probe = recv_until(&rx, is_ready);
        assert!(probe.iter().any(is_ready), "worker unresponsive after churn/wraparound");
        assert!(!probe.iter().any(is_load_start), "worker lost its recognizer during churn");

        drop(cmd);
        h.join().unwrap();
    }

    /// Soak: N sequential utterances (distinct generations -> a fresh stream each)
    /// on ONE loaded recognizer. Asserts per-utterance stream isolation — every
    /// utterance must decode to the SAME final hypothesis (no cross-utterance
    /// cache carryover / drift). Also the memory-growth harness: set
    /// `SOAK_UTTERANCES` (default 20) and compare the process PeakWorkingSet64
    /// across N=1 and N=20 runs — create/drop-per-gen keeps steady state at one
    /// recognizer + one stream, so peak must not scale with N.
    #[test]
    #[ignore]
    fn soak_utterances_one_recognizer() {
        if !model::model_files(model::stream_spec()).all_present() {
            eprintln!("stream model not installed — skipping");
            return;
        }
        let n: u64 = std::env::var("SOAK_UTTERANCES").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
        let (coord_tx, rx) = channel();
        let (cmd, cmd_rx) = channel();
        let h = thread::spawn(move || run(cmd_rx, coord_tx));
        cmd.send(StreamCmd::EnsureLoaded).unwrap();
        let _ = recv_until(&rx, is_ready);

        let samples = load_wav();
        let chunk = 1600;
        let mut finals: Vec<String> = Vec::new();
        for g in 1..=n {
            feed_wav(&cmd, g, &samples, chunk);
            // A loaded EnsureLoaded emits a lone Ready AFTER all this gen's partials
            // (FIFO) — a deterministic utterance-end sentinel with no reload gap.
            cmd.send(StreamCmd::EnsureLoaded).unwrap();
            let msgs = recv_until(&rx, is_ready);
            let last = msgs.iter().rev().find_map(|m| match m {
                CoordMsg::PartialText { text, .. } => Some(text.clone()),
                _ => None,
            });
            finals.push(last.unwrap_or_default());
        }
        drop(cmd);
        h.join().unwrap();

        eprintln!("soak: {n} utterances, final hypothesis each:");
        for (i, f) in finals.iter().enumerate() {
            eprintln!("  [{i}] {f}");
        }
        assert!(!finals[0].is_empty(), "utterance 0 produced no partial");
        assert!(
            finals.iter().all(|f| f == &finals[0]),
            "streams not isolated across generations — hypotheses diverged: {finals:?}"
        );
    }
}
