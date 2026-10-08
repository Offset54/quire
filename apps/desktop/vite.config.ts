import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tauri serves the dev build from a fixed port; see src-tauri/tauri.conf.json.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2023",
    sourcemap: true,
  },
});
