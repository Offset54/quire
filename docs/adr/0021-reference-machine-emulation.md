# 0021. Reference-machine emulation on the dev machine ("ref-sim")

- Status: Accepted (approach); the factors get measured in M0 task 12
- Date: 2026-10-08

## Context

§4 budgets are defined on a reference machine (4-core/8-thread 2021-era laptop CPU, 16 GB, integrated GPU, NVMe, 1920×1080 at 150 %). The only machine available is faster: Core Ultra 7 258V (4 P-cores + 4 LP E-cores, no SMT), 32 GB, Arc 140V, PCIe 4 NVMe, 2560×1600 at 150 %. §2 forbids asserting results we haven't measured and forbids quietly relaxing budgets. Raw numbers from this machine would overstate performance.

Public Geekbench 6 results put the 258V at about 1.43× the i5-1135G7 (a typical 4c/8t 2021 laptop CPU) in single-core (2757 vs 1924) and about 1.95× in multi-core (10878 vs 5582). These figures are user-submitted and vary between runs, so they're only a starting point.

## Decision

Benchmarks that count against §4 run in a **ref-sim profile** on this machine. Each part of the reference machine is either reproduced exactly, approximated pessimistically, or covered by a stricter pass threshold. Nothing in the profile changes system settings; it's all per process, so the harness can apply it automatically.

| Reference property | How ref-sim handles it | Fidelity |
|---|---|---|
| 1920×1080 at 150 % | The benchmark window is 1280×720 logical px. This display is also 150 %, so the app renders exactly 1920×1080 device pixels and the same tile counts. | Exact |
| 4 cores / 8 threads | The app and its worker processes run in a Windows Job Object pinned to the 4 P-cores (found through CPU-set efficiency classes). The app is told the machine has 4 physical / 8 logical cores, so thread and worker pools size as they would on the reference. Running 8 threads on 4 cores is a harsher oversubscription than SMT, so this errs pessimistic. | Pessimistic |
| Slower CPU cores | **Derated pass threshold.** A ref-sim result passes only if `measured × D_cpu ≤ budget`. `D_cpu` starts at 1.5 (the Geekbench 6 single-core ratio rounded up). We use the single-core ratio for multi-threaded work too, because the core pinning already removes this machine's extra cores. | Conservative estimate |
| NVMe of the era | I/O-bound scenarios use `D_io = measured sequential read ÷ 2.0 GB/s` (a conservative PCIe 3 laptop drive). Mixed scenarios use `max(D_cpu, D_io)`. Each scenario in the harness is tagged `cpu`, `io` or `mixed`. | Conservative estimate |
| Cold OS file cache | Before each cold run, the fixture's cached pages are purged. First we test whether opening the file with `FILE_FLAG_NO_BUFFERING` evicts its pages (no admin needed). If it doesn't, an elevated helper purges the standby list. Either way, the harness verifies coldness by checking first-read throughput against the drive's measured raw speed. | Exact, once verified |
| 16 GB RAM | A ballast process commits and periodically touches (installed RAM − 16 GB). The harness checks available memory before each run. The same tool simulates 8 GB for the §10 low-memory QA. | Close |
| Integrated GPU (Iris Xe class) | Not emulated. Frame-time scenarios run twice: on the GPU, which is the gated result, and with WebView2 software compositing (set per process with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--disable-gpu`), which is reported as a pessimistic bound. If software compositing misses 60 fps, that's raised as a risk with data. | Bounded, not exact |
| Power state | Runs are refused on battery (Lunar Lake throttles hard there). The power source and Windows power mode are recorded with each run. | Controlled |

**Calibration.** The harness includes `quire-calibrate`, a fixed kernel suite that mirrors our hot paths: `memchr` newline counting, regex scanning, DEFLATE inflate, shaping-like table lookups, tiny-skia fills, and piece-tree style allocation churn. It reports per-kernel scores in ref-sim. If anyone ever runs it on a real reference-class laptop, the measured ratio replaces the Geekbench-derived `D_cpu` and the profile version is bumped.

**Labeling.** Every result JSON records `profile: "ref-sim-v1"`, all of the factors above, and the machine spec. `docs/PROGRESS.md` reports these as "ref-sim", never as "reference".

## Alternatives considered

| Option | Why not |
|---|---|
| Report raw dev-machine numbers | Overstates performance; violates §2. |
| Cap CPU frequency through the power plan's maximum processor state | Closest emulation of per-core speed, but it changes a system-wide setting, can't be applied automatically per run, and Lunar Lake's firmware doesn't map the percentage linearly. Kept only as an optional manual cross-check that you'd run yourself. |
| Job Object hard CPU-rate cap | Throttles by suspending threads for whole scheduling intervals. That adds stalls of tens of milliseconds, which would corrupt every latency and p95 metric. |
| A VM sized like the reference | Hypervisor overhead and a virtual GPU make frame times meaningless. Disk I/O goes through a virtual controller. |
| Buy or borrow a reference laptop | Still the gold standard. Ref-sim doesn't block this; it can be added later as a calibration point. |

## Consequences

- Effective budgets on this machine are stricter than §4. For example, typing p95 has to be ≤ 10.7 ms (16 ÷ 1.5) and TXT first paint ≤ 200 ms. Budgets are never relaxed; if one looks unreachable even before derating, we stop and bring data (§2).
- GPU-bound results are the remaining blind spot (risk #9).
- M0 task 12 grows by 2 days for the job object, ballast, calibration suite and power check.

## Evidence

- Geekbench 6 results: 258V [2757 / 10878](https://browser.geekbench.com/v6/cpu/17395602); laptop review databases list its single-core score at 2749–2836. For the i5-1135G7, individual well-cooled results are about 1924 / 5582.
- The i5-1135G7 [processor chart](https://browser.geekbench.com/processors/intel-core-i5-1135g7) average is lower (1395 / 3894). It includes thin-and-light laptops running at low power limits, and it would imply `D_cpu ≈ 2.0`. We start at 1.5, which models a well-cooled reference machine. If you'd rather model a throttled one, set `D_cpu = 2.0` in the profile; it's a single value.
- The `D_cpu`, `D_io` and cold-cache verification measurements will be added in M0 task 12.
