# Fixture generators (M0 task 11)

Deterministic generators for every §4 benchmark scenario. Output goes to
`.cache/fixtures/` (ignored by git, about 7 GB) with a `manifest.json` that
records each file's size, SHA-256, seed and details. The same generator
version always produces byte-identical files. The committed copy of the
manifest is `bench/fixtures-manifest.json`.

```powershell
cargo run -p quire-fixtures --release              # everything
cargo run -p quire-fixtures --release -- --list    # names and descriptions
cargo run -p quire-fixtures --release -- txt-2g md-20m
tools/fixtures/validate.ps1                        # qpdf, Word and PowerPoint checks
```

| Name | File | Size |
|---|---|---|
| `txt-2g` | mixed-lines-2gib.txt | exactly 2 GiB; line lengths from 0 to 1 MiB; every line starts with its number |
| `json-300m` | single-line-300mib.json | 300 MiB, a single line |
| `md-20m` | document-20mib.md | 20 MiB of CommonMark + GFM |
| `docx-1000p` | report-1000-pages.docx | 1,000 pages in Word (validated), 333 tables, 334 inline images |
| `pptx-500s` | deck-500-slides.pptx | 500 slides, ~497 MB of unique JPEG media |
| `pdf-5000p` | book-5000-pages.pdf (+ `-linearized`) | 5,000 pages, ~1.0 GB; linearized copy made with `qpdf --deterministic-id` |
| `pdf-scanned-1000p` | scanned-1000-pages.pdf | 1,000 pages of 200 dpi grayscale scans, no text layer |

`validate.ps1` needs qpdf. Its Office checks need Word and PowerPoint, and are
skipped (and reported as skipped) if either app is already running, so they
never touch your open documents.
