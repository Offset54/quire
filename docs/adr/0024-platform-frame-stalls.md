# 0024. Platform frame stalls: judge budgets on our own frame work, report the stalls

- Status: Accepted (user decision, 2026-10-12)
- Date: 2026-10-12

## Context

RISKS #12: on the dev machine (Arc 140V, 144 Hz panel with a 48 Hz minimum), once nothing on screen changes for about 0.6 s, the next frames are delayed 90–340 ms. A Chromium trace of one stall shows the GPU process blocked in `DXGISwapChainImageBacking::Present` (334 ms) while our renderer was idle. Plain Microsoft Edge with a trivial page shows the same (`bench/results/m1/edge-idle-probe-2026-10-09.json`). In ref-sim run 2 these stalls failed three TXT rows (2 GB first paint, steady-scroll max frame, wrapped 300 MB scroll max frame) that our own work meets.

The options were in the M1 report: (1) keep every budget and judge the rows on our own frame work, reporting the stalls separately; (2) ship a compositor keep-alive while a document is focused (removes the stalls in Edge, at 42–48 % of a CPU core while idle); (3) have the user change a display power setting so the cause can be confirmed. The user chose (1).

## Decision

Budgets are unchanged. The TXT rows that end on a presented frame are judged on **own** values: the measured time minus the time the platform did not deliver a frame. The in-app scenarios (`apps/desktop/src/bench/txt.ts`) record the raw value, the stalls and the own value; `bench/budgets.json` points those rows at the own values. `tools/bench/refsim-txt.ps1` prints the raw values and the stalls next to the budget table, and stores them under `platformStalls` in the results.

**What counts as a platform stall.** From the Long Animation Frames API: in a long frame with `blockingDuration` 0 (no task over 50 ms), the idle time before rendering started, that is, from the frame's start to `renderStart`, less our scripts in that span, minus one 60 Hz frame (16.7 ms). Our rendering work comes after that wait and can't have caused it, so it stays ours. If any task in the frame blocked, the whole frame counts as ours.

**Waits with an idle main thread.** A long animation frame starts with a task. If the main thread runs no task while it waits for a requested frame, no long frame is reported, and the rule above sees nothing. For the first paint, the bench also marks when the first rows arrive and a frame is requested (`txt-first-rows-ready`) and when they are drawn (`txt-first-content`). If no long frame with a blocking task overlaps that window, the wait beyond one 16.7 ms frame counts as a platform stall: the page renders as soon as a frame is delivered between tasks. Where both rules see the same wait, the larger value is used, never the sum. Found on 2026-10-13: two of five cold runs waited 223 and 276 ms for the first frame, and the first rule caught 50 ms of one of them. The jumps and scroll rows use the first rule only.

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

## Revision (2026-10-13, after ref-sim run 4)

The first version also counted a whole long frame as ours when our work in it, scripts plus rendering, reached one frame (16.7 ms). Run 4 (`bench/results/m1/refsim-txt-2026-10-13-170045.json`) showed what that does. A steady-scroll frame of 421 ms had 401 ms of idle main thread before rendering started, then a 16.7 ms rAF callback and rendering, 20 ms in all. Because of those 20 ms of rendering after the wait, the old rule charged the 401 ms wait to us, and the steady-scroll max row failed at 404 ms. Similar frames in the other two runs (140 and 247 ms, with less than 16.7 ms of work) came down to 18–19 ms. The rule now looks only at the idle time before rendering started, which work done after it can't have caused. For run 2's frame the result is the same (223.2 ms of stall). The suite was rerun after the change; the measurement code and tests (`apps/desktop/src/bench/stalls.ts`, `stalls.test.ts`) carry both frames.
