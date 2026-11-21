import { ByteSource, Bytes, bitsToF64, hex, toBytes, utf8 } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";

/** RLOG R1 value tags as RLOGDecoder reads them. */
export const R1 = {
  null: 0,
  boolean: 1,
  booleanArray: 2,
  integer: 3,
  integerArray: 4,
  double: 5,
  doubleArray: 6,
  string: 7,
  stringArray: 8,
  byte: 9,
  byteArray: 10
} as const;

/**
 * Builds RLOG bytes. Every cycle is written as a 0x00 "new timestamp" marker followed by a big-endian f64; the
 * marker of the first cycle doubles as the header byte the decoder skips.
 */
export class RlogBuilder {
  private out = new Bytes();
  readonly cycleStarts: number[] = [];

  constructor(readonly revision: number) {
    this.out.u8(revision);
  }

  get length(): number {
    return this.out.length;
  }

  cycle(timestamp: number): this {
    this.cycleStarts.push(this.out.length);
    this.out.u8(0).f64(timestamp, false);
    return this;
  }

  cycleBits(bits: bigint): this {
    return this.cycle(bitsToF64(bits));
  }

  key(id: number, name: ByteSource, type?: ByteSource): this {
    const nameBytes = toBytes(name);
    this.out.u8(1).be(id, 2).be(nameBytes.length, 2).raw(nameBytes);
    if (this.revision === 2 || type !== undefined) {
      const typeBytes = toBytes(type ?? "");
      this.out.be(typeBytes.length, 2).raw(typeBytes);
    }
    return this;
  }

  /** An R2 value update. `length` overrides the declared length. */
  value(id: number, data: ByteSource, length?: number): this {
    const bytes = toBytes(data);
    this.out.u8(2).be(id, 2).be(length ?? bytes.length, 2).raw(bytes);
    return this;
  }

  /** An R1 value update: tag byte then the tag's body. */
  r1(id: number, tag: number, body: ByteSource = []): this {
    this.out.u8(2).be(id, 2).u8(tag).raw(body);
    return this;
  }

  raw(bytes: ByteSource): this {
    this.out.raw(bytes);
    return this;
  }

  bytes(): Uint8Array {
    return this.out.finish();
  }
}

export const r1Body = {
  boolean: (v: boolean) => Uint8Array.of(v ? 1 : 0),
  byte: (v: number) => Uint8Array.of(v),
  integer: (v: number) => new Bytes().be(v, 4).finish(),
  double: (v: number) => new Bytes().f64(v, false).finish(),
  string: (v: ByteSource) => {
    const b = toBytes(v);
    return new Bytes().be(b.length, 2).raw(b).finish();
  },
  booleanArray: (v: boolean[]) => new Bytes().be(v.length, 2).raw(v.map((x) => (x ? 1 : 0))).finish(),
  byteArray: (v: number[]) => new Bytes().be(v.length, 2).raw(v).finish(),
  integerArray: (v: number[]) => {
    const b = new Bytes().be(v.length, 2);
    v.forEach((x) => b.be(x, 4));
    return b.finish();
  },
  doubleArray: (v: number[]) => {
    const b = new Bytes().be(v.length, 2);
    v.forEach((x) => b.f64(x, false));
    return b.finish();
  },
  stringArray: (v: ByteSource[]) => {
    const b = new Bytes().be(v.length, 2);
    v.forEach((s) => {
      const bytes = toBytes(s);
      b.be(bytes.length, 2).raw(bytes);
    });
    return b.finish();
  }
};

/** R2 payloads are WPILOG payloads with every number big-endian. */
export const r2Payload = {
  boolean: (v: boolean) => Uint8Array.of(v ? 1 : 0),
  int64: (v: bigint | number) => new Bytes().be(v, 8).finish(),
  float: (v: number) => new Bytes().f32(v, false).finish(),
  double: (v: number) => new Bytes().f64(v, false).finish(),
  int64Array: (v: (bigint | number)[]) => {
    const b = new Bytes();
    v.forEach((x) => b.be(x, 8));
    return b.finish();
  },
  floatArray: (v: number[]) => {
    const b = new Bytes();
    v.forEach((x) => b.f32(x, false));
    return b.finish();
  },
  doubleArray: (v: number[]) => {
    const b = new Bytes();
    v.forEach((x) => b.f64(x, false));
    return b.finish();
  },
  stringArray: (v: ByteSource[], count = v.length) => {
    const b = new Bytes().be(count, 4);
    v.forEach((s) => {
      const bytes = toBytes(s);
      b.be(bytes.length, 4).raw(bytes);
    });
    return b.finish();
  }
};

type R2Gen = (r: Rng) => Uint8Array;

const R2_TYPES: [string, R2Gen][] = [
  ["boolean", (r) => r2Payload.boolean(r.bool())],
  ["int", (r) => r2Payload.int64(BigInt(r.range(-100000, 100000)))],
  ["int64", (r) => r2Payload.int64(r.below(3) === 0 ? r.u64() : BigInt(r.range(-9, 9)))],
  ["float", (r) => r2Payload.float(r.double())],
  ["double", (r) => r2Payload.double(r.double())],
  ["string", (r) => utf8(r.ascii(12))],
  ["boolean[]", (r) => Uint8Array.from({ length: r.below(6) }, () => r.below(3))],
  ["int[]", (r) => r2Payload.int64Array(Array.from({ length: r.below(5) }, () => BigInt(r.range(-50, 50))))],
  ["int64[]", (r) => r2Payload.int64Array(Array.from({ length: r.below(5) }, () => r.u64()))],
  ["float[]", (r) => r2Payload.floatArray(Array.from({ length: r.below(5) }, () => r.double()))],
  ["double[]", (r) => r2Payload.doubleArray(Array.from({ length: r.below(5) }, () => r.double()))],
  ["string[]", (r) => r2Payload.stringArray(Array.from({ length: r.below(4) }, () => r.ascii(5)))],
  ["json", (r) => utf8(JSON.stringify({ v: r.range(0, 99), "2": r.bool(), s: [r.ascii(3)] }))],
  ["msgpack", (r) => Uint8Array.of(0x82, 0xa1, 0x78, r.below(0x80), 0xa1, 0x79, r.bool() ? 0xc3 : 0xc2)],
  ["raw", (r) => r.bytes(r.below(10))],
  ["unknown:custom", (r) => r.bytes(r.below(6))]
];

const SECOND_STEP = 0.02;

/** Every R2 type plus struct and child-schema keys, written the way AdvantageKit's legacy RLOG writer does. */
function r2EveryType(corpus: Corpus) {
  const rng = corpus.rng("rlog/r2/every-type");
  const b = new RlogBuilder(2);
  b.cycle(0);
  R2_TYPES.forEach(([type], i) => b.key(i, `/Types/${type}`, type));
  b.key(100, "/.schema/struct:Translation2d", "structschema");
  b.key(101, "/.schema/struct:Pose2d", "structschema");
  b.key(102, "/Types/Pose2d", "struct:Pose2d");
  b.key(103, "/Types/Translation2d[]", "struct:Translation2d[]");
  b.key(104, "/Types/LatePose", "struct:LatePose");
  b.key(105, "/Types/photon", "photonstruct:Missing");
  b.key(106, "/Types/proto", "proto:Missing");
  b.value(100, "double x;double y").value(101, "Translation2d translation;Rotation2d rotation");
  for (let cycle = 1; cycle <= 6; cycle++) {
    b.cycle(cycle * SECOND_STEP);
    R2_TYPES.forEach(([, gen], i) => b.value(i, gen(rng)));
    const pose = new Bytes().f64(cycle).f64(-cycle).f64(cycle / 10).finish();
    b.value(102, pose).value(103, new Bytes().f64(1).f64(2).f64(3).f64(4).finish());
    b.value(104, new Bytes().f64(cycle).finish());
    b.value(105, rng.bytes(4)).value(106, rng.bytes(4));
    if (cycle === 3) {
      b.key(107, "/.schema/struct:Rotation2d", "structschema").value(107, "double value");
      b.key(108, "/.schema/struct:LatePose", "structschema").value(108, "double z");
    }
  }
  corpus.add("rlog/r2/every-type", "every RLOG R2 type string, struct values before and after their schema arrives, unknown photon/proto schemas", [
    { path: "rlog/r2/every-type.rlog", bytes: b.bytes() }
  ]);
}

function r2Edges(corpus: Corpus) {
  const file = (id: string, about: string, bytes: Uint8Array) =>
    corpus.add(`rlog/r2/${id}`, about, [{ path: `rlog/r2/${id}.rlog`, bytes }]);

  const int64 = new RlogBuilder(2).cycle(1).key(1, "/I", "int64").key(2, "/F", "float").key(3, "/D", "double");
  const extremes = [0n, -1n, 2n ** 53n + 1n, 2n ** 63n - 1n, -(2n ** 63n), 0x7ffffffffffffdffn];
  const floatBits = [0x7fc00001, 0xff800000, 0x00000001, 0x80000000, 0x3dcccccd];
  const doubleBits = [0x7ff8000000000001n, 0xfff0000000000000n, 0x8000000000000000n, 0x0000000000000001n, 0x3fb999999999999an];
  for (let i = 0; i < extremes.length; i++) {
    int64.cycle(1 + (i + 1) * SECOND_STEP).value(1, r2Payload.int64(extremes[i]));
    if (i < floatBits.length) int64.value(2, new Bytes().be(floatBits[i], 4).finish());
    if (i < doubleBits.length) int64.value(3, new Bytes().be(doubleBits[i], 8).finish());
  }
  file("numeric-edges", "big-endian int64 extremes past 2^53, float32 and float64 NaN payloads, infinities, -0 and subnormals", int64.bytes());

  const lengthErrors: [string, string, Uint8Array][] = [
    ["boolean", "boolean of 2 bytes", Uint8Array.of(1, 0)],
    ["int64", "int64 of 7 bytes", new Uint8Array(7)],
    ["float", "float of 8 bytes", new Uint8Array(8)],
    ["double", "double of 4 bytes", new Uint8Array(4)],
    ["int64[]", "int64[] of 12 bytes", new Uint8Array(12)],
    ["float[]", "float[] of 6 bytes", new Uint8Array(6)],
    ["double[]", "double[] of 9 bytes", new Uint8Array(9)],
    ["string[]", "string[] whose count exceeds (length - 4) / 4", r2Payload.stringArray(["a"], 3)]
  ];
  for (const [type, about, data] of lengthErrors) {
    const b = new RlogBuilder(2).cycle(0).key(1, "/Good", "double").value(1, r2Payload.double(1));
    b.cycle(0.02).key(2, "/Bad", type).value(2, data).cycle(0.04).value(1, r2Payload.double(2));
    file(`length-${type.replace("[]", "-array")}`, `${about}: the decoder throws and the whole load fails`, b.bytes());
  }

  const strings = new RlogBuilder(2).cycle(0).key(1, "/S", "string").key(2, "/SA", "string[]").key(3, "/J", "json");
  const texts = [hex("ef bb bf 41"), hex("c3 28"), hex("ed a0 80"), hex("f0 9f 98 80"), utf8(""), hex("00 0a 0d")];
  texts.forEach((t, i) => strings.cycle((i + 1) * 0.02).value(1, t).value(2, r2Payload.stringArray([t, "x"])));
  strings.cycle(1).value(3, '{"1":true,"a":[1,2],"0":null}').cycle(1.02).value(3, "not json");
  file("strings", "UTF-8 decoding of BOMs, invalid and surrogate bytes in string and string[] values, and an invalid JSON value", strings.bytes());

  const stringArrayEdges = new RlogBuilder(2).cycle(0).key(1, "/SA", "string[]");
  stringArrayEdges.cycle(0.02).value(1, r2Payload.stringArray([]));
  stringArrayEdges.cycle(0.04).value(1, new Bytes().be(1, 4).be(100, 4).raw("abc").finish());
  stringArrayEdges.cycle(0.06).value(1, new Bytes().be(2, 4).be(1, 4).raw("axyz").finish());
  stringArrayEdges.cycle(0.08).value(1, r2Payload.stringArray(["end", "of", "file"]));
  file("string-array-overrun", "string[] whose element sizes run past the value: sizes are read from the following record bytes", stringArrayEdges.bytes());
  const overrunEnd = new RlogBuilder(2).cycle(0).key(1, "/SA", "string[]").value(1, new Bytes().be(2, 4).be(1, 4).raw("axyz").finish());
  file("string-array-overrun-eof", "a string[] size read past the end of the file throws and the load fails", overrunEnd.bytes());

  const keys = new RlogBuilder(2).cycle(0).key(1, "/A", "double").value(1, r2Payload.double(1));
  keys.cycle(0.02).key(1, "/A", "string").value(1, "now a string");
  keys.cycle(0.04).key(1, "/B", "boolean").value(1, r2Payload.boolean(true));
  keys.cycle(0.06).key(-2, "/Negative", "double").value(-2, r2Payload.double(-2));
  keys.cycle(0.1).key(3, "", "double").value(3, r2Payload.double(3));
  keys.cycle(0.12).key(4, "/Caf\u00e9", "double").value(4, r2Payload.double(4));
  file("key-ids", "key id redeclared with a new type and a new name, negative id, empty and non-ASCII key names", keys.bytes());

  const undeclared = new RlogBuilder(2).cycle(0).key(1, "/A", "double").value(1, r2Payload.double(1));
  undeclared.cycle(0.02).value(77, r2Payload.double(77)).cycle(0.04).value(1, r2Payload.double(2));
  file("undeclared-id", "a value for an id that was never declared: writing to an undefined key throws and the load fails", undeclared.bytes());

  const urcl = new RlogBuilder(2).cycle(0);
  urcl.key(1, "/URCL/Raw/Aliases", "URCLr3_aliases").key(2, "/URCL/Raw/Persistent", "URCLr3_persistent");
  urcl.key(3, "/URCL/Raw/Periodic", "URCLr3_periodic");
  urcl.value(1, '{"5":"Shooter"}').value(2, hex("05 26 1a 00 07 00 00 00"));
  urcl.cycle(0.02).value(3, new Bytes().le(20, 4).le((46 << 10) | (1 << 6) | 5, 2).raw(hex("00 00 48 42 00 00 a0 41")).finish());
  file("urcl", "URCL r3 fields inside an RLOG file, decoded in file order", urcl.bytes());
}

function r1Cases(corpus: Corpus) {
  const file = (id: string, about: string, bytes: Uint8Array) =>
    corpus.add(`rlog/r1/${id}`, about, [{ path: `rlog/r1/${id}.rlog`, bytes }]);
  const rng = corpus.rng("rlog/r1/every-tag");

  const every = new RlogBuilder(1).cycle(0);
  const tags = Object.entries(R1).filter(([name]) => name !== "null");
  tags.forEach(([name], i) => every.key(i + 1, `/Tags/${name}`));
  for (let cycle = 1; cycle <= 4; cycle++) {
    every.cycle(cycle * 0.02);
    every.r1(1, R1.boolean, r1Body.boolean(rng.bool()));
    every.r1(2, R1.booleanArray, r1Body.booleanArray([rng.bool(), rng.bool()]));
    every.r1(3, R1.integer, r1Body.integer(rng.u32()));
    every.r1(4, R1.integerArray, r1Body.integerArray([rng.u32(), -1, 0x7fffffff]));
    every.r1(5, R1.double, r1Body.double(rng.double()));
    every.r1(6, R1.doubleArray, r1Body.doubleArray([rng.double(), rng.double()]));
    every.r1(7, R1.string, r1Body.string(rng.ascii(8)));
    every.r1(8, R1.stringArray, r1Body.stringArray([rng.ascii(3), "", hex("c3 28")]));
    every.r1(9, R1.byte, r1Body.byte(rng.below(256)));
    every.r1(10, R1.byteArray, r1Body.byteArray(Array.from(rng.bytes(5))));
  }
  every.cycle(0.1);
  tags.forEach((_, i) => every.r1(i + 1, R1.null));
  file("every-tag", "every RLOG R1 value tag, then a null for each key, which writes the type's default value", every.bytes());

  const nulls = new RlogBuilder(1).cycle(0).key(1, "/Never").key(2, "/Later");
  nulls.r1(1, R1.null).r1(2, R1.null).cycle(0.02).r1(2, R1.double, r1Body.double(5)).cycle(0.04).r1(2, R1.null);
  file("null-without-type", "null for a key that has no value yet (nothing written) and after a double", nulls.bytes());

  const unknownTag = new RlogBuilder(1).cycle(0).key(1, "/A").r1(1, 11, hex("00 00 00 05")).r1(1, R1.double, r1Body.double(1));
  unknownTag.cycle(0.02).r1(1, R1.double, r1Body.double(2));
  file("unknown-tag", "R1 value tag 11 has no case: the body bytes are then read as the next record types", unknownTag.bytes());

  const negative = new RlogBuilder(1).cycle(0).key(1, "/A").r1(1, R1.string, hex("ff ff"));
  negative.cycle(0.02).r1(1, R1.booleanArray, hex("80 00")).cycle(0.04).r1(1, R1.double, r1Body.double(4));
  file("negative-lengths", "negative i16 string and array lengths", negative.bytes());

  const r2Keys = new RlogBuilder(1).cycle(0).key(1, "/Typed", "double").r1(1, R1.double, r1Body.double(1));
  file("typed-key", "an R1 file whose key record carries an R2-style type string, desynchronising the reader", r2Keys.bytes());
}

function timestampCases(corpus: Corpus) {
  const file = (id: string, about: string, bytes: Uint8Array) =>
    corpus.add(`rlog/timestamps/${id}`, about, [{ path: `rlog/timestamps/${id}.rlog`, bytes }]);
  const base = (rev: number) => new RlogBuilder(rev).cycle(0.5).key(1, "/V", rev === 2 ? "double" : undefined);
  const put = (b: RlogBuilder, v: number) => (b.revision === 2 ? b.value(1, r2Payload.double(v)) : b.r1(1, R1.double, r1Body.double(v)));

  for (const rev of [1, 2]) {
    const steps = base(rev);
    put(steps, 0);
    const series = [0.5001, 0.6, 0.60005, 0.61, 60.61, 60.62, 120.63, 61, 61.5];
    series.forEach((t, i) => put(steps.cycle(t), i + 1));
    file(`r${rev}-step-limits`, "timestamp steps exactly at, under and over the 0.0001 s and 60 s corruption limits", steps.bytes());

    const negative = new RlogBuilder(rev).cycle(-5).key(1, "/V", rev === 2 ? "double" : undefined);
    put(negative, 1);
    put(negative.cycle(-1), 2);
    put(negative.cycle(0.001), 3);
    put(negative.cycle(-0), 4);
    file(`r${rev}-negative`, "negative timestamps clamp to 0, so a second negative one is a corrupt step", negative.bytes());

    const special = base(rev);
    put(special, 0);
    put(special.cycleBits(0x7ff8000000000000n), 1);
    put(special.cycle(Infinity), 2);
    put(special.cycle(1), 3);
    put(special.cycleBits(0xfff0000000000000n), 4);
    put(special.cycle(1.5), 5);
    file(`r${rev}-nan-infinity`, "NaN and Infinity timestamps are corrupt; -Infinity clamps to 0 and is too small", special.bytes());

    const junk = base(rev);
    put(junk, 0);
    junk.raw(hex("de ad be ef 00 01 02"));
    put(junk.cycle(0.6), 1);
    junk.raw(Uint8Array.from({ length: 23 }, (_, i) => (i * 37) & 0xff));
    put(junk.cycle(0.7), 2);
    put(junk.cycle(0.8), 3);
    file(`r${rev}-junk-resync`, "junk bytes between cycles force the one-byte-at-a-time resync scan", junk.bytes());

    const tail = base(rev);
    put(tail, 0);
    put(tail.cycle(0.6), 1);
    put(tail.cycle(0.55), 2);
    file(`r${rev}-corrupt-tail`, "a backwards final timestamp starts the resync scan over the file's last bytes", tail.bytes());
  }

  const overflow = new RlogBuilder(2).cycle(1.8e299).key(1, "/Timestamp", "double").key(2, "/V", "double");
  overflow.value(1, r2Payload.double(1)).value(2, r2Payload.double(2));
  file(
    "r2-overflow-nanos",
    "a timestamp whose nanoseconds overflow to Infinity, so BigInt throws: changes MCAP export rejects, sampled export drops every message",
    overflow.bytes()
  );
}

function emptyAndHeader(corpus: Corpus) {
  const file = (id: string, about: string, bytes: Uint8Array) =>
    corpus.add(`rlog/header/${id}`, about, [{ path: `rlog/header/${id}.rlog`, bytes }]);
  file("empty", "zero bytes: revision is undefined and the load fails", new Uint8Array());
  file("revision-0", "revision byte 0 is unsupported", Uint8Array.of(0, 0));
  file("revision-3", "revision byte 3 is unsupported", Uint8Array.of(3, 0));
  file("revision-only", "only the revision byte: no cycles", Uint8Array.of(2));
  file("header-only", "revision and marker bytes with no timestamp", Uint8Array.of(2, 0));
  file("short-timestamp", "a first timestamp of 5 bytes: DataView throws and the load fails", Uint8Array.of(2, 0, 0x3f, 0xf0, 0, 0, 0));
  file("second-byte-nonzero", "the skipped second byte is not 0", new RlogBuilder(2).raw(hex("ff")).bytes());
  const ok = new RlogBuilder(2).cycle(0).key(1, "/A", "double").value(1, r2Payload.double(1)).bytes();
  const odd = Uint8Array.from(ok);
  odd[1] = 0x7f;
  file("second-byte-ignored", "the second byte is skipped unread whatever its value", odd);
}

function truncationCases(corpus: Corpus) {
  for (const rev of [1, 2]) {
    const b = new RlogBuilder(rev).cycle(0).key(1, "/A", rev === 2 ? "double" : undefined).key(2, "/S", rev === 2 ? "string" : undefined);
    const putD = (v: number) => (rev === 2 ? b.value(1, r2Payload.double(v)) : b.r1(1, R1.double, r1Body.double(v)));
    const putS = (v: string) => (rev === 2 ? b.value(2, v) : b.r1(2, R1.string, r1Body.string(v)));
    putD(1);
    putS("first");
    b.cycle(0.02);
    putD(2);
    putS("second");
    const full = b.bytes();
    const lastCycle = b.cycleStarts[b.cycleStarts.length - 1];
    for (let at = lastCycle; at < full.length; at++) {
      corpus.add(
        `rlog/truncate/r${rev}/at-${at}`,
        `R${rev} file cut at byte ${at} of ${full.length} (inside the last cycle)`,
        [{ path: `rlog/truncate/r${rev}/at-${at}.rlog`, bytes: full.slice(0, at) }]
      );
    }
  }
}

export function randomRlog(rng: Rng, revision: number, cycles: number): Uint8Array {
  const b = new RlogBuilder(revision);
  const keys: { id: number; type: string; tag: number }[] = [];
  let t = rng.unit() * 10;
  for (let c = 0; c < cycles; c++) {
    b.cycle(t);
    t += 0.0002 + rng.unit() * 0.05;
    if (keys.length < 12 && (keys.length === 0 || rng.below(3) === 0)) {
      const id = keys.length;
      const [type] = rng.pick(R2_TYPES);
      const tag = rng.pick(Object.values(R1));
      keys.push({ id, type, tag });
      b.key(id, `/Random/${id}/${type}`, revision === 2 ? type : undefined);
    }
    const updates = rng.below(keys.length + 1);
    for (let u = 0; u < updates; u++) {
      const k = rng.pick(keys);
      if (revision === 2) b.value(k.id, R2_TYPES.find(([type]) => type === k.type)![1](rng));
      else b.r1(k.id, k.tag, randomR1Body(rng, k.tag));
    }
  }
  return b.bytes();
}

function randomR1Body(rng: Rng, tag: number): Uint8Array {
  switch (tag) {
    case R1.boolean:
      return r1Body.boolean(rng.bool());
    case R1.byte:
      return r1Body.byte(rng.below(256));
    case R1.integer:
      return r1Body.integer(rng.u32());
    case R1.double:
      return r1Body.double(rng.double());
    case R1.string:
      return r1Body.string(rng.ascii(10));
    case R1.booleanArray:
      return r1Body.booleanArray(Array.from({ length: rng.below(4) }, () => rng.bool()));
    case R1.byteArray:
      return r1Body.byteArray(Array.from(rng.bytes(rng.below(5))));
    case R1.integerArray:
      return r1Body.integerArray(Array.from({ length: rng.below(4) }, () => rng.u32()));
    case R1.doubleArray:
      return r1Body.doubleArray(Array.from({ length: rng.below(4) }, () => rng.double()));
    case R1.stringArray:
      return r1Body.stringArray(Array.from({ length: rng.below(4) }, () => rng.ascii(4)));
    default:
      return new Uint8Array();
  }
}

export function rlogMatrix(corpus: Corpus): void {
  r2EveryType(corpus);
  r2Edges(corpus);
  r1Cases(corpus);
  timestampCases(corpus);
  emptyAndHeader(corpus);
  truncationCases(corpus);
  for (const rev of [1, 2]) {
    for (let i = 0; i < 4; i++) {
      const id = `rlog/random/r${rev}-${i}`;
      corpus.add(id, `seeded random R${rev} log, ${50 * (i + 1)} cycles`, [
        { path: `${id}.rlog`, bytes: randomRlog(corpus.rng(id), rev, 50 * (i + 1)) }
      ]);
    }
  }
}
