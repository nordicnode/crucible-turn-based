// Assembles terrain-preview-dist.html: a single self-contained page with the
// esbuild bundle inlined (the preview server serves only one file).
import { readFileSync, writeFileSync } from "node:fs";

const shell = readFileSync(new URL("./terrain-preview.html", import.meta.url), "utf8");
const bundle = readFileSync(new URL("./terrain-preview-bundle.js", import.meta.url), "utf8");

const out = shell.replace(
  '<script type="module" src="./terrain-preview-entry.ts"></script>',
  `<script>${bundle}</script>`,
);
writeFileSync(new URL("./terrain-preview-dist.html", import.meta.url), out);
console.log(`wrote terrain-preview-dist.html (${out.length} bytes)`);
