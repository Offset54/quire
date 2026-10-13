"""For GFM spec examples, find the same Markdown in CommonMark 0.31.2 and
compare the expected HTML: drift.py <gfm-spec.json> <commonmark-spec.json> <n>...
(the spec-*.json files written by the spike)."""

import json
import sys

sys.stdout.reconfigure(encoding="utf-8")
gfm = json.load(open(sys.argv[1], encoding="utf-8"))
cm = json.load(open(sys.argv[2], encoding="utf-8"))
by_md = {}
for r in cm:
    by_md.setdefault(r["markdown"], r)
for n in sys.argv[3:]:
    g = gfm[int(n) - 1]
    c = by_md.get(g["markdown"])
    if c is None:
        print(f"GFM {n}: no CommonMark 0.31.2 example with the same input")
        continue
    same = c["expected"] == g["expected"]
    print(f"GFM {n} = CommonMark {c['number']}: expected HTML {'same' if same else 'DIFFERS'}")
    if not same:
        print("   0.29 (GFM):", repr(g["expected"]))
        print("   0.31.2    :", repr(c["expected"]))
