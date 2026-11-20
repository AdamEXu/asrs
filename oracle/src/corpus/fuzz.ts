import { Rng } from "./rng";

const INTERESTING_8 = [0x00, 0x01, 0x7f, 0x80, 0xff, 0x20, 0x2c, 0x0a, 0x52];
const INTERESTING_16 = [0x0000, 0x7fff, 0x8000, 0xffff, 0x0100, 0x00ff];
const INTERESTING_32 = [0x00000000, 0x7fffffff, 0x80000000, 0xffffffff, 0x00010000, 0x0000ffff];
const INTERESTING_F64 = [NaN, Infinity, -Infinity, -0, 1e21, 5e-324, 2 ** 53 + 1, 60.0001];

export interface Mutation {
  bytes: Uint8Array;
  steps: string[];
}

/**
 * Applies 1 to `maxSteps` random mutations: bit flips, interesting bytes and words in both byte orders,
 * inserted, deleted, duplicated and swapped ranges, truncation, and splicing from `donor` (a file of the same
 * format). Every choice comes from `rng`, so a seed reproduces the output exactly.
 */
export function mutate(input: Uint8Array, rng: Rng, donor: Uint8Array | null = null, maxSteps = 4): Mutation {
  let data = Array.from(input);
  const steps: string[] = [];
  const count = 1 + rng.below(maxSteps);
  for (let s = 0; s < count; s++) {
    const n = data.length;
    const at = n === 0 ? 0 : rng.below(n);
    switch (rng.below(donor === null ? 10 : 11)) {
      case 0:
        if (n === 0) break;
        data[at] ^= 1 << rng.below(8);
        steps.push(`flip@${at}`);
        break;
      case 1:
        if (n === 0) break;
        data[at] = rng.pick(INTERESTING_8);
        steps.push(`byte@${at}`);
        break;
      case 2:
      case 3: {
        const width = rng.pick([2, 4, 8]);
        if (n < width) break;
        const pos = rng.below(n - width + 1);
        const word = encodeWord(rng, width);
        if (rng.bool()) word.reverse();
        data.splice(pos, width, ...word);
        steps.push(`word${width}@${pos}`);
        break;
      }
      case 4: {
        const len = 1 + rng.below(16);
        data.splice(at, 0, ...Array.from({ length: len }, () => rng.below(256)));
        steps.push(`insert${len}@${at}`);
        break;
      }
      case 5: {
        if (n === 0) break;
        const len = 1 + rng.below(Math.min(64, n - at));
        data.splice(at, len);
        steps.push(`delete${len}@${at}`);
        break;
      }
      case 6: {
        if (n === 0) break;
        const len = 1 + rng.below(Math.min(64, n - at));
        const chunk = data.slice(at, at + len);
        const to = rng.below(n + 1);
        data.splice(to, 0, ...chunk);
        steps.push(`dup${len}@${at}->${to}`);
        break;
      }
      case 7: {
        if (n < 4) break;
        const len = 1 + rng.below(Math.min(32, n >> 1));
        const a = rng.below(n - len + 1);
        const b = rng.below(n - len + 1);
        const chunkA = data.slice(a, a + len);
        const chunkB = data.slice(b, b + len);
        data.splice(a, len, ...chunkB);
        data.splice(b, len, ...chunkA);
        steps.push(`swap${len}@${a},${b}`);
        break;
      }
      case 8:
        if (n === 0) break;
        data = data.slice(0, at);
        steps.push(`truncate@${at}`);
        break;
      case 9:
        if (n === 0) break;
        data[at] = (data[at] + (rng.bool() ? 1 : 255)) & 0xff;
        steps.push(`nudge@${at}`);
        break;
      case 10: {
        const d = donor!;
        if (d.length === 0) break;
        const from = rng.below(d.length);
        const len = 1 + rng.below(Math.min(256, d.length - from));
        data.splice(at, rng.below(Math.min(256, n - at) + 1), ...d.slice(from, from + len));
        steps.push(`splice${len}@${at}`);
        break;
      }
    }
  }
  if (steps.length === 0) steps.push("none");
  return { bytes: Uint8Array.from(data), steps };
}

function encodeWord(rng: Rng, width: number): number[] {
  const view = new DataView(new ArrayBuffer(width));
  if (width === 8) view.setFloat64(0, rng.pick(INTERESTING_F64), true);
  else if (width === 4) view.setUint32(0, rng.pick(INTERESTING_32), true);
  else view.setUint16(0, rng.pick(INTERESTING_16), true);
  return Array.from(new Uint8Array(view.buffer));
}
