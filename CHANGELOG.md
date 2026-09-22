# Changelog

## Unreleased

### Changed
- **Main window redesign pass.** One 44px header band (wordmark · nav · caption
  controls) replaces the titlebar plus per-view masthead, so the wordmark appears
  once and WORDS/SETUP start at the top of the window.
- **The tape prints machine status only when something is wrong.** A loaded model,
  the mic name and the zero-egress slogan no longer print on every open; the model
  card and input select in SETUP carry them, and the footer already carries the
  privacy line. Rows at rest show time, app and text — the take's numbers print in
  the expanded row instead, on one line.
- Masthead counters are WORDS TODAY and ON THE TAPE. BY APP percentages and the
  toolbar's idle meta line are gone; the search meta speaks only during a search.
- **Segmented strips** replace square radio inputs everywhere (hotkey mode,
  reformatter mode and compute, retention), and REFORMATTER splits into labelled
  MODE / COMPUTE / MODELS sub-rows instead of two stacked lists with two AUTOs.
- Toggles fill the track with ink when ON — the old OFF state read as disabled.
- Caption glyphs are inline SVG rather than font characters; click-to-cycle table
  cells carry a dotted underline; the selected theme card is a 2px ink frame.

### Fixed
- **Scrollbars are drawn by Dictum**: 8px gutter, square `--line` thumb, no arrow
  buttons, themed per palette. The 17px Chromium default is gone.
- **The second scrollbar is gone.** The hidden radio inputs were absolutely
  positioned inside a `static` parent, so they escaped the view's clip and made the
  document itself scrollable — which also let the header and footer scroll away and
  left a blank band under the footer.
- The INJECTION table no longer overflows at the 720px minimum window width; its
  APP column flexes and SETUP's grid cells can shrink.
- Caption buttons meet the 24px hit-target minimum.

### Added
- `src/dev-mock.ts`: a DEV-only browser harness (`main.html?mock`) that runs the UI
  in plain Chrome against fake IPC and fixture history, so design work needs no
  Rust build. Excluded from production bundles.

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
- **SETUP → REFORMATTER** section (AUTO/ON/OFF mode, an AUTO/GPU/CPU **compute
  device** selector, GPU-gate readout, model cards with live status) and a
  REFORMATTING HUD state. The device choice lets you keep the reformatter on the
  GPU when plugged in and force CPU on battery, all in the one installed build.
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
