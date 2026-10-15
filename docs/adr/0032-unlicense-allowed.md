# 0032. Unlicense allowed in shipped dependencies (for Mermaid)

- Status: Accepted (user decision, 2026-10-15)
- Date: 2026-10-15

## Context

§0 allows only permissive licenses in the shipped binary: MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0. ADR-0019 put the list in `deny.toml` (Rust) and `tools/check-js-licenses.mjs` (JS), with additions approved one at a time (Unicode-DFS-2016 on 2026-10-13).

M2 task 7 needs Mermaid (§3.1). Its dependency trees, checked with the JS license gate on 2026-10-15:

- Every Mermaid version pulls `robust-predicates` 3.0.3 (Unlicense) through `d3` → `d3-delaunay` → `delaunator`.
- Mermaid 12.0.0 also pulls `elkjs` 0.9.3 (EPL-2.0, weak copyleft) directly. Mermaid 11.17.2 doesn't.

The Unlicense is a public-domain dedication with a permissive fallback license; it is OSI-approved and asks for nothing (no notice, no source).

Options put to the user: (1) allow Unlicense and pin Mermaid 11.17.2; (2) allow Unlicense and EPL-2.0 and use Mermaid 12; (3) no Mermaid in v1.

## Decision

The user chose (1). `Unlicense` is added to both allowlists (`deny.toml`, `tools/check-js-licenses.mjs`). Mermaid is pinned at 11.17.2. EPL-2.0 stays out, so a Mermaid upgrade that brings `elkjs` back fails the gate and comes back to the user.

## Consequences

- The gate passes with 118 production JS packages.
- Mermaid stays on the 11.x line until its dependency tree changes or the user decides otherwise.
- Any other Unlicense package passes the gate from now on, Rust or JS.
