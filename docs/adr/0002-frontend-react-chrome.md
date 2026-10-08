# 0002. Frontend: React for chrome, imperative surfaces

- Status: Accepted
- Date: 2026-10-08

## Decision

- React 19 (MIT) with strict TypeScript 6.0 (Apache-2.0) and Vite 8 (MIT), for app chrome only.
- Document surfaces, the virtual scroller, the tile cache, input and the performance HUD are imperative TypeScript, outside React's render cycle (`DocumentSurface`, `perf/hud.ts`).
- UI strings go through ICU MessageFormat (ADR-0018).
- TypeScript is pinned to 6.0.x because typescript-eslint 8.71 supports `<6.1`. TypeScript 7 waits for it.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| Solid / Svelte | MIT | Viable, but React's ecosystem (accessibility, i18n, testing) is broader. Hot paths don't use the framework, so its runtime cost doesn't matter there. |
| Vanilla TS everywhere | — | The chrome (tabs, rails, inspector, command palette) benefits from components |
| React for surfaces too | MIT | Reconciliation per frame would eat the frame budget |

## Evidence

Scrolling the 10M-row list in ref-sim: compose work is 0.9 ms p95 and 2.7 ms max per frame, with no long tasks (ADR-0005, `bench/results/m0`).
