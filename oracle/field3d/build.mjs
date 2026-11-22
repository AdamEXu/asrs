// Builds the 3D reference harness. It lives outside src/ so that it does not feed the generator
// hash of the committed data vectors.
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const [upstream, harness, out] = process.argv.slice(2).map((p) => path.resolve(p));
if (!upstream || !harness || !out) {
  console.error("usage: node build.mjs <upstream dir> <harness dir with node_modules> <out dir>");
  process.exit(2);
}
const here = path.dirname(fileURLToPath(import.meta.url));
const esbuild = createRequire(path.join(harness, "package.json"))("esbuild");

fs.mkdirSync(out, { recursive: true });
await esbuild.build({
  entryPoints: [path.join(here, "main.ts")],
  outfile: path.join(out, "field3dMain.mjs"),
  bundle: true,
  format: "esm",
  platform: "node",
  target: "esnext",
  external: ["electron"],
  logLevel: "warning"
});
await esbuild.build({
  entryPoints: [path.join(here, "page.ts")],
  outfile: path.join(out, "field3dPage.js"),
  bundle: true,
  format: "iife",
  platform: "browser",
  target: "esnext",
  nodePaths: [path.join(upstream, "node_modules")],
  alias: { "@upstream": path.join(upstream, "src") },
  logLevel: "warning"
});
fs.writeFileSync(
  path.join(out, "field3dPage.html"),
  '<!doctype html><html><head><meta charset="utf-8"><style>body{margin:0}</style></head>' +
    '<body><script src="field3dPage.js"></script></body></html>\n'
);
