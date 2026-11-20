import "./shims";
import { parseCanFrame } from "@upstream/hub/dataSources/schema/spark/can-spec-util";
import { sparkFramesSpec as spec2025 } from "@upstream/hub/dataSources/schema/spark/spark-frames-2025";
import { sparkFramesSpec as spec2026 } from "@upstream/hub/dataSources/schema/spark/spark-frames-2026";
import { Cve1, toTree } from "./cve1";
import { quietly } from "./corpus/emit";
import { Rng } from "./corpus/rng";

// Emits upstream `parseCanFrame` ground truth for the asrs_formats::spark vectors.
//   genSpark <2025|2026> <seed>
//     {"t":"meta","year":<y>,"seed":<s>,"sweep":<n>,"frames":[keys in pick order]}
//     {"f":<key>,"d":"<hex>","ok"|"throw":<CVE-1>}   explicit payload records, then
//     {"h":"<16 hex fnv1a-64>"}                      one digest per seeded sweep frame.
//     The digest is fnv1a-64 over each result object: for every [key, value] pair the
//     UTF-8 key, a 0 separator, then 0x01 + a bool byte or 0x02 + the 8 LE bytes of
//     Number(result). A thrown error hashes as 0x03 + the UTF-8 message.
//     The Rust side regenerates the same frame pick/mode/byte sequence from <seed> with
//     a port of corpus/rng.ts, so the sweep records carry no inputs.

type Frame = (typeof spec2026.periodicFrames)[keyof typeof spec2026.periodicFrames];

const SPECS: Record<string, { keys: string[]; frames: Frame[] }> = {
  "2025": {
    keys: Array.from({ length: 8 }, (_, i) => `STATUS_${i}`),
    frames: Array.from({ length: 8 }, (_, i) => (spec2025.periodicFrames as any)[`STATUS_${i}`] as Frame)
  },
  "2026": {
    keys: [...Array.from({ length: 8 }, (_, i) => `STATUS_${i}`), "GET_FIRMWARE_VERSION"],
    frames: [
      ...Array.from({ length: 8 }, (_, i) => (spec2026.periodicFrames as any)[`STATUS_${i}`] as Frame),
      spec2026.nonPeriodicFrames.GET_FIRMWARE_VERSION as Frame
    ]
  }
};

const SWEEP = 100000;

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function bytesToHex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

/** The parse result with each BigNumber replaced by its toNumber(), then CVE-1. */
function resultTree(f: () => object): { ok: Cve1 } | { throw: Cve1 } {
  try {
    const parsed = quietly(f) as Record<string, unknown>;
    return {
      ok: toTree(
        Object.fromEntries(
          Object.entries(parsed).map(([key, value]) => [
            key,
            typeof value === "boolean" ? value : (value as { toNumber(): number }).toNumber()
          ])
        )
      )
    };
  } catch (e) {
    return { throw: toTree(e) };
  }
}

class Digest {
  private h = 0xcbf29ce484222325n;

  private feed(b: number) {
    this.h = ((this.h ^ BigInt(b & 0xff)) * 0x100000001b3n) & 0xffffffffffffffffn;
  }

  private text(s: string) {
    for (const b of new TextEncoder().encode(s)) this.feed(b);
  }

  of(f: () => object): string {
    let result: Record<string, unknown>;
    try {
      result = quietly(f) as Record<string, unknown>;
    } catch (e: any) {
      this.feed(3);
      this.text(e instanceof Error ? e.message : String(e));
      return this.hex();
    }
    for (const [key, value] of Object.entries(result)) {
      this.text(key);
      this.feed(0);
      if (typeof value === "boolean") {
        this.feed(1);
        this.feed(value ? 1 : 0);
      } else {
        this.feed(2);
        const bits = new DataView(new ArrayBuffer(8));
        bits.setFloat64(0, (value as { toNumber(): number }).toNumber(), true);
        for (let i = 0; i < 8; i++) this.feed(bits.getUint8(i));
      }
    }
    return this.hex();
  }

  private hex(): string {
    return this.h.toString(16).padStart(16, "0");
  }
}

function* edgePayloads(): Generator<Uint8Array> {
  yield new Uint8Array(8);
  yield new Uint8Array(8).fill(0xff);
  yield new Uint8Array(8).fill(0xaa);
  yield new Uint8Array(8).fill(0x55);
  for (let i = 0; i < 64; i++) {
    const b = new Uint8Array(8);
    b[i >> 3] = 1 << (i & 7);
    yield b;
  }
  for (let i = 0; i < 8; i++) {
    for (const v of [0x01, 0x7f, 0x80, 0xfe, 0xff]) {
      const b = new Uint8Array(8);
      b[i] = v;
      yield b;
    }
  }
  const words32 = [
    0x00000000, 0x80000000, 0x7f800000, 0xff800000, 0x7fc00000, 0x7fe00000, 0x7f800001,
    0x7f7fffff, 0x3f800000, 0xbf800000, 0x00000001, 0x007fffff, 0x00800000, 0xff7fffff
  ];
  for (const offset of [0, 4]) {
    for (const w of words32) {
      const b = new Uint8Array(8);
      new DataView(b.buffer).setUint32(offset, w, true);
      yield b;
    }
  }
  const words64 = [
    0n, 0x8000000000000000n, 0x7ff0000000000000n, 0xfff0000000000000n, 0x7ff8000000000000n,
    0x7ff0000000000001n, 0x7fefffffffffffffn, 0x3ff0000000000000n, 0xbff0000000000000n, 1n,
    0x800fffffffffffffn, 0x0010000000000000n, 0xffffffffffffffffn
  ];
  for (const w of words64) {
    const b = new Uint8Array(8);
    new DataView(b.buffer).setBigUint64(0, w, true);
    yield b;
  }
  for (const n of [0, 1, 7, 9, 12]) yield new Uint8Array(n).fill(0xa5);
}

const [year, seedArg] = process.argv.slice(2);
const spec = SPECS[year ?? ""];
const seed = Number(seedArg);
if (spec === undefined || !Number.isInteger(seed) || seed < 0 || seed > 0xffffffff) {
  throw new Error("usage: genSpark <2025|2026> <seed>");
}

out({ t: "meta", year: Number(year), seed, sweep: SWEEP, frames: spec.keys });

for (let i = 0; i < spec.frames.length; i++) {
  const frame = spec.frames[i];
  const key = spec.keys[i];
  for (const data of edgePayloads()) {
    out({ f: key, d: bytesToHex(data), ...resultTree(() => parseCanFrame(frame as any, { data })) });
  }
}

const rng = new Rng(seed, `spark/${year}`);
for (let i = 0; i < SWEEP; i++) {
  const index = rng.below(spec.frames.length);
  const mode = rng.below(10);
  let data: Uint8Array;
  switch (mode) {
    case 6:
      data = new Uint8Array(8);
      break;
    case 7:
      data = new Uint8Array(8).fill(0xff);
      break;
    case 8:
      data = Uint8Array.from({ length: 8 }, () => (rng.bool() ? 0xff : 0));
      break;
    case 9:
      data = Uint8Array.from({ length: 8 }, () => rng.pick([0x00, 0x7f, 0x80, 0xff]));
      break;
    default:
      data = rng.bytes(8);
  }
  out({ h: new Digest().of(() => parseCanFrame(spec.frames[index] as any, { data })) });
}
