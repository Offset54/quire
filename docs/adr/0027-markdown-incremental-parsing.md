# 0027. Markdown incremental parsing: a top-level block index, re-scanned until it lines up

- Status: Accepted
- Date: 2026-10-13

## Context

§6.2: the text buffer is the source of truth; re-parse only the blocks an edit touches, widened to the enclosing container; track reference definitions globally. §4: first paint of source and preview on 20 MB ≤ 500 ms (≈ 287 ms effective in ref-sim), keystroke → preview update ≤ 50 ms (≈ 33 ms). §5.5: never one whole-file buffer.

The parser spike (ADR-0026) measured a whole-file parse of the 20 MiB fixture at 693 ms (comrak), so neither first paint nor an edit can wait for one. Parsing one top-level block on its own takes about 2 µs.

## Decision

`crates/markdown` (`quire-markdown`) keeps, per document:

**A block index** (`index.rs`): the document's top-level blocks as byte ranges, in order, each with a stable id, its kind and a hash of its bytes.

- *Blocks tile the text.* A block runs from the line where its node starts to the line where the next top-level node starts, blank lines and link reference definitions after it included. Only start lines are used: comrak sets them when a block opens, so they are exact, while its end positions are not always (an unclosed fence in a list item closed by a blank line reports the item's first line as its end).
- *Built in stretches from the start.* A stretch (64 KiB, moved to a line end) is parsed with comrak; all its blocks but the last are final, since text after a block that is followed by another block can't change it. The next stretch starts where the last block starts (the *frontier*). The first viewport needs one or two stretches; the rest is indexed in the background (the scheduler's `Background` priority, per stretch).
- *Edits re-scan from the block before the touched one* (an edit can join the previous block: a setext underline, a lazy continuation line, a removed blank line) in 4 KiB stretches, and stop at the first block that starts, after the edit, exactly where an old block started (shifted by the edit's length change): from there the parser state is the same as before, so everything after it is unchanged. Edits re-parse at most 128 KiB before the index is cut at the new frontier and left to the background (an unclosed fence opened at the top of a long document changes everything after it).
- *Ids* of replaced blocks with the same bytes and kind are reused, so the preview re-renders only blocks whose bytes changed.

Cases found by the property test (incremental index == whole-document scan after random edit sequences; 100,000 cases) and handled:

1. A paragraph that starts with `[` may still turn out to be link reference definitions only, and then has no node at all, once later lines join it (a label or title can span lines). As the last block of a stretch it holds back the block before it too; an edit never starts its re-scan at one.
2. A GFM table can be carved out of an open paragraph (the paragraph's last line becomes the header). Such a table doesn't start from a fresh parser state, so it stays in the paragraph's block.
3. Whether a block starts at the frontier can depend on the line after it (`<div>` interrupts a paragraph, `<paradiv>` doesn't; a definitions-only paragraph). Edits beyond the frontier are ignored only once a blank line separates them from it.
4. Text appended to a block's last line touches that block.
5. Front matter can only start at offset 0, so the re-scan never lines up there.

**Front matter** is detected by the scanner, not comrak (whose rule prefers a CRLF closing line anywhere in the document over an earlier LF one, so it needs a whole-text search): an opening `---` line at the start and the first closing `---` line within 1 MiB. Longer front matter is shown as Markdown; this limit is documented. An edit that adds a closing line to a leading `---` resets the index (rare).

**Analysis** (`analysis.rs`): what each block contributes to the document. comrak doesn't expose the reference definitions it collects, so they are found by a probe: each label written before `]:` is appended as a paragraph `[label]`, the block is parsed without other blocks' definitions, and a probe that comes out as a link was defined in the block, with that link's destination and title. Footnote references only parse next to their definition, so a stub definition is appended for each `[^label]` the block mentions. Headings give the outline. Blocks with none of `]:`, `[^`, `#` or a setext underline skip the parse.

**Document-wide tables** (`tables.rs`), rebuilt when a block that takes part in them changes: definitions (the first in document order wins), footnote numbers (by first reference, as GitHub numbers them; references counted across blocks for their ids), the outline with unique GitHub-style slugs.

**Rendering a block** (`render.rs`) parses it again with comrak's broken-link callback answering from the tables. Every label the callback is asked about is a dependency: when that label's definition changes, the block is re-rendered.

## Evidence

Raw dev machine (not ref-sim), `cargo run --release -p quire-markdown --example indexbench -- .cache/fixtures/document-20mib.md`, through the text engine (mapped file, piece-tree snapshot):

| | Time |
|---|---|
| First stretch (what first paint needs) | 0.03–2.7 ms |
| Whole index, 210,938 blocks, 322 stretches (background) | 593–658 ms; worst stretch 5.4 ms |
| Typing one character, index update, at 0 / 25 / 50 / 75 / 99.9 % | 0.63 / 0.55 / 0.45 / 0.56 / 0.12 ms (re-parses ≈ 4 KB; the rest is moving later blocks' offsets) |

Spec suites through this pipeline (`crates/markdown/tests/spec.rs`): CommonMark 0.31.2 652/652, GFM spec 661/670, extensions 29/30; the same failures as comrak on whole documents plus none, so parsing blocks in isolation costs no conformance (`docs/markdown-spec-exceptions.md`).

## Alternatives considered

| Option | Why not |
|---|---|
| Re-parse the whole document on each edit | 0.7 s per keystroke on 20 MB. |
| Container-level widening by our own block scanner (no parser for boundaries) | A second CommonMark block parser to keep in step with comrak's; the property test compares against comrak's own parse instead. |
| Block ranges from comrak's end positions | Wrong for some lists (above); tiling by start lines is exact. |
| Extract definitions with a second parser (pulldown-cmark exposes them) | Two parsers can disagree on what a definition is; the probe uses comrak's own rules. |
| Collect footnotes at the end of the preview, as GitHub's page does | A synthetic block per footnote after the last block, far from the text. The preview keeps definitions where they are written (numbered, with a back reference); exports and the spec harness produce GitHub's footnotes section. |

## Consequences

- A label defined in two different blocks and used in the later one resolves to the later definition (comrak resolves inside a block first); CommonMark gives the earlier one precedence. No spec example has this shape; listed in `docs/markdown-spec-exceptions.md`.
- Until the background index completes, references to definitions further down render as text and footnotes show their labels instead of numbers; both are patched when the tables change (M2 plan risk 1).
- Moving later blocks' offsets is O(blocks) per edit (≈ 0.5 ms for 211k blocks). If it ever matters, offsets can become relative in a tree.
- `cargo fuzz` targets for the scanner and the incremental index (differential against a whole parse) come with task 12.
