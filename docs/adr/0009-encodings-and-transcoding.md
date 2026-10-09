# 0009. Encodings: UTF-8 in place, everything else decoded once in parallel segments with raw copy on save

- Status: Accepted
- Date: 2026-10-09

## Context

§6.1: detect BOM → declared encoding → heuristic, let the user override and re-decode, and support UTF-8, UTF-16 LE/BE and the legacy code pages (CP949/EUC-KR, Shift_JIS, GB18030, Big5, Windows-125x). §2 and the DoD require untouched content to survive a save byte-for-byte, which re-encoding can't guarantee: Shift_JIS has duplicate mappings (NEC and IBM extensions), and malformed bytes decode to U+FFFD.

The editing model (ADR-0008) works on UTF-8. The question is how a non-UTF-8 file becomes UTF-8 without breaking §5.5 (no whole-file copies in one buffer) or the byte-exact save.

## Decision

1. **Detection** (`quire-text::encoding`, first 64 KiB): BOM, then a declared encoding in the first 4 KiB (XML declaration, HTML `meta charset`, or a coding comment in the first two lines: PEP 263, Emacs, Vim `fileencoding=`), then a heuristic: UTF-16 without a BOM from the pattern of NUL bytes, UTF-8 if the sample is valid, otherwise chardetng. Pure ASCII and empty files are UTF-8. NULs or control characters in the decoded sample mark the file as probably binary, and the UI shows a binary-file banner. The status bar shows how the encoding was chosen; the user can reopen with another one.
2. **UTF-8 files are used in place**: the mapping is buffer 0 and nothing is copied.
3. **Other encodings are decoded once to UTF-8** (`quire-text::transcode`), into memory up to 64 MiB, else into a delete-on-close temp file that is then mapped. The raw file is cut into ~64 KiB segments at **clean boundaries**, where a fresh decoder produces exactly what one continuous decoder would:
   - single-byte encodings: anywhere;
   - UTF-16: at an even offset not just after a high surrogate (or a run of them; the fuzzer found that case);
   - multi-byte legacy encodings: just after a byte below 0x30, which is never part of a multi-byte sequence in Shift_JIS, CP949, GB18030, Big5 or EUC-JP;
   - stateful encodings (ISO-2022-JP): no cuts.
   Segments decode in parallel waves and are written in order. The (raw offset, UTF-8 offset) of each segment is kept.
4. **Raw copy on save.** When saving in the file's own encoding with unchanged line endings, every untouched piece of the original is mapped back to its raw byte range (through the segment table, re-decoding at most one segment) and copied as it is. Only edited text is encoded. Saving into a different encoding encodes everything and reports characters the target can't represent; the save fails unless the user agrees to replace them.
5. **Progress and cancel.** Opening reports decode progress (at most every 100 ms) and can be cancelled; a command-line file reports through the startup path.

Dependencies: `encoding_rs` (MIT/Apache-2.0; WHATWG decoders, CP949 as "EUC-KR"), `chardetng` (MIT/Apache-2.0).

## Evidence

Opening generated 512 MiB files (`transcodebench`, deterministic, `bench/results/m1/transcode-2026-10-09.json`), ref-sim pinning to the 4 P-cores, 4 decode threads, cold = file purged first:

| Encoding | UTF-8 out | Cold (3 runs) | Warm |
|---|---|---|---|
| windows-1252 | 607 MB | 0.96–1.17 s (460–561 MB/s) | 0.72 s (747 MB/s) |
| CP949 | 687 MB | 1.32–1.39 s (385–407 MB/s) | 0.84 s (639 MB/s) |
| Shift_JIS | 748 MB | 1.47–1.52 s (354–366 MB/s) | 1.00 s (538 MB/s) |
| UTF-16LE | 342 MB | 1.00–1.15 s (466–538 MB/s) | 0.57 s (946 MB/s) |

So a 2 GB legacy file takes roughly 4–6 s to open cold on the reference profile before its first paint. §4 has no budget for legacy encodings (its 2 GB row is a UTF-8 file, which opens in about 150 ms), but it is far from the UTF-8 experience; see Consequences.

Correctness: byte-exact save round trips across every supported encoding, BOMs and line endings (`file::tests`); segmented decoding equals one decoder on Japanese, Korean, Chinese and Cyrillic samples and on malformed input (`transcode::tests`). The `encoding` fuzz target checks detection on arbitrary bytes, segments cut at every clean boundary against one decoder, the parallel pipeline, and monotonic raw offsets: it found the UTF-16 high-surrogate-run bug within seconds (fixed, with a regression input), then ran 662,665 inputs in 10 minutes without failure.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| Decode on the fly per view request, never materialize UTF-8 | — | Every offset in the piece tree, the line index and search would need an encoding-aware mapping; search over a legacy encoding would need per-encoding automata. |
| Re-encode the whole document on save | — | Not byte-exact (Shift_JIS duplicates, malformed bytes, ISO-2022-JP escape choices). |
| ICU charset detection (ICU4C) | Unicode-3.0 | C++ build dependency; chardetng is what Firefox ships for the same job. |
| uchardet | MPL-1.1 / GPL-2.0 / LGPL-2.1 | MPL-1.1 isn't on the allowlist (MPL-2.0 is); a C++ build dependency besides. |

## Consequences

- Opening a large legacy file costs a full decode before the first paint (≈ 4–6 s for 2 GB on the reference profile), with a progress banner and Cancel. Two improvements are possible if that matters: overlap decoding with writing the temp file (they currently alternate per wave), and show the first segments while the rest decode (progressive transcoding). Both are recorded in the M1 report as follow-ups.
- Temporary UTF-8 copies of legacy files use disk space next to the app's data (deleted on close, even after a crash, by delete-on-close).
- Edits in a legacy file are saved in that encoding; characters it can't hold are reported before anything is written.
- Raw copy depends on the original being unchanged on disk; external change detection (task 9) turns it off when another program modified the file in place.
