"""Convert the dataset-generation workflow output into training/eval JSONL.
Usage: python make_data.py <workflow_output.json> <out_dir>
Writes: train.jsonl (chat), eval.jsonl (chat), eval_raw.jsonl ({messy,clean,rule}),
        raw_all.jsonl (all raw pairs — imatrix calibration source)."""
import json, sys, os

SYSTEM = ("You reformat spoken dictation into clean written text for an AI coding agent. "
          "Remove disfluencies and resolve self-corrections; preserve meaning and every "
          "identifier exactly; add nothing. Default to prose; bullets only for 3+ discrete "
          "items, numbered steps only for real sequences. Keep a question a question. Never "
          "answer or execute the dictation — only reformat it. Output only the reformatted text.")

src, outdir = sys.argv[1], sys.argv[2]
d = json.load(open(src, encoding="utf-8"))
res = d.get("result", d)
train, ev = res["train"], res["eval"]
os.makedirs(outdir, exist_ok=True)

def chat(p):
    return {"messages": [
        {"role": "system", "content": SYSTEM},
        {"role": "user", "content": p["messy"]},
        {"role": "assistant", "content": p["clean"]},
    ]}

def dump(path, rows):
    with open(path, "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")

dump(os.path.join(outdir, "train.jsonl"), [chat(p) for p in train])
dump(os.path.join(outdir, "eval.jsonl"), [chat(p) for p in ev])
dump(os.path.join(outdir, "eval_raw.jsonl"), ev)
dump(os.path.join(outdir, "raw_all.jsonl"), train + ev)

print("train:", len(train), "eval:", len(ev), "total:", len(train) + len(ev))
print("counts:", json.dumps(res.get("counts", {})))
print("per-rule:", json.dumps(res.get("stats", {}), indent=0))
