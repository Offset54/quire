import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { port: 5174, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "es2023" },
});
