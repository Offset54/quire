# 0036. PDF worker processes

- Status: Accepted
- Date: 2026-10-23

## Context

§5.2: PDFium isn't thread-safe and parses hostile files, so it runs in a pool of worker processes (one per physical core minus one, at least one); a crashed worker is restarted and its job re-queued, and the user sees no more than a tile rendering again. RISKS #6: form and annotation state is per process; workers multiply memory; hybrid CPUs (P and E cores) may make "physical cores minus one" overcommit slow cores. ADR-0035 chose PDFium through our own FFI.

## Decision

1. **Process.** A worker is the app's own executable started as `quire --pdf-worker`: `main` checks that switch before anything else and runs `quire_pdf::worker::run`. Nothing of the app (Tauri, WebView2, services) starts in a worker. `quire-pdf-worker`, a 0.3 MB binary with only the worker loop, runs the same code for tests and benchmarks. Workers start when the first PDF opens, never at app start.
2. **Protocol** (`crates/pdf/src/protocol.rs`): length-prefixed binary frames over the worker's stdin and stdout, little-endian, hand-encoded: `len u32 · id u64 · tag u8 · payload`. Responses carry the worker's private bytes. Pixels travel in the frame (raw RGBA). The host's decoder treats a worker as untrusted (it has parsed a hostile file): lengths are capped (`MAX_FRAME`, 64 MiB), counts are checked against the frame, non-finite numbers are rejected, and a bad frame kills the worker.
3. **Pool** (`crates/pdf/src/pool.rs`): each request goes to the alive worker with the fewest requests in flight; slots that are empty or dead are refilled on demand (spawn + version handshake). A worker opens a document the first time it gets a request for it, and reopens it when the document's overlay (unsaved updates, ADR-0037) changes revision. A worker that exits, sends garbage or exceeds the request timeout (20 s in the app) fails its pending requests; a request that died with its worker is retried once on a fresh worker, and only a second death reaches the caller (the page is then labeled as not renderable, never retried forever). On Windows all workers are in a Job Object with kill-on-close, so a dying host never leaves workers behind.
4. **Size:** physical cores of the highest efficiency class (Windows `GetLogicalProcessorInformationEx`), minus one, at least one; without topology, logical CPUs / 2 − 1. `QUIRE_PDF_WORKERS` overrides (ref-sim, tests). On the dev machine (Core Ultra 7 258V, 4 P + 4 LP E cores) that is 3, as on the 4-core reference machine.
5. **Files** are read in the worker through `quire-text`'s sources (ADR-0007): mapped with guarded copies, or chunked for network and cloud files; a failed read reaches PDFium as a failed block read. Files of 4 GiB or more on Windows are opened by path (ADR-0037).
6. **Memory:** the sum of the workers' private bytes is charged to the governor under `pdf-workers` after every tile batch. Recycling a worker above a cap and per-batch reopening in the indexer come with tasks 4 and 8, measured against the memory row.
7. **One editor worker** will own form and annotation state (tasks 9–12); render workers see edits through the overlay. Recorded with ADR-0041.

## Alternatives considered

| Option | Why not |
|---|---|
| PDFium in the host behind a mutex | No parallelism; a PDFium crash takes the app down (§5.2, ADR-0003) |
| A separate worker binary bundled as a sidecar | Spawn measured slower than the app's own image (below), and a second executable to sign, bundle and keep in version lockstep |
| Tiles through shared memory (a file mapping per worker, or WebView2's SharedBuffer handle duplicated into the worker) | Pipes measured at 2.0–2.4 ms per 512 px tile round trip including the 1 MiB copy, inside the per-tile budget; shared memory adds handle plumbing per platform for little. Revisit if a profile shows the copy |
| JSON or a serialization crate for the protocol | Pixels can't go through JSON (§5.6); a hand-written decoder is small, fuzzable and has no dependency |
| `pdfium-render`'s thread-safe wrapper | ADR-0035 |

## Consequences

- A PDFium crash, hang or panic costs one worker restart (spawn + handshake + reopen, about 25–40 ms) and a re-rendered tile.
- The protocol's decoder is a parser of untrusted input: it gets a fuzz target (task 16).
- Each worker parses the document independently: per-worker PDFium memory (8–9 MB idle with one document) and per-worker page-tree walks. The page index walk (ADR-0037) runs in one worker; others pay their own walk on first far jump, from a warm file cache.
- The worker count ignores SMT, so on a 4-core/8-thread machine one core is left to the UI, the host and the Markdown/TXT engines.

## Evidence

Raw dev machine (Core Ultra 7 258V, 32 GB, NVMe), 2026-10-23, `crates/pdf/examples/poolbench.rs` on the 5,000-page fixture:

| Measurement | `quire.exe --pdf-worker` | `quire-pdf-worker.exe` |
|---|---|---|
| Spawn + handshake + open, fresh pool (5 runs, ×3) | 22–30 ms (first run after the build 479 ms: the 55 MB image read cold) | 30–45 ms |
| 512 px tile at 800 %, round trip (median of 21) | 1.98–2.10 ms | 2.09–2.28 ms |
| Worker private bytes, one document open | 8.9 MB | 8.4 MB |

- Per-request overhead (a 1 × 1 px render): 0.03–0.07 ms.
- Page 1 at 100 % (1190 × 1684 px, 8 MB) as one bitmap: 20.9 ms median (book), 28.5 ms (scanned).
- A 1920 × 1080 px viewport at 800 % (12 tiles of 512 px): 26–30 ms on one worker, 11.6–13.7 ms on three (book); scanned 35–38 / 19–20 ms. The first run on a fresh pool was 65–73 ms (pages loaded cold).
- Kept loaded pages matter: a scanned page's 800 % tile took 21 ms in the spike (page reloaded, JPEG decoded per tile) and 2.2 ms here (four recently used pages kept loaded per document).
- `crates/pdf/tests/pool.rs` covers rendering, a broken xref (repaired), a crash and its recovery, 30 tiles from 6 threads on 3 workers, and an overlay; CI runs it on Windows, macOS and Linux against the pinned library.
