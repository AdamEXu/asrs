import type { TextColors } from "./dom";

export type TextMode = "lcd" | "grayscale";

interface Image {
  width: number;
  height: number;
  rgba: Uint8Array;
}

const BLEND_ROUNDING = 2;

function channels(css: string): number[] {
  return [1, 3, 5].map((i) => parseInt(css.slice(i, i + 2), 16));
}

/** Whether one coverage for all three channels, blending `color` onto `background`, makes `pixel`. */
function oneCoverage(pixel: ArrayLike<number>, color: number[], background: number[]): boolean {
  let low = 0;
  let high = 1;
  for (let c = 0; c < 3; c++) {
    const range = color[c] - background[c];
    const offset = pixel[c] - background[c];
    if (range === 0) {
      if (Math.abs(offset) > BLEND_ROUNDING) return false;
      continue;
    }
    const a = (offset - BLEND_ROUNDING) / range;
    const b = (offset + BLEND_ROUNDING) / range;
    low = Math.max(low, Math.min(a, b));
    high = Math.min(high, Math.max(a, b));
  }
  return low <= high;
}

/** How Chromium antialiased the text in `rect` (device px): LCD if any pixel needs a coverage per
 *  channel. Text without `colors` is grey on grey, where that is any pixel that is not grey. */
export function textMode(image: Image, rect: number[], name: string, colors?: TextColors): TextMode {
  const [x, y, width, height] = rect;
  const clamp = (v: number, max: number) => Math.min(Math.max(v, 0), max);
  const [left, right] = [clamp(Math.floor(x), image.width), clamp(Math.ceil(x + width), image.width)];
  const [top, bottom] = [clamp(Math.floor(y), image.height), clamp(Math.ceil(y + height), image.height)];
  const color = colors && channels(colors.color);
  const background = colors && channels(colors.background);
  let first: number[] | null = null;
  let inked = false;
  let lcd = false;
  for (let py = top; py < bottom; py++) {
    for (let px = left; px < right; px++) {
      const i = (py * image.width + px) * 4;
      const pixel = [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]];
      first ??= pixel;
      inked ||= pixel.some((v, c) => Math.abs(v - first![c]) > 16);
      lcd ||= color && background ? !oneCoverage(pixel, color, background) : pixel[0] !== pixel[1] || pixel[1] !== pixel[2];
    }
  }
  if (!inked) throw new Error(`${name}: no text in the box`);
  return lcd ? "lcd" : "grayscale";
}
