# 0034. Patched tree-sitter grammars: Markdown, Bash and SQL vendored with scanner fixes

- Status: Accepted
- Date: 2026-10-17

## Context

The `highlight` fuzz target (M2 task 12, RISKS #16) crashed on the Linux runner with a SEGV in tree-sitter-md's block scanner (`parse_ordered_list_marker`, `scanner.c:740`), on the 12-byte input "\0\0\0" + "0" + U+86F22 + "</>". tree-sitter hands external scanners `lexer->lookahead` as a Unicode code point; the scanner passes it to C's `isdigit()`, which is defined only for `unsigned char` values and EOF. glibc indexes a 256-entry table with it and reads far outside. The C standard makes it undefined everywhere, and the Windows CRT documents the same (it asserts in debug builds). A hostile document reaches it with a code block fenced as `markdown` (or `md`), in the host process.

An audit of the external scanners of all 21 grammars for `ctype.h` calls on code points: tree-sitter-md 0.5.3 (block scanner, 2 sites) and tree-sitter-bash 0.25.1 (brace expansions, 2 sites, the same `isdigit(lexer->lookahead)`); the other 15 scanners make no such calls. Both crates are the latest on crates.io, and upstream `main` (checked 2026-10-17) still has the calls.

The next fuzz run, symbolized, found the leak the target had reported since 2026-10-16: tree-sitter-sequel 0.3.11's scanner (SQL) dropped the pointer to its dollar-quote tag in `deserialize` without freeing it, a small leak per highlighted SQL block with `$tag$` strings. Upstream `main` has the fix (free first), not yet released.

## Decision

Vendor the three published crates, unchanged except for the fixes, under `third_party/tree-sitter-md`, `third_party/tree-sitter-bash` and `third_party/tree-sitter-sequel`, and use them through `[patch.crates-io]` in the main workspace and in the fuzz workspace. In Markdown and Bash the four calls become `quire_is_ascii_digit(lexer->lookahead)` (`'0'..='9'`); in SQL, `deserialize` frees the held tag first, as upstream does. Each change is marked "quire patch". All three crates stay MIT-licensed; their license and README files stay with them. They are excluded from the workspace, so they build as dependencies with their own settings.

When upstream releases a fix, drop the vendored copy and the patch entry (compare the scanner first). For SQL that is the release after 0.3.11.

## Alternatives

- *Wait for upstream*: the crash is reachable from any document now.
- *Drop Markdown and Bash highlighting*: decision D is yours; fixing four calls costs less.
- *Pre-filter code before highlighting* (map code points past 255): changes what is highlighted, and only works around the inputs we know.
- *Compile the scanners ourselves from the registry sources with a macro override*: fragile across machines and CI.

## Consequences

- About 33 MB of generated C source in the repository (`parser.c` files), a few MB compressed.
- Upgrading any of them now means re-vendoring and re-applying the patch.
- An upstream report and fix are worth sending (not done: that's yours to approve).

## Evidence

- `crates/highlight/src/lib.rs` test `digits_before_code_points_past_255`: the crash input and similar code in Markdown and Bash highlight, and the text survives.
- `fuzz/regressions/highlight/md-isdigit-code-point` and `sql-deserialize-leak`: the CI inputs, replayed by the fuzz workflow under AddressSanitizer and LeakSanitizer.
