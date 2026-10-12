# M1 TXT engine: task breakdown

Estimates are in focused engineer-days for one engineer, and include tests and docs for each task. They'll be re-estimated from actuals at the M1 report.

## Scope and DoD (§11)

Scope: piece tree, mapped/chunked sources and the background line index; encodings; the viewer and editor, including multi-cursor and search/replace; atomic save using the mapped-file sequence; journal recovery and external change detection.

DoD:
- Every TXT row in §4 is met, measured in the ref-sim profile against `bench/budgets.json`.
- Property and fuzz tests pass.
- Killing the process mid-edit loses at most one second of work.
- Untouched content survives save byte-for-byte.

Carried into M1: open and render a file passed on the command line in parallel with WebView2 startup (ADR-0022), with time-to-first-content measured for the 2 GiB fixture; WebDriver UI-flow tests (ADR-0020).

## Effective budgets in ref-sim

ADR-0021 derates: `cpu` work passes if `value × 1.5 ≤ budget`, `mixed` work (cold I/O plus CPU) if `value × max(1.5, D_io) ≤ budget`. `D_io` was 1.74 in M0.

| §4 row | Budget | Kind | Effective here |
|---|---|---|---|
| 2 GB TXT first paint | 300 ms | mixed | ≈ 172 ms |
| Full line index (background) | 4 s | mixed | ≈ 2.3 s |
| Jump to any line, indexed | 100 ms | cpu | 67 ms |
| Jump to any line, before indexing (approximate) | 300 ms | mixed | ≈ 172 ms |
| Literal search, full pass (results stream from the first hit) | 3 s | mixed | ≈ 1.7 s |
| Insert at any offset | 5 ms | cpu | 3.3 ms |
| 300 MB single line: first paint | 300 ms | mixed | ≈ 172 ms |
| 300 MB single line: caret movement, wrap toggle | no hang | cpu | no UI task over 50 ms; each response ≤ 100 ms |
| Typing → visual response | p95 16 ms, p99 33 ms | cpu | p95 10.7 ms, p99 22 ms |
| Scrolling | 60 fps, no frame over 50 ms | raw | p99 frame ≤ 16.7 ms, max ≤ 50 ms, judged without platform frame stalls, which are reported separately (user decision, 2026-10-12; ADR-0024; also first paint and jumps) |
| Save after a small edit, 2 GB | sequential write speed | io | save time ≤ 1.25 × the mean of raw sequential writes of the same bytes on the same volume, taken right before and right after each save; median of 5 runs (user decision, 2026-10-12: raw 2 GiB writes alone vary 1.0–2.1 s on the dev SSD) |
| Memory beyond the mapped source | ≤ 1.5 GB per document | raw | process private bytes growth after open, index, search and edits |

## Tasks

| # | Task | Output | Est. (d) | Depends on |
|---|---|---|---|---|
| 1 | Sources | `crates/text` scaffold; `Source` trait; memory-mapped source with a guarded copy (SEH on Windows, SIGBUS on Unix) so I/O errors on a mapped view become `io::Error`; pread + LRU chunk source for network and cloud-placeholder files (governor-registered); remote/placeholder detection; spike: cold sequential scan through the mapping vs. pread vs. mapping + prefetch; **ADR-0007** | 3 | none |
| 2 | Encodings | Detection BOM → declared (XML, HTML meta, coding modeline) → chardetng, UTF-16 heuristic, binary detection; UTF-8 documents map the original directly, all others are transcoded to UTF-8 in clean segments (parallel) with checkpoints so untouched ranges can be copied raw on save; user override; **ADR-0009** | 3 | 1 |
| 3 | Line index | Background chunk scan (64 KiB chunks) on the scheduler: break counts (LF, CR, CRLF; mixed endings flagged), NUL counts, per-chunk fingerprints; progressive (prefix known early); exact and estimated line↔offset | 2 | 1 |
| 4 | Piece tree | B-tree of pieces over buffers (original, add buffer, derived), cached bytes and line breaks per node; unknown counts until indexed; CRLF junction fix-up; line↔offset in O(log n); bulk build; proptests against a naive model; criterion benches; **ADR-0008** | 4 | 3 |
| 5 | Editing model | Selections and multi-cursor, column (box) selection, grapheme/word/line movements, wrap-aware rows, transactions with inverses, typing coalescing, undo/redo with a memory bound; proptest: undo is an exact inverse | 4 | 4 |
| 6 | Search and replace | Chunked parallel `regex-automata` search with overlap and exact merge, streaming results with a memory cap, incremental update after edits; replace, and replace-all as one undoable background transaction with progress and cancel | 3 | 4, 5 |
| 7 | Save pipeline | Atomic save (temp in the same directory, fsync, `ReplaceFileW` / `rename`, directory fsync), the §5.8 mapped-file sequence, encoding/BOM/line-ending output, raw copy of untouched ranges, attributes preserved, save report; byte-exact round-trip tests across every encoding | 3 | 2, 4 |
| 8 | Journal and recovery | Per-document journal (records written at once, fsync batched ≤ 1/s), recovery on next launch, deletion after save/close, hostile-input-safe parser; kill test: a child process is killed mid-edit and recovery loses ≤ 1 s | 2.5 | 5 |
| 9 | External changes | File watcher plus mtime/size/identity check on focus; fingerprint verification; reload / keep mine / compare (small files); write-deny lock while dirty; auto-reload when clean | 2 | 3, 7 |
| 10 | TXT surface spike | Virtualized DOM rows vs. core tiles for TXT: frame cost, long lines, IME with the Microsoft IMEs, accessibility; **ADR-0023** | 1.5 | none |
| 11 | Host integration | Tauri commands, binary row and search protocols (no JSON for bulk text), per-document status channel, panic isolation; opening a command-line file before the webview is ready (ADR-0022) | 3 | 1–9 |
| 12 | Editor frontend | `TxtView` (rows, virtual scroll by byte anchor, custom scrollbar, gutter, wrap, column window for long lines, caret/selection/highlight overlay, IME, clipboard, mouse, keyboard), status bar, find/replace panel, go-to-line, encoding and line-ending menus, banners (binary, decode progress, external change, recovery), open/save dialogs, drag and drop, i18n strings | 6 | 10, 11 |
| 13 | Fuzzing | `fuzz/` targets: encoding detection + transcoding, piece tree vs. model, journal parser, chunked search vs. whole-buffer search; nightly fuzz workflow | 1.5 | 2, 4, 6, 8 |
| 14 | Benchmarks | TXT budgets in `bench/budgets.json`; in-app scenarios `txt-2g`, `txt-300m`, `launch-2g`; typing latency; save vs. raw sequential write; memory; `refsim.ps1` runs them; criterion benches | 3 | 11, 12 |
| 15 | WebDriver UI flows | tauri-driver + msedgedriver matching the WebView2 runtime; minimal W3C client; flows: open, type, undo, find/replace, go-to-line, save, kill and recover; CI job on Windows | 2 | 12 |
| 16 | M1 report | `docs/M1_REPORT.md` with DoD evidence; ADRs; PROGRESS, RISKS, ARCHITECTURE updated | 1 | all |
| | **Total** | | **44.5** | |

With a 20 % contingency, M1 takes about 53 working days of one engineer's time.

## Order

1. Tasks 1, 3 and 4 first (the data structures everything else stands on), with task 10's spike in parallel.
2. Then 2, 5 and 6; then 7, 8 and 9.
3. Tasks 11 and 12 once the engine's API is stable; 13 alongside.
4. Tasks 14 and 15 last, then the report.

## Decisions to make in M1 (ADRs)

- **0007 Source access:** mapping vs. pread for bulk scans; how I/O errors on mapped views are contained.
- **0008 Text buffer:** piece tree shape, unknown line counts before indexing, how replace-all stays bounded in memory.
- **0009 Encodings:** UTF-8 in place vs. transcoding; how legacy encodings keep untouched bytes exact.
- **0023 TXT surface:** DOM text rows vs. core-rendered tiles.
