import { bitsToF64 } from "./bytes";

function fnv1a32(text: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash;
}

/** mulberry32, seeded per case from the corpus seed and the case id so cases stay independent. */
export class Rng {
  private state: number;

  constructor(seed: number, stream: string) {
    this.state = (seed ^ fnv1a32(stream)) >>> 0;
  }

  u32(): number {
    this.state = (this.state + 0x6d2b79f5) >>> 0;
    let t = this.state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return (t ^ (t >>> 14)) >>> 0;
  }

  below(n: number): number {
    return this.u32() % n;
  }

  range(lo: number, hi: number): number {
    return lo + this.below(hi - lo + 1);
  }

  bool(): boolean {
    return (this.u32() & 1) === 1;
  }

  pick<T>(items: readonly T[]): T {
    return items[this.below(items.length)];
  }

  u64(): bigint {
    return (BigInt(this.u32()) << 32n) | BigInt(this.u32());
  }

  bytes(n: number): Uint8Array {
    const out = new Uint8Array(n);
    for (let i = 0; i < n; i++) out[i] = this.u32() & 0xff;
    return out;
  }

  /** Uniform in [0, 1) with 32 bits of precision. */
  unit(): number {
    return this.u32() / 2 ** 32;
  }

  /** A double that is usually ordinary and sometimes an edge value or an arbitrary bit pattern. */
  double(): number {
    switch (this.below(5)) {
      case 0:
        return this.pick(SPECIAL_DOUBLES);
      case 1:
        return bitsToF64(this.u64());
      case 2:
        return this.range(-1000, 1000);
      default:
        return (this.unit() * 2 - 1) * 10 ** this.range(-8, 8);
    }
  }

  ascii(maxLength: number): string {
    let out = "";
    const n = this.below(maxLength + 1);
    for (let i = 0; i < n; i++) out += String.fromCharCode(this.range(0x20, 0x7e));
    return out;
  }
}

export const SPECIAL_DOUBLES: readonly number[] = [
  0,
  -0,
  1,
  -1,
  0.1,
  0.5,
  1e21,
  1e-7,
  2 ** 53,
  2 ** 53 + 2,
  Number.MIN_VALUE,
  Number.MAX_VALUE,
  -Number.MAX_VALUE,
  Infinity,
  -Infinity,
  NaN
];
