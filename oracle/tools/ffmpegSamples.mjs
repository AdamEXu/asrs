// Records ffmpeg's stderr, chunked exactly as a Node child process delivers it, for the inputs
// `genVideo` replays through upstream's VideoProcessor. Run under the oracle's runtime
// (ELECTRON_RUN_AS_NODE=1) so the pipe reads match the app's:
//
//   node oracle/tools/ffmpegSamples.mjs <ffmpeg binary> <work dir> <out dir>
//
// Each synthetic input is made with lavfi in <work dir>, then converted with upstream's argv
// (`-i <input> -vf scale=1920:-2,setsar=1:1 -q:v 2 <cache>/%08d.jpg`). ffmpeg runs with
// <work dir> as its cwd and relative paths, so no local path reaches the samples. One JSON file
// per input lands in <out dir>: the ffmpeg version line, the converted file name, the stderr
// chunks in order and the exit code.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const [ffmpeg, workDir, outDir] = process.argv.slice(2);
if (!ffmpeg || !workDir || !outDir) {
  console.error("usage: ffmpegSamples.mjs <ffmpeg> <work dir> <out dir>");
  process.exit(2);
}
fs.mkdirSync(workDir, { recursive: true });
fs.mkdirSync(outDir, { recursive: true });

const SOURCES = [
  { name: "h264-720p30-aac", file: "h264-720p30.mp4", make: ["-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=3", "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"] },
  { name: "h264-long-progress", file: "h264-long.mp4", make: ["-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=60", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"] },
  { name: "h264-1080p2997", file: "h264-1080p2997.mp4", make: ["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30000/1001:duration=4", "-c:v", "libx264", "-pix_fmt", "yuv420p"] },
  { name: "vp9-360p25", file: "vp9-360p25.webm", make: ["-f", "lavfi", "-i", "testsrc=size=640x360:rate=25:duration=3", "-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8"] },
  { name: "prores-480p60", file: "prores-480p60.mov", make: ["-f", "lavfi", "-i", "smptebars=size=854x480:rate=60:duration=2", "-c:v", "prores"] },
  { name: "hevc-portrait-23976", file: "hevc-portrait.mkv", make: ["-f", "lavfi", "-i", "testsrc=size=720x1280:rate=24000/1001:duration=2", "-c:v", "libx265", "-preset", "ultrafast", "-x265-params", "log-level=error"] },
  { name: "mpeg4-240p15", file: "mpeg4-240p15.avi", make: ["-f", "lavfi", "-i", "testsrc=size=320x240:rate=15:duration=3", "-c:v", "mpeg4"] },
  { name: "h264-2160p-downscale", file: "h264-2160p.mp4", make: ["-f", "lavfi", "-i", "testsrc=size=3840x2160:rate=30:duration=1", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"] },
  { name: "h264-raw-no-duration", file: "raw.h264", make: ["-f", "lavfi", "-i", "testsrc=size=640x480:rate=30:duration=2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "h264"] },
  { name: "mjpeg-1000fps", file: "fast.mkv", make: ["-f", "lavfi", "-i", "testsrc=size=320x240:rate=1000:duration=0.2", "-c:v", "mjpeg"] },
  { name: "audio-only", file: "audio.m4a", make: ["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-c:a", "aac"] },
  { name: "missing-input", file: "does-not-exist.mp4", make: null }
];

const version = spawnSync(ffmpeg, ["-version"], { encoding: "utf8" }).stdout.split("\n")[0];

function record(source) {
  const cache = "cache-" + source.name;
  fs.rmSync(path.join(workDir, cache), { recursive: true, force: true });
  fs.mkdirSync(path.join(workDir, cache));
  const args = ["-i", source.file, "-vf", "scale=1920:-2,setsar=1:1", "-q:v", "2", cache + "/%08d.jpg"];
  return new Promise((resolve) => {
    const child = spawn(ffmpeg, args, { cwd: workDir });
    const chunks = [];
    child.stderr.on("data", (data) => chunks.push(data.toString("latin1")));
    child.on("close", (code) => {
      fs.rmSync(path.join(workDir, cache), { recursive: true, force: true });
      resolve({ name: source.name, version, file: source.file, chunks, exitCode: code });
    });
  });
}

for (const source of SOURCES) {
  if (source.make !== null) {
    const made = spawnSync(ffmpeg, ["-y", "-hide_banner", "-loglevel", "error", ...source.make, source.file], { cwd: workDir });
    if (made.status !== 0) throw new Error(`making ${source.file} failed: ${made.stderr}`);
  }
  const sample = await record(source);
  if (sample.chunks.some((chunk) => /[^\x00-\x7f]/.test(chunk))) throw new Error(`${source.name}: non-ASCII stderr`);
  fs.writeFileSync(path.join(outDir, source.name + ".json"), JSON.stringify(sample, null, 1) + "\n");
  console.log(`${source.name}: ${sample.chunks.length} chunks, exit ${sample.exitCode}`);
}
