import { defineConfig } from "vite";

// The dev server proxies API calls to the Rust server; in production the Rust
// server serves the built bundle from client/dist directly.
export default defineConfig({
  server: {
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:8787",
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
