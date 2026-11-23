// Extracts painted Swerve and Joysticks frames from the dom/swerve and dom/joysticks vectors into
// oracle/src/canvas/r07Traces.json, the canvas pixel scenes `scenes.ts` builds from them.
// Run after regenerating the vectors: `node oracle/tools/r07Traces.mjs`.

import fs from "fs";
import path from "path";
import zlib from "zlib";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const VECTORS = path.join(ROOT, "parity/vectors/dom");
const OUT = path.join(ROOT, "oracle/src/canvas/r07Traces.json");

const WIDE = { "/Users/pilot/Assets/Joystick_Wide/image.png": [1250, 800] };
const TALL = { "bundledAssets/Joystick_Tall/image.png": [333, 1000] };

// [scene name, fixture, painted-frame index within the fixture, {image path: natural size}].
// Chosen among frames that pass the GPU-distance rule of the canvas gate. In a sample of 85 frames
// 18 fail that rule (17 of them Joysticks) and all pass the CPU-canvas gate: #257.
const PICKS = [
  ["swerve-trace-typical-light-1x", "swerve/basic", 5, {}],
  ["swerve-trace-many-light-1x", "swerve/many", 13, {}],
  ["swerve-trace-typical-light-1.5x", "swerve/dpr15", 1, {}],
  ["swerve-trace-typical-dark-2x", "swerve/dpr2-dark", 1, {}],
  ["joysticks-trace-empty-light-1x", "joysticks/generic", 1, {}],
  ["joysticks-trace-generic-light-1x", "joysticks/generic", 3, {}],
  ["joysticks-trace-layout-light-1x", "joysticks/layouts", 1, WIDE],
  ["joysticks-trace-pair-light-1.5x", "joysticks/dpr15", 1, WIDE],
  ["joysticks-trace-layout-dark-1x", "joysticks/many", 40, WIDE],
  ["joysticks-trace-tall-dark-1x", "joysticks/many", 42, TALL]
];

const view = new DataView(new ArrayBuffer(8));
function untree(v) {
  if (v === null || typeof v === "boolean" || typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(untree);
  if ("n" in v) {
    view.setUint32(0, parseInt(v.n.slice(0, 8), 16));
    view.setUint32(4, parseInt(v.n.slice(8), 16));
    const x = view.getFloat64(0);
    if (!Number.isFinite(x)) throw new Error("JSON cannot carry a non-finite argument");
    return x;
  }
  throw new Error(`unexpected value ${JSON.stringify(v)}`);
}

const frames = (fixture) =>
  zlib
    .zstdDecompressSync(fs.readFileSync(path.join(VECTORS, `${fixture}.jsonl.zst`)))
    .toString()
    .split("\n")
    .filter((line) => line !== "")
    .map((line) => JSON.parse(line))
    .filter((r) => r.r07 === "render" && r.calls !== undefined);

const out = PICKS.map(([name, fixture, index, images]) => {
  const record = frames(fixture)[index];
  if (record === undefined) throw new Error(`${name}: no frame ${index}`);
  const cmds = untree(record.calls);
  const [cw, ch] = untree(record.size);
  const dpr = untree(record.dpr);
  const [width, height] = [cw / dpr, ch / dpr];
  for (const cmd of cmds) {
    if (cmd[0] === "drawImage" && !(cmd[1].slice(1) in images)) throw new Error(`${name}: ${cmd[1]} has no size`);
  }
  for (const src of Object.keys(images)) {
    if (!cmds.some((cmd) => cmd[0] === "drawImage" && cmd[1] === "@" + src)) throw new Error(`${name}: ${src} is not drawn`);
  }
  return { name, dark: record.dark, dpr, width, height, images, cmds };
});
fs.writeFileSync(OUT, JSON.stringify(out) + "\n");
for (const t of out) console.log(t.name, t.width, t.height, t.dpr, t.dark, t.cmds.length, t.cmds.filter((c) => c[0] === "fillText").length);
