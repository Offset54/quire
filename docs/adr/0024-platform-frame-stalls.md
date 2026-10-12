# 0024. Platform frame stalls: judge budgets on our own frame work, report the stalls

- Status: Accepted (user decision, 2026-10-12)
- Date: 2026-10-12

## Context

RISKS #12: on the dev machine (Arc 140V, 144 Hz panel with a 48 Hz minimum), once nothing on screen changes for about 0.6 s, the next frames are delayed 90–340 ms. A Chromium trace of one stall shows the GPU process blocked in `DXGISwapChainImageBacking::Present` (334 ms) while our renderer was idle. Plain Microsoft Edge with a trivial page shows the same (`bench/results/m1/edge-idle-probe-2026-10-09.json`). In ref-sim run 2 these stalls failed three TXT rows (2 GB first paint, steady-scroll max frame, wrapped 300 MB scroll max frame) that our own work meets.

The options were in the M1 report: (1) keep every budget and judge the rows on our own frame work, reporting the stalls separately; (2) ship a compositor keep-alive while a document is focused (removes the stalls in Edge, at 42–48 % of a CPU core while idle); (3) have the user change a display power setting so the cause can be confirmed. The user chose (1).

## Decision

Budgets are unchanged. The TXT rows that end on a presented frame are judged on **own** values: the measured time minus the time the platform did not deliver a frame. The in-app scenarios (`apps/desktop/src/bench/txt.ts`) record the raw value, the stalls and the own value; `bench/budgets.json` points those rows at the own values. `tools/bench/refsim-txt.ps1` prints the raw values and the stalls next to the budget table, and stores them under `platformStalls` in the results.

**What counts as a platform stall.** From the Long Animation Frames API: a long frame with `blockingDuration` 0 (no task over 50 ms) whose own main-thread work (scripts before rendering, plus rendering) is under one 60 Hz frame (16.7 ms). Its stall time is the frame's duration minus that work, minus one 16.7 ms frame. If any task in the frame blocked, or our work took a frame or more, the whole frame counts as ours.

**Where it applies:** the 2 GB and 300 MB first paints, the jumps before and after indexing, and the scrolling rows (steady, fling, wrapped 300 MB): p99 and max frame interval, each interval reduced by the stall time inside it. Typing latency and the core-timed rows are judged raw. `launch-2g` reports both values; it has no row of its own.

## Why the attribution holds

- All these measurements keep a frame requested (the polling and scroll loops wait on `requestAnimationFrame`), so main-thread idle time inside a long frame is time spent waiting for a frame, not for our host. Our host's work happens in other processes and does not hold back `requestAnimationFrame`.
- One 60 Hz frame is always left in, so a stall never takes out time a normal frame would take on the reference display.
- The raw values and every stall (time, frame length, our work, stall time) are stored with the results, so the split can be checked run by run.

## Consequences

- On machines like this one, users see a 0.1–0.3 s hitch when they start scrolling after a pause, in quire as in Edge. This is recorded in RISKS #12 and is not hidden by the budget table.
- A stall the browser attributes to a frame where our own work took a frame or more counts fully against us (conservative).
- If a future WebView2 reports such waits differently, or the API is unavailable, nothing is subtracted and the rows fall back to raw values.
- Option 3 (confirming the cause with a display power setting) stays open; it needs the user to change a system setting.
