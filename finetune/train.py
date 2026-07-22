"""
Dictum reformatter SFT — bf16 LoRA via HF TRL on native Windows.
Verified recipe: docs/PLAN-0.3-finetune.md. NO QLoRA/bitsandbytes/Unsloth — bf16
LoRA fits a 3B in ~6-8GB on the 5090's 32GB, and sidesteps the Blackwell bnb pain.

Usage (from finetune/, after `pip install -r requirements.txt`):
  # 3B (GPU SKU):
  python train.py --model Qwen/Qwen2.5-3B-Instruct   --out out/reformat-3b   --epochs 2
  # 1.5B (CPU-fallback SKU):
  python train.py --model Qwen/Qwen2.5-1.5B-Instruct --out out/reformat-1.5b --epochs 3 --lr 1.5e-4

Some TRL/transformers arg names drift across versions — if an arg errors, that's
the loop: paste the error and we adjust. Keep the mask-verification output.
"""
import argparse
import torch
from datasets import load_dataset
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import LoraConfig
from trl import SFTTrainer, SFTConfig


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True, help="HF id or local path")
    ap.add_argument("--train", default="data/train.jsonl")
    ap.add_argument("--eval", default="data/eval.jsonl")
    ap.add_argument("--out", required=True, help="output dir for the LoRA adapter")
    ap.add_argument("--epochs", type=float, default=2.0)      # 2 for 3B, 3 for 1.5B
    ap.add_argument("--lr", type=float, default=1e-4)         # gentle: over-editing is the failure mode
    ap.add_argument("--rank", type=int, default=16)
    ap.add_argument("--alpha", type=int, default=16)          # ratio 1 — conservative
    ap.add_argument("--dropout", type=float, default=0.05)
    ap.add_argument("--seqlen", type=int, default=1024)
    ap.add_argument("--bs", type=int, default=4)
    ap.add_argument("--accum", type=int, default=4)           # effective batch ~16
    ap.add_argument("--all-linear", action="store_true",
                    help="expand LoRA to all-linear if attention-only under-cleans")
    args = ap.parse_args()

    # --- Blackwell sanity: sm_120 needs a cu128+ torch. Fail loud, not silent-CPU. ---
    assert torch.cuda.is_available(), "CUDA not available. Install torch cu128 (see README)."
    cap = torch.cuda.get_device_capability()
    print(f"[env] {torch.cuda.get_device_name(0)}  capability={cap}  torch={torch.__version__}")
    if cap[0] < 12:
        print(f"[warn] expected (12, 0) for a 5090; got {cap}. Wrong GPU or fine.")
    _ = (torch.randn(8, 8, device="cuda") @ torch.randn(8, 8, device="cuda")).sum().item()  # real matmul

    tok = AutoTokenizer.from_pretrained(args.model)
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token
    # Qwen2.5's stock template has no {% generation %} tags, and this TRL version
    # does NOT auto-patch them in — assistant_only_loss then finds zero assistant
    # tokens and errors. Our data is plain system/user/assistant ChatML, so set an
    # explicit ChatML template with generation tags (assistant content + <|im_end|>
    # inside the tags: the model must learn to emit the stop token). This patched
    # template is saved with the adapter, so GGUF export stays consistent.
    tok.chat_template = (
        "{% for message in messages %}"
        "{% if message['role'] == 'assistant' %}"
        "{{ '<|im_start|>assistant\n' }}"
        "{% generation %}{{ message['content'] }}{{ '<|im_end|>' }}{% endgeneration %}{{ '\n' }}"
        "{% else %}"
        "{{ '<|im_start|>' + message['role'] + '\n' + message['content'] + '<|im_end|>\n' }}"
        "{% endif %}"
        "{% endfor %}"
        "{% if add_generation_prompt %}{{ '<|im_start|>assistant\n' }}{% endif %}"
    )

    model = AutoModelForCausalLM.from_pretrained(
        args.model, torch_dtype=torch.bfloat16, device_map="auto",
        attn_implementation="sdpa",  # avoids flash-attn/xformers compile on Blackwell
    )

    targets = "all-linear" if args.all_linear else ["q_proj", "k_proj", "v_proj", "o_proj"]
    peft_cfg = LoraConfig(
        r=args.rank, lora_alpha=args.alpha, lora_dropout=args.dropout,
        target_modules=targets, bias="none", task_type="CAUSAL_LM",
    )

    ds_train = load_dataset("json", data_files=args.train, split="train")
    ds_eval = load_dataset("json", data_files=args.eval, split="train")
    print(f"[data] train={len(ds_train)} eval={len(ds_eval)}")

    cfg = SFTConfig(
        output_dir=args.out,
        num_train_epochs=args.epochs,
        learning_rate=args.lr,
        lr_scheduler_type="cosine",
        warmup_ratio=0.08,
        per_device_train_batch_size=args.bs,
        gradient_accumulation_steps=args.accum,
        max_length=args.seqlen,          # (older TRL: max_seq_length)
        bf16=True,
        packing=False,
        assistant_only_loss=True,        # << train on completions only (mask the messy prompt)
        weight_decay=0.1,
        max_grad_norm=1.0,
        logging_steps=10,
        eval_strategy="epoch",
        save_strategy="epoch",
        load_best_model_at_end=True,     # << keep the BEST checkpoint, not the last
        metric_for_best_model="eval_loss",
        greater_is_better=False,
        report_to="none",
    )

    trainer = SFTTrainer(
        model=model, args=cfg,
        train_dataset=ds_train, eval_dataset=ds_eval,
        processing_class=tok, peft_config=peft_cfg,
    )

    verify_completion_mask(trainer, tok)  # one-time: confirm only the clean output is unmasked

    trainer.train()
    trainer.save_model(args.out)
    tok.save_pretrained(args.out)
    print(f"[done] LoRA adapter -> {args.out}  (merge + GGUF: see README)")


def verify_completion_mask(trainer, tok):
    """Decode the tokens where label != -100 for one example. It MUST be only the
    assistant's clean text (+ trailing <|im_end|>) — never the disfluent prompt.
    Off-by-one on the response template is the classic silent SFT bug."""
    try:
        batch = next(iter(trainer.get_train_dataloader()))
        labels = batch["labels"][0]
        ids = batch["input_ids"][0]
        kept = [int(i) for i, l in zip(ids, labels) if int(l) != -100]
        print("\n[mask-check] tokens with loss (should be ONLY the clean completion):")
        print("  " + repr(tok.decode(kept)) + "\n")
    except Exception as e:  # never let the check block training
        print(f"[mask-check] skipped: {e}")


if __name__ == "__main__":
    main()
