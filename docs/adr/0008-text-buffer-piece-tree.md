# 0008. Text buffer: a persistent piece treap over the mapped original and an add buffer

- Status: Accepted
- Date: 2026-10-09

## Context

§6.1 asks for a 2 GB TXT file to open and paint in ≤ 300 ms, index in the background in ≤ 4 s, insert at any offset in ≤ 5 ms, and jump to any line in ≤ 100 ms (≤ 300 ms approximate before indexing finishes). §5.5 forbids copying a whole file into one buffer. Untouched bytes must survive a save byte-for-byte, and search, save and replace-all must run in the background while the user keeps typing (§5.7).

The plan proposed a B-tree of pieces with cached byte and newline sums, over the mapped original plus an append-only add buffer.

## Decision

`quire-text::tree` and `quire-text::buffer`:

1. **Pieces over a buffer table.** A document is a sequence of pieces, each a span of one buffer: buffer 0 is the original (mapped, or transcoded to UTF-8, ADR-0009), buffer 1 the append-only add buffer (every inserted byte is appended once and never moves), and later buffers are derived content (a large replace-all result) or history spill (§5.8).
2. **A persistent treap**, not a B-tree. Nodes are immutable and shared through `Arc`; an edit copies the O(log n) nodes on its path. Each node caches a summary of its subtree: bytes, known line breaks, and bytes whose breaks are still unknown. A snapshot (for search, save, index refresh, undo) is one `Arc` clone. Priorities are a deterministic splitmix sequence; bulk builds assign them so the tree starts balanced.
3. **Line breaks are counted, not stored.** The original's line index (task 3) keeps break counts per 64 KiB chunk, so the index of a 2 GB file is a few hundred KiB. A piece's break count is known once the chunks it spans are indexed and `None` before; line lookups return `None` when a needed piece is unknown, and callers fall back to estimates that the UI labels as such. As the background index completes, `refresh_index` fills in counts without changing the tree's shape.
4. **CRLF.** A break is an LF, or a CR not followed by an LF. When an edit leaves a CR at the end of one piece and an LF at the start of the next, the pair is rejoined into one add-buffer piece so counts stay exact; typing that extends the last piece recounts it rather than summing.
5. **Typing** appends to the add buffer and grows the piece that ends at the caret when it is contiguous there, so a burst of keystrokes stays one piece.
6. **Undo** keeps the removed and inserted pieces of each step (no byte copies), coalesces typing within a second, and is bounded at 10,000 steps.
7. **Replace-all** builds the new document from a snapshot off the document lock: up to 500,000 matches as pieces (untouched spans keep their origin, so they still save raw), beyond that streamed into a new buffer (in memory up to 64 MiB, else a delete-on-close temp file that is then mapped). Either way it is one undo step.

## Evidence

criterion, raw dev machine (Core Ultra 7 258V, AC), `bench/results/m1/criterion-text-2026-10-09.json`. The tree is 8 MiB of text fragmented by 100,000 random inserts (198,802 pieces):

| Operation | Median |
|---|---|
| Insert 1 byte at a random offset | 46 µs |
| Delete 1 byte at a random offset | 13 µs |
| Start of a random line | 2.7 µs |
| Line of a random offset | 2.7 µs |
| Snapshot | 22 ns |

In the app (ref-sim, 2 GiB fixture, `txt-2g`): inserts at random offsets 0.04–0.06 ms p95 (budget 5 ms, effective 3.3 ms); indexed jumps 19–20 ms p95 including the view; the background index completes in 0.58–0.76 s cold (effective 2.4 s); a jump before indexing completes lands on an estimated line in 97–130 ms max (effective 186 ms). Memory beyond the mapped file after opening, indexing, searching and editing: 8–9 MiB (budget 1.5 GB).

Correctness: proptests against a naive model (tree operations, undo as an exact inverse, journal replay); the `piece_tree` fuzz target checks content, line count and line lookups after every random edit against a byte vector (79,417 runs in 10 minutes, no failure; nightly in CI).

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| ropey | MIT | Loads the whole text into memory (violates §5.5 for 2 GB); counts every line eagerly. |
| crop | MIT | Same: an in-memory rope. |
| Gap buffer | — | O(n) moves for edits far apart; no cheap snapshots for background work. |
| Red-black piece tree as in VS Code | — | Mutable, so background search and save need copies or locks; caches only one metric. |
| B-tree of pieces (the plan's proposal) | — | Equal asymptotics; a persistent B-tree needs node splitting and merging on copy-on-write paths. The treap's split and merge are a few dozen lines and were simpler to make persistent and to test. Node fan-out would help cache locality; the numbers above leave two orders of magnitude to the budgets, so it isn't needed. |

## Consequences

- Line numbers can be estimates until the index finishes, and the UI says so (the status bar and go-to-line label estimated counts, with an indexing indicator).
- The add buffer only grows while the document is open; its bytes are registered with the memory governor. A save rebases the document on the new file (one piece), but the add buffer keeps its bytes, since undo steps may refer to them. Compacting it is future work if long editing sessions need it.
- Memory per edit is a handful of tree nodes; a pathological document with millions of pieces (e.g. a replace-all with more than 500,000 matches) is materialized into a new buffer instead.
