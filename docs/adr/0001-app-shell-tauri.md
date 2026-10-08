# 0001. App shell: Tauri 2

- Status: Accepted
- Date: 2026-10-08

## Context

§5.1 names Tauri 2 with the system webview as the default shell, to be challenged only with evidence. The shell decides startup cost, the IPC paths available, and binary size.

## Decision

Tauri 2 (MIT/Apache-2.0): tauri 2.12 and tauri-cli 2.12 at adoption.
- The main window is created in code, so navigation, new-window and download guards can be attached (task 4).
- App commands are allowlisted at build time and granted per window through capabilities.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| Electron | MIT (Chromium BSD-3) | Bundles its own Chromium (~150 MB) and uses more memory per window. Startup isn't better: Chromium also starts multiple processes. |
| Qt 6 | LGPL-3 / commercial | Excluded by §0 |
| egui / iced | MIT / Apache-2.0 | Weak IME and accessibility for rich UI, and no web stack for the Markdown preview, KaTeX and Mermaid |
| Native per-OS UI | — | Three UI codebases |
| Bare WebView2 host (no Tauri) | MIT (webview2-com) | Measured in ADR-0022: saves only 30–40 ms at startup, but loses the cross-platform shell and the IPC layer |

## Evidence

- Startup (ADR-0022): Tauri and our code add 30–40 ms over a bare WebView2 host. The rest is WebView2 itself.
- Tile transport (ADR-0004): Tauri's binary paths reach ~150 MB/s. On Windows, the WebView2 SharedBuffer reached through Tauri's `with_webview` reaches ~490 MB/s.
- Gotchas found and handled:
  - Channel message ordering is per invocation (ADR-0004).
  - The Windows SxS manifest parser rejects comments and non-ASCII text (task 4).
  - Non-Windows builds need a PNG icon.

## Consequences

Startup is bounded below by WebView2 (ADR-0022, decision pending). macOS and Linux webviews are unmeasured until hardware is available (risk #5).
