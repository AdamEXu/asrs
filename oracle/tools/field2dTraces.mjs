// Extracts painted Field2d frames from the dom/field-2d vectors into
// oracle/src/canvas/field2dTraces.json, the canvas pixel scenes `scenes.ts` builds from them.
// Run after regenerating the vectors: `node oracle/tools/field2dTraces.mjs`.

import fs from "fs";
import path from "path";
import zlib from "zlib";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const VECTORS = path.join(ROOT, "parity/vectors/dom/field-2d");
const OUT = path.join(ROOT, "oracle/src/canvas/field2dTraces.json");

// [scene name, fixture, painted-frame index within the fixture, dark]. The fixture's theme
// is not on render records, so the pick names it.
const PICKS = [
  ["field2d-trace-objects-light-1x", "objects", 4, false],
  ["field2d-trace-aliasing-light-1x", "aliasing", 2, false],
  ["field2d-trace-edges-light-1x", "edges", 3, false],
  ["field2d-trace-ftc-grid-light-2x", "ftc-grid", 6, false],
  // Samples its image on pixel boundaries.
  ["field2d-trace-ftc-ties-light-2x", "ftc-grid", 3, false],
  ["field2d-trace-satellite-dark-1.5x", "satellite", 3, true]
];

const view = new DataView(new ArrayBuffer(8));
function untree(v) {
  if (v === null || typeof v === "boolean" || typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(untree);
  if ("n" in v) {
    view.setUint32(0, parseInt(v.n.slice(0, 8), 16));
    view.setUint32(4, parseInt(v.n.slice(8), 16));
    return view.getFloat64(0);
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
    .filter((r) => r.f2dr === "render" && r.calls !== undefined);

const out = PICKS.map(([name, fixture, index, dark]) => {
  const record = frames(fixture)[index];
  const cmds = untree(record.calls);
  const [cw, ch] = untree(record.size);
  const [width, height] = untree(record.style).slice(0, 2).map((s) => Number(s.replace(/px$/, "")));
  const dpr = untree(record.dom.o.find(([k]) => k === "dpr")[1]);
  if (width * dpr !== cw || height * dpr !== ch) throw new Error(`${name}: canvas ${cw}x${ch} is not ${width}x${height} at ${dpr}x`);
  const images = {};
  const imageTable = record.dom.o.find(([k]) => k === "images")[1].o;
  for (const [src, size] of imageTable) images[src] = untree(size);
  for (const cmd of cmds) {
    if (cmd[0] === "drawImage" && cmd[1] !== "@heatmap" && !(cmd[1].slice(1) in images)) {
      throw new Error(`${name}: ${cmd[1]} has no recorded size`);
    }
  }
  return { name, dark, dpr, width, height, images, cmds };
});
fs.writeFileSync(OUT, JSON.stringify(out) + "\n");
console.log(`field-2d traces: wrote ${out.length} frames`);
