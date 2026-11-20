import StructDecoder from "@upstream/shared/log/StructDecoder";
import { ByteSource, Bytes, concat, hex, utf8 } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";
import { LogSpec } from "./wpilog";

export const WPILIB_STRUCTS: [string, string][] = [
  ["Translation2d", "double x;double y"],
  ["Rotation2d", "double value"],
  ["Pose2d", "Translation2d translation;Rotation2d rotation"],
  ["Transform2d", "Translation2d translation;Rotation2d rotation"],
  ["Translation3d", "double x;double y;double z"],
  ["Quaternion", "double w;double x;double y;double z"],
  ["Rotation3d", "Quaternion q"],
  ["Pose3d", "Translation3d translation;Rotation3d rotation"],
  ["Transform3d", "Translation3d translation;Rotation3d rotation"],
  ["SwerveModuleState", "double speed;Rotation2d angle"],
  ["ChassisSpeeds", "double vx;double vy;double omega"]
];

/** Byte length of each schema as upstream's StructDecoder lays it out (bit length rounded up). */
export function structSizes(schemas: [string, ByteSource][]): Map<string, number> {
  const decoder = new StructDecoder();
  schemas.forEach(([name, text]) =>
    decoder.addSchema(name, typeof text === "string" ? utf8(text) : Uint8Array.from(text))
  );
  const compiled = decoder.toSerialized().schemas as { [name: string]: { length: number } };
  return new Map(Object.entries(compiled).map(([name, schema]) => [name, Math.ceil(schema.length / 8)]));
}

export function putStructSchema(spec: LogSpec, name: string, text: ByteSource, ts = 0, prefix = "") {
  spec.put(spec.entry(`${prefix}/.schema/struct:${name}`, "structschema"), ts, text);
}

class StructCase {
  readonly spec = new LogSpec();
  private sizes = new Map<string, number>();
  private schemas: [string, ByteSource][] = [];
  private ids = new Map<string, number>();
  private t = 1000;

  constructor(readonly rng: Rng) {}

  schema(name: string, text: ByteSource, prefix = "") {
    putStructSchema(this.spec, name, text, this.t, prefix);
    this.schemas.push([name, text]);
    this.sizes = structSizes(this.schemas);
    return this;
  }

  declare(name: string, schemaRef: string, ensure = true) {
    if (ensure && !this.ids.has(name)) this.ids.set(name, this.spec.entry(name, `struct:${schemaRef}`));
    return this.ids.get(name)!;
  }

  data(name: string, schemaRef: string, bytes: ByteSource) {
    this.t += 20000;
    this.spec.put(this.declare(name, schemaRef), this.t, bytes);
    return this;
  }

  size(schema: string): number {
    const size = this.sizes.get(schema);
    if (size === undefined) throw new Error(`schema ${schema} did not compile`);
    return Number.isFinite(size) ? size : 16;
  }

  random(name: string, schema: string, count: number, arrayLengths: number[] = []) {
    for (let i = 0; i < count; i++) {
      if (arrayLengths.length === 0) this.data(name, schema, this.rng.bytes(this.size(schema)));
      else this.data(name, `${schema}[]`, this.rng.bytes(this.size(schema) * this.rng.pick(arrayLengths)));
    }
    return this;
  }
}

function enumPayload(e1: number, e2: number, flag: number, e3: number, bf: number, tail: number): Uint8Array {
  return new Bytes()
    .le(e1, 1)
    .le(e2, 1)
    .le(flag, 1)
    .le(e3, 4)
    .le(bf & 0xf, 2)
    .le(tail, 1)
    .finish();
}

function randomSchemas(rng: Rng, prefix: string): [string, string][] {
  const primitives = [
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
  const bitfieldTypes = ["bool", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64"];
  const widths: { [t: string]: number } = {
    bool: 1,
    int8: 8,
    uint8: 8,
    int16: 16,
    uint16: 16,
    int32: 32,
    uint32: 32,
    int64: 64,
    uint64: 64
  };
  const out: [string, string][] = [];
  for (let s = 0; s < 4; s++) {
    const fields: string[] = [];
    const n = rng.range(1, 7);
    for (let f = 0; f < n; f++) {
      const name = `f${f}`;
      const roll = rng.below(10);
      if (roll < 4) {
        const type = rng.pick(bitfieldTypes);
        fields.push(`${type} ${name}:${type === "bool" ? 1 : rng.range(1, widths[type])}`);
      } else if (roll < 6) {
        fields.push(`${rng.pick(primitives)} ${name}[${rng.range(0, 4)}]`);
      } else if (roll < 7 && out.length > 0) {
        const [inner] = rng.pick(out);
        fields.push(rng.bool() ? `${inner} ${name}` : `${inner} ${name}[${rng.range(1, 3)}]`);
      } else {
        fields.push(`${rng.pick(primitives)} ${name}`);
      }
    }
    out.push([`${prefix}${s}`, fields.join(";")]);
  }
  return out;
}

export function structMatrix(corpus: Corpus) {
  {
    const id = "payload/struct/primitives";
    const c = new StructCase(corpus.rng(id));
    c.schema(
      "Prims",
      "bool b;char c;int8 i8;int16 i16;int32 i32;int64 i64;uint8 u8;uint16 u16;uint32 u32;uint64 u64;float f;float32 f32;double d;float64 f64"
    );
    const size = c.size("Prims");
    c.data("/Struct/Prims", "Prims", new Uint8Array(size)).data(
      "/Struct/Prims",
      "Prims",
      new Uint8Array(size).fill(0xff)
    );
    c.data("/Struct/Prims", "Prims", new Uint8Array(size).fill(0x80)).random("/Struct/Prims", "Prims", 24);
    c.data("/Struct/Prims", "Prims", c.rng.bytes(size - 3)).data("/Struct/Prims", "Prims", c.rng.bytes(size + 5));
    corpus.spec(
      id,
      "every struct primitive type, all-zero, all-ones, 0x80 fill, random, short and long payloads",
      c.spec
    );
  }
  {
    const id = "payload/struct/bitfields";
    const c = new StructCase(corpus.rng(id));
    c.schema(
      "Bits",
      "bool a:1;bool b:1;int8 c:4;uint8 d:4;int16 e:9;uint16 f:7;uint32 g:20;int32 h:12;bool i:1;uint64 j:33;" +
        "int64 k:31;int8 big:12;double after;bool bad:2;float nope:3;uint8 tail:3;uint8 x:0;int32 y:32"
    );
    c.data("/Struct/Bits", "Bits", new Uint8Array(c.size("Bits")).fill(0xff)).random("/Struct/Bits", "Bits", 24);
    corpus.spec(
      id,
      "bitfield packing: new units on type change, overflow, width capping, invalid widths dropped",
      c.spec
    );
  }
  {
    const id = "payload/struct/enums";
    const c = new StructCase(corpus.rng(id));
    c.schema("enumThing", "bool z");
    c.schema(
      "Enums",
      "enum {a=1, b=2, c=-1} int8 e1;enum{x=0,y=1}uint8 e2;enum {on=1} bool flag;" +
        "enum {bad, k=2, =3, z=q, w=1.5, 0x10=16} int32 e3;enum {lo=1,hi=15} int16 bf:4;enum {a=1} uint8 tail;enumThing nested"
    );
    const values: number[][] = [
      [1, 0, 1, 2, 1, 1],
      [2, 1, 0, 3, 15, 2],
      [-1, 2, 2, 1, 7, 0],
      [3, 1, 1, 16, 0, 1],
      [0, 0, 0, 0, 1, 1]
    ];
    values.forEach((v) =>
      c.data("/Struct/Enums", "Enums", concat(enumPayload(v[0], v[1], v[2], v[3], v[4], v[5]), [v[0] & 1]))
    );
    corpus.spec(
      id,
      "enum maps: negative, empty-name, malformed pairs, bitfield enums and an enum-prefixed struct name",
      c.spec
    );
  }
  {
    const id = "payload/struct/nested";
    const c = new StructCase(corpus.rng(id));
    c.declare("/Struct/Outer", "Outer");
    const early = c.rng.bytes(40);
    c.data("/Struct/Outer", "Outer", early);
    c.schema("Outer", "Inner a;Inner b[2];uint8 tag;Leaf leaf");
    c.data("/Struct/Outer", "Outer", c.rng.bytes(40));
    c.schema("Inner", "double x;int16 y;Leaf l");
    c.data("/Struct/Outer", "Outer", c.rng.bytes(40));
    c.schema("Leaf", "bool z:1;uint8 w:3");
    c.random("/Struct/Outer", "Outer", 8)
      .random("/Struct/Inner", "Inner", 4)
      .random("/Struct/Inners", "Inner", 4, [0, 1, 3]);
    corpus.spec(
      id,
      "nested structs whose schemas arrive after data and after their dependents, then nested arrays",
      c.spec
    );
  }
  {
    const id = "payload/struct/arrays";
    const c = new StructCase(corpus.rng(id));
    c.schema("Arr", "int16 vals[3];char name[8];bool flags[5];double d[0];float f[2];char one[1];uint64 big[2]");
    const named = (name: Uint8Array) => {
      const b = new Bytes().raw(c.rng.bytes(6));
      b.raw(concat(name, new Uint8Array(Math.max(0, 8 - name.length))).slice(0, 8));
      return b.raw(c.rng.bytes(c.size("Arr") - 14)).finish();
    };
    [utf8("robot"), hex("61 00 62 00"), hex("c3 a9 e4 b8 ad 7f 80 ff"), hex("ef bb bf 61"), utf8("12345678")].forEach(
      (n) => c.data("/Struct/Arr", "Arr", named(n))
    );
    c.random("/Struct/Arr", "Arr", 10);
    corpus.spec(id, "fixed arrays of every kind, char arrays decoded byte by byte, zero-length arrays", c.spec);
  }
  {
    const id = "payload/struct/odd-length";
    const c = new StructCase(corpus.rng(id));
    c.schema("Bit3", "bool a:1;bool b:1;bool c:1").schema("Nib", "uint8 v:4").schema("Wrap", "Bit3 t;Nib n[3]");
    [1, 2, 3, 5].forEach((n) => c.data("/Struct/Bit3s", "Bit3[]", c.rng.bytes(n)));
    c.random("/Struct/Bit3", "Bit3", 4).random("/Struct/Wrap", "Wrap", 6);
    [0, 1, 2, 4].forEach((n) => c.data("/Struct/Nibs", "Nib[]", c.rng.bytes(n)));
    corpus.spec(
      id,
      "schemas whose bit length is not a multiple of 8, as scalars, nested and as fractional arrays",
      c.spec
    );
  }
  {
    const id = "payload/struct/integer-names";
    const c = new StructCase(corpus.rng(id));
    c.schema("Num", "double 1;double 0;double 10;double b;double 2;double a");
    c.schema("NumArr", "Num 3;Num x");
    c.random("/Struct/Num", "Num", 4).random("/Struct/NumArr", "NumArr", 3).random("/Struct/Nums", "Num", 2, [2, 11]);
    corpus.spec(id, "integer-like field names, which reorder the decoded object and child field creation", c.spec);
  }
  {
    const id = "payload/struct/wpilib";
    const c = new StructCase(corpus.rng(id));
    WPILIB_STRUCTS.forEach(([name, text]) => c.schema(name, text));
    WPILIB_STRUCTS.forEach(([name]) => c.random(`/Struct/WPILib/${name}`, name, 3));
    c.random("/Struct/WPILib/Pose2dArray", "Pose2d", 3, [0, 1, 4]);
    c.random("/Struct/WPILib/Pose3dArray", "Pose3d", 3, [0, 2, 5]);
    c.random("/Struct/WPILib/SwerveStates", "SwerveModuleState", 3, [4]);
    c.data("/Struct/WPILib/Translation2dBig", "Translation2d[]", c.rng.bytes(16 * 312));
    c.data("/Struct/WPILib/Translation2dBig", "Translation2d[]", c.rng.bytes(16 * 313));
    c.data("/Struct/WPILib/Pose3dBig", "Pose3d[]", c.rng.bytes(56 * 90));
    c.data("/Struct/WPILib/Pose2dBig", "Pose2d[]", c.rng.bytes(24 * 208));
    c.data("/Struct/WPILib/Pose2dBig", "Pose2d[]", c.rng.bytes(24 * 209));
    c.data("/Struct/WPILib/ChassisBig", "ChassisSpeeds[]", c.rng.bytes(24 * 300));
    corpus.spec(
      id,
      "WPILib geometry and kinematics structs, plus geometry arrays over 5000 bytes that skip decoding",
      c.spec
    );
  }
  {
    const id = "payload/struct/schema-variants";
    const c = new StructCase(corpus.rng(id));
    c.schema("Dup", "double a");
    c.schema("Dup", "int8 a;int8 b");
    c.schema("Ws", "  \n double   a ;; int16\tb ; ;bool  c :  1;\n");
    c.schema("Bom", concat(hex("ef bb bf"), "double a;double b"));
    c.schema("Self", "Self inner;double a");
    c.schema("Unknown", "vec3 a;double b");
    c.schema("NaNLen", "double x[abc];double y[];int8 z");
    c.schema("Prefixed", "uint16 v", "NT:");
    c.schema("Empty", "");
    c.schema("Semis", ";;;");
    ["Dup", "Bom", "NaNLen", "Prefixed", "Empty", "Semis"].forEach((name) => c.random(`/Struct/${name}`, name, 3));
    ["Ws", "Self", "Unknown", "Never"].forEach((name) => c.data(`/Struct/${name}`, name, c.rng.bytes(16)));
    c.data("/Struct/NeverArray", "Never[]", c.rng.bytes(16));
    corpus.spec(
      id,
      "duplicate, BOM, whitespace that never compiles, self-referencing, unknown-type, NaN-length, prefixed, empty and missing schemas",
      c.spec
    );
  }
  for (let n = 0; n < 3; n++) {
    const id = `payload/struct/random-${n}`;
    const c = new StructCase(corpus.rng(id));
    const schemas = randomSchemas(c.rng, `R${n}_`);
    schemas.forEach(([name, text]) => c.schema(name, text));
    schemas.forEach(([name]) => {
      c.random(`/Struct/Random/${name}`, name, 6).random(`/Struct/Random/${name}Array`, name, 3, [0, 1, 2, 5]);
      c.data(`/Struct/Random/${name}`, name, c.rng.bytes(c.rng.below(12)));
    });
    corpus.spec(
      id,
      "seeded random schemas mixing primitives, bitfields, arrays and nesting, with random payloads",
      c.spec
    );
  }
}
