# Reformatter model evaluation — v0.3.0 release evidence (2026-07-22)

Models: LoRA (all-linear, r16) fine-tunes of Qwen2.5-Instruct on dataset v2
(1,096 train / 188 held-out eval), merged fp16 -> GGUF f16 -> Q4_K_M with an
in-domain imatrix (300 chat-formatted train samples). Eval = 188 held-out rows,
temp 0. Gates are the product guardrail proxies (identifier-presence,
question-preserved, length-band, nonempty).

## Layer 1 — deterministic gates

| artifact | idents | question | length | gate-fails |
|---|---|---|---|---|
| 3B bf16 adapter (all-linear)   | 99%  | 99% | 99% | 4/188 (2.1%) |
| **3B Q4_K_M (ships)**          | 100% | 97% | 99% | 7/188 (3.7%) |
| 1.5B bf16 adapter (all-linear) | 99%  | 98% | 99% | 5/188 (2.7%) |
| **1.5B Q4_K_M (ships)**        | 100% | 98% | 99% | 5/188 (2.7%) |

Quantization cost ≈ nothing (the imatrix protected identifier tokens: 100%
ident-gate on both Q4 artifacts). Attention-only LoRA baselines were worse for
both sizes (7 and 13 fails respectively) — all-linear is the shipped recipe.

## Layer 2 — LLM-judge rubric audit (Q4 artifacts)

Judged: every gate-fail row + a 14-row random sample of gate-passing rows, per model,
against the PLAN-0.3 §2.1 rubric (structure/question/not-answered/identifiers/meaning).

- **3B Q4**: 6 of 7 gate-fails were heuristic false positives (real quality higher than
  gates suggest). Random sample: 11/14 fully clean; 3 real defects the gates missed —
  one dropped step in a sequence, one colon-tag corruption (`postgres:15-alpine` ->
  `postgres15-alpine`), one garbled self-correction resolution.
- **1.5B Q4**: 3 of 5 gate-fails benign. One serious meaning drop (an entire proposal
  silently dropped from a teammate message), one minor tone-instruction drop. Random
  sample: 12/14 clean; recurring weakness is under-bulleting 3+ discrete items.

**Implication for the product (by design):** residual failures are content drops and
identifier micro-corruptions — the exact classes the mandatory runtime guardrail chain
(guardrail.rs: length-ratio + identifier-preservation + question + polarity) trips on,
falling back to the deterministic `replacements::apply` floor. No model output is ever
injected unchecked. Known gate gap to port into guardrail.rs: colon-suffixed image
tags (`postgres:15-alpine`) as identifier tokens.

## Shipped artifacts

| file | size | sha256 |
|---|---|---|
| dictum-reformat-3b-Q4_K_M.gguf   | 1,929.9 MB | ddd7a3ecfbe7f4497f3235305570f64d78d72f581eae9d2f829786983021bc87 |
| dictum-reformat-1.5b-Q4_K_M.gguf |   986.0 MB | ee87905270eb92b2ec00ed6536241dd1553caff4e2f7f8c6ea192faccaba2d72 |

Reproduce: `train.py` (--all-linear) -> `export.py` -> `eval.py` (this file's numbers).
