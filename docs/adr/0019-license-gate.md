# 0019. License gate

- Status: Accepted
- Date: 2026-10-08

## Decision

- **Rust:** `cargo deny check` with `deny.toml`. It checks every target platform and all features, denies wildcards and unknown registries or git sources, and allows only these licenses:
  - from §0: MIT, Apache-2.0, BSD-2/3, ISC, Zlib, MPL-2.0;
  - approved on 2026-10-08: Unicode-3.0, BSL-1.0, CC0-1.0, 0BSD;
  - strictly more permissive than an already-allowed license: MIT-0 and Apache-2.0 WITH LLVM-exception.
- OFL-1.1 (bundled fonts) and FTL (FreeType inside PDFium) are approved for those uses only. Neither ships as a crate.
- **JS:** `tools/check-js-licenses.mjs` gates the shipped (production) packages against the same list, using pnpm's license listing. Dev-only tools don't ship.
- Both run in CI.

## Exceptions

- `RUSTSEC-2024-0370`: `proc-macro-error` is unmaintained. It's compile-time only, pulled in by the gtk-rs 0.18 stack that Tauri uses on Linux. Revisit when Tauri moves to gtk4-rs.

## Evidence

The gate caught `jpeg-encoder` (IJG license) during task 11; we used the `image` crate's MIT/Apache encoders instead.
