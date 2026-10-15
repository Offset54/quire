// Prototype-pollution hardening for the app's document (ADR-0033): what
// Tauri's `freezePrototype` did, but only here. Tauri's version also runs
// in subframes on Windows, where it breaks Mermaid in its sandboxed frame.
// Imported first by main.tsx, so it runs before any dependency's code.
Object.freeze(Object.prototype);
