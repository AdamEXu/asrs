// Renders the synthetic match clips `genTimer` reads and converts them with upstream's argv. Run
// under the oracle's runtime (ELECTRON_RUN_AS_NODE=1) so the stderr chunks match the app's:
//
//   node oracle/tools/timerClips.mjs <ffmpeg> <font.ttf> <work dir> <out dir>
//
// The committed clips were made with the ffmpeg-static b6.0 darwin-arm64 binary upstream downloads
// and Roboto (OFL). A clip is a little-endian u32 header length, a JSON header ({ name, ffmpeg,
// stderr, exit, frames: [[number, byte length], ...] }) and the JPEG frames. Only the frames the
// timer search can ask for are kept: one per second and one second of frames from the first
// sample showing 0:04.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const [ffmpeg, font, workDir, outDir] = process.argv.slice(2);
if (!ffmpeg || !font || !workDir || !outDir) {
  console.error("usage: timerClips.mjs <ffmpeg> <font.ttf> <work dir> <out dir>");
  process.exit(2);
}
fs.mkdirSync(workDir, { recursive: true });
fs.mkdirSync(outDir, { recursive: true });

const AUTO_START = 2.4;
const AUTO_SECS = 15;

function timeline(duration) {
  const steps = [["0:15", 0, AUTO_START]];
  for (let left = AUTO_SECS - 1; left >= 0; left--) {
    const start = AUTO_START + (AUTO_SECS - 1 - left);
    steps.push([`0:${String(left).padStart(2, "0")}`, start, start + 1]);
  }
  for (let start = AUTO_START + AUTO_SECS, left = 135; start < duration; start++, left--) {
    steps.push([`${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`, start, start + 1]);
  }
  return steps;
}

const CLIPS = [
  {
    name: "banner-lower-2025",
    size: "1920x1080",
    rate: "30",
    duration: 20,
    fontSize: 72,
    color: "0x101010",
    center: [965, 1025],
    panels: [
      [0, 900, 1920, 180, "0xe8e8e8"],
      [0, 900, 700, 180, "0xc02020"],
      [1230, 900, 690, 180, "0x2040c0"]
    ],
    labels: [["87", 600, 990, 110, "white"], ["104", 1320, 990, 110, "white"], ["Qualification 12 of 80", 965, 940, 36, "black"]]
  },
  {
    name: "small-2023-2997",
    size: "1920x1080",
    rate: "30000/1001",
    duration: 20,
    fontSize: 34,
    color: "white",
    center: [1080, 780],
    panels: [
      [760, 740, 400, 80, "0x202020"],
      [760, 740, 120, 80, "0xb01818"],
      [880, 740, 120, 80, "0x1838b0"]
    ],
    labels: [["31", 820, 780, 44, "white"], ["28", 940, 780, 44, "white"]]
  },
  {
    name: "web-lower-2025-lowbitrate",
    size: "1920x1080",
    rate: "25",
    duration: 20,
    fontSize: 48,
    color: "white",
    center: [960, 994],
    panels: [
      [480, 950, 960, 88, "0x303030@0.8"],
      [480, 950, 400, 88, "0xd02020@0.8"],
      [1040, 950, 400, 88, "0x2050d0@0.8"]
    ],
    labels: [["45", 780, 994, 60, "white"], ["03", 1140, 994, 60, "white"]],
    degrade: "scale=480:270,scale=1920:1080:flags=bilinear",
    encode: ["-b:v", "250k"]
  },
  {
    name: "banner-lower-720p",
    size: "1280x720",
    rate: "30",
    duration: 9,
    start: 10,
    fontSize: 48,
    color: "0x101010",
    center: [643, 683],
    panels: [[0, 600, 1280, 120, "0xe8e8e8"]],
    labels: []
  }
];

const escapeText = (text) => text.replace(/:/g, "\\:");

function filters(clip) {
  const [width, height] = clip.size.split("x").map(Number);
  const out = [];
  for (let i = 0; i < 4; i++) {
    out.push(`drawbox=x='mod(t*${60 + i * 25}+${i * 430},${width})':y=${Math.round(height * (0.12 + i * 0.13))}:w=${Math.round(width / 24)}:h=${Math.round(height / 18)}:color=${["0x804020", "0x206080", "0x508030", "0x705090"][i]}:t=fill`);
  }
  for (const [x, y, w, h, color] of clip.panels) out.push(`drawbox=x=${x}:y=${y}:w=${w}:h=${h}:color=${color}:t=fill`);
  const text = (value, x, y, size, color, enable) =>
    `drawtext=fontfile=${font}:text='${escapeText(value)}':fontsize=${size}:fontcolor=${color}:x=${x}-tw/2:y=${y}-th/2` +
    (enable === undefined ? "" : `:enable='${enable}'`);
  for (const [value, x, y, size, color] of clip.labels) out.push(text(value, x, y, size, color));
  const offset = clip.start ?? 0;
  for (const [value, start, end] of timeline(clip.duration + offset)) {
    if (end <= offset) continue;
    out.push(text(value, clip.center[0], clip.center[1], clip.fontSize, clip.color, `gte(t,${(start - offset).toFixed(3)})*lt(t,${(end - offset).toFixed(3)})`));
  }
  if (clip.degrade) out.push(clip.degrade);
  return out.join(",");
}

function render(clip) {
  const file = clip.name + ".mp4";
  const args = ["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", `gradients=s=${clip.size}:r=${clip.rate}:d=${clip.duration}:c0=0x283038:c1=0x586470:x0=0:y0=0:x1=${clip.size.split("x")[0]}:y1=0:speed=0.00001:seed=7`];
  args.push("-vf", filters(clip), "-c:v", "libx264", "-pix_fmt", "yuv420p", ...(clip.encode ?? ["-crf", "20"]), file);
  const made = spawnSync(ffmpeg, args, { cwd: workDir });
  if (made.status !== 0) throw new Error(`rendering ${file} failed: ${made.stderr}`);
  return file;
}

function convert(clip, file) {
  const cache = path.join(workDir, "cache-" + clip.name);
  fs.rmSync(cache, { recursive: true, force: true });
  fs.mkdirSync(cache);
  const args = ["-i", file, "-vf", "scale=1920:-2,setsar=1:1", "-q:v", "2", `cache-${clip.name}/%08d.jpg`];
  return new Promise((resolve) => {
    const child = spawn(ffmpeg, args, { cwd: workDir });
    const stderr = [];
    child.stderr.on("data", (data) => stderr.push(data.toString("latin1")));
    child.on("close", (exit) => resolve({ cache, stderr, exit }));
  });
}

function wanted(clip, total) {
  const [num, den] = clip.rate.split("/").map(Number);
  // ffmpeg prints the rate rounded to two decimals, and upstream parses that text.
  const fps = Number((num / (den ?? 1)).toFixed(2));
  const frames = new Set();
  for (let sample = 0; sample < clip.duration; sample++) frames.add(Math.round(sample * fps) + 1);
  const four = Math.ceil(AUTO_START + AUTO_SECS - 5 - (clip.start ?? 0));
  const first = Math.round(four * fps) + 1;
  for (let frame = first; frame < first + fps; frame++) frames.add(frame);
  return [...frames].filter((frame) => frame <= total).sort((a, b) => a - b);
}

const version = spawnSync(ffmpeg, ["-version"], { encoding: "utf8" }).stdout.split("\n")[0];
for (const clip of CLIPS) {
  const { cache, stderr, exit } = await convert(clip, render(clip));
  if (stderr.some((chunk) => /[^\x00-\x7f]/.test(chunk))) throw new Error(`${clip.name}: non-ASCII stderr`);
  const total = fs.readdirSync(cache).length;
  const frames = wanted(clip, total);
  const images = frames.map((frame) => fs.readFileSync(path.join(cache, String(frame).padStart(8, "0") + ".jpg")));
  const header = Buffer.from(
    JSON.stringify({ name: clip.name, ffmpeg: version, stderr, exit, frames: frames.map((frame, i) => [frame, images[i].length]) })
  );
  const length = Buffer.alloc(4);
  length.writeUInt32LE(header.length);
  fs.writeFileSync(path.join(outDir, clip.name + ".clip"), Buffer.concat([length, header, ...images]));
  fs.rmSync(cache, { recursive: true, force: true });
  console.log(`${clip.name}: ${total} frames, kept ${frames.length}, ${stderr.length} chunks, exit ${exit}`);
}
