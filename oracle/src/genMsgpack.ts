import "./shims";
import { Decoder, Encoder, ExtData, encode } from "@msgpack/msgpack";
import { Cve1, toTree } from "./cve1";
import { Corpus } from "./corpus/corpus";
import { msgpackMatrix } from "./corpus/msgpack";
import { Rng } from "./corpus/rng";

// Emits @msgpack/msgpack ground truth for the asrs_log::msgpack vectors.
//   genMsgpack decode <seed>
//     {"case":"<corpus id>","d":{"b":"<hex>"},"ok"|"throw":<CVE-1>}
//     Every payload of the corpus msgpack matrix plus seeded mutations (truncations, byte
//     flips, trailing garbage) and random byte strings.
//   genMsgpack multi <seed>
//     {"m":{"b":"<hex>"},"ok":[<CVE-1>...]} | {"m":..,"throw":<CVE-1>}
//     decodeMulti over concatenated values, NT4 wire frames and truncated streams.
//   genMsgpack encode <count> <seed>
//     {"e":<CVE-1 input>,"ok":{"b":"<hex>"}} | {"e":<CVE-1 input>,"throw":<CVE-1>}
//     <count> values covering every encoder branch, then seeded random values.

const [mode, a1, a2] = process.argv.slice(2);
const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function bytesToHex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

function hexToBytes(text: string): Uint8Array {
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(2 * i, 2 * i + 2), 16);
  return out;
}

function attempt(f: () => unknown): { ok: Cve1 } | { throw: Cve1 } {
  try {
    return { ok: toTree(f()) };
  } catch (e) {
    return { throw: toTree(e) };
  }
}

// A fresh Decoder per record: decode errors leave the container stack half-full, and the
// upstream API the port matches is `new Decoder().decode(...)` per payload.
// `keyDecoder: null` because the default is a process-global shared cache keyed on the raw
// key bytes while its utf8DecodeJs reads continuation bytes past the key's own slice, so a
// malformed key's decoded string depends on which record populated the cache first.

function decodeRecord(bytes: Uint8Array, cse?: string): object {
  const r: Record<string, unknown> = { d: { b: bytesToHex(bytes) } };
  if (cse !== undefined) r.case = cse;
  return { ...r, ...attempt(() => new Decoder({ keyDecoder: null }).decode(bytes)) };
}

function matrixPayloads(seed: number): [string, Uint8Array][] {
  const corpus = new Corpus(seed);
  msgpackMatrix(corpus);
  const payloads: [string, Uint8Array][] = [];
  const decoder8 = new TextDecoder();
  for (const c of corpus.cases) {
    if (!c.id.startsWith("payload/msgpack/")) continue;
    const opsFile = c.files.find((f) => f.path.endsWith(".ops.jsonl"));
    if (!opsFile) continue;
    for (const line of decoder8.decode(opsFile.bytes).split("\n")) {
      if (!line.trim()) continue;
      const op = JSON.parse(line);
      if (op.fn !== "putMsgpack") continue;
      payloads.push([c.id, hexToBytes(op.args[2].b)]);
    }
  }
  return payloads;
}

if (mode === "decode") {
  const seed = Number(a1);
  const corpus = new Corpus(seed);
  const payloads = matrixPayloads(seed);
  const seen = new Set<string>();
  for (const [id, bytes] of payloads) {
    const hex = bytesToHex(bytes);
    if (seen.has(hex)) continue;
    seen.add(hex);
    out(decodeRecord(bytes, id));
  }
  const r = corpus.rng("msgpack/fuzz");
  const sources = payloads.map(([, b]) => b).filter((b) => b.length > 0);
  for (let i = 0; i < 4000; i++) {
    const source = r.pick(sources);
    switch (r.below(6)) {
      case 0: {
        const at = r.below(source.length + 1);
        out(decodeRecord(source.slice(0, at), "truncate"));
        break;
      }
      case 1: {
        const mutated = source.slice();
        mutated[r.below(mutated.length)] ^= 1 << r.below(8);
        out(decodeRecord(mutated, "flip"));
        break;
      }
      case 2: {
        const mutated = source.slice();
        mutated[r.below(mutated.length)] = r.below(256);
        out(decodeRecord(mutated, "replace"));
        break;
      }
      case 3:
        out(decodeRecord(new Uint8Array([...source, ...r.bytes(r.below(4))])), "trailing");
        break;
      case 4:
        out(decodeRecord(r.bytes(r.below(24)), "random"));
        break;
      default: {
        const mutated = source.slice();
        const at = r.below(mutated.length);
        mutated.copyWithin(at, at + 1);
        out(decodeRecord(mutated.subarray(0, mutated.length - 1), "delete"));
      }
    }
  }
} else if (mode === "multi") {
  const seed = Number(a1);
  const corpus = new Corpus(seed);
  const payloads = matrixPayloads(seed).map(([, b]) => b).filter((b) => b.length > 0 && b[0] !== 0xc1);
  const r = corpus.rng("msgpack/multi");
  // `Array.from` on a number iterable stores into a PACKED_DOUBLE array and V8
  // canonicalizes NaN payloads there; iterating the generator and treeing each yielded
  // value keeps the raw f64 bits, matching `for (const x of decodeMulti(buf))`.
  const emit = (bytes: Uint8Array) => {
    try {
      const trees: Cve1[] = [];
      const it = new Decoder({ keyDecoder: null }).decodeMulti(bytes);
      for (let r = it.next(); !r.done; r = it.next()) trees.push(toTree(r.value));
      out({ m: { b: bytesToHex(bytes) }, ok: trees });
    } catch (e) {
      out({ m: { b: bytesToHex(bytes) }, throw: toTree(e) });
    }
  };
  const cat = (...parts: Uint8Array[]) => {
    const total = parts.reduce((n, p) => n + p.length, 0);
    const joined = new Uint8Array(total);
    let at = 0;
    for (const p of parts) {
      joined.set(p, at);
      at += p.length;
    }
    return joined;
  };
  for (let i = 0; i + 1 < payloads.length; i += 2) emit(cat(payloads[i], payloads[i + 1]));
  for (let i = 0; i < 200; i++) {
    const n = 1 + r.below(6);
    const parts = Array.from({ length: n }, () => r.pick(payloads));
    emit(cat(...parts));
  }
  // NT4-style frames: [pubuid, time_us, typeIdx, value], and the time-sync frames on -1.
  for (let i = 0; i < 100; i++) {
    const frames = Array.from({ length: 1 + r.below(4) }, () =>
      r.bool()
        ? encode([1 + r.below(3), Math.floor(r.unit() * 2 ** 40), r.below(5), r.bool() ? r.unit() * 100 : r.below(100)])
        : encode([-1, Math.floor(r.unit() * 2 ** 40), 2, Math.floor(r.unit() * 2 ** 40)])
    );
    emit(cat(...frames));
  }
  for (let i = 0; i < 100; i++) {
    const stream = cat(r.pick(payloads), r.pick(payloads));
    emit(stream.subarray(0, r.below(stream.length + 1)));
  }
  emit(new Uint8Array(0));
} else if (mode === "encode") {
  const count = Number(a1);
  const r = new Rng(Number(a2), "msgpack/encode");
  const emitValue = (value: unknown) =>
    out({ e: toTree(value), ...attempt(() => new Encoder().encode(value)) });
  const emitBytes = (bytes: Uint8Array) => emitValue(bytes);

  const edgeNumbers: number[] = [
    0, -0, 1, -1, 1.0, 0.5, -0.5, 127, 128, 255, 256, 32767, 32768, 65535, 65536,
    2147483647, 2147483648, 4294967295, 4294967296, 9007199254740991, 9007199254740992,
    9007199254740994, 2 ** 53 + 2, -32, -33, -128, -129, -32768, -32769, -2147483648,
    -2147483649, -4294967296, -9007199254740991, -9007199254740992, -9007199254740994,
    NaN, Infinity, -Infinity, 0.1, -0.1, 1e-7, 1e21, -1e21, Number.MIN_VALUE,
    Number.MAX_VALUE, -Number.MAX_VALUE, 5e-324, 3.14159, 1e15 + 0.5, 4294967295.5
  ];
  for (const x of edgeNumbers) emitValue(x);

  const strings = [
    "", "a", "x".repeat(31), "x".repeat(32), "x".repeat(33), "x".repeat(49), "x".repeat(50),
    "x".repeat(51), "x".repeat(255), "x".repeat(256), "x".repeat(1000),
    "é".repeat(25), "é".repeat(26), "中".repeat(17), "中".repeat(18), "🤖".repeat(25), "🤖".repeat(26),
    "abc", "def", "\ud800", "\udc00", "\ud800\udc00", "a\ud800", "\ud800a", "a\ud800b",
    "\ud800".repeat(25), "\ud800".repeat(26), "\udc00\ud800", "�", "\0", "a\0b",
    "x".repeat(30) + "é", "x".repeat(50) + "é", "é" + "x".repeat(60),
    "\ud800" + "y".repeat(50), "y".repeat(50) + "\ud800", "y".repeat(49) + "\ud800",
    "\ud83d\ude16".repeat(30), "__proto__", "constructor", "4294967294", "4294967295",
    "0", "1.5", "-0", "1e21", "10", "01", "  1", "1 "
  ];
  for (const s of strings) emitValue(s);

  emitBytes(new Uint8Array(0));
  emitBytes(new Uint8Array([0]));
  emitBytes(new Uint8Array(255).fill(1));
  emitBytes(new Uint8Array(256).fill(2));
  emitBytes(new Uint8Array(65535).fill(3));
  emitBytes(new Uint8Array(65536).fill(4));
  emitBytes(new Uint8Array(65537).fill(5));

  emitValue(new Int8Array([-1, 2, -3]));
  emitValue(new Uint8ClampedArray([1, 2, 300 & 0xff]));
  emitValue(new Int16Array([0x0102, -2]));
  emitValue(new Uint16Array([0xaabb]));
  emitValue(new Int32Array([0x01020304, -1]));
  emitValue(new Uint32Array([0xaabbccdd]));
  emitValue(new Float32Array([1.5, -0]));
  emitValue(new Float64Array([Math.PI]));
  emitValue(new BigInt64Array([1n, -2n]));
  emitValue(new BigUint64Array([0xffffffffffffffffn]));
  emitValue(new DataView(new Uint8Array([9, 8, 7]).buffer));
  emitValue(new ArrayBuffer(4));
  emitValue(new Uint8Array([1, 2, 3, 4]).subarray(1, 3));

  for (const t of [
    0, 1, 999, 1000, 1001, 1700000000000, -1, -1000, -62135596800000,
    0xffffffff * 1000 - 1, 0xffffffff * 1000, 0xffffffff * 1000 + 999,
    0x100000000 * 1000, 0x3ffffffff * 1000 - 1, 0x3ffffffff * 1000, 0x3ffffffff * 1000 + 999,
    8.64e15 - 1, 8.64e15, -8.64e15, -8.64e15 - 1, 1e20, -1e20, NaN, Infinity, -Infinity, 0.9, -0.9
  ]) {
    emitValue(new Date(t));
  }

  for (const [type, size] of [
    [0, 0], [1, 1], [2, 2], [4, 4], [8, 8], [16, 16], [127, 1], [-128, 1], [-1, 1],
    [-1, 4], [-1, 8], [-1, 12], [-1, 3], [3, 3], [5, 5], [15, 15], [17, 17],
    [6, 255], [6, 256], [7, 300]
  ] as [number, number][]) {
    emitValue(new ExtData(type, r.bytes(size)));
  }

  emitValue(null);
  emitValue(undefined);
  emitValue(true);
  emitValue(false);
  emitValue(1n);
  emitValue(-1n);
  emitValue(9007199254740993n);
  emitValue(new Map([[1, 2]]));
  emitValue(new Set([1, 2]));
  emitValue(Object(5));
  emitValue(Object("s"));
  emitValue(Object(true));
  emitValue(Object.create(null));
  {
    const o: Record<string, unknown> = {};
    Object.defineProperty(o, "__proto__", { value: 1, enumerable: true });
    emitValue(o);
  }
  emitValue({ a: undefined, b: 1 });
  emitValue([undefined, 1]);
  {
    const sparse = new Array(3);
    sparse[1] = 1;
    emitValue(sparse);
  }
  emitValue({ "10": "a", "2": "b", x: 1, "0": "c", "01": "d", "1.5": "e", "4294967295": "f" });
  {
    const o: Record<string, number> = {};
    for (let i = 0; i < 17; i++) o[`k${i}`] = i;
    emitValue(o);
  }
  {
    const o: Record<string, number> = {};
    for (let i = 0; i < 16; i++) o[`k${i}`] = i;
    emitValue(o);
  }
  emitValue(Array.from({ length: 15 }, (_, i) => i));
  emitValue(Array.from({ length: 16 }, (_, i) => i));
  emitValue(Array.from({ length: 17 }, (_, i) => i));
  {
    let deep: unknown = 1;
    for (let i = 0; i < 98; i++) deep = [deep];
    emitValue(deep);
    let tooDeep: unknown = 1;
    for (let i = 0; i < 101; i++) tooDeep = [tooDeep];
    emitValue(tooDeep);
    let edge: unknown = 1;
    for (let i = 0; i < 99; i++) edge = [edge];
    emitValue(edge);
  }

  const randomString = (r: Rng): string => {
    const parts: string[] = [];
    const n = r.below(12);
    for (let i = 0; i < n; i++) {
      switch (r.below(6)) {
        case 0: parts.push(String.fromCharCode(r.range(0x20, 0x7e))); break;
        case 1: parts.push(String.fromCharCode(r.range(0xa0, 0x2fff))); break;
        case 2: parts.push(String.fromCharCode(0xd800 + r.below(0x800))); break;
        case 3: parts.push(String.fromCodePoint(r.range(0x10000, 0x10ffff))); break;
        case 4: parts.push("x".repeat(r.below(60))); break;
        default: parts.push(r.pick(["é", "中", "🤖", "�", "\0", "\n"])); break;
      }
    }
    return parts.join("");
  };

  const randomValue = (r: Rng, depth: number): unknown => {
    const roll = r.below(depth > 3 ? 13 : 20);
    switch (roll) {
      case 0: return null;
      case 1: return r.bool();
      case 2: return r.pick(edgeNumbers);
      case 3: return r.double();
      case 4: return r.range(-(2 ** 31), 2 ** 32);
      case 5: return randomString(r);
      case 6: return r.bytes(r.below(64));
      case 7: return new Date(r.range(-(10 ** 14), 10 ** 14) * (r.bool() ? 1 : 1000));
      case 8: return new ExtData(r.range(-128, 127), r.bytes(r.pick([0, 1, 2, 3, 4, 8, 16, 17, 40])));
      case 9: return undefined;
      case 10: return r.bool() ? Math.floor(r.unit() * 2 ** 53) : -Math.floor(r.unit() * 2 ** 53);
      case 11: return new Int32Array(Array.from({ length: r.below(8) }, () => r.below(2 ** 31) - (2 ** 30)));
      case 12: return r.bool() ? Object(r.double()) : new Map();
      case 13: {
        const n = r.below(8);
        const arr = Array.from({ length: n }, () => randomValue(r, depth + 1));
        if (r.bool() && n > 0) delete arr[r.below(n)];
        return arr;
      }
      default: {
        const o: Record<string, unknown> = {};
        const n = r.below(8);
        for (let i = 0; i < n; i++) {
          let key = r.pick(["k", "x" + i, String(i), String(r.below(100)), randomString(r), "0" + i, "4294967294"]);
          // `o["__proto__"] = v` mutates the prototype instead of creating a key.
          if (key === "__proto__") key = "_proto_";
          o[key] = randomValue(r, depth + 1);
        }
        return o;
      }
    }
  };

  let emitted = 0;
  for (let i = 0; i < count; i++) {
    emitValue(randomValue(r, 0));
    emitted++;
  }
  void emitted;
} else {
  throw new Error("usage: genMsgpack decode <seed> | multi <seed> | encode <count> <seed>");
}
