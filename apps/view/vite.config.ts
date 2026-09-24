import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `base` is the path the site is served under: `/` locally, `/<repo>/` on GitHub Pages,
// `/testnet/` for the testnet flavour. Set through VITE_BASE or `vite build --base`.
export default defineConfig({
  base: process.env.VITE_BASE ?? "/",
  plugins: [react()],
  optimizeDeps: {
    // A `file:` dependency carrying its own wasm; the dev-server pre-bundler must leave it alone.
    exclude: ["@synonymdev/mayfly", "@synonymdev/mayfly-browser", "@synonymdev/pubky"],
  },
  build: {
    target: "es2022",
    chunkSizeWarningLimit: 4000,
  },
});
