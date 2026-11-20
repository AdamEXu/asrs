const MASK = (1n << 64n) - 1n;
const CACHE_SIZE = 64;

function murmurHash3(h: bigint): bigint {
  h ^= h >> 33n;
  h = (h * 0xff51afd7ed558ccdn) & MASK;
  h ^= h >> 33n;
  h = (h * 0xc4ceb9fe1a85ec53n) & MASK;
  h ^= h >> 33n;
  return h;
}

/**
 * V8's `Math.random` under `--random-seed=<seed>`: xorshift128+ seeded through MurmurHash3,
 * refilled 64 doubles at a time and served from the end of the cache. The port uses
 * `asrs_js::time::V8MathRandom::from_seed` for the same stream.
 */
export class V8MathRandom {
  private s0: bigint;
  private s1: bigint;
  private cache: number[] = [];

  constructor(seed: bigint) {
    seed &= MASK;
    this.s0 = murmurHash3(seed);
    this.s1 = murmurHash3(~seed & MASK);
  }

  private refill(): void {
    this.cache = [];
    for (let i = 0; i < CACHE_SIZE; i++) {
      let s1 = this.s0;
      const s0 = this.s1;
      this.s0 = s0;
      s1 ^= (s1 << 23n) & MASK;
      s1 ^= s1 >> 17n;
      s1 ^= s0;
      s1 ^= s0 >> 26n;
      this.s1 = s1;
      this.cache.push(Number(this.s0 >> 11n) / 2 ** 53);
    }
  }

  next(): number {
    if (this.cache.length === 0) this.refill();
    return this.cache.pop()!;
  }
}
