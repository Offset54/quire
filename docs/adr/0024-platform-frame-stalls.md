# 0024. Platform frame stalls: judge budgets on our own frame work, report the stalls

- Status: Accepted (user decision, 2026-10-12)
- Date: 2026-10-12

## Context

RISKS #12: on the dev machine (Arc 140V, 144 Hz panel with a 48 Hz minimum), once nothing on screen changes for about 0.6 s, the next frames are delayed 90–340 ms. A Chromium trace of one stall shows the GPU process blocked in `DXGISwapChainImageBacking::Present` (334 ms) while our renderer was idle. Plain Microsoft Edge with a trivial page shows the same (`bench/results/m1/edge-idle-probe-2026-10-09.json`). In ref-sim run 2 these stalls failed three TXT rows (2 GB first paint, steady-scroll max frame, wrapped 300 MB scroll max frame) that our own work meets.

The options were in the M1 report: (1) keep every budget and judge the rows on our own frame work, reporting the stalls separately; (2) ship a compositor keep-alive while a document is focused (removes the stalls in Edge, at 42–48 % of a CPU core while idle); (3) have the user change a display power setting so the cause can be confirmed. The user chose (1).

## Decision

Budgets are unchanged. The TXT rows that end on a presented frame are judged on **own** values: the measured time minus the time the platform did not deliver a frame. The in-app scenarios (`apps/desktop/src/bench/txt.ts`) record the raw value, the stalls and the own value; `bench/budgets.json` points those rows at the own values. `tools/bench/refsim-txt.ps1` prints the raw values and the stalls next to the budget table, and stores them under `platformStalls` in the results.

**What counts as a platform stall.** A span in which the page waits for a frame it requested, from the request to the frame's rAF: the scroll loop's frame intervals, the first paint's wait from the first rows' arrival (`txt-first-rows-ready`) to the frame that drew them (`txt-first-content`), and the last frame wait of each jump. The page renders as soon as a frame is delivered between tasks, and while a frame is pending, every task falls inside a long animation frame once the wait passes 50 ms. So the stall is the span, less all our work (scripts and rendering) in the long animation frames that overlap it, less one 60 Hz frame (16.7 ms). If a task blocked in the span (`blockingDuration` > 0), nothing is subtracted. Spans that also wait for our host, such as a jump's IPC, are never reduced.

For the first paint, the stalls inside long frames over the whole measurement (the idle time before `renderStart`, less our scripts, less one frame) are also computed; the larger of the two values is used, never the sum.

**Where it applies:** the 2 GB and 300 MB first paints, the jumps before and after indexing, and the scrolling rows (steady, fling, wrapped 300 MB): p99 and max frame interval. Typing latency and the core-timed rows are judged raw. `launch-2g` reports both values; it has no row of its own.

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

## Revision 2 (2026-10-13, after ref-sim run 5)

Run 5 (`bench/results/m1/refsim-txt-2026-10-13-170828.json`) failed two rows that the rule above did not fully cover. A long animation frame starts at its first task, so the idle time before it appears in no entry. A 509.9 ms wrapped-scroll interval held a 427 ms long frame with 0.8 ms of our work and about 83 ms of idle time before it, which was charged to us (100.4 ms). An early jump of 540 ms spent about 500 ms waiting for its last frame after 41 ms of IPC and view work, with no long frame recorded. The rule is now the same for every pure frame wait: the span, less all our work in the long frames that overlap it, less one frame. Against the recorded frames it is slightly stricter than revision 1 (run 2's 244 ms interval keeps 28 ms as ours instead of 21 ms; run 4's 404 ms interval keeps 37 ms). Jumps are now reduced only by their last frame wait, never by IPC time. The suite was rerun after the change.
