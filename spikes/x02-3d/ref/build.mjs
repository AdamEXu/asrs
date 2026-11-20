import { createRequire } from "module";
import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../../..");
const upstream = path.join(root, "target/oracle/upstream");
const out = process.argv[2] ?? "/Volumes/T9/asrs/target/spikes/s3/ref/www";
const require = createRequire(path.join(root, "target/oracle/harness/package.json"));
const esbuild = require("esbuild");

fs.mkdirSync(out, { recursive: true });
await esbuild.build({
  entryPoints: [path.join(here, "entry.ts")],
  bundle: true,
  format: "esm",
  outfile: path.join(out, "ref.js"),
  alias: { "@upstream": path.join(upstream, "src") },
  nodePaths: [path.join(upstream, "node_modules")],
  logLevel: "warning"
});
for (const f of ["page.html", "main.cjs"]) fs.copyFileSync(path.join(here, f), path.join(out, f));
const scene = {
  robotPath: path.join(upstream, "bundledAssets/Robot_FRCKitBot/model.glb"),
  cameras: {
    field: { position: [0, 6, -12], target: [0, 0.5, 0] },
    robot: { position: [-0.6, 1.1, -0.6], target: [-2, 0.2, 1] }
  },
  pose: { translation: [2, 1, 0], rotation: [Math.cos(Math.PI / 12), 0, 0, Math.sin(Math.PI / 12)] }
};
fs.writeFileSync(path.join(out, "scene.js"), `const SCENE = ${JSON.stringify(scene)};\n`);
fs.writeFileSync(path.join(out, "scene.json"), JSON.stringify(scene, null, 2));
const bundles = path.join(out, "../bundles");
if (!fs.existsSync(bundles)) fs.symlinkSync(path.join(upstream, "bundles"), bundles);
console.log(out);
