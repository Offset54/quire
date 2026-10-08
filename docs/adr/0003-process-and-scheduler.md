# 0003. Process and scheduling model

- Status: Accepted
- Date: 2026-10-08

## Decision

- The core runs in the app process. Native code that's crash-prone or not thread-safe (PDFium, soffice) runs in child processes (M3, M6).
- `quire-core::scheduler` is a custom priority scheduler on std threads:
  - five priorities, cancel tokens, per-view generations with stale-job dropping, and keyed coalescing (priority upgrade, generation refresh, shared results, cancellation once every requester withdraws);
  - one worker reserved for Interaction/Visible work, and panic isolation.
- The worker count is logical CPUs minus one. `QUIRE_CPU_LOGICAL` overrides it for ref-sim (ADR-0021).
- rayon (MIT/Apache-2.0) is allowed later for data-parallel kernels inside a job.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| tokio for everything | MIT | Built for I/O concurrency, not CPU-bound priority work. No priorities or coalescing. |
| rayon thread pool only | MIT/Apache-2.0 | No priorities, cancellation or generations |
| PDFium in-process behind a mutex | — | No parallelism and no crash isolation (§5.2) |

## Evidence

Criterion, dev machine:
- Hand-off of an Interaction job: 5.7 µs.
- 10k mixed-priority no-op jobs: 16.7 ms (≈ 600k jobs/s).
- Coalescing hit: 130 ns.

The scroll benchmark ran ~5,600 tile jobs with ~1,400 dropped as stale and no panics. Twelve unit tests and a model-based property test cover ordering and coalescing.
