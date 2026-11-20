import "./shims";
import StructDecoder from "@upstream/shared/log/StructDecoder";
import { toTree } from "./cve1";
import { concat, hex, utf8 } from "./corpus/bytes";
import { Corpus } from "./corpus/corpus";
import { structMatrix } from "./corpus/struct";
import { Rng } from "./corpus/rng";
import { ownView } from "./helpers";

// Emits upstream StructDecoder ground truth for the asrs_log::struct_decoder vectors.
//   genStruct replay <seed>
//     Replays every payload/struct/* corpus case's ops through a fresh decoder the way
//     Log.ts drives it: putRaw schema keys call addSchema then drain the two queues
//     (structs first, then arrays), and putStruct decodes directly, queueing whatever
//     throws. Records:
//       {"case":id,"s":{"n":name,"b":hex},"ok":<toSerialized()>}
//       {"case":id,"d":{"n":type,"b":hex,"a":bool,"q":bool},"ok":<{data,schemaTypes}>|"throw":<err>}
//       {"case":id,"d":{...},"skip":true}  the >5000-byte geometry skip (#477)
//   genStruct fuzz <seed>
//     Same "s"/"d" records one case per schema set, from targeted schemas
//     (proto-member names and types, enum edge cases, invalid bitfields,
//     fractional/negative/NaN array lengths, integer-like and odd names, nesting,
//     encoding corner cases) and seeded random schemas, against targeted and random
//     payloads. Extra op kinds:
//       {"case":id,"a":{"n":type,"b":hex,"l":<len tree>},"ok"|"throw"}   decodeArray with an
//         explicit length ("l" absent means undefined)
//       {"case":id,"a":{...},"never":true}   decodeArray that never returns upstream
//       {"case":id,"r":<serialized>,"ok":<toSerialized()>}            the decoder is replaced
//         with StructDecoder.fromSerialized(structuredClone(r)); later records reuse it

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const decoder8 = new TextDecoder();

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

type Item = { name: string; value: Uint8Array };

const BIG_GEOMETRY = new Set([
  "Translation2d",
  "Pose2d",
  "Transform2d",
  "Translation3d",
  "Pose3d",
  "Transform3d"
]);

/** A decoder plus Log.ts's two decode queues, emitting a record per decoder call. */
class Session {
  decoder = new StructDecoder();

  constructor(
    readonly id: string,
    private structs: Item[] = [],
    private arrays: Item[] = []
  ) {}

  private rec(input: object, result: object) {
    out({ case: this.id, ...input, ...result });
  }

  addSchema(name: string, value: Uint8Array) {
    this.decoder.addSchema(name, value);
    this.rec({ s: { n: name, b: bytesToHex(value) } }, { ok: toTree(this.decoder.toSerialized()) });
    this.drain();
  }

  restore(serialized: unknown) {
    this.decoder = StructDecoder.fromSerialized(structuredClone(serialized));
    this.rec({ r: toTree(serialized) }, { ok: toTree(this.decoder.toSerialized()) });
  }

  private drain() {
    const structs = this.structs;
    const arrays = this.arrays;
    this.structs = [];
    this.arrays = [];
    structs.forEach((item) => this.putStruct(item, false, true));
    arrays.forEach((item) => this.putStruct(item, true, true));
  }

  private putStruct(item: Item, isArray: boolean, queued: boolean) {
    if (BIG_GEOMETRY.has(item.name) && item.value.length > 5000) {
      this.rec({ d: { n: item.name, b: bytesToHex(item.value), a: isArray, q: queued } }, { skip: true });
      return;
    }
    const ok = this.decode(item, isArray, queued);
    if (!ok) (isArray ? this.arrays : this.structs).push(item);
  }

  /** Emits the decode record; returns whether it did not throw. */
  decode(item: Item, isArray: boolean, queued: boolean): boolean {
    const input = isArray
      ? { a: { n: item.name, b: bytesToHex(item.value), q: queued } }
      : { d: { n: item.name, b: bytesToHex(item.value), a: false, q: queued } };
    try {
      const decoded = isArray
        ? this.decoder.decodeArray(item.name, item.value)
        : this.decoder.decode(item.name, item.value);
      this.rec(input, { ok: toTree(ownView(decoded)) });
      return true;
    } catch (e) {
      this.rec(input, { throw: toTree(e) });
      return false;
    }
  }

  decodeArrayExplicit(item: Item, len: number | undefined) {
    const d: Record<string, unknown> = { n: item.name, b: bytesToHex(item.value), a: true, q: false };
    if (len !== undefined) d.l = toTree(len);
    try {
      if (len === undefined && item.value.length > 0 && this.zeroLength(item.name)) {
        // value.length / 0 is +Infinity and every item decodes the same empty
        // slice: upstream loops until the realm dies of OOM unless that throws.
        this.decoder.decodeArray(item.name, item.value, 1);
        this.rec({ a: d }, { never: true });
        return;
      }
      this.rec({ a: d }, { ok: toTree(ownView(this.decoder.decodeArray(item.name, item.value, len))) });
    } catch (e) {
      this.rec({ a: d }, { throw: toTree(e) });
    }
  }

  private zeroLength(name: string): boolean {
    const schemas = this.decoder.toSerialized().schemas;
    return Object.hasOwn(schemas, name) && schemas[name].length === 0;
  }

  /** putRaw's schema-key handling plus the decodes a putStruct performs. */
  ops(lines: string[]) {
    const schemaRegex = /(?:^|\/)\.schema\/(.*)/;
    const putRawSchema = (key: unknown, bytes: unknown) => {
      if (typeof key !== "string" || typeof bytes !== "object" || bytes === null || !("b" in bytes)) return;
      const match = key.match(schemaRegex);
      if (match === null) return;
      if (match[1].startsWith("struct:")) {
        this.addSchema(match[1].slice("struct:".length), hexToBytes((bytes as { b: string }).b));
      } else if (match[1].startsWith("photonstruct:")) {
        this.rec({ k: { n: match[1], b: bytesToHex((bytes as { b: string }).b) } }, {});
        this.drain();
      }
    };
    for (const line of lines) {
      if (!line.trim()) continue;
      const op = JSON.parse(line);
      switch (op.fn) {
        case "putRaw":
          putRawSchema(op.args[0], op.args[2]);
          break;
        case "putStruct":
          putRawSchema(op.args[0], op.args[2]);
          this.putStruct({ name: op.args[3], value: hexToBytes(op.args[2].b) }, op.args[4] === true, false);
          break;
        case "putProto":
          if (op.args[3] === "FileDescriptorProto") {
            this.rec({ k: { n: op.args[3], b: bytesToHex(hexToBytes(op.args[2].b)) } }, {});
            this.drain();
          }
          break;
      }
    }
  }
}

const [mode, a1] = process.argv.slice(2);

if (mode === "replay") {
  const seed = Number(a1);
  const corpus = new Corpus(seed);
  structMatrix(corpus);
  for (const c of corpus.cases) {
    if (!c.id.startsWith("payload/struct/")) continue;
    const opsFile = c.files.find((f) => f.path.endsWith(".ops.jsonl"));
    if (opsFile === undefined) continue;
    new Session(c.id).ops(decoder8.decode(opsFile.bytes).split("\n"));
  }
} else if (mode === "fuzz") {
  fuzz(Number(a1));
} else {
  throw new Error("usage: genStruct replay <seed> | fuzz <seed>");
}

function fuzz(seed: number) {
  const rng = new Rng(seed, "struct/fuzz");

  const fixed: [string, Uint8Array][] = [
    // Object.prototype members as types, field names and schema names
    ["ProtoToString", utf8("toString a")],
    ["ProtoCtor", utf8("constructor a;int8 b")],
    ["ProtoHOP", utf8("hasOwnProperty a")],
    ["ProtoDefine", utf8("__defineGetter__ a")],
    ["ProtoProto", utf8("__proto__ a")],
    ["ProtoValueOf", utf8("valueOf a;double b")],
    [
      "ProtoNames",
      utf8(
        "int8 toString;int8 valueOf;int8 constructor;int8 hasOwnProperty;int8 __proto__;int8 isPrototypeOf"
      )
    ],
    ["UndefinedType", utf8("undefined a")],
    ["undefined", utf8("int8 u")],
    ["toString", utf8("double x")],
    ["__proto__", utf8("double x")],
    ["hasOwnProperty", utf8("int8 y")],
    ["enum", utf8("int8 e")],

    // Enum parsing edges
    ["EnumEmpty", utf8("enum {} int8 a")],
    ["EnumBare", utf8("enum int8 a")],
    ["EnumClose", utf8("enum} int8 a")],
    ["EnumOpen", utf8("enum{x=1 int8 a")],
    ["EnumSpacey", utf8("enum { a = 1 , b = 2 } int8 x")],
    [
      "EnumWeird",
      utf8(
        "enum {a=,b==2,c=1=2,=3,d=0x10,e=1e2,f=.5,g=5.,h=+3,i=-0,j=NaN,k=Infinity,l=-Infinity,m= 4 ,n=4x,o=x4} int8 v"
      )
    ],
    ["EnumDups", utf8("enum {a=1,b=1,c=2} uint8 v;enum {x=3} uint8 w")],
    ["EnumFloat", utf8("enum {one=1.5,neg=-2.25} double v;enum {nan=NaN} double w")],
    ["EnumOnBool", utf8("enum {t=1,f=0} bool b")],
    ["EnumOnChar", utf8("enum {x=65} char c")],
    ["EnumBit", utf8("enum {lo=1,hi=15} int16 v:4")],
    ["EnumOnStruct", utf8("ProtoLeaf ref;enum {a=1} ProtoLeaf ref2")],
    ["EnumPrefixWeird", utf8("enumx {a=1} int8 v")],
    ["EnumNoSpace", utf8("enum{a=1}int8 v")],
    ["EnumMulti", utf8("enum {a=1} int8 x;enum {b=2} int16 y;enum {c=3} uint32 z")],

    // Bitfield edges
    [
      "BfWidths",
      utf8(
        "int8 a:0x10;int8 b:1e1;int8 c:-1;int8 d:+3;int8 e:3.5;int8 f:0;int8 g:64;int8 h:65;int8 i:Infinity;int8 j:-Infinity"
      )
    ],
    [
      "BfColon",
      utf8(
        "int8 a:2:4;int8 b:c;int8 :2;int8 d:;bool ok:1;bool bad0:0;bool bad2:2;double f:4;char g:1;float h:3"
      )
    ],
    ["BfNan", utf8("int8 a:NaN;uint16 b:2;int8 c:NaN")],
    ["BfMix", utf8("bool a:1;int8 b:7;bool c:1;uint8 d:8;int16 e:16;bool f:1;int16 g:15;int8 h:8;bool i:1")],
    ["BfAllBool", utf8("bool a:1;bool b:1;bool c:1;bool d:1;bool e:1;bool f:1;bool g:1;bool h:1;bool i:1")],
    [
      "BfPack",
      utf8(
        "uint8 a:4;uint8 b:4;int16 c:9;bool d:1;int16 e:6;uint16 f:7;int32 g:20;uint32 h:12;int64 i:33;uint64 j:31"
      )
    ],

    // Array length edges
    [
      "ArrLens",
      utf8(
        "int8 a[0];int8 b[2.5];int8 c[-1];int8 d[NaN];int8 e[Infinity];int8 f[-Infinity];int8 g[0x10];int8 h[1e2];int8 i[];int8 j[3x];int8 k[x3];double l[3]"
      )
    ],
    ["ArrBrackets", utf8("int8 a[2][3];int8 b[;int8 c];int8 d]];int8 e[[2]")],
    ["ArrNested", utf8("ProtoLeaf a[2];ProtoLeaf b[0];ProtoLeaf c[-1];ProtoLeaf d[NaN];ProtoLeaf e[2.5]")],
    ["ArrChars", utf8("char s[8];char t[0];char u[-1];char v[2.5]")],

    // Name and layout edges
    [
      "IntNames",
      utf8(
        "double 1;double 0;double 10;double b;double 2;double a;double 4294967294;double 4294967295;double 00;double 01;double -1;double 1.5;double NaN;double Infinity"
      )
    ],
    ["EmptyNames", utf8("int8 ;int8  ;bool :1;int8 []")],
    ["DupFields", utf8("int8 a;int16 a;double a")],
    ["Tabbed", utf8("int8\ta\t;\tint16\tb")],
    ["Nbsp", utf8("int8 a;double b")],
    ["Newlines", utf8("\nint8 a\n;\nint16 b\n")],
    ["NoSemi", utf8("int8 a int16 b")],
    ["TypeConcat", utf8("int8a;doubleb")],
    ["CaseSens", utf8("Int8 a;DOUBLE b;Bool c")],
    ["LeadingSemi", utf8(";;int8 a;;")],
    ["JustType", utf8("int8")],
    ["JustName", utf8("a")],
    ["SpaceType", utf8("  int8  a  ")],
    ["Empty", utf8("")],
    ["Semis", utf8(";;;")],
    ["StructSelf", utf8("StructSelf inner;double a")],
    ["CycleA", utf8("CycleB a")],
    ["CycleB", utf8("CycleA a")],
    ["MissingRef", utf8("NotThere a;int8 b")],
    ["enumref", utf8("enum a")],

    // Odd schema names
    ["0", utf8("double x")],
    ["with space", utf8("int8 s")],
    ["with/slash", utf8("int8 s")],
    ["struct:x", utf8("int8 s")],
    ["x[]", utf8("int8 s")],
    ["A.B", utf8("int8 s")],
    ["null", utf8("int8 s")],

    // Nesting and sub-byte lengths
    ["ProtoLeaf", utf8("double v")],
    ["NestedBf", utf8("Inner8 a;Inner8 b")],
    ["Inner8", utf8("bool x:1;uint8 y:7")],
    ["WrapArr", utf8("Inner8 a[3];uint8 b")],
    ["Deep", utf8("Mid m;int8 t")],
    ["Mid", utf8("Leaf l;Inner8 n[2]")],
    ["Leaf", utf8("double v")],
    ["Bits3", utf8("bool a:1;bool b:1;bool c:1")],
    ["Nibble", utf8("uint8 v:4")],
    ["Bits20", utf8("int32 v:20")],
    ["NibArr", utf8("uint8 v:4;uint8 w:4;bool x:1")],
    ["Wrap", utf8("Bits3 t;Nibble n[3]")],

    // Encoded schema bytes
    ["Bom8", concat(hex("ef bb bf"), utf8("double a"))],
    ["BomMid", concat(utf8("int8 a;"), hex("ef bb bf"), utf8("int8 b"))],
    ["BadUtf8", concat(hex("ff fe 80"), utf8("int8 a"))],
    ["NullByte", concat(utf8("int8 a"), hex("00"), utf8("int8 b"))],
    ["Utf8Name", utf8("int8 caf\u00e9;double \u4e2d\u6587")]
  ];

  const session = new Session("fuzz/fixed");
  for (const [name, bytes] of fixed) session.addSchema(name, bytes);

  const sizes = new Map<string, number>();
  for (const [name, schema] of Object.entries(session.decoder.toSerialized().schemas)) {
    sizes.set(name, Math.ceil(schema.length / 8));
  }
  const sizer = (name: string) => {
    const n = sizes.get(name);
    return n !== undefined && Number.isFinite(n) ? n : 8;
  };
  const payloads = (n: number): Uint8Array[] => [
    new Uint8Array(0),
    new Uint8Array(n),
    new Uint8Array(n).fill(0xff),
    new Uint8Array(n).fill(0x80),
    new Uint8Array(Math.max(0, n - 1)),
    new Uint8Array(n + 3),
    rng.bytes(n + rng.below(8))
  ];
  const arrayLens: (number | undefined)[] = [undefined, 0, 1, 2, 2.5, -1, NaN];
  for (const [name] of fixed) {
    for (const value of payloads(sizer(name))) {
      session.decode({ name, value }, false, false);
    }
    for (const len of arrayLens) {
      session.decodeArrayExplicit({ name, value: rng.bytes(sizer(name) * 2 + 4) }, len);
      session.decodeArrayExplicit({ name, value: new Uint8Array(0) }, len);
    }
  }
  // Names that were never added and proto members the decoder resolves anyway.
  for (const name of ["Missing", "NeverDefined", "toLocaleString", "propertyIsEnumerable", "isPrototypeOf"]) {
    session.decode({ name, value: rng.bytes(8) }, false, false);
    session.decodeArrayExplicit({ name, value: rng.bytes(8) }, undefined);
    session.decodeArrayExplicit({ name, value: new Uint8Array(0) }, undefined);
  }

  // Serialized round trips, then decoding on restored decoders.
  const serialized = session.decoder.toSerialized();
  session.restore(serialized);
  for (const name of ["ProtoNames", "EnumWeird", "BfPack", "ArrLens", "Deep", "Wrap"]) {
    session.decode({ name, value: rng.bytes(sizer(name) + 2) }, false, false);
  }
  // A merge-style restore (Log.mergeWith spreads two decoders' maps): a schema
  // string whose compiled schema did not come along is compiled by the next addSchema.
  session.restore({
    schemaStrings: { ...serialized.schemaStrings, Late: "Leaf l;uint8 n:3" },
    schemas: serialized.schemas
  });
  session.decode({ name: "Late", value: rng.bytes(9) }, false, false);
  session.addSchema("AfterMerge", utf8("Late x[2]"));
  session.decode({ name: "AfterMerge", value: rng.bytes(18) }, false, false);

  // Exact int64/uint64 rounding and float32 NaN payload widening.
  const numbers = new Session("fuzz/numbers");
  numbers.addSchema("I64", utf8("int64 i;uint64 u"));
  numbers.addSchema("F32", utf8("float f;float32 g"));
  numbers.addSchema("BfI64", utf8("int64 a:53;int64 b:11;uint64 c:63;bool d:1"));
  for (const [i, u] of [
    ["0100000000002000", "0100000000002000"],
    ["0300000000002000", "0300000000002000"],
    ["ffffffffffffff7f", "ffffffffffffffff"],
    ["0000000000000080", "0004000000000080"],
    ["0104000000000080", "0104000000000080"],
    ["ffffffffffffdfff", "01feffffffffffff"]
  ]) {
    numbers.decode({ name: "I64", value: hex(i + u) }, false, false);
    numbers.decode({ name: "BfI64", value: hex(i + u) }, false, false);
  }
  for (const f of ["0100807f0100c07f", "ffffffffffffff7f", "00008000000080ff", "010000000000807f"]) {
    numbers.decode({ name: "F32", value: hex(f) }, false, false);
  }

  // Seeded random schemas mixing every feature.
  for (let set = 0; set < 8; set++) {
    const s = new Session(`fuzz/random-${set}`);
    const schemas = randomSchemas(rng, `FZ${set}_`);
    for (const [name, text] of schemas) s.addSchema(name, utf8(text));
    const own = new Map(
      Object.entries(s.decoder.toSerialized().schemas).map(([n, sc]) => [n, Math.ceil(sc.length / 8)])
    );
    for (const [name] of schemas) {
      const n = own.get(name);
      const size = n !== undefined && Number.isFinite(n) ? n : 8;
      for (const value of payloads(size)) s.decode({ name, value }, false, false);
      for (const len of [undefined, 0, 1, 3, 2.5]) {
        s.decodeArrayExplicit({ name, value: rng.bytes(size * 2 + 2) }, len);
      }
    }
  }
}

function randomSchemas(rng: Rng, prefix: string): [string, string][] {
  const primitives = [
    "bool", "char", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64",
    "float", "float32", "double", "float64"
  ];
  const bitfieldTypes = ["bool", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64"];
  const widths: { [t: string]: number } = {
    bool: 1, int8: 8, uint8: 8, int16: 16, uint16: 16, int32: 32, uint32: 32, int64: 64, uint64: 64
  };
  const names = [
    "a", "f0", "x", "toString", "valueOf", "__proto__", "0", "1", "01", "4294967294", "4294967295",
    "hasOwnProperty", "b", "field name", "", "caf\u00e9"
  ];
  const out: [string, string][] = [];
  for (let s = 0; s < 5; s++) {
    const fields: string[] = [];
    const n = rng.range(1, 7);
    for (let f = 0; f < n; f++) {
      const name = rng.pick(names);
      const roll = rng.below(12);
      if (roll < 3) {
        const type = rng.pick(bitfieldTypes);
        const width =
          type === "bool"
            ? rng.pick([1, 1, 1, 0, 2])
            : rng.pick([rng.range(1, widths[type]), 0, -1, 2.5, widths[type], widths[type] + 1, NaN]);
        fields.push(`${type} ${name}:${width}`);
      } else if (roll < 5) {
        fields.push(`${rng.pick(primitives)} ${name}[${rng.pick([rng.range(0, 4), -1, 2.5, "NaN", "x", ""])}]`);
      } else if (roll < 6 && out.length > 0) {
        const [inner] = rng.pick(out);
        fields.push(rng.bool() ? `${inner} ${name}` : `${inner} ${name}[${rng.range(0, 3)}]`);
      } else if (roll < 7) {
        fields.push(
          `${rng.pick(["toString", "valueOf", "undefined", "NotThere", "__proto__", "hasOwnProperty"])} ${name}`
        );
      } else if (roll < 8) {
        fields.push(
          `enum {${rng.pick(["a=1,b=2", "x=0,y=-1", "", "k=1.5", "p=0x10", "=2", "q="])}} ${rng.pick(primitives)} ${name}`
        );
      } else {
        fields.push(`${rng.pick(primitives)} ${name}`);
      }
    }
    out.push([`${prefix}${s}`, fields.join(";")]);
  }
  return out;
}
