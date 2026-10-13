"""Show spec examples from a spike result: show.py <spec-*.json> <parser> <n>..."""

import json
import sys

sys.stdout.reconfigure(encoding="utf-8")

rows = json.load(open(sys.argv[1], encoding="utf-8"))
parser = sys.argv[2]
for n in sys.argv[3:]:
    r = rows[int(n) - 1]
    print(f"=== {r['number']} ({r['section']}) ok={r[parser + '_ok']}")
    print("md:      ", repr(r["markdown"]))
    print("expected:", repr(r["expected"]))
    print("got:     ", repr(r[parser]))
