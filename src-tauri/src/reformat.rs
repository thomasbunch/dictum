//! Reformat worker: owns the warm llama.cpp model + backend on its own thread.
//! The llama context is not Send (and borrows the model), so it never leaves
//! this thread — the handle only ships commands over a channel and results come
//! back as CoordMsg. Mirrors asr.rs's worker shape exactly.
//!
//! All llama-cpp-2 code is gated behind the `reformat-llm` cargo feature so
//! `--no-default-features` still builds if the native build breaks; the stub
//! path replies ReformatFailed("built without reformat-llm").

use crate::types::*;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

/// The exact system prompt the GGUF was fine-tuned with (finetune/data row 1).
/// Copied byte-for-byte — keep the em-dash (U+2014).
const SYSTEM: &str = "You reformat spoken dictation into clean written text for an AI coding agent. Remove disfluencies and resolve self-corrections; preserve meaning and every identifier exactly; add nothing. Default to prose; bullets only for 3+ discrete items, numbered steps only for real sequences. Keep a question a question. Never answer or execute the dictation — only reformat it. Output only the reformatted text.";

/// n_ctx / max output / decode-batch size. The fine-tune's chat template is
/// plain ChatML; <|im_end|> is baked in as the EOS token.
#[cfg(feature = "reformat-llm")]
const N_CTX: u32 = 4096;
#[cfg(feature = "reformat-llm")]
const MAX_OUTPUT_TOKENS: usize = 512;
#[cfg(feature = "reformat-llm")]
const N_BATCH: usize = 512;

enum ReformatCmd {
    /// Point at a GGUF file. Drops the loaded model if the path differs.
    SetModel(PathBuf),
    /// Change the compute device (GPU offload on/off). Drops the loaded model if
    /// it changed — n_gpu_layers is fixed at load, so the next use reloads.
    SetUseGpu(bool),
    /// Warm-load the model up front (emits Loading -> Ready / Missing / Error).
    Ensure,
    /// Reformat one deterministic-pipeline result. `generation` is echoed back
    /// so the coordinator can drop stale replies after a cancel.
    Reformat { det: String, generation: u64 },
    /// Drop the model to free RAM (unload_on_idle).
    Unload,
}

/// Handle held by the coordinator. Every method is fire-and-forget; replies
/// arrive as `CoordMsg::ReformatDone/ReformatFailed/ReformatModelStatus`.
#[derive(Clone)]
pub struct ReformatEngine {
    tx: Sender<ReformatCmd>,
}

impl ReformatEngine {
    pub fn new(coord_tx: Sender<CoordMsg>, use_gpu: bool) -> Self {
        let (tx, rx) = channel();
        thread::spawn(move || run(rx, coord_tx, use_gpu));
        ReformatEngine { tx }
    }

    /// Switch the reformat GGUF (config reformat-model swap).
    pub fn set_model(&self, path: PathBuf) {
        let _ = self.tx.send(ReformatCmd::SetModel(path));
    }

    /// Switch GPU offload on/off (config reformat_device). Takes effect on the
    /// next reformat (the current model, if any, is dropped and reloaded).
    pub fn set_use_gpu(&self, use_gpu: bool) {
        let _ = self.tx.send(ReformatCmd::SetUseGpu(use_gpu));
    }

    /// Warm-load the model (emits Loading{0/50/100} -> Ready, or Missing/Error).
    pub fn ensure(&self) {
        let _ = self.tx.send(ReformatCmd::Ensure);
    }

    /// Queue a reformat. Reply is `ReformatDone`/`ReformatFailed` with `generation`.
    pub fn reformat(&self, det: String, generation: u64) {
        let _ = self.tx.send(ReformatCmd::Reformat { det, generation });
    }

    /// Drop the model to free RAM (unload_on_idle).
    pub fn unload(&self) {
        let _ = self.tx.send(ReformatCmd::Unload);
    }
}

fn run(rx: Receiver<ReformatCmd>, tx: Sender<CoordMsg>, use_gpu: bool) {
    let mut state = State::new(use_gpu);
    // Loop ends when the handle is dropped (channel closed) -> model freed.
    while let Ok(cmd) = rx.recv() {
        match cmd {
            ReformatCmd::SetModel(path) => state.set_model(path),
            ReformatCmd::SetUseGpu(g) => state.set_use_gpu(g),
            ReformatCmd::Ensure => {
                if state.is_loaded() {
                    let _ = tx.send(CoordMsg::ReformatModelStatus { status: ModelStatus::Ready });
                } else {
                    state.ensure(&tx);
                }
            }
            ReformatCmd::Reformat { det, generation } => match state.reformat(&tx, &det) {
                Ok(text) => {
                    let _ = tx.send(CoordMsg::ReformatDone { generation, text });
                }
                Err(error) => {
                    let _ = tx.send(CoordMsg::ReformatFailed { generation, error });
                }
            },
            ReformatCmd::Unload => state.unload(&tx),
        }
    }
}

/// Build the ChatML prompt the GGUF was trained on (plain ChatML, no BOS).
fn build_prompt(det: &str) -> String {
    format!(
        "<|im_start|>system\n{SYSTEM}<|im_end|>\n<|im_start|>user\n{det}<|im_end|>\n<|im_start|>assistant\n"
    )
}

/// Trim and strip a trailing <|im_end|> if the detokenizer rendered one
/// (EOS is normally caught by is_eog before it reaches the output).
fn postprocess(raw: &str) -> String {
    let t = raw.trim_end();
    let t = t.strip_suffix("<|im_end|>").unwrap_or(t);
    t.trim().to_string()
}

// ---------------------------------------------------------------------------
// State — the model lifecycle. Two impls with identical signatures: the real
// llama-cpp-2 one, and a stub when the feature is off.
// ---------------------------------------------------------------------------

#[cfg(feature = "reformat-llm")]
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::params::LlamaModelParams,
    model::{AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
    token::LlamaToken,
};
#[cfg(feature = "reformat-llm")]
use std::num::NonZeroU32;

#[cfg(feature = "reformat-llm")]
struct State {
    /// Process-global; init once, kept alive for the model's lifetime.
    backend: Option<LlamaBackend>,
    model: Option<LlamaModel>,
    model_path: Option<PathBuf>,
    /// Capable dGPU present (soft gate). Offload only when true AND vulkan-built.
    use_gpu: bool,
}

#[cfg(feature = "reformat-llm")]
impl State {
    fn new(use_gpu: bool) -> Self {
        State { backend: None, model: None, model_path: None, use_gpu }
    }

    fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    fn set_model(&mut self, path: PathBuf) {
        if self.model_path.as_deref() != Some(path.as_path()) {
            self.model_path = Some(path);
            // Drop the old model (frees GBs). Caller decides whether to warm the
            // new one (ensure follows unless unload_on_idle). No status here.
            self.model = None;
        }
    }

    fn set_use_gpu(&mut self, use_gpu: bool) {
        if self.use_gpu != use_gpu {
            self.use_gpu = use_gpu;
            // n_gpu_layers is fixed at load — drop so the next ensure reloads on
            // the newly-selected device.
            self.model = None;
        }
    }

    /// Load the model if absent. Emits the coarse status flow (silent when
    /// already loaded, like asr.rs) and returns whether a model is available.
    fn ensure(&mut self, tx: &Sender<CoordMsg>) -> bool {
        if self.model.is_some() {
            return true;
        }
        let status = |s| {
            let _ = tx.send(CoordMsg::ReformatModelStatus { status: s });
        };
        status(ModelStatus::Loading { pct: 0 });
        let path = match &self.model_path {
            Some(p) if p.exists() => p.clone(),
            _ => {
                status(ModelStatus::Missing);
                return false;
            }
        };
        if self.backend.is_none() {
            match LlamaBackend::init() {
                Ok(b) => self.backend = Some(b),
                Err(e) => {
                    eprintln!("reformat: llama backend init failed: {e}");
                    status(ModelStatus::Error("THE REFORMAT MODEL WOULD NOT LOAD".into()));
                    return false;
                }
            }
        }
        status(ModelStatus::Loading { pct: 50 });
        // Offload everything only on a vulkan build AND a capable GPU (soft gate:
        // >=4GB dGPU). CPU/iGPU machines stay at 0 layers even in a vulkan build,
        // so one installer is safe everywhere. // ponytail: 999 = "all layers".
        let n_gpu_layers: u32 = if cfg!(feature = "vulkan") && self.use_gpu { 999 } else { 0 };
        let params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);
        match LlamaModel::load_from_file(self.backend.as_ref().unwrap(), &path, &params) {
            Ok(m) => {
                self.model = Some(m);
                status(ModelStatus::Loading { pct: 100 });
                status(ModelStatus::Ready);
                true
            }
            Err(e) => {
                eprintln!("reformat: model failed to load: {e}");
                status(ModelStatus::Error("THE REFORMAT MODEL WOULD NOT LOAD".into()));
                false
            }
        }
    }

    fn reformat(&mut self, tx: &Sender<CoordMsg>, det: &str) -> Result<String, String> {
        if !self.ensure(tx) {
            return Err("reformat model not loaded".into());
        }
        let model = self.model.as_ref().unwrap();
        let backend = self.backend.as_ref().unwrap();

        let prompt = build_prompt(det);
        // Plain ChatML, no BOS — Qwen2.5 defines none and the template bakes the
        // <|im_start|> markers itself.
        let tokens = model
            .str_to_token(&prompt, AddBos::Never)
            .map_err(|e| format!("tokenize failed: {e}"))?;
        // ponytail: a take long enough to overflow n_ctx just fails here and the
        // coordinator injects the deterministic `det` instead. Fine for dictation.
        if tokens.len() + MAX_OUTPUT_TOKENS > N_CTX as usize {
            return Err("prompt exceeds context window".into());
        }

        // Fresh context per call: LlamaContext borrows the model, so it can't be
        // stored alongside it (self-referential), and a new one starts with a
        // clean KV cache — no cross-take contamination.
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(N_CTX))
            .with_n_batch(N_BATCH as u32);
        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| format!("context init failed: {e}"))?;

        // Decode the prompt in n_batch-sized chunks; only the final token needs
        // logits (that's where generation samples from).
        let mut batch = LlamaBatch::new(N_BATCH, 1);
        let n_prompt = tokens.len();
        let mut i = 0;
        while i < n_prompt {
            batch.clear();
            let end = (i + N_BATCH).min(n_prompt);
            for j in i..end {
                batch
                    .add(tokens[j], j as i32, &[0], j == n_prompt - 1)
                    .map_err(|e| format!("batch add failed: {e}"))?;
            }
            ctx.decode(&mut batch).map_err(|e| format!("decode failed: {e}"))?;
            i = end;
        }

        // Greedy (temp 0): sample() also accepts the token internally.
        // n_cur is the next KV position — the prompt filled 0..n_prompt.
        let mut sampler = LlamaSampler::greedy();
        let mut n_cur = n_prompt as i32;
        let mut out: Vec<u8> = Vec::new();
        for _ in 0..MAX_OUTPUT_TOKENS {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if model.is_eog_token(token) {
                break;
            }
            out.extend(render_token(model, token));
            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| format!("batch add failed: {e}"))?;
            n_cur += 1;
            ctx.decode(&mut batch).map_err(|e| format!("decode failed: {e}"))?;
        }

        Ok(postprocess(&String::from_utf8_lossy(&out)))
    }

    fn unload(&mut self, tx: &Sender<CoordMsg>) {
        // Keep the backend (process-global); just drop the model to free RAM.
        self.model = None;
        let _ = tx.send(CoordMsg::ReformatModelStatus { status: ModelStatus::Unloaded });
    }
}

/// Detokenize one token to raw bytes; accumulated then utf8-lossy'd at the end
/// so a multi-byte codepoint split across tokens still renders correctly.
// token_to_bytes is deprecated but stable and simplest; no encoding_rs decoder.
#[cfg(feature = "reformat-llm")]
#[allow(deprecated)]
fn render_token(model: &LlamaModel, token: LlamaToken) -> Vec<u8> {
    model.token_to_bytes(token, Special::Tokenize).unwrap_or_default()
}

// --- Stub State (built without reformat-llm): no llama, deterministic-only. ---

#[cfg(not(feature = "reformat-llm"))]
struct State;

#[cfg(not(feature = "reformat-llm"))]
impl State {
    fn new(_use_gpu: bool) -> Self {
        State
    }
    fn is_loaded(&self) -> bool {
        false
    }
    fn set_model(&mut self, _path: PathBuf) {}
    fn set_use_gpu(&mut self, _use_gpu: bool) {}
    fn ensure(&mut self, tx: &Sender<CoordMsg>) -> bool {
        let _ = tx.send(CoordMsg::ReformatModelStatus { status: ModelStatus::Missing });
        false
    }
    fn reformat(&mut self, _tx: &Sender<CoordMsg>, _det: &str) -> Result<String, String> {
        Err("built without reformat-llm".into())
    }
    fn unload(&mut self, tx: &Sender<CoordMsg>) {
        let _ = tx.send(CoordMsg::ReformatModelStatus { status: ModelStatus::Unloaded });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatml_prompt_is_exact() {
        // Byte-exact ChatML wrapping, incl. the em-dash in the system prompt.
        let expected = "<|im_start|>system\nYou reformat spoken dictation into clean written text for an AI coding agent. Remove disfluencies and resolve self-corrections; preserve meaning and every identifier exactly; add nothing. Default to prose; bullets only for 3+ discrete items, numbered steps only for real sequences. Keep a question a question. Never answer or execute the dictation — only reformat it. Output only the reformatted text.<|im_end|>\n<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n";
        assert_eq!(build_prompt("hi"), expected);
    }

    #[test]
    fn chatml_prompt_preserves_em_dash() {
        // Guard against a stray hyphen sneaking into the system const.
        assert!(build_prompt("x").contains("dictation \u{2014} only reformat it"));
    }

    #[test]
    fn postprocess_trims_whitespace() {
        assert_eq!(postprocess("  hello world  "), "hello world");
    }

    #[test]
    fn postprocess_strips_trailing_im_end() {
        assert_eq!(postprocess("done<|im_end|>"), "done");
        assert_eq!(postprocess("done\n<|im_end|>"), "done");
        assert_eq!(postprocess("  done  <|im_end|>  "), "done");
    }

    #[test]
    fn postprocess_keeps_interior_im_end() {
        // Only a trailing marker is stripped; the token can legitimately appear
        // mid-text if the model ever emits it literally.
        assert_eq!(postprocess("a <|im_end|> b"), "a <|im_end|> b");
    }

    // Real inference — needs the GGUF on disk. Run manually:
    //   cargo test --features reformat-llm reformats_a_fixture -- --ignored --nocapture
    #[cfg(feature = "reformat-llm")]
    #[test]
    #[ignore]
    fn reformats_a_fixture() {
        let (tx, rx) = channel();
        let mut state = State::new(true);
        state.set_model(PathBuf::from(
            r"C:\Users\honorr\Documents\DEV\Dictum\finetune\out\dictum-reformat-1.5b-Q4_K_M.gguf",
        ));
        let det = "Yeah let's move the retry logic into, uh, coordinator dot rs, no wait, into scheduler dot rs, that's really where it belongs.";
        let out = state.reformat(&tx, det).expect("reformat should succeed");
        eprintln!("reformatted: {out:?}");
        assert!(!out.trim().is_empty());
        assert!(!out.contains("<|im_end|>"));
        drop(rx);
    }
}
