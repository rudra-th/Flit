import { defineConfig } from "vite";

// The site is static and shipped as files: no runtime dependencies, no
// third-party requests, and one output folder GitHub Pages can serve as-is.
export default defineConfig({
  root: ".",
  base: "/",
  build: {
    outDir: "dist",
    assetsInlineLimit: 0,
    target: "es2020",
  },
});
