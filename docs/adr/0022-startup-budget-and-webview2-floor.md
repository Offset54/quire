# 0022. Startup budget vs. the WebView2 floor

- Status: Accepted (decided by the product owner, 2026-10-08)
- Date: 2026-10-08

## Context

§4: app cold start to interactive ≤ 1.0 s, warm ≤ 0.4 s, on the reference machine. M0's DoD includes "the empty app starts within budget". In the ref-sim profile (ADR-0021), a result passes if `value × factor ≤ budget`. For warm start the factor is `D_cpu = 1.5`, giving an effective 267 ms here. For cold start it's `max(D_cpu, D_io) = 1.74`, giving an effective 575 ms.

## Measurements

All times run from OS process creation to the first painted frame, on the dev machine, AC power, 2026-10-08.

**The quire app** (Tauri 2, ref-sim pinned, `tools/bench/refsim.ps1`):

| | p50 | Effective budget | Result |
|---|---|---|---|
| Warm | 480 ms | 267 ms | Fail (1.8×) |
| Cold | 936 ms | 575 ms | Fail (1.6×) |

**Where the time goes**, traced with `QUIRE_TRACE`, warm:
- About 12 ms to reach `main`.
- About 6 ms of our initialization.
- **370–615 ms creating the window and WebView2.**
- About 60 ms from navigation start to first frame. That includes the React bundle; the HTML response ends at about 15 ms and DOMContentLoaded at about 52 ms.

**Bare WebView2 floor** (`spikes/webview2-floor`: no Tauri, no wry, a trivial page, its own user data folder; p50 of 8 runs):

| Stage, ms from process creation | Pinned to P-cores | Unpinned |
|---|---|---|
| `main` | 19 | 17 |
| Window created | 36 | 30 |
| Environment created | 43 | 41 |
| Controller created | 328 | 310 |
| **First frame** | **449** | **415** |

Other experiments:

| Variant | First frame p50 |
|---|---|
| Unpinned (8 cores) quire app | 488 ms: core count doesn't help, the time is latency-bound |
| Extra WebView2 flags (no background networking, component updates or Translate) | 510 ms: no gain |
| **Bare WebView2 with a resident browser process** (another instance already running with the same user data folder) | **137 ms** (min 132, max 154); controller 84 ms |

## Analysis

- Tauri and our code add only about 30–40 ms over bare WebView2. The rest is WebView2 starting its browser and renderer processes.
- WebView2 alone needs about 450 ms on this machine, which is faster than the reference machine. Under ref-sim that's about 675 ms against a 267 ms effective budget. **The warm-start budget can't be met with a WebView2 stack that starts its own browser process**, whatever the host code does. Cold start is the same story.
- The only measured way under budget is a WebView2 browser process that's already running: 137 ms × 1.5 ≈ 206 ms ≤ 267 ms.

## Options

| # | Option | Effect on §4 | Cost and risks |
|---|---|---|---|
| 1 | **Opt-in "fast start" resident process.** A small background process, started at login or kept after the last window closes, holds the WebView2 environment. Launching attaches to it. | Warm start ≈ 140 ms (measured on the bare spike; the Tauri integration still needs measuring). Meets the budget **while the resident process runs**. | About 100–200 MB RAM held at idle (to be measured); one more process; needs a setting and an installer option. Off by default, the first launch after login still takes ~450 ms. |
| 2 | **Redefine "interactive" as a native first frame.** Rust paints the window (background and title) immediately while WebView2 starts. Real input works once WebView2 is ready. | First pixels in about 40 ms. Real interactivity unchanged (about 480 ms). | This changes what the budget means, so it's a budget relaxation and needs your explicit approval. |
| 3 | **Revise the budget to the measured floor**, e.g. warm ≤ 0.7 s and cold ≤ 1.4 s on the reference machine. | Honest budget for this stack | Doesn't make the app faster |
| 4 | **Overlap document work with WebView2 startup.** When launched with a file, the core starts parsing and rendering page 1 in parallel with WebView2, so time to first page ≈ max(WebView2, render) rather than the sum. | Doesn't change the empty-app budget, but improves the case users actually notice | Moderate work in M1/M3; worth doing under any option |
| 5 | Change the stack (non-webview UI) | Could meet the budget natively | Contradicts §5.1, would mean rewriting the UI layer, and loses the web stack for Markdown preview, KaTeX and Mermaid. Not recommended. |

**Recommendation: 3 + 4 now, and 1 as an opt-in setting in M7.**
- Revise the startup budget to what the mandated stack can do (Option 3), and record that here.
- Overlap document opening with WebView2 startup (Option 4, M1 onwards).
- Offer "fast start" as an opt-in (Option 1, M7, with measured memory cost), the way Office does.

Option 2 on its own would give the right number for the wrong reason.

## Decision

The product owner chose Options 3 + 4 + 1:

1. **Budget revised to the stack's measured floor** (Option 3). On the reference machine:

   | Start | Was | Now | Effective in ref-sim |
   |---|---|---|---|
   | Warm | ≤ 0.4 s | **≤ 0.75 s** | 500 ms (÷ 1.5) |
   | Cold | ≤ 1.0 s | **≤ 1.7 s** | ≈ 980–1,020 ms (÷ max(D_cpu, D_io)) |

   These replace the §4 "App cold start" row; `bench/budgets.json` records the change. The "e.g. 0.7 s / 1.4 s" in Option 3 was too low once derating applies: the measured warm p50 of 480 ms × 1.5 is 720 ms.
2. **Overlap document opening with WebView2 startup** (Option 4), from M1. When the app is launched with a file, the core starts opening and rendering it before the webview is ready. The M1 TXT provider is the first to do this.
3. **Opt-in "fast start" resident process** (Option 1), in M7: a setting plus an installer option, off by default, with its idle memory cost measured and shown to the user.

Option 2 (counting a natively painted frame as "interactive") is not adopted.

**Verification under the revised budgets** (`bench/results/m0/refsim-2026-10-08-215918.json`): warm 457 ms p50, effective budget 500 ms: pass. The cold samples are unreliable on a machine with other Edge/WebView2 processes running, because their mapped files can't be purged. That run's cold p50 was 441 ms (effectively warm). The colder earlier run, 936 ms p50, also passes its effective budget of about 977 ms. Every scroll budget passes.
