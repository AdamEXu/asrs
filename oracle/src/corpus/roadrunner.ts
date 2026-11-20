import { ByteSource, Bytes, hex, toBytes } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";

export type RrSchema =
  | { kind: "struct"; fields: [string, RrSchema][] }
  | { kind: "int" | "long" | "double" | "string" | "boolean" }
  | { kind: "enum"; constants: string[] }
  | { kind: "array"; of: RrSchema }
  | { kind: "raw"; typeId: number };

export type RrValue = number | bigint | boolean | ByteSource | RrValue[] | { [key: string]: RrValue };

export const rr = {
  int: { kind: "int" } as RrSchema,
  long: { kind: "long" } as RrSchema,
  double: { kind: "double" } as RrSchema,
  string: { kind: "string" } as RrSchema,
  boolean: { kind: "boolean" } as RrSchema,
  struct: (...fields: [string, RrSchema][]): RrSchema => ({ kind: "struct", fields }),
  enum: (...constants: string[]): RrSchema => ({ kind: "enum", constants }),
  array: (of: RrSchema): RrSchema => ({ kind: "array", of })
};

const TYPE_IDS = { struct: 0, int: 1, long: 2, double: 3, string: 4, boolean: 5, enum: 6, array: 7 } as const;

function arrayCount(schema: RrSchema): number {
  if (schema.kind === "struct") return schema.fields.reduce((n, [, s]) => n + arrayCount(s), 0);
  if (schema.kind === "array") return 1 + arrayCount(schema.of);
  return 0;
}

/** Builds Road Runner FTC `.log` bytes (big-endian), including revision 0's trailing zero padding per array schema. */
export class RrBuilder {
  private out = new Bytes();
  private schemas: RrSchema[] = [];
  readonly messageStarts: number[] = [];

  constructor(public revision = 1, magic: ByteSource = "RR") {
    this.out.raw(magic).be(revision, 2);
  }

  get length(): number {
    return this.out.length;
  }

  private str(text: ByteSource) {
    const bytes = toBytes(text);
    this.out.be(bytes.length, 4).raw(bytes);
  }

  private schema(s: RrSchema) {
    if (s.kind === "raw") {
      this.out.be(s.typeId, 4);
      return;
    }
    this.out.be(TYPE_IDS[s.kind], 4);
    if (s.kind === "struct") {
      this.out.be(s.fields.length, 4);
      s.fields.forEach(([name, field]) => {
        this.str(name);
        this.schema(field);
      });
    } else if (s.kind === "enum") {
      this.out.be(s.constants.length, 4);
      s.constants.forEach((c) => this.str(c));
    } else if (s.kind === "array") {
      this.schema(s.of);
    }
  }

  /** Declares a channel and returns its id (ids count up from 0 per concatenated log). */
  channel(name: ByteSource, schema: RrSchema, padding = this.revision === 0 ? arrayCount(schema) : 0): number {
    this.messageStarts.push(this.out.length);
    this.out.be(0, 4);
    this.str(name);
    this.schema(schema);
    for (let i = 0; i < padding; i++) this.out.be(0, 4);
    this.schemas.push(schema);
    return this.schemas.length - 1;
  }

  private value(s: RrSchema, v: RrValue) {
    switch (s.kind) {
      case "struct": {
        const obj = v as { [key: string]: RrValue };
        s.fields.forEach(([name, field]) => this.value(field, obj[name]));
        break;
      }
      case "int":
      case "enum":
        this.out.be(v as number, 4);
        break;
      case "long":
        this.out.be(v as bigint, 8);
        break;
      case "double":
        this.out.f64(v as number, false);
        break;
      case "string":
        this.str(v as ByteSource);
        break;
      case "boolean":
        this.out.u8(typeof v === "number" ? v : v ? 1 : 0);
        break;
      case "array": {
        const items = v as RrValue[];
        this.out.be(items.length, 4);
        items.forEach((item) => this.value(s.of, item));
        break;
      }
      case "raw":
        throw new Error("raw schemas have no values");
    }
  }

  message(channel: number, v: RrValue, schema = this.schemas[channel]): this {
    this.messageStarts.push(this.out.length);
    this.out.be(1, 4).be(channel, 4);
    this.value(schema, v);
    return this;
  }

  /** Starts a concatenated log: magic and revision again, channel ids restart. */
  restart(revision: number): this {
    this.out.raw("RR").be(revision, 2);
    this.revision = revision;
    this.schemas = [];
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

const NS = 1000000000n;
const POSE = rr.struct(["x", rr.double], ["y", rr.double], ["heading", rr.double]);

/** A typical op mode: OPMODE_PRE_INIT, PRE_START, TIMESTAMP per loop, Road Runner messages, POST_STOP. */
function opMode(b: RrBuilder, rng: Rng, start: bigint, loops: number) {
  const preInit = b.channel("OPMODE_PRE_INIT", rr.long);
  const preStart = b.channel("OPMODE_PRE_START", rr.long);
  const postStop = b.channel("OPMODE_POST_STOP", rr.long);
  const ts = b.channel("TIMESTAMP", rr.long);
  const pose = b.channel("ESTIMATED_POSE", POSE);
  const target = b.channel("TARGET_POSE", rr.struct(["timestamp", rr.long], ["x", rr.double], ["y", rr.double], ["heading", rr.double]));
  const drive = b.channel("MECANUM_COMMAND", rr.struct(["timestamp", rr.long], ["voltage", rr.double], ["leftFrontPower", rr.double], ["rightBackPower", rr.double]));
  const voltage = b.channel("VOLTAGE", rr.double);
  b.message(preInit, start);
  b.message(preStart, start + 2n * NS);
  let t = start + 2n * NS;
  for (let i = 0; i < loops; i++) {
    t += BigInt(15000000 + rng.below(10000000));
    b.message(ts, t);
    b.message(pose, { x: rng.unit() * 72, y: rng.unit() * 72 - 36, heading: rng.unit() * 6.28 });
    if (i % 3 === 0) b.message(target, { timestamp: t + 1000n, x: i, y: -i, heading: i / 10 });
    b.message(drive, { timestamp: t + 2000n, voltage: 12 + rng.unit(), leftFrontPower: rng.unit() * 2 - 1, rightBackPower: -0.5 });
    b.message(voltage, 12.5 - i / 1000);
  }
  b.message(postStop, t + 5000000n);
}

function typeCases(corpus: Corpus) {
  const add = (id: string, about: string, bytes: Uint8Array) => corpus.add(`roadrunner/${id}`, about, [{ path: `roadrunner/${id}.log`, bytes }]);
  for (const rev of [0, 1]) {
    const b = new RrBuilder(rev);
    const ts = b.channel("TIMESTAMP", rr.long);
    b.message(ts, 1000n * NS);
    const ch = {
      int: b.channel("INT", rr.int),
      long: b.channel("LONG", rr.long),
      double: b.channel("DOUBLE", rr.double),
      string: b.channel("STRING", rr.string),
      boolean: b.channel("BOOLEAN", rr.boolean),
      enumeration: b.channel("ENUM", rr.enum("IDLE", "RUNNING", "")),
      array: b.channel("DOUBLE_ARRAY", rr.array(rr.double)),
      nested: b.channel("NESTED_ARRAY", rr.array(rr.array(rr.int))),
      structArray: b.channel("POSE_ARRAY", rr.array(POSE)),
      struct: b.channel("STRUCT", rr.struct(["a", rr.int], ["inner", rr.struct(["s", rr.string], ["list", rr.array(rr.boolean)])], ["e", rr.enum("A", "B")])),
      pose: b.channel("POSE", POSE),
      poseExtra: b.channel("POSE_EXTRA", rr.struct(["heading", rr.double], ["y", rr.double], ["x", rr.double], ["vel", rr.double])),
      poseInt: b.channel("POSE_INT", rr.struct(["x", rr.int], ["y", rr.int], ["heading", rr.int])),
      empty: b.channel("EMPTY_STRUCT", rr.struct()),
      keyOrder: b.channel("KEY_ORDER", rr.struct(["b", rr.int], ["10", rr.int], ["a", rr.int], ["2", rr.int]))
    };
    const steps = [
      () => b.message(ch.int, -2147483648).message(ch.int, 2147483647),
      () => b.message(ch.long, 2n ** 63n - 1n).message(ch.long, -(2n ** 63n)).message(ch.long, 2n ** 53n + 1n),
      () => b.message(ch.double, NaN).message(ch.double, -0).message(ch.double, Number.MIN_VALUE),
      () => b.message(ch.string, "").message(ch.string, hex("c3 28 f0 9f 98 80")),
      () => b.message(ch.boolean, 0).message(ch.boolean, 2).message(ch.boolean, 255),
      () => b.message(ch.enumeration, 1).message(ch.enumeration, 2).message(ch.enumeration, 3).message(ch.enumeration, -1),
      () => b.message(ch.array, []).message(ch.array, [1.5, -2.5, Infinity]),
      () => b.message(ch.nested, [[1, 2], [], [3]]),
      () => b.message(ch.structArray, [{ x: 1, y: 2, heading: 3 }, { x: -1, y: -2, heading: -3 }]),
      () => b.message(ch.struct, { a: 7, inner: { s: "hi", list: [true, false] }, e: 5 }),
      () => b.message(ch.pose, { x: 72, y: -36.5, heading: Math.PI }),
      () => b.message(ch.poseExtra, { heading: 1, y: 2, x: 3, vel: 4 }),
      () => b.message(ch.poseInt, { x: 1, y: 2, heading: 3 }),
      () => b.message(ch.empty, {}),
      () => b.message(ch.keyOrder, { b: 1, "10": 2, a: 3, "2": 4 })
    ];
    steps.forEach((step, i) => {
      b.message(ts, 1000n * NS + BigInt(i + 1) * 20000000n);
      step();
    });
    add(`rev${rev}/every-schema`, `revision ${rev}: every schema kind, out-of-range enum ordinals, pose detection, integer-like struct keys`, b.bytes());
  }

  const rev0Arrays = new RrBuilder(0);
  const deep = rev0Arrays.channel("DEEP", rr.struct(["a", rr.array(rr.array(rr.double))], ["b", rr.array(rr.int)]));
  rev0Arrays.message(deep, { a: [[1], [2, 3]], b: [4] });
  add("rev0/array-padding", "revision 0 skips 4 zero bytes per array schema after a channel definition (3 here)", rev0Arrays.bytes());

  const rev1Padded = new RrBuilder(1);
  const padded = rev1Padded.channel("ARR", rr.array(rr.int), 1);
  rev1Padded.message(padded, [1, 2]);
  add("rev1/unexpected-padding", "revision 1 with revision 0 style padding: the zero bytes read as an empty channel definition", rev1Padded.bytes());
}

function timestampCases(corpus: Corpus) {
  const add = (id: string, about: string, bytes: Uint8Array) => corpus.add(`roadrunner/${id}`, about, [{ path: `roadrunner/${id}.log`, bytes }]);
  const rng = corpus.rng("roadrunner/opmode");
  const op = new RrBuilder(1);
  opMode(op, rng, 123456789012345678n, 120);
  add("opmode", "a full op mode: PRE_INIT, PRE_START (RUNNING at +50 ms), TIMESTAMP per loop, messages with timestamp fields, POST_STOP", op.bytes());

  const b = new RrBuilder(1);
  const v = b.channel("VALUE", rr.double);
  const ts = b.channel("TIMESTAMP", rr.long);
  const intStamp = b.channel("INT_STAMPED", rr.struct(["timestamp", rr.int], ["v", rr.int]));
  b.message(v, 1);
  b.message(ts, 2n ** 62n);
  b.message(v, 2);
  b.message(ts, 2n ** 62n - 1000n);
  b.message(v, 3);
  b.message(ts, -(2n ** 63n));
  b.message(v, 4);
  b.message(intStamp, { timestamp: 99, v: 5 });
  b.message(v, 6);
  b.message(ts, 2n ** 62n + 7n);
  b.message(v, 7);
  add("timestamps", "values before any timestamp, timestamps going backwards and wrapping, an int timestamp field that is ignored", b.bytes());

  const otherKey = new RrBuilder(1);
  const d = otherKey.channel("TIMESTAMP", rr.double);
  otherKey.message(d, 5);
  add("timestamp-double", "a TIMESTAMP channel with a double schema is an ordinary value", otherKey.bytes());
}

function concatCases(corpus: Corpus) {
  const add = (id: string, about: string, bytes: Uint8Array) => corpus.add(`roadrunner/${id}`, about, [{ path: `roadrunner/${id}.log`, bytes }]);
  const rng = corpus.rng("roadrunner/concat");
  const b = new RrBuilder(1);
  opMode(b, rng, 5000000000n, 30);
  b.restart(0);
  opMode(b, rng, 5000000000n + 40n * NS, 30);
  b.restart(1);
  const swapped = b.channel("VOLTAGE", rr.string);
  b.message(swapped, "channel ids restart, names may change type");
  add("concatenated", "auto then teleop logs concatenated (revisions 1, 0, 1): channel ids reset, first timestamp does not", b.bytes());

  const bad = new RrBuilder(1);
  bad.message(bad.channel("A", rr.int), 1).restart(2);
  bad.message(bad.channel("A", rr.int), 2);
  add("concatenated-bad-revision", "a second log with revision 2: the load fails", bad.bytes());
}

function errorCases(corpus: Corpus) {
  const add = (id: string, about: string, bytes: Uint8Array) => corpus.add(`roadrunner/errors/${id}`, about, [{ path: `roadrunner/errors/${id}.log`, bytes }]);
  add("empty", "zero bytes: the magic is not RR", new Uint8Array());
  add("magic-only", "\"RR\" and nothing else: reading the revision throws", hex("52 52"));
  add("bad-magic", "magic \"rr\"", new RrBuilder(1, "rr").bytes());
  add("revision-2", "revision 2 is unsupported", new RrBuilder(2).bytes());
  add("revision-negative", "revision -1 is unsupported", new RrBuilder(-1).bytes());
  add("header-only", "a valid header with no messages: an empty log", new RrBuilder(1).bytes());

  const unknownSchema = new RrBuilder(1);
  unknownSchema.channel("OK", rr.int);
  unknownSchema.channel("BAD", { kind: "raw", typeId: 8 });
  add("unknown-schema-type", "schema type 8 throws a string: the load fails", unknownSchema.bytes());

  const unknownChannel = new RrBuilder(1);
  unknownChannel.message(unknownChannel.channel("A", rr.int), 1);
  unknownChannel.message(5, 0, rr.int);
  add("unknown-channel", "a message for channel 5 of 1: readMsg throws a string and the load fails", unknownChannel.bytes());

  const unknownType = new RrBuilder(1);
  unknownType.message(unknownType.channel("A", rr.int), 1).raw(hex("00 00 00 02 00 00 00 00"));
  unknownType.message(0, 2);
  add("message-type-2", "message type 2 is ignored and its following bytes are read as the next message type", unknownType.bytes());

  const midstreamMagic = new RrBuilder(1);
  midstreamMagic.message(midstreamMagic.channel("A", rr.int), 1).raw("RR").raw(hex("00 01"));
  midstreamMagic.message(1, 2, rr.int);
  add("midstream-magic", "\"RR\" at a message boundary starts a new log, so channel 1 of the old log is unknown and the load fails", midstreamMagic.bytes());

  const negLen = new RrBuilder(1);
  negLen.channel("S", rr.string);
  negLen.raw(hex("00 00 00 01 00 00 00 00 ff ff ff fe 41 42"));
  add("negative-string-length", "a string with length -2: decodes empty and moves the offset back", negLen.bytes());
}

function truncationCases(corpus: Corpus) {
  const b = new RrBuilder(1);
  const ts = b.channel("TIMESTAMP", rr.long);
  const s = b.channel("S", rr.struct(["name", rr.string], ["v", rr.array(rr.double)]));
  b.message(ts, 100n).message(s, { name: "first", v: [1] }).message(ts, 200n).message(s, { name: "second", v: [2, 3] });
  const full = b.bytes();
  const last = b.messageStarts[b.messageStarts.length - 1];
  for (let at = last + 1; at < full.length; at++) {
    corpus.add(`roadrunner/truncate/at-${at}`, `cut at byte ${at} of ${full.length}, inside the last message`, [{ path: `roadrunner/truncate/at-${at}.log`, bytes: full.slice(0, at) }]);
  }
  const defStart = b.messageStarts[1];
  for (const at of [defStart + 2, defStart + 9, defStart + 14]) {
    corpus.add(`roadrunner/truncate/definition-at-${at}`, `cut at byte ${at}, inside a channel definition`, [
      { path: `roadrunner/truncate/definition-at-${at}.log`, bytes: full.slice(0, at) }
    ]);
  }
}

function randomSchema(rng: Rng, depth: number): RrSchema {
  const kinds = depth > 2 ? ["int", "long", "double", "string", "boolean", "enum"] : ["int", "long", "double", "string", "boolean", "enum", "array", "struct"];
  const kind = rng.pick(kinds);
  if (kind === "struct") {
    const names = ["x", "y", "heading", "timestamp", "f0", "f1"];
    const fields: [string, RrSchema][] = [];
    for (let n = rng.below(4); n > 0; n--) fields.push([names.splice(rng.below(names.length), 1)[0], randomSchema(rng, depth + 1)]);
    return rr.struct(...fields);
  }
  if (kind === "enum") return rr.enum(...Array.from({ length: 1 + rng.below(3) }, (_, i) => `C${i}`));
  if (kind === "array") return rr.array(randomSchema(rng, depth + 1));
  return { kind } as RrSchema;
}

function randomValue(rng: Rng, s: RrSchema): RrValue {
  switch (s.kind) {
    case "struct":
      return Object.fromEntries(s.fields.map(([name, f]) => [name, randomValue(rng, f)]));
    case "int":
      return rng.range(-1000, 1000);
    case "long":
      return rng.u64();
    case "double":
      return rng.double();
    case "string":
      return rng.ascii(6);
    case "boolean":
      return rng.bool();
    case "enum":
      return rng.below(s.constants.length + 1);
    case "array":
      return Array.from({ length: rng.below(3) }, () => randomValue(rng, s.of));
    default:
      return 0;
  }
}

export function randomRoadRunner(rng: Rng, messages: number): Uint8Array {
  const b = new RrBuilder(rng.below(2));
  const schemas: RrSchema[] = [];
  const ts = b.channel("TIMESTAMP", rr.long);
  schemas.push(rr.long);
  let t = rng.u64() >> 2n;
  for (let i = 0; i < messages; i++) {
    if (schemas.length < 10 && rng.below(4) === 0) {
      const s = randomSchema(rng, 0);
      b.channel(`CH_${schemas.length}`, s);
      schemas.push(s);
    }
    if (rng.below(3) === 0) {
      t += BigInt(rng.below(50000000));
      b.message(ts, t);
    } else {
      const id = rng.below(schemas.length);
      b.message(id, randomValue(rng, schemas[id]));
    }
  }
  return b.bytes();
}

export function roadRunnerMatrix(corpus: Corpus): void {
  typeCases(corpus);
  timestampCases(corpus);
  concatCases(corpus);
  errorCases(corpus);
  truncationCases(corpus);
  for (let i = 0; i < 4; i++) {
    const id = `roadrunner/random/${i}`;
    corpus.add(id, "seeded random channels and messages", [{ path: `${id}.log`, bytes: randomRoadRunner(corpus.rng(id), 60 + 40 * i) }]);
  }
}
