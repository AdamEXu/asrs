import "./shims";
import PhotonStructDecoder from "@upstream/shared/log/PhotonStructDecoder";
import { utf8 } from "./corpus/bytes";
import { Corpus } from "./corpus/corpus";
import { photonMatrix } from "./corpus/photon";
import { Rng } from "./corpus/rng";
import { encode, fromTree } from "./cve1";

// Emits PhotonStructDecoder ground truth for the asrs_log photon vectors.
//   genPhoton <seed>
//     {"fn":"oracle:photon","args":[[<op>...]],"ok":[<result>...]}
//     One record per session; each op is ["addSchema", name, bytes],
//     ["decode", name, bytes] or ["decodeImpl", name, bytes], and each result is
//     {"ok": value} or {"throw": error}. Records cover the corpus photon matrix
//     (schema puts and decodes in file order), schema-compile edges and seeded
//     schema/payload fuzz with per-byte truncations for short-buffer errors.

const USAGE = "usage: genPhoton <seed>";
const seed = Number(process.argv[2]);
if (!Number.isInteger(seed) || seed < 0 || seed > 0xffffffff) throw new Error(USAGE);

type Op = [kind: string, name: string, bytes: Uint8Array];

function run(ops: Op[]): unknown[] {
  const decoder = new PhotonStructDecoder();
  return ops.map((op) => {
    try {
      switch (op[0]) {
        case "addSchema":
          decoder.addSchema(op[1], op[2]);
          return { ok: Object.keys(decoder.getSchemas()) };
        case "decode":
          return { ok: decoder.decode(op[1], op[2]) };
        case "decodeImpl":
          return { ok: decoder.decodeImpl(op[1], op[2]) };
        default:
          throw new Error(`oracle:photon: bad op ${op[0]}`);
      }
    } catch (e) {
      return { throw: e };
    }
  });
}

function emit(ops: Op[]): void {
  process.stdout.write(`{"fn":"oracle:photon","args":[${encode(ops)}],"ok":${encode(run(ops))}}\n`);
}

// The corpus photon matrix, as decoder ops in the order Log.ts would see them.
const SCHEMA_KEY = /(?:^|\/)\.schema\/(.*)/;
const corpus = new Corpus(seed);
photonMatrix(corpus);
for (const c of corpus.cases) {
  if (!c.id.startsWith("payload/photon/")) continue;
  const file = c.files.find((f) => f.path === `${c.id}.ops.jsonl`);
  if (file === undefined) continue;
  const ops: Op[] = [];
  for (const line of new TextDecoder().decode(file.bytes).split("\n")) {
    if (line === "") continue;
    const { fn, args } = JSON.parse(line) as { fn: string; args: unknown[] };
    const decoded = args.map((a) => fromTree(a));
    if (fn === "putRaw") {
      const schemaType = String(decoded[0]).match(SCHEMA_KEY)?.[1];
      if (schemaType?.startsWith("struct:")) {
        ops.push(["addSchema", schemaType.slice("struct:".length), decoded[2] as Uint8Array]);
      } else if (schemaType?.startsWith("photonstruct:")) {
        ops.push(["addSchema", schemaType.slice("photonstruct:".length), decoded[2] as Uint8Array]);
      }
    } else if (fn === "putPhotonStruct") {
      ops.push(["decode", decoded[3] as string, decoded[2] as Uint8Array]);
    }
  }
  emit(ops);
}

const rng = new Rng(seed, "genPhoton");
const zeros = (n: number) => new Uint8Array(n);
const seq = (n: number) => Uint8Array.from({ length: n }, (_, i) => i & 0xff);
const add = (name: string, text: string): Op => ["addSchema", name, utf8(text)];
const dec = (name: string, bytes: Uint8Array): Op => ["decode", name, bytes];

// Compile-time edges: every declaration shape compileSchema sees.
for (const [name, text] of [
  ["Empty", ""],
  ["Semis", ";;;"],
  ["NoType", " a;"],
  ["BareName", "int8;"],
  ["NoSemi", "int8 a"],
  ["ExtraSpaces", "  int8   a  ;"],
  ["OptNoTrail", "optional"],
  ["OptPrefix", "optionalx int8 a;"],
  ["OptDouble", "optional  int8 a;"],
  ["OptArr", "optional int8 a[?];"],
  ["FixedArr", "int8 a[3];"],
  ["EmptyArr", "int8 a[];"],
  ["DoubleArr", "int8 a[?][?];"],
  ["NegArr", "int8 a[-1];"],
  ["ArrJunk", "int8 a[?]junk;"],
  ["ArrOpen", "int8 a[?;"],
  ["Bracket", "int8 [;"],
  ["Tab", "int8\ta;"],
  ["Newline", "int8 a;\nfloat b;"],
  ["MultiName", "int8 a b c;"],
  ["UnknownType", "nonexistent x;"],
  ["CaseType", "Int8 a;"],
  ["SlashName", "int8 x/length;"],
  ["IntName", "int8 0;int8 2;int8 z;"],
  ["BigName", "int8 4294967294;int8 4294967295;"],
  ["ProtoName", "bool __proto__;int8 ok;"],
  ["ProtoType", "hasOwnProperty x;"],
  ["CtorType", "constructor x;"],
  ["Self", "Self x;"],
  ["Undef", " ;int8 ok;"]
] as [string, string][]) {
  emit([add(name, text), dec(name, seq(64)), dec(name, zeros(8))]);
}

// Compile order and dependency resolution.
emit([add("Dep", "Dep child;"), add("Dep2", "Dep c;int8 v;"), dec("Dep2", seq(32))]);
emit([
  add("First", "Missing m;int8 v;"),
  dec("First", seq(16)),
  add("Missing", "int8 a;"),
  dec("First", seq(16))
]);
emit([
  add("undefined", "int8 a;"),
  add("HasUndef", " ;float f;"),
  dec("HasUndef", seq(16)),
  dec("undefined", seq(4))
]);
emit([add("0", "int8 a;"), add("1", "0 x;"), dec("1", seq(8))]);
emit([dec("toString", seq(4)), dec("__proto__", seq(4)), dec("hasOwnProperty", seq(4)), dec("Nope", seq(4))]);
emit([add("Dup", "int8 a;"), add("Dup", "float64 b;"), dec("Dup", seq(16))]);

// Every primitive, every truncation length, then decodeImpl with trailing garbage.
const prim =
  "bool a;char c;int8 i8;int16 i16;int32 i32;int64 i64;uint8 u8;uint16 u16;uint32 u32;uint64 u64;float f;float32 f32;double d;float64 f64;";
const primFull = seq(56);
emit([add("Prim", prim), ["decodeImpl", "Prim", Uint8Array.from([...primFull, 0xaa, 0xbb])]]);
emit([add("Prim", prim), ...Array.from({ length: primFull.length + 1 }, (_, i) => dec("Prim", primFull.slice(0, i)))]);
emit([add("PrimFF", prim), dec("PrimFF", new Uint8Array(56).fill(0xff)), dec("PrimFF", rng.bytes(64))]);

// char consumes the rest of the buffer but only advances one byte.
emit([
  add("Chars", "char c;"),
  ...[zeros(0), seq(1), seq(4), utf8("abc"), new Uint8Array([0xef, 0xbb, 0xbf, 0x61]), new Uint8Array([0xff, 0xfe, 0x80]), rng.bytes(9)].map(
    (b) => dec("Chars", b)
  )
]);
emit([add("CharMid", "char c;int8 i;"), dec("CharMid", seq(8)), dec("CharMid", seq(2))]);

// VLA and optional edges.
emit([
  add("VLA", "int8 a[?];float f;"),
  ...[0x00, 0x01, 0x02, 0x7f, 0x80, 0xff].map((n) => dec("VLA", Uint8Array.from([n, ...seq(12)]))),
  dec("VLA", new Uint8Array([0x02, 0x11])),
  dec("VLA", new Uint8Array([0x05])),
  dec("VLA", new Uint8Array([]))
]);
emit([
  add("Opt", "optional int8 a;int8 b;"),
  ...[0x00, 0x01, 0x02, 0x80, 0xff].map((n) => dec("Opt", Uint8Array.from([n, 0x77, 0x88]))),
  dec("Opt", new Uint8Array([0x00])),
  dec("Opt", new Uint8Array([0x01])),
  dec("Opt", new Uint8Array([]))
]);
emit([
  add("Inner", "int16 x;float y;"),
  add("Outer", "Inner a;Inner b;optional Inner c;Inner d[?];"),
  dec("Outer", seq(48)),
  dec("Outer", seq(20)),
  dec("Outer", Uint8Array.from([...seq(16), 0x00, 0x02, ...seq(12)])),
  dec("Outer", Uint8Array.from([...seq(16), 0x01, ...seq(32), 0x7f, ...seq(8)]))
]);
emit([
  add("Arr", "Inner x[?];int8 b;"),
  dec("Arr", Uint8Array.from([0x02, ...seq(12), 0xaa])),
  dec("Arr", Uint8Array.from([0xfe, 0xbb])),
  dec("Arr", Uint8Array.from([0x02, ...seq(5)])),
  dec("Arr", Uint8Array.from([0x00, 0xcc]))
]);

// A deep schema chain keeps recursion well within both engines.
{
  const ops: Op[] = [];
  ops.push(add("Chain0", "int8 v;"));
  for (let i = 1; i <= 40; i++) ops.push(add(`Chain${i}`, `Chain${i - 1} x;`));
  ops.push(dec("Chain40", zeros(41)), dec("Chain40", zeros(20)));
  emit(ops);
}

// Seeded schema + payload fuzz: generated schemas can nest earlier ones; payloads are
// layout-valid, then every prefix is decoded for the short-buffer matrix.
interface Field {
  name: string;
  type: string;
  optional: boolean;
  vla: boolean;
}

const PRIMS = [
  "bool",
  "char",
  "int8",
  "int16",
  "int32",
  "int64",
  "uint8",
  "uint16",
  "uint32",
  "uint64",
  "float",
  "float32",
  "double",
  "float64"
];
const PRIM_SIZE: Record<string, number> = {
  bool: 1,
  char: 1,
  int8: 1,
  int16: 2,
  int32: 4,
  int64: 8,
  uint8: 1,
  uint16: 2,
  uint32: 4,
  uint64: 8,
  float: 4,
  float32: 4,
  double: 8,
  float64: 8
};
const FIELD_NAMES = ["a", "b", "pos", "x", "0", "2", "4294967294", "x/length", "__proto__", "k"];

function genFields(r: Rng, names: string[]): Field[] {
  const fields: Field[] = [];
  const count = r.range(1, 5);
  for (let i = 0; i < count; i++) {
    const optional = r.below(6) === 0;
    fields.push({
      name: r.pick(FIELD_NAMES),
      type: r.pick([...PRIMS, ...names]),
      optional,
      vla: !optional && r.below(4) === 0
    });
  }
  return fields;
}

function payloadFor(fields: Field[], byName: Map<string, Field[]>, r: Rng, depth: number): number[] {
  const out: number[] = [];
  const value = (type: string) => {
    const child = byName.get(type);
    if (child !== undefined && depth < 8) out.push(...payloadFor(child, byName, r, depth + 1));
    else for (let i = 0; i < (PRIM_SIZE[type] ?? 1); i++) out.push(r.below(256));
  };
  for (const f of fields) {
    if (f.optional) {
      if (r.below(4) === 0) {
        out.push(0);
        continue;
      }
      out.push(r.pick([1, 2, 255]));
    }
    if (f.vla) {
      const n = r.pick([-2, -1, 0, 1, 1, 2, 3, 127]);
      out.push(n & 0xff);
      for (let i = 0; i < n; i++) value(f.type);
    } else {
      value(f.type);
    }
  }
  return out;
}

const byName = new Map<string, Field[]>();
for (let iter = 0; iter < 6; iter++) {
  const ops: Op[] = [];
  const local: string[] = [];
  for (let s = 0; s < 3; s++) {
    const name = `Gen${iter * 3 + s}`;
    const fields = genFields(rng, [...byName.keys()]);
    byName.set(name, fields);
    local.push(name);
    ops.push([
      "addSchema",
      name,
      utf8(fields.map((f) => `${f.optional ? "optional " : ""}${f.type} ${f.name}${f.vla ? "[?]" : ""};`).join(""))
    ]);
  }
  for (const name of local) {
    const payload = Uint8Array.from(payloadFor(byName.get(name)!, byName, rng, 0));
    ops.push(dec(name, payload));
    for (let len = 0; len < payload.length; len++) ops.push(dec(name, payload.slice(0, len)));
    ops.push(dec(name, rng.bytes(rng.range(0, 40))));
    ops.push(["decodeImpl", name, Uint8Array.from([...payload, ...rng.bytes(rng.range(0, 8))])]);
  }
  emit(ops);
}
