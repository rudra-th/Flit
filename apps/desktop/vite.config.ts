import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The Tauri shell serves this from `dist/` and knows the dev server's port.
// `strictPort` keeps the two in agreement instead of silently moving to 1421.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // The webviews Tauri 2 supports on Windows and Linux are newer than this.
    target: "chrome110",
    sourcemap: false,
  },
});
