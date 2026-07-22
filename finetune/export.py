"""
Dictum reformatter export: LoRA adapter -> merged fp16 -> GGUF f16 -> in-domain
imatrix -> Q4_K_M. Toolchain: C:\\llama (clone + b10081 Vulkan release bins).

  python export.py --base Qwen/Qwen2.5-3B-Instruct   --adapter out/reformat-3b-al   --name dictum-reformat-3b
  python export.py --base Qwen/Qwen2.5-1.5B-Instruct --adapter out/reformat-1.5b-al --name dictum-reformat-1.5b

The trained chat template rides along in the merged tokenizer -> GGUF metadata
(run llama.cpp with --jinja). Steps are resumable: existing outputs are skipped.
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import PeftModel

LLAMA = r"C:\llama"
BIN = os.path.join(LLAMA, "bin")
CONVERT = os.path.join(LLAMA, "llama.cpp", "convert_hf_to_gguf.py")


def sh(cmd):
    print("+ " + " ".join(map(str, cmd)), flush=True)
    subprocess.run([str(c) for c in cmd], check=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--name", required=True)
    ap.add_argument("--imatrix-rows", type=int, default=300)
    args = ap.parse_args()
    out = os.path.join("out", args.name)
    merged = out + "-merged"

    if not os.path.exists(merged):
        tok = AutoTokenizer.from_pretrained(args.adapter)
        model = AutoModelForCausalLM.from_pretrained(
            args.base, torch_dtype=torch.bfloat16, device_map="cpu")
        model = PeftModel.from_pretrained(model, args.adapter)
        model = model.merge_and_unload()  # never merge into a quantized base
        model.to(torch.float16).save_pretrained(merged)
        tok.save_pretrained(merged)
        print(f"[merge] -> {merged}", flush=True)

    f16 = out + "-f16.gguf"
    if not os.path.exists(f16):
        sh([sys.executable, CONVERT, merged, "--outfile", f16, "--outtype", "f16"])

    corpus = out + "-imatrix-corpus.txt"
    if not os.path.exists(corpus):
        tok = AutoTokenizer.from_pretrained(merged)
        rows = [json.loads(l) for l in open("data/train.jsonl", encoding="utf-8")]
        with open(corpus, "w", encoding="utf-8") as f:
            for r in rows[: args.imatrix_rows]:  # in-domain, NOT wikitext
                f.write(tok.apply_chat_template(r["messages"], tokenize=False) + "\n")

    imat = out + "-imatrix.dat"
    if not os.path.exists(imat):
        sh([os.path.join(BIN, "llama-imatrix.exe"), "-m", f16, "-f", corpus,
            "-o", imat, "-ngl", "99"])

    q4 = out + "-Q4_K_M.gguf"
    if not os.path.exists(q4):
        sh([os.path.join(BIN, "llama-quantize.exe"), "--imatrix", imat, f16, q4, "Q4_K_M"])

    h = hashlib.sha256()
    with open(q4, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 22), b""):
            h.update(chunk)
    print(f"[done] {q4}  sha256={h.hexdigest()}  size={os.path.getsize(q4)/1e6:.1f}MB")


if __name__ == "__main__":
    main()
