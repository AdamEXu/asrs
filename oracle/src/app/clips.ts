import { nativeImage } from "electron";
import * as fs from "node:fs";
import * as path from "node:path";

type Clip = { width: number; height: number; frames: number };

// Synthetic clips as ffmpeg leaves them: 00000001.jpg and on. Drawn with integer arithmetic only,
// so the bitmaps are the same on every host.
const CLIPS: Record<string, Clip> = {
  wide: { width: 1920, height: 1080, frames: 12 },
  portrait: { width: 1920, height: 3414, frames: 1 },
  small: { width: 160, height: 90, frames: 1 }
};
const JPEG_QUALITY = 95;

type Colour = [number, number, number];
const BARS: Colour[] = [
  [235, 235, 235],
  [225, 215, 30],
  [30, 205, 215],
  [40, 190, 50],
  [200, 50, 190],
  [215, 40, 35],
  [40, 60, 205],
  [20, 20, 20]
];

function drawFrame({ width, height }: Clip, frame: number): Buffer {
  const bgra = Buffer.alloc(width * height * 4);
  const unit = Math.max(1, Math.floor(width / 64));
  const discX = Math.floor(width / 4) + frame * unit * 3;
  const discY = Math.floor((height * 5) / 8);
  const radius = Math.floor(Math.min(width, height) / 8);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      let c: Colour;
      const dx = x - discX;
      const dy = y - discY;
      if (dx * dx + dy * dy <= radius * radius) c = [250, 140, 20];
      else if (y * 3 < height) c = BARS[Math.floor((x * BARS.length) / width)];
      else if (y * 2 < height) {
        const v = Math.floor((x * 255) / (width - 1));
        c = [v, v, v];
      } else if (x % (unit * 4) < Math.max(1, Math.floor(unit / 4)) || y % (unit * 4) < Math.max(1, Math.floor(unit / 4))) c = [245, 245, 250];
      else c = [30 + Math.floor((x * 90) / width), 60 + Math.floor((y * 100) / height), 70];
      const o = (y * width + x) * 4;
      bgra[o] = c[2];
      bgra[o + 1] = c[1];
      bgra[o + 2] = c[0];
      bgra[o + 3] = 255;
    }
  }
  return bgra;
}

// Writes the clip under `dir` unless it is there, and returns its folder as upstream's imgFolder.
export function clipFolder(dir: string, name: string): string {
  const clip = CLIPS[name];
  if (clip === undefined) throw new Error(`no generated clip ${name}; known: ${Object.keys(CLIPS).join(", ")}`);
  const folder = path.join(dir, name);
  if (!fs.existsSync(folder)) {
    fs.mkdirSync(folder, { recursive: true });
    for (let frame = 1; frame <= clip.frames; frame++) {
      const image = nativeImage.createFromBitmap(drawFrame(clip, frame), { width: clip.width, height: clip.height });
      fs.writeFileSync(path.join(folder, `${String(frame).padStart(8, "0")}.jpg`), image.toJPEG(JPEG_QUALITY));
    }
  }
  return folder + path.sep;
}
