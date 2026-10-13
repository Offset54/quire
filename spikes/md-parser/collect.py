"""Collect the spike's summaries into one result file: collect.py <dir> <out.json>."""

import json
import platform
import sys
from pathlib import Path

d = Path(sys.argv[1])
out = {
    "spike": "M2 task 1: comrak 0.56.0 vs. pulldown-cmark 0.13.4 (ADR-0026)",
    "machine": "dev machine, raw (not ref-sim): Core Ultra 7 258V, AC power",
    "python": platform.python_version(),
    "spec": {},
    "throughput_20mib": json.load(open(d / "bench.json", encoding="utf-8")),
}
for suite in ("commonmark", "gfm", "gfm-ext"):
    s = json.load(open(d / f"summary-{suite}.json", encoding="utf-8"))
    out["spec"][suite] = s
Path(sys.argv[2]).write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
