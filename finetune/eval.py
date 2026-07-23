"""
Layer-1 gate eval on the QUANTIZED GGUF (the artifact that ships) via llama-server.
Gates mirror the product guardrail chain: identifiers, question-preserved,
length-ratio, nonempty. Layer-2 (LLM judge) runs separately on the outputs file.

  python eval.py --gguf out/dictum-reformat-3b-Q4_K_M.gguf --out results-3b-q4.json
"""
import argparse
import json
import re
import subprocess
import sys
import time
import urllib.request

SERVER = r"C:\llama\bin\llama-server.exe"
PORT = 8087
IDENT = re.compile(r"[A-Za-z_][\w]*(?:\.[A-Za-z_][\w]*)+|[A-Za-z]+_[\w]+|[a-z]+[A-Z][A-Za-z]*")


def chat(messages):
    body = json.dumps({"messages": messages, "temperature": 0, "max_tokens": 512}).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{PORT}/v1/chat/completions", body,
        {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.load(r)["choices"][0]["message"]["content"].strip()


def gates(user, ref, out):
    idents = set(IDENT.findall(user)) & set(IDENT.findall(ref))
    wr = len(out.split()) / max(1, len(ref.split()))
    return {
        "idents": all(i in out for i in idents),
        "question": ("?" in out) == ("?" in ref),
        "length": 0.5 <= wr <= 1.7,
        "nonempty": len(out) > 0,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gguf", required=True)
    ap.add_argument("--eval", default="data/eval.jsonl")
    ap.add_argument("--out", required=True)
    ap.add_argument("--ngl", type=int, default=99)
    args = ap.parse_args()

    rows = [json.loads(l) for l in open(args.eval, encoding="utf-8")]
    srv = subprocess.Popen(
        [SERVER, "-m", args.gguf, "--port", str(PORT), "-ngl", str(args.ngl),
         "--jinja", "-c", "4096"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(120):  # wait for server health
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{PORT}/health", timeout=2)
                break
            except Exception:
                time.sleep(1)
        else:
            sys.exit("server never became healthy")

        counts = {"idents": 0, "question": 0, "length": 0, "nonempty": 0, "exact": 0}
        results, fails = [], 0
        for i, r in enumerate(rows):
            user, ref = r["messages"][1]["content"], r["messages"][2]["content"]
            out = chat(r["messages"][:2])
            g = gates(user, ref, out)
            for k, v in g.items():
                counts[k] += v
            counts["exact"] += out == ref
            if not all(g.values()):
                fails += 1
            results.append({"i": i, "user": user, "ref": ref, "out": out, "gates": g})
            if (i + 1) % 25 == 0:
                print(f"  {i+1}/{len(rows)}", flush=True)

        n = len(rows)
        print(f"\n== {args.gguf} on {n} held-out (temp 0, Q4_K_M) ==")
        for k, v in counts.items():
            print(f"  {k:>9}: {v}/{n}  ({100*v/n:.0f}%)")
        print(f"  gate-fails: {fails}")
        json.dump({"gguf": args.gguf, "counts": counts, "n": n, "gate_fails": fails,
                   "results": results},
                  open(args.out, "w", encoding="utf-8"), indent=1, ensure_ascii=False)
    finally:
        srv.terminate()


if __name__ == "__main__":
    main()
