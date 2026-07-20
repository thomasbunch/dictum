//! ASR worker: owns the warm sherpa-onnx OfflineRecognizer on its own thread.
//! The recognizer type is not Send, so it never leaves this thread — the handle
//! only ships commands over a channel and results come back as CoordMsg.

use crate::model;
use crate::types::*;
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

enum AsrCmd {
    EnsureLoaded,
    Decode { generation: u64, samples: Vec<f32> },
    Unload,
}

/// Handle held by the coordinator. Every method is fire-and-forget; replies
/// arrive as `CoordMsg::DecodeDone/DecodeFailed/ModelStatus`.
pub struct AsrEngine {
    tx: Sender<AsrCmd>,
}

impl AsrEngine {
    pub fn new(coord_tx: Sender<CoordMsg>) -> Self {
        let (tx, rx) = channel();
        thread::spawn(move || run(rx, coord_tx));
        AsrEngine { tx }
    }

    /// Warm-load the model (emits Loading{0/50/100} -> Ready, or Missing/Error).
    pub fn ensure_loaded(&self) {
        let _ = self.tx.send(AsrCmd::EnsureLoaded);
    }

    /// Queue an utterance. `generation` is echoed back so the coordinator can
    /// drop stale results after a cancel.
    pub fn decode(&self, generation: u64, samples: Vec<f32>) {
        let _ = self.tx.send(AsrCmd::Decode { generation, samples });
    }

    /// Drop the recognizer to free RAM (unload_on_idle).
    pub fn unload(&self) {
        let _ = self.tx.send(AsrCmd::Unload);
    }
}

fn run(rx: Receiver<AsrCmd>, tx: Sender<CoordMsg>) {
    let mut rec: Option<OfflineRecognizer> = None;
    // Loop ends when the handle is dropped (channel closed) -> recognizer freed.
    while let Ok(cmd) = rx.recv() {
        match cmd {
            AsrCmd::EnsureLoaded => {
                if rec.is_some() {
                    let _ = tx.send(CoordMsg::ModelStatus(ModelStatus::Ready));
                } else {
                    ensure(&mut rec, &tx);
                }
            }
            AsrCmd::Decode { generation, samples } => {
                if samples.is_empty() {
                    let _ = tx.send(CoordMsg::DecodeDone { generation, text: String::new() });
                    continue;
                }
                if !ensure(&mut rec, &tx) {
                    let _ = tx.send(CoordMsg::DecodeFailed {
                        generation,
                        error: "model not loaded".into(),
                    });
                    continue;
                }
                let text = decode(rec.as_ref().unwrap(), &samples);
                let _ = tx.send(CoordMsg::DecodeDone { generation, text });
            }
            AsrCmd::Unload => rec = None,
        }
    }
}

/// Load the recognizer if absent. Emits the coarse status flow and returns
/// whether a recognizer is available afterwards.
fn ensure(rec: &mut Option<OfflineRecognizer>, tx: &Sender<CoordMsg>) -> bool {
    if rec.is_some() {
        return true;
    }
    let status = |s| {
        let _ = tx.send(CoordMsg::ModelStatus(s));
    };
    status(ModelStatus::Loading { pct: 0 });
    let files = model::model_files();
    if !files.all_present() {
        status(ModelStatus::Missing);
        return false;
    }
    status(ModelStatus::Loading { pct: 50 });

    let mut cfg = OfflineRecognizerConfig::default();
    cfg.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(path_str(&files.encoder)),
        decoder: Some(path_str(&files.decoder)),
        joiner: Some(path_str(&files.joiner)),
    };
    cfg.model_config.tokens = Some(path_str(&files.tokens));
    cfg.model_config.provider = Some("cpu".into());
    cfg.model_config.num_threads = 4;
    cfg.model_config.debug = false;
    // Parakeet-TDT: model_type auto-detected, greedy_search is the default.

    match OfflineRecognizer::create(&cfg) {
        Some(r) => {
            *rec = Some(r);
            status(ModelStatus::Loading { pct: 100 });
            status(ModelStatus::Ready);
            true
        }
        None => {
            status(ModelStatus::Error("recognizer failed to initialize".into()));
            false
        }
    }
}

fn decode(rec: &OfflineRecognizer, samples: &[f32]) -> String {
    let stream = rec.create_stream();
    stream.accept_waveform(16_000, samples);
    rec.decode(&stream);
    match stream.get_result() {
        Some(res) => res.text.trim().to_string(),
        None => String::new(),
    }
}

// ponytail: to_string_lossy is fine for ASCII %APPDATA% paths; sherpa's C API
// takes a UTF-8 char* and non-ASCII usernames are a known upstream limitation.
fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
