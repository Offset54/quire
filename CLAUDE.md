# quire: working notes

- The spec is `docs/MASTER_PROMPT.md`, the verbatim brief. `[PRODUCT_NAME]` is the working codename `quire` (bundle id `dev.quire.app`) until a real name is chosen before M7.
- Re-read the brief sections for the current milestone before starting on it. Its §2 rules govern all work here: measure, don't assert; no fake completeness; preserve what you don't understand; one ADR per decision; keep `docs/PROGRESS.md`, `docs/RISKS.md` and `docs/adr/` current.
- Phase gate: no application code until the user approves the §13 plan. After that, milestones go in §11 order, and each DoD is reported before the next milestone starts.
- Windows first. The Rust toolchain is pinned in `rust-toolchain.toml`; JS uses pnpm.
- Line endings are LF everywhere (`.gitattributes`). Files under `testdata/`, `tests/data/` and `fuzz/regressions/` are byte-exact test inputs: never normalize, reformat or re-encode them.
- Generated fixtures and fetched corpora go in `/.cache/`, which is ignored. Never commit them.
