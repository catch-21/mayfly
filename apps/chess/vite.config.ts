import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Static site. `--base` and `--outDir` come from the npm scripts (`/` for mainnet,
// `/testnet/` for the testnet flavour), as pubky-explorer does.
export default defineConfig({
  plugins: [react()],
  optimizeDeps: {
    exclude: ["@synonymdev/mayfly", "@synonymdev/mayfly-browser", "@synonymdev/pubky"],
  },
  build: {
    target: "es2022",
  },
});
