//! ASR worker: owns the warm sherpa-onnx OfflineRecognizer on its own thread.
//! The recognizer type is not Send, so it never leaves this thread — the handle
//! only ships commands over a channel and results come back as CoordMsg.

use crate::model;
use crate::types::*;
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

enum AsrCmd {
    EnsureLoaded,
    Decode { generation: u64, samples: Vec<f32> },
    Unload,
    /// Switch the active model (config model_id). Drops the loaded recognizer
    /// if the id differs; the next EnsureLoaded/Decode loads the new files.
    SetModel(String),
    /// Replace the contextual-biasing word list (sherpa wire format). Cheap —
    /// the list rides on each stream, so no reload.
    SetHotwords(String),
    /// Turn contextual biasing on/off. Expensive — `decoding_method` is fixed
    /// when the recognizer is constructed, so this drops and reloads it.
    SetBiasing(bool),
}

/// Handle held by the coordinator. Every method is fire-and-forget; replies
/// arrive as `CoordMsg::DecodeDone/DecodeFailed/ModelStatus`.
#[derive(Clone)]
pub struct AsrEngine {
    tx: Sender<AsrCmd>,
}

impl AsrEngine {
    pub fn new(coord_tx: Sender<CoordMsg>, model_id: String, biasing: bool) -> Self {
        let (tx, rx) = channel();
        thread::spawn(move || run(rx, coord_tx, model_id, biasing));
        AsrEngine { tx }
    }

    /// Set the biasing word list (already joined by `terms::join_hotwords`).
    pub fn set_hotwords(&self, hotwords: String) {
        let _ = self.tx.send(AsrCmd::SetHotwords(hotwords));
    }

    /// Enable/disable contextual biasing (config asr_biasing). Reloads the model.
    pub fn set_biasing(&self, on: bool) {
        let _ = self.tx.send(AsrCmd::SetBiasing(on));
    }

    /// Switch the active model SKU (SETUP model picker).
    pub fn set_model(&self, id: String) {
        let _ = self.tx.send(AsrCmd::SetModel(id));
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

/// A loaded recognizer plus whether it was actually built for biasing.
///
/// These two travel together on purpose. `create_stream_with_hotwords()` on a
/// recognizer that was NOT configured with a BPE modeling unit dereferences a
/// null `bpe_encoder` inside sherpa — a hard crash, not an error return. Making
/// `biased` a property of the loaded recognizer rather than of the config means
/// the check can never drift from what was actually constructed.
struct Loaded {
    rec: OfflineRecognizer,
    biased: bool,
}

fn run(rx: Receiver<AsrCmd>, tx: Sender<CoordMsg>, mut model_id: String, mut biasing: bool) {
    let mut rec: Option<Loaded> = None;
    let mut hotwords = String::new();
    // Loop ends when the handle is dropped (channel closed) -> recognizer freed.
    while let Ok(cmd) = rx.recv() {
        match cmd {
            AsrCmd::EnsureLoaded => {
                if rec.is_some() {
                    let _ = tx.send(CoordMsg::ModelStatus(ModelStatus::Ready));
                } else {
                    ensure(&mut rec, &tx, &model_id, biasing);
                }
            }
            AsrCmd::Decode { generation, samples } => {
                if samples.is_empty() {
                    let _ = tx.send(CoordMsg::DecodeDone { generation, text: String::new() });
                    continue;
                }
                if !ensure(&mut rec, &tx, &model_id, biasing) {
                    let _ = tx.send(CoordMsg::DecodeFailed {
                        generation,
                        error: "model not loaded".into(),
                    });
                    continue;
                }
                let text = decode(rec.as_ref().unwrap(), &samples, &hotwords);
                let _ = tx.send(CoordMsg::DecodeDone { generation, text });
            }
            AsrCmd::Unload => {
                rec = None;
                // Distinct from Ready so the next take warms the model up front
                // and SETUP can print "○ IDLE — UNLOADED".
                let _ = tx.send(CoordMsg::ModelStatus(ModelStatus::Unloaded));
            }
            AsrCmd::SetHotwords(h) => {
                // Per-stream, so no reload — the next take picks it up.
                hotwords = h;
            }
            AsrCmd::SetBiasing(on) => {
                if on != biasing {
                    biasing = on;
                    // decoding_method is baked in at construction; nothing short
                    // of a reload changes it. Same contract as SetModel: the
                    // caller decides whether to warm it back up.
                    rec = None;
                }
            }
            AsrCmd::SetModel(id) => {
                if id != model_id {
                    model_id = id;
                    // Drop the old recognizer (frees ~600 MB); the caller decides
                    // whether to warm the new one (ensure_model follows unless
                    // unload_on_idle). No status here — ensure() reports.
                    rec = None;
                }
            }
        }
    }
}

/// Load the recognizer if absent. Emits the coarse status flow and returns
/// whether a recognizer is available afterwards.
fn ensure(
    rec: &mut Option<Loaded>,
    tx: &Sender<CoordMsg>,
    model_id: &str,
    biasing: bool,
) -> bool {
    if rec.is_some() {
        return true;
    }
    let status = |s| {
        let _ = tx.send(CoordMsg::ModelStatus(s));
    };
    status(ModelStatus::Loading { pct: 0 });
    let files = model::model_files(model::spec(model_id));
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

    // Contextual biasing. Every line below is load-bearing — miss one and this
    // either silently does nothing or takes the process down with it:
    //   - decoding_method: the context graph is only consulted by beam search.
    //     Greedy ignores hotwords entirely (no error, no warning).
    //   - modeling_unit: defaults to "cjkchar", which encodes English hotwords
    //     one Chinese-character-class unit at a time and never matches. This is
    //     the single most common way biasing appears to "not work".
    //   - bpe_vocab: what the encoder needs to turn a hotword into model pieces.
    //   - hotwords_score: the crate defaults it to 0.0 — a boost of nothing.
    // Anything sherpa rejects here exits via SHERPA_ONNX_EXIT -> _Exit(), which
    // kills the Tauri process with no panic and no unwind, so the fallible part
    // (deriving the vocab) is done up front and failure downgrades to greedy.
    let biased = biasing
        && match bpe_vocab(&files.tokens) {
            Some(v) => {
                cfg.decoding_method = Some("modified_beam_search".into());
                cfg.model_config.modeling_unit = Some("bpe".into());
                cfg.model_config.bpe_vocab = Some(path_str(&v));
                cfg.hotwords_score = HOTWORDS_SCORE;
                true
            }
            None => {
                eprintln!("asr: no bpe vocab — contextual biasing off, decoding greedily");
                false
            }
        };

    match OfflineRecognizer::create(&cfg) {
        Some(r) => {
            *rec = Some(Loaded { rec: r, biased });
            status(ModelStatus::Loading { pct: 100 });
            status(ModelStatus::Ready);
            true
        }
        None => {
            // Files present but the recognizer wouldn't init — surface the wire
            // voice (DESIGN §6). Keep the detail in the log.
            eprintln!("asr: recognizer failed to initialize");
            status(ModelStatus::Error("THE MODEL WOULD NOT LOAD".into()));
            false
        }
    }
}

fn decode(loaded: &Loaded, samples: &[f32], hotwords: &str) -> String {
    // ponytail: beam search degrades unpredictably on long audio — reported cases
    // lose capitalisation and punctuation, and one dropped a whole leading
    // sentence. The VAD force-splits at max_speech_duration = 30 s, so in healthy
    // operation nothing here is close to the limit; what this actually guards is
    // the no-VAD fallback in Segmenter::finish, which hands over a whole untrimmed
    // recording when Silero fails to load. Ceiling: this drops the biasing, it
    // does NOT drop to greedy — decoding_method is fixed on the recognizer, so
    // greedy would mean a second resident recognizer (~600 MB). Pay that only if
    // long-take quality is measured bad.
    let too_long = samples.len() > LONG_TAKE_SAMPLES;
    let stream = if loaded.biased && !hotwords.is_empty() && !too_long {
        loaded.rec.create_stream_with_hotwords(hotwords)
    } else {
        loaded.rec.create_stream()
    };
    stream.accept_waveform(16_000, samples);
    loaded.rec.decode(&stream);
    match stream.get_result() {
        Some(res) => res.text.trim().to_string(),
        None => String::new(),
    }
}

/// Test-only one-shot recognizer for the eval harness (`eval.rs`).
///
/// Deliberately built from the same `ensure()` and `decode()` the worker thread
/// uses, so the harness measures the shipped decode path — including the biasing
/// config, the vocab derivation and the long-take guard — rather than a
/// re-implementation that can drift from it.
#[cfg(test)]
pub(crate) struct EvalRecognizer {
    loaded: Loaded,
    /// Keeps the status channel alive; `ensure` sends progress into it.
    _rx: Receiver<CoordMsg>,
}

#[cfg(test)]
impl EvalRecognizer {
    pub(crate) fn load(model_id: &str, biasing: bool) -> Option<Self> {
        let (tx, rx) = channel();
        let mut slot: Option<Loaded> = None;
        ensure(&mut slot, &tx, model_id, biasing).then(|| EvalRecognizer {
            loaded: slot.expect("ensure returned true"),
            _rx: rx,
        })
    }

    /// True when biasing was actually configured — false means the vocab
    /// derivation failed and this silently fell back to greedy.
    pub(crate) fn biased(&self) -> bool {
        self.loaded.biased
    }

    pub(crate) fn decode(&self, samples: &[f32], hotwords: &str) -> String {
        decode(&self.loaded, samples, hotwords)
    }
}

/// Boost applied to hotword paths during beam search.
///
/// The crate default is 0.0 (no boost at all). sherpa's own CLI uses 1.5, which
/// is the top of the safe window — reported side effects climb from spurious
/// apostrophes at 2.0 to outright word substitution at 4.0. 1.0 buys the
/// correction with margin to spare; raise it only against the eval fixtures.
const HOTWORDS_SCORE: f32 = 1.0;

/// 60 s at 16 kHz.
const LONG_TAKE_SAMPLES: usize = 60 * 16_000;

/// Path to a SentencePiece-style vocab for the active model, deriving it from
/// `tokens.txt` the first time.
///
/// sherpa encodes each hotword into model pieces before building the context
/// graph, and needs a vocab file to do it — which the Parakeet export does not
/// ship. It does ship `tokens.txt`, one `piece index` per line, and that is
/// enough: SentencePiece's own trainer writes each piece's score as the negative
/// of its index (`bpe_model_trainer.cc`), so the ordering already in `tokens.txt`
/// reconstructs the scores exactly. Only the relative order matters when the
/// encoder compares merges, so do not "correct" the scale later.
///
/// Returns None on anything unexpected; the caller then decodes greedily rather
/// than handing sherpa a malformed file and losing the process to `_Exit()`.
fn bpe_vocab(tokens: &Path) -> Option<PathBuf> {
    let vocab = tokens.with_file_name("bpe.vocab");
    if vocab.exists() {
        return Some(vocab);
    }
    let src = std::fs::read_to_string(tokens).ok()?;
    let mut out = String::with_capacity(src.len() * 2);
    for line in src.lines() {
        // Split on the LAST space: a piece can itself be (or contain) a space.
        let (piece, idx) = line.rsplit_once(' ')?;
        let idx: i64 = idx.trim().parse().ok()?;
        out.push_str(piece);
        out.push('\t');
        out.push_str(&(-idx).to_string());
        out.push('\n');
    }
    std::fs::write(&vocab, out).ok()?;
    Some(vocab)
}

// ponytail: to_string_lossy is fine for ASCII %APPDATA% paths; sherpa's C API
// takes a UTF-8 char* and non-ASCII usernames are a known upstream limitation.
fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn bpe_vocab_derives_scores_from_token_order() {
        let dir = scratch("dictum-bpe-vocab-ok");
        let tokens = dir.join("tokens.txt");
        // Real shape: SentencePiece pieces keep their \u{2581} prefix, index ascending.
        std::fs::write(&tokens, "<unk> 0\n\u{2581}t 1\n\u{2581}the 2\n<blk> 3\n").unwrap();

        let v = bpe_vocab(&tokens).expect("derivation should succeed");
        assert_eq!(v.file_name().unwrap(), "bpe.vocab");
        assert_eq!(
            std::fs::read_to_string(&v).unwrap(),
            "<unk>\t0\n\u{2581}t\t-1\n\u{2581}the\t-2\n<blk>\t-3\n"
        );

        // Derived once: a second call reuses the file instead of rewriting it.
        std::fs::write(&v, "sentinel").unwrap();
        assert_eq!(bpe_vocab(&tokens).unwrap(), v);
        assert_eq!(std::fs::read_to_string(&v).unwrap(), "sentinel");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_piece_containing_a_space_splits_on_the_last_one() {
        let dir = scratch("dictum-bpe-vocab-space");
        let tokens = dir.join("tokens.txt");
        std::fs::write(&tokens, "<unk> 0\n\u{2581} 1\n").unwrap();
        let v = bpe_vocab(&tokens).unwrap();
        assert_eq!(std::fs::read_to_string(&v).unwrap(), "<unk>\t0\n\u{2581}\t-1\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_tokens_file_fails_closed() {
        // Handing sherpa a malformed vocab is a process kill (SHERPA_ONNX_EXIT ->
        // _Exit(), no panic, no unwind), so this must return None and let the
        // caller fall back to greedy decoding instead.
        let dir = scratch("dictum-bpe-vocab-bad");
        let tokens = dir.join("tokens.txt");
        std::fs::write(&tokens, "<unk> 0\nbroken-line-with-no-index\n").unwrap();
        assert!(bpe_vocab(&tokens).is_none());
        assert!(!dir.join("bpe.vocab").exists(), "a partial vocab must never be left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end against the installed model. The unit tests above prove the
    /// derived vocab's SHAPE; only this proves sherpa accepts it — and a rejection
    /// is not an error return, it is `_Exit()` taking the process with it.
    ///
    /// Also the only check that beam search decodes at all on Parakeet-TDT
    /// (k2-fsa/sherpa-onnx#3267 claims it drops roughly 1 take in 5).
    ///
    ///   cargo test -- --ignored --nocapture biasing_decodes_the_reference_wav
    #[test]
    #[ignore]
    fn biasing_decodes_the_reference_wav() {
        let files = model::model_files(model::spec(model::DEFAULT_MODEL_ID));
        assert!(files.all_present(), "install the default ASR SKU first");
        let wav = files.tokens.with_file_name("test_wavs").join("0.wav");
        let mut r = hound::WavReader::open(&wav).expect("upstream ships test_wavs/0.wav");
        assert_eq!(r.spec().sample_rate, 16_000);
        let samples: Vec<f32> =
            r.samples::<i16>().filter_map(Result::ok).map(|s| s as f32 / 32768.0).collect();

        let (tx, rx) = channel();

        // Greedy baseline — what ships today.
        let mut plain: Option<Loaded> = None;
        assert!(ensure(&mut plain, &tx, model::DEFAULT_MODEL_ID, false));
        assert!(!plain.as_ref().unwrap().biased);
        let greedy = decode(plain.as_ref().unwrap(), &samples, "");
        drop(plain); // ~600 MB; don't hold two recognizers at once

        // Beam search + derived bpe.vocab + the real built-in hotword list.
        let mut biased: Option<Loaded> = None;
        assert!(ensure(&mut biased, &tx, model::DEFAULT_MODEL_ID, true));
        assert!(
            biased.as_ref().unwrap().biased,
            "bpe.vocab derivation failed — biasing silently downgraded to greedy"
        );
        let hw = crate::terms::join_hotwords(crate::terms::hotwords());
        let boosted = decode(biased.as_ref().unwrap(), &samples, &hw);

        eprintln!("=== BIASING SMOKE ===");
        eprintln!("hotwords: {} entries", hw.split('/').count());
        eprintln!("greedy  : {greedy:?}");
        eprintln!("biased  : {boosted:?}");
        assert!(!greedy.trim().is_empty(), "greedy decode produced nothing");
        assert!(
            !boosted.trim().is_empty(),
            "beam search produced nothing — this is the #3267 symptom"
        );
        // Negative control: 130 coding hotwords must not rewrite ordinary speech.
        // This clip is Victorian prose with no technical vocabulary in it.
        assert_eq!(boosted, greedy, "coding hotwords corrupted a non-technical take");

        // Positive control, and the only proof biasing has teeth on THIS model:
        // the reference transcript spells the name "Phebe". Bias toward the other
        // spelling and the decoder should follow.
        let targeted = decode(biased.as_ref().unwrap(), &samples, "Phoebe");
        eprintln!("targeted: {targeted:?}");
        assert!(
            greedy.contains("Phebe") && !greedy.contains("Phoebe"),
            "the reference transcript changed; re-derive this test's expectation"
        );
        assert!(
            targeted.contains("Phoebe"),
            "hotword had no effect — biasing is wired but inert:
  {targeted}"
        );
        drop(rx);
    }
}
