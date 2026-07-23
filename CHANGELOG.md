# Changelog

## 0.3.0 — 2026-07-22

The flagship release: a fully local AI reformatter plus a deterministic trio.

### Added
- **Local LLM reformatter** (opt-in): fine-tuned Qwen2.5 LoRA SKUs
  (`dictum-reformat-3b` for GPU, `dictum-reformat-1.5b` for CPU; Q4_K_M GGUF,
  llama.cpp runtime). Soft GPU-gate picks the SKU (DXGI probe, ≥4 GB VRAM);
  lazy load, unload-on-idle via the existing eviction path; never a required
  dependency. Held-out eval: 100% identifier gate on both quantized SKUs
  (see finetune/EVAL.md).
- **Guardrail chain** on every LLM output: empty/preamble/length-ratio/
  question-stays-question/identifier-preservation (incl. bare numbers, docker
  colon-tags, self-correction awareness)/polarity. Any trip injects the
  deterministic cleanup instead. Raw transcript always recoverable in history.
- **Voice command grammar**: "new line", "new paragraph", "scratch that",
  "delete last sentence", "all caps that", "make that a list" — segment-exact
  matching that cannot false-trigger inside prose.
- **Voice snippets**: multi-line replacement values; `{cursor}` placeholder
  reserved (stripped, caret positioning lands with a paste-complete signal).
- **SETUP → REFORMATTER** section (AUTO/ON/OFF, GPU-gate readout, model cards
  with live status) and a REFORMATTING HUD state.
- Fine-tune pipeline in `finetune/`: dataset v2 (1,096/188), train/export/eval
  scripts, full release evidence in EVAL.md.

### Fixed
- Model download resume trap: a full-size partial no longer bricks FETCH
  (verify-or-restart instead of HTTP 416 loop).
- Hotkey during a pending reformat commits the deterministic text immediately
  and starts the next take (previously silently swallowed).
- First-run masthead now keys off the active model, not the first registry entry.

### Notes
- The installer is a **Vulkan (GPU-accelerated)** build. On a capable discrete
  GPU (≥4 GB VRAM) AUTO offloads the 3B reformatter to the GPU (reformat <1s);
  CPU/iGPU machines automatically stay on CPU with the 1.5B SKU via a runtime GPU
  gate (no offload where it wouldn't help), so one installer is safe everywhere.
  Explicit ON overrides.
- Per-app auto-profiles moved to 0.3.x.

## 0.2.0 — 2026-06

FILE TAG spoken `@file` mentions, multilingual model registry (Parakeet v2+v3),
TAPE redesign, NSIS per-user installer.

## 0.1.0

Walking skeleton: hold-to-talk dictation, Parakeet-TDT v2 int8 via sherpa-onnx,
injection fallback chain, HUD.
