import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tauri serves the dev build from a fixed port; see src-tauri/tauri.conf.json.
// `--mode bench` bundles the benchmark scenarios (VITE_QUIRE_BENCH); it's set
// here rather than in an .env file, which the repo ignores.
export default defineConfig(({ mode }) => ({
  plugins: [react()],
  clearScreen: false,
  define: {
    "import.meta.env.VITE_QUIRE_BENCH": JSON.stringify(mode === "bench" ? "1" : ""),
  },
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2023",
    sourcemap: true,
  },
}));
