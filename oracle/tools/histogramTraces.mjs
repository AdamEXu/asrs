// Extracts painted histogram frames from the dom/histogram vectors into
// oracle/src/canvas/histogramTraces.json, the canvas pixel scenes `scenes.ts` builds from them.
// Run after regenerating the vectors: `node oracle/tools/histogramTraces.mjs`.

import fs from "fs";
import path from "path";
import zlib from "zlib";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const VECTORS = path.join(ROOT, "parity/vectors/dom/histogram");
const OUT = path.join(ROOT, "oracle/src/canvas/histogramTraces.json");

// [scene name, fixture, index among the fixture's records that cleared the canvas, dark].
const PICKS = [
  ["histogram-light-1x", "basic", 3, false],
  ["histogram-dark-1x", "dark", 1, true],
  ["histogram-light-2x", "dpr2", 1, false],
  ["histogram-rotated-2x", "dpr2", 2, false],
  ["histogram-fading-tooltip-2x", "dpr2", 5, false],
  ["histogram-tooltip-dark-1x", "dark", 3, true],
  ["histogram-tooltip-light-1x", "dark", 32, false],
  ["histogram-tooltip-1.5x", "dpr15", 25, false],
  ["histogram-tooltip-2x", "dpr15", 28, false],
  ["histogram-1.5x", "dpr15", 2, false],
  ["histogram-legend-1x", "legend", 6, false],
  ["histogram-rotated-1x", "ranges-narrow", 12, false],
  ["histogram-scientific-1x", "ranges-wide", 24, false],
  ["histogram-hover-edge-1x", "hover-edges", 60, false]
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
  if ("o" in v) return Object.fromEntries(v.o.map(([k, x]) => [k, untree(x)]));
  if ("u" in v) return undefined;
  throw new Error(`unexpected value ${JSON.stringify(v)}`);
}

const out = PICKS.map(([name, fixture, index, dark]) => {
  let transform;
  const frames = [];
  for (const line of zlib
    .zstdDecompressSync(fs.readFileSync(path.join(VECTORS, `${fixture}.jsonl.zst`)))
    .toString()
    .split("\n")) {
    if (line === "") continue;
    const record = JSON.parse(line);
    if (record.calls === undefined) continue;
    const calls = untree(record.calls);
    const set = calls.findLast((c) => c[0] === "setTransform");
    if (set !== undefined) transform = set;
    if (calls.some((c) => c[0] === "clearRect") && calls.some((c) => c[0] === "stroke")) {
      frames.push({ calls, transform, canvas: untree(record.canvas) });
    }
  }
  const { calls, transform: set, canvas } = frames[index];
  const dpr = set[1];
  const [width, height] = canvas.map((v) => v / dpr);
  if (width * dpr !== canvas[0] || height * dpr !== canvas[1]) throw new Error(`${name}: ${canvas} at ${dpr}x`);
  // `ctx.lineDashOffset = undefined` is ignored by a canvas and has no JSON.
  const cmds = [set, ...calls].filter((c) => !(c[0] === "=" && c[2] === undefined) && c[0] !== "setTransform");
  return { name, width, height, dpr, dark, cmds: [set, ...cmds] };
});
fs.writeFileSync(OUT, "[\n" + out.map((t) => JSON.stringify(t)).join(",\n") + "\n]\n");
console.log(out.map((t) => `${t.name}: ${t.width}x${t.height} at ${t.dpr}x, ${t.cmds.length} calls`).join("\n"));
