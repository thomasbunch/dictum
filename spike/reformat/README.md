# Dictum 0.3 reformatter spike

**Goal:** before we build the LLM subsystem, prove two things on real hardware —
**(1) latency** (is a 3B reformat fast enough on your GPU? how slow on CPU?) and
**(2) quality** (does a 3B model actually reformat per our rules — no invented
bullets, no answering the prompt, identifiers preserved?).

This drives `llama-cli` directly. It is throwaway. The `system-prompt.txt` and
`fixtures.jsonl` here are NOT throwaway — they become the product's few-shot
prompt and test suite.

## Prereqs (you download these — I don't)

1. **llama.cpp, Vulkan build for Windows.** From the llama.cpp releases page
   (github.com/ggml-org/llama.cpp/releases), grab `llama-<ver>-bin-win-vulkan-x64.zip`,
   unzip somewhere (e.g. `C:\llama\`). `llama-cli.exe` is inside. Vulkan build =
   vendor-neutral GPU, which is the runtime we chose for shipping.
2. **The 3B model (GPU + desktop-CPU test):** Qwen2.5-3B-Instruct, **Q4_K_M** GGUF.
   From Hugging Face `Qwen/Qwen2.5-3B-Instruct-GGUF` (file
   `qwen2.5-3b-instruct-q4_k_m.gguf`, ~1.9 GB), or bartowski's equivalent.
3. **The 1.5B model (CPU-fallback test):** Qwen2.5-1.5B-Instruct **Q4_K_M** GGUF,
   same sources. This is the soft-gate CPU fallback we're validating.

## Run (work desktop w/ GPU first)

```powershell
cd spike\reformat

# 3B — both GPU and CPU passes on the desktop:
.\bench.ps1 -Model "C:\models\qwen2.5-3b-instruct-q4_k_m.gguf" -LlamaCli "C:\llama\llama-cli.exe"

# 1.5B — the CPU fallback (CPU pass only is enough):
.\bench.ps1 -Model "C:\models\qwen2.5-1.5b-instruct-q4_k_m.gguf" -LlamaCli "C:\llama\llama-cli.exe" -Modes CPU
```

Later, on the laptop, run the same two commands to confirm the battery-CPU numbers.

It prints a per-fixture line (time + the cleaned output) and a per-mode summary
(avg end-to-end seconds, gen tok/s, prefill/TTFT), and writes a full
`results-<timestamp>.txt` for eyeballing GOT vs EXPECTED.

> Flag drift: llama.cpp renames flags between builds. If no timings parse or the
> prompt is echoed into the output, drop `--no-display-prompt` and/or `-no-cnv`
> from `bench.ps1` and re-run — the CLEANED-extraction is robust to prompt echo
> either way.

## What we're looking for (go / no-go)

**Latency** (predicted in PLAN-0.3 §5 — this confirms or kills it):

| | short (~60 out) | long (~180 out) | read |
|---|---|---|---|
| **3B on GPU** | < ~2s | < ~4s | ✅ ship the GPU path on-by-default |
| **3B on CPU** | ~4–5s | ~13s | ⚠ confirms 3B is *not* a CPU default |
| **1.5B on CPU** | ~1.5–3s | ~4–9s | ✅ confirms the usable CPU fallback |

**Quality** — look only at the **8 held-out fixtures** (`held` in the output;
the 5 `shot` ones are in the prompt, so they're a sanity check, not a signal).
Count a held-out case as PASS if it: preserves meaning + every identifier, does
**not** invent bullets/steps, keeps a question a question (id 4-style), and does
**not** answer/execute (id 13). Targets:

- **3B: ≥ 6/8 held-out clean** → proceed to build with confidence.
- **3B: < 6/8** → iterate `system-prompt.txt` (more/better few-shot) and re-run
  before writing any subsystem code. Cheap to fix here, expensive later.
- **1.5B:** expect it to fail more (that's why it's the *fallback* and why the
  guardrail chain is mandatory) — just note *how* it fails (invents structure?
  answers? drops identifiers?) to size the guardrail work.

Paste me the summary lines + the `results-*.txt` and we make the go/no-go call
together, then move to the build phase.
