import { defineConfig } from "vite";
import path from "path";

export default defineConfig({
  root: "client",
  publicDir: path.resolve(process.cwd(), "client/public"),
  build: {
    outDir: path.resolve(process.cwd(), "dist"),
    emptyOutDir: true,
  },
  server: {
    host: "0.0.0.0",
    port: 3000,
  },
});
