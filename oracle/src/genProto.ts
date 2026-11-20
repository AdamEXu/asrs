import "./shims";
import ProtoDecoder from "@upstream/shared/log/ProtoDecoder";
import descriptor from "protobufjs/ext/descriptor";
import { toTree } from "./cve1";
import { hex } from "./corpus/bytes";
import { Corpus } from "./corpus/corpus";
import { protoMatrix } from "./corpus/proto";
import { Rng } from "./corpus/rng";

// Emits upstream ProtoDecoder ground truth for the asrs_log::proto_decoder vectors. Every case
// owns one decoder; all cases share this process's protobufjs realm, whose anonymous-name
// counters (Type0, Enum0, oneof0, Service0, Method0) carry over from case to case.
//   genProto replay <seed>
//     corpus A's payload/proto/* ops, driven the way Log.putProto drives the decoder: a
//     FileDescriptorProto calls addDescriptor and, when that returns, retries the queued decodes
//     in order; any other type decodes and queues the value when decode throws.
//   genProto matrix <seed>
//     Generated descriptors and messages through the same session logic.
//   genProto fuzz <seed>
//     Seeded random descriptors and messages built from them, some mutated.
// Records, in call order:
//   {"case":id,"s":{"b":hex},"ok":[hex...]}            addDescriptor, then toSerialized()
//   {"case":id,"s":{"b":hex},"throw":<error>}
//   {"case":id,"d":{"n":type,"b":hex,"q":bool},"ok":{data,schemaTypes}|"throw":<error>}
//   {"case":id,"r":[hex...],"ok":[hex...]|"throw":<error>}  the decoder is replaced with
//     ProtoDecoder.fromSerialized(r); later records use the new decoder

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function bytesToHex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

function hexToBytes(text: string): Uint8Array {
  const result = new Uint8Array(text.length / 2);
  for (let i = 0; i < result.length; i++) result[i] = parseInt(text.slice(2 * i, 2 * i + 2), 16);
  return result;
}

type Item = { name: string; value: Uint8Array };

class Session {
  decoder = new ProtoDecoder();
  private queue: Item[] = [];

  constructor(readonly id: string) {}

  private rec(input: object, result: object) {
    out({ case: this.id, ...input, ...result });
  }

  private serialized(): string[] {
    return (this.decoder.toSerialized() as Uint8Array[]).map(bytesToHex);
  }

  schema(value: Uint8Array) {
    try {
      this.decoder.addDescriptor(value);
    } catch (e) {
      this.rec({ s: { b: bytesToHex(value) } }, { throw: toTree(e) });
      return;
    }
    this.rec({ s: { b: bytesToHex(value) } }, { ok: this.serialized() });
    const queued = this.queue;
    this.queue = [];
    queued.forEach((item) => this.decode(item, true));
  }

  data(name: string, value: Uint8Array) {
    this.decode({ name, value }, false);
  }

  private decode(item: Item, queued: boolean) {
    const input = { d: { n: item.name, b: bytesToHex(item.value), q: queued } };
    try {
      this.rec(input, { ok: toTree(this.decoder.decode(item.name, item.value)) });
    } catch (e) {
      this.rec(input, { throw: toTree(e) });
      this.queue.push(item);
    }
  }

  restore(serialized: Uint8Array[]) {
    const r = serialized.map(bytesToHex);
    try {
      this.decoder = ProtoDecoder.fromSerialized(serialized.map((b) => Uint8Array.from(b)));
    } catch (e) {
      this.rec({ r }, { throw: toTree(e) });
      return;
    }
    this.rec({ r }, { ok: this.serialized() });
  }

  current(): Uint8Array[] {
    return this.decoder.toSerialized() as Uint8Array[];
  }
}

// ---------------------------------------------------------------------------------------------
// Descriptor and message builders

const FDP = (descriptor as any).FileDescriptorProto;
const file = (object: object): Uint8Array => Uint8Array.from(FDP.encode(FDP.fromObject(object)).finish());

const T = {
  DOUBLE: 1,
  FLOAT: 2,
  INT64: 3,
  UINT64: 4,
  INT32: 5,
  FIXED64: 6,
  FIXED32: 7,
  BOOL: 8,
  STRING: 9,
  GROUP: 10,
  MESSAGE: 11,
  BYTES: 12,
  UINT32: 13,
  ENUM: 14,
  SFIXED32: 15,
  SFIXED64: 16,
  SINT32: 17,
  SINT64: 18
};

const SCALARS: [string, number][] = [
  ["double", T.DOUBLE],
  ["float", T.FLOAT],
  ["int64", T.INT64],
  ["uint64", T.UINT64],
  ["int32", T.INT32],
  ["fixed64", T.FIXED64],
  ["fixed32", T.FIXED32],
  ["bool", T.BOOL],
  ["string", T.STRING],
  ["bytes", T.BYTES],
  ["uint32", T.UINT32],
  ["sfixed32", T.SFIXED32],
  ["sfixed64", T.SFIXED64],
  ["sint32", T.SINT32],
  ["sint64", T.SINT64]
];
const WIRE: { [t: number]: number } = {
  [T.DOUBLE]: 1,
  [T.FLOAT]: 5,
  [T.INT64]: 0,
  [T.UINT64]: 0,
  [T.INT32]: 0,
  [T.FIXED64]: 1,
  [T.FIXED32]: 5,
  [T.BOOL]: 0,
  [T.STRING]: 2,
  [T.BYTES]: 2,
  [T.UINT32]: 0,
  [T.ENUM]: 0,
  [T.SFIXED32]: 5,
  [T.SFIXED64]: 1,
  [T.SINT32]: 0,
  [T.SINT64]: 0,
  [T.MESSAGE]: 2
};

type FieldOpts = {
  label?: number;
  typeName?: string;
  oneofIndex?: number;
  defaultValue?: string;
  packed?: boolean;
  extendee?: string;
  proto3Optional?: boolean;
  jsonName?: string;
};

function f(name: string, number: number, type: number | undefined, o: FieldOpts = {}) {
  return {
    name,
    number,
    ...(type !== undefined ? { type } : {}),
    label: o.label ?? 1,
    ...(o.typeName !== undefined ? { typeName: o.typeName } : {}),
    ...(o.oneofIndex !== undefined ? { oneofIndex: o.oneofIndex } : {}),
    ...(o.defaultValue !== undefined ? { defaultValue: o.defaultValue } : {}),
    ...(o.packed !== undefined ? { options: { packed: o.packed } } : {}),
    ...(o.extendee !== undefined ? { extendee: o.extendee } : {}),
    ...(o.proto3Optional ? { proto3Optional: true } : {}),
    ...(o.jsonName !== undefined ? { jsonName: o.jsonName } : {})
  };
}
const rep = (name: string, number: number, type: number, o: FieldOpts = {}) => f(name, number, type, { ...o, label: 3 });
const msg = (name: string, number: number, typeName: string, o: FieldOpts = {}) =>
  f(name, number, T.MESSAGE, { ...o, typeName });

class Pb {
  private parts: number[] = [];

  static varint(value: bigint | number): number[] {
    let v = BigInt.asUintN(64, BigInt(value));
    const result: number[] = [];
    do {
      const byte = Number(v & 0x7fn);
      v >>= 7n;
      result.push(v > 0n ? byte | 0x80 : byte);
    } while (v > 0n);
    return result;
  }

  raw(bytes: Uint8Array | number[] | string) {
    if (typeof bytes === "string") this.parts.push(...new TextEncoder().encode(bytes));
    else this.parts.push(...bytes);
    return this;
  }
  tag(no: number, wire: number) {
    return this.raw(Pb.varint(BigInt(no) * 8n + BigInt(wire)));
  }
  vint(no: number, v: bigint | number) {
    return this.tag(no, 0).raw(Pb.varint(v));
  }
  sint(no: number, v: bigint) {
    return this.vint(no, (v << 1n) ^ (v >> 63n));
  }
  fixed64(no: number, v: bigint) {
    const b: number[] = [];
    let x = BigInt.asUintN(64, v);
    for (let i = 0; i < 8; i++, x >>= 8n) b.push(Number(x & 0xffn));
    return this.tag(no, 1).raw(b);
  }
  fixed32(no: number, v: number) {
    const x = v >>> 0;
    return this.tag(no, 5).raw([x & 255, (x >>> 8) & 255, (x >>> 16) & 255, x >>> 24]);
  }
  f64(no: number, x: number) {
    const b = new Uint8Array(8);
    new DataView(b.buffer).setFloat64(0, x, true);
    return this.tag(no, 1).raw(b);
  }
  f64Bits(no: number, bits: bigint) {
    return this.fixed64(no, bits);
  }
  f32(no: number, x: number) {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setFloat32(0, x, true);
    return this.tag(no, 5).raw(b);
  }
  len(no: number, data: Pb | Uint8Array | number[] | string) {
    const bytes =
      data instanceof Pb ? data.finish() : typeof data === "string" ? new TextEncoder().encode(data) : Uint8Array.from(data);
    return this.tag(no, 2).raw(Pb.varint(bytes.length)).raw(bytes);
  }
  packed(no: number, items: Pb) {
    return this.len(no, items);
  }
  finish(): Uint8Array {
    return Uint8Array.from(this.parts);
  }
}
const pb = () => new Pb();

// V8 stores a NaN into an array element either unchanged, quieted or canonicalized depending
// on inline-cache and tiering state, so payload NaNs are only generated for singular fields.
const SINGULAR_DOUBLE_BITS = [0x7ff0000000000001n, 0xfff8000000000123n, 0x8000000000000000n, 1n];
const ELEMENT_DOUBLE_BITS = [0x7ff8000000000000n, 0x8000000000000000n, 1n, 0x7ff0000000000000n];
const SINGULAR_FLOAT_BITS = [0x7f800001, 0xffc00123, 0x80000000, 1, 0x7f7fffff];
const ELEMENT_FLOAT_BITS = [0x7fc00000, 0x80000000, 1, 0x7f7fffff, 0xff800000];

/** Writes one value of the given descriptor type without its tag. */
function value(r: Rng, type: number, element = false): Pb {
  const p = pb();
  switch (type) {
    case T.DOUBLE: {
      const b = new Uint8Array(8);
      const dv = new DataView(b.buffer);
      if (r.below(6) === 0) dv.setBigUint64(0, r.pick(element ? ELEMENT_DOUBLE_BITS : SINGULAR_DOUBLE_BITS), true);
      else {
        const x = r.double();
        dv.setFloat64(0, element && Number.isNaN(x) ? NaN : x, true);
      }
      return p.raw(b);
    }
    case T.FLOAT: {
      const b = new Uint8Array(4);
      const dv = new DataView(b.buffer);
      if (r.below(6) === 0) dv.setUint32(0, r.pick(element ? ELEMENT_FLOAT_BITS : SINGULAR_FLOAT_BITS), true);
      else {
        const x = r.double();
        dv.setFloat32(0, element && Number.isNaN(x) ? NaN : x, true);
      }
      return p.raw(b);
    }
    case T.INT64:
    case T.UINT64:
      return p.raw(Pb.varint(r.pick([0n, 1n, -1n, 2n ** 53n + 1n, 2n ** 63n - 1n, -(2n ** 63n), 2n ** 64n - 1n, r.u64()])));
    case T.SINT64: {
      const v = r.pick([0n, -1n, 1n, -(2n ** 63n), 2n ** 63n - 1n, BigInt.asIntN(64, r.u64())]);
      return p.raw(Pb.varint((v << 1n) ^ (v >> 63n)));
    }
    case T.INT32:
    case T.ENUM:
      return p.raw(Pb.varint(r.pick([0, 1, -1, 2 ** 31 - 1, -(2 ** 31), 2 ** 32 + 5, r.range(-1000, 1000)])));
    case T.UINT32:
      return p.raw(Pb.varint(r.pick([0, 2 ** 32 - 1, 2 ** 32, r.u32()])));
    case T.SINT32: {
      const v = r.pick([0, -1, 1, -(2 ** 31), 2 ** 31 - 1, r.range(-5000, 5000)]);
      return p.raw(Pb.varint(BigInt(((v << 1) ^ (v >> 31)) >>> 0)));
    }
    case T.FIXED32:
    case T.SFIXED32: {
      const x = r.u32();
      return p.raw([x & 255, (x >>> 8) & 255, (x >>> 16) & 255, x >>> 24]);
    }
    case T.FIXED64:
    case T.SFIXED64: {
      let x = r.u64();
      const b: number[] = [];
      for (let i = 0; i < 8; i++, x >>= 8n) b.push(Number(x & 0xffn));
      return p.raw(b);
    }
    case T.BOOL:
      return p.raw(Pb.varint(r.pick([0, 1, 2, 0x80, 2 ** 40])));
    case T.STRING:
    case T.BYTES: {
      const s = r.below(5) === 0 ? r.bytes(r.below(9)) : new TextEncoder().encode(r.ascii(8));
      return p.raw(Pb.varint(s.length)).raw(s);
    }
  }
  throw new Error(`no value for type ${type}`);
}

function field(r: Rng, m: Pb, no: number, type: number, element = false) {
  m.tag(no, WIRE[type]).raw(value(r, type, element).finish());
}

// ---------------------------------------------------------------------------------------------
// Matrix

function matrix(seed: number) {
  const cases: [string, (s: Session, r: Rng) => void][] = [];
  const add = (id: string, run: (s: Session, r: Rng) => void) => cases.push([id, run]);

  const scalarFields = (offset = 0) => SCALARS.map(([name, t], i) => f(`${name}_v`, i + 1 + offset, t));

  add("scalars/proto3", (s, r) => {
    s.schema(file({ name: "s3.proto", package: "m.s3", syntax: "proto3", messageType: [{ name: "S", field: scalarFields() }] }));
    for (let i = 0; i < 60; i++) {
      const m = pb();
      for (let k = r.below(12); k > 0; k--) {
        const idx = r.below(SCALARS.length);
        const t = SCALARS[idx][1];
        if (r.below(10) === 0) m.tag(idx + 1, r.pick([0, 1, 2, 5].filter((w) => w !== WIRE[t]))).raw(value(r, r.pick([T.INT32, T.DOUBLE, T.FLOAT, T.STRING])).finish());
        else field(r, m, idx + 1, t);
      }
      s.data("m.s3.S", m.finish());
    }
    s.data("m.s3.S", new Uint8Array(0));
    s.data(".m.s3.S", pb().vint(5, 7).finish());
  });

  add("scalars/proto2-defaults", (s) => {
    const defaults: [number, string][] = [
      [T.INT32, "7"],
      [T.INT32, "-7"],
      [T.INT32, "1.5"],
      [T.INT32, "1e5"],
      [T.INT32, "."],
      [T.INT32, "0x10"],
      [T.INT32, " 7"],
      [T.INT32, "abc"],
      [T.INT32, "true"],
      [T.INT32, "FALSE"],
      [T.INT32, "00012"],
      [T.INT32, "123456789012345678901234"],
      [T.DOUBLE, "inf"],
      [T.DOUBLE, "-inf"],
      [T.DOUBLE, "nan"],
      [T.DOUBLE, "2.5"],
      [T.DOUBLE, "-2.5"],
      [T.FLOAT, "1e40"],
      [T.BOOL, "true"],
      [T.BOOL, "1"],
      [T.STRING, "hello"],
      [T.STRING, "123"],
      [T.STRING, ""],
      [T.STRING, "TRUE"],
      [T.BYTES, "AAEC"],
      [T.BYTES, "hello"],
      [T.BYTES, "aGk="],
      [T.BYTES, "\\x00"],
      [T.BYTES, "é"],
      [T.UINT32, "4294967295"],
      [T.INT64, "9007199254740993"],
      [T.INT64, "-5"],
      [T.INT64, "-9223372036854775808"],
      [T.INT64, "+9e18"],
      [T.INT64, "9223372036854775807"],
      [T.INT64, "abc"],
      [T.INT64, "true"],
      [T.INT64, " 12 "],
      [T.UINT64, "-5"],
      [T.UINT64, "18446744073709551615"],
      [T.UINT64, "1e30"],
      [T.SINT64, "-1"],
      [T.FIXED64, "5"],
      [T.SFIXED64, "-3"],
      [T.SINT32, "-3"],
      [T.FIXED32, "3000000000"]
    ];
    s.schema(
      file({
        name: "d2.proto",
        package: "m.d2",
        syntax: "proto2",
        messageType: [{ name: "D", field: defaults.map(([t, d], i) => f(`f${i}`, i + 1, t, { defaultValue: d })) }]
      })
    );
    s.data("m.d2.D", new Uint8Array(0));
    s.data("m.d2.D", pb().vint(1, 3).len(21, "x").vint(31, 4).finish());
  });

  add("scalars/bad-bytes-defaults", (s) => {
    s.schema(
      file({
        name: "bb.proto",
        package: "m.bb",
        messageType: [
          { name: "Num", field: [f("a", 1, T.INT32), f("b", 2, T.BYTES, { defaultValue: "123" })] },
          { name: "Bool", field: [f("b", 1, T.BYTES, { defaultValue: "true" })] },
          { name: "Opt", field: [f("b", 1, T.BYTES, { proto3Optional: true })] },
          { name: "Outer", field: [msg("n", 1, ".m.bb.Num"), f("x", 2, T.INT32)] }
        ]
      })
    );
    s.data("m.bb.Num", pb().vint(1, 1).finish());
    s.data("m.bb.Num", pb().vint(1, 1).finish());
    s.data("m.bb.Bool", new Uint8Array(0));
    s.data("m.bb.Opt", new Uint8Array(0));
    s.data("m.bb.Outer", pb().vint(2, 1).finish());
    s.data("m.bb.Outer", pb().len(1, pb().vint(1, 2)).finish());
    s.data("m.bb.Outer", pb().len(1, pb().vint(1, 2)).raw([0x10]).finish());
  });

  add("enums", (s, r) => {
    s.schema(
      file({
        name: "e.proto",
        package: "m.e",
        syntax: "proto2",
        enumType: [
          {
            name: "Color",
            value: [
              { name: "RED", number: 3 },
              { name: "GREEN", number: 1 },
              { name: "ALIAS", number: 3 },
              { name: "10", number: 10 },
              { name: "", number: 4 },
              { name: "NEG", number: -1 },
              { name: "__proto__", number: 9 },
              { name: "toString", number: 6 }
            ]
          },
          { name: "Empty" },
          { name: "", value: [{ name: "Z", number: 0 }] }
        ],
        messageType: [
          {
            name: "E",
            field: [
              f("c", 1, T.ENUM, { typeName: ".m.e.Color" }),
              f("d1", 2, T.ENUM, { typeName: ".m.e.Color", defaultValue: "GREEN" }),
              f("d2", 3, T.ENUM, { typeName: ".m.e.Color", defaultValue: "MISSING" }),
              f("d3", 4, T.ENUM, { typeName: ".m.e.Color", defaultValue: "-1" }),
              f("d4", 5, T.ENUM, { typeName: ".m.e.Color", defaultValue: "toString" }),
              f("d5", 6, T.ENUM, { typeName: ".m.e.Color", defaultValue: "__proto__" }),
              f("d6", 7, T.ENUM, { typeName: ".m.e.Color", defaultValue: "7" }),
              f("d7", 8, T.ENUM, { typeName: ".m.e.Color", defaultValue: "constructor" }),
              f("empty", 9, T.ENUM, { typeName: ".m.e.Empty" }),
              rep("rc", 10, T.ENUM, { typeName: "Color" }),
              f("anon", 11, T.ENUM, { typeName: "Enum0" }),
              f("d8", 12, T.ENUM, { typeName: ".m.e.Color", defaultValue: "10" }),
              f("d9", 13, T.ENUM, { typeName: ".m.e.Color", defaultValue: "4" })
            ]
          }
        ]
      })
    );
    for (let i = 0; i < 10; i++) {
      const m = pb();
      if (r.bool()) field(r, m, 1, T.ENUM);
      if (r.bool()) m.packed(10, pb().raw(Pb.varint(3)).raw(Pb.varint(-1)).raw(Pb.varint(99)));
      if (r.bool()) field(r, m, 10, T.ENUM);
      if (r.bool()) field(r, m, 9, T.ENUM);
      s.data("m.e.E", m.finish());
    }
  });

  add("repeated", (s, r) => {
    const packable = SCALARS.filter(([, t]) => t !== T.STRING && t !== T.BYTES);
    s.schema(
      file({
        name: "r.proto",
        package: "m.r",
        syntax: "proto3",
        messageType: [
          { name: "Item", field: [f("v", 1, T.INT32), f("w", 2, T.STRING)] },
          {
            name: "R",
            field: [
              ...SCALARS.map(([name, t], i) => rep(`r_${name}`, i + 1, t, { packed: i % 2 === 0 })),
              rep("items", 20, T.MESSAGE, { typeName: ".m.r.Item" }),
              rep("nested", 21, T.MESSAGE, { typeName: "R" })
            ]
          }
        ]
      })
    );
    for (let i = 0; i < 50; i++) {
      const m = pb();
      for (let k = r.below(10); k > 0; k--) {
        const which = r.below(SCALARS.length + 3);
        if (which < SCALARS.length) {
          const t = SCALARS[which][1];
          const isPackable = packable.some(([, pt]) => pt === t);
          if (isPackable && r.bool()) {
            const items = pb();
            for (let n = r.below(4); n > 0; n--) items.raw(value(r, t, true).finish());
            m.packed(which + 1, items);
          } else field(r, m, which + 1, t, true);
        } else if (which === SCALARS.length) {
          m.len(20, pb().vint(1, r.range(-5, 5)).len(2, r.ascii(3)));
        } else if (which === SCALARS.length + 1) {
          m.len(21, pb().len(20, pb().vint(1, 1)).vint(5, 2));
        } else {
          m.len(20, pb());
        }
      }
      s.data("m.r.R", m.finish());
    }
    s.data("m.r.R", pb().len(1, [0, 0, 0]).finish());
    s.data("m.r.R", pb().len(5, [0x80]).finish());
    s.data("m.r.R", pb().len(5, [0x80, 0x80]).vint(1, 1).finish());
  });

  add("maps", (s, r) => {
    const keyTypes = SCALARS.filter(([, t]) => t !== T.DOUBLE && t !== T.FLOAT && t !== T.BYTES);
    const valueTypes: [string, number, string?][] = [
      ["double", T.DOUBLE],
      ["string", T.STRING],
      ["bytes", T.BYTES],
      ["int64", T.INT64],
      ["color", T.ENUM, ".m.mp.Color"],
      ["msg", T.MESSAGE, ".m.mp.V"]
    ];
    const nested: object[] = [];
    const fields: object[] = [];
    let n = 1;
    for (const [kn, kt] of keyTypes) {
      for (const [vn, vt, vtn] of valueTypes) {
        const entry = `${kn}_${vn}Entry`;
        nested.push({
          name: entry,
          options: { mapEntry: true },
          field: [f("key", 1, kt), f("value", 2, vt, vtn ? { typeName: vtn } : {})]
        });
        fields.push(rep(`${kn}_${vn}`, n++, T.MESSAGE, { typeName: `.m.mp.M.${entry}` }));
      }
    }
    s.schema(
      file({
        name: "mp.proto",
        package: "m.mp",
        syntax: "proto3",
        enumType: [{ name: "Color", value: [{ name: "A", number: 0 }, { name: "B", number: 1 }] }],
        messageType: [
          { name: "V", field: [f("x", 1, T.INT32), f("ControlWord", 2, T.UINT32)] },
          { name: "M", nestedType: nested, field: fields }
        ]
      })
    );
    const total = keyTypes.length * valueTypes.length;
    const specialKeys: [number, () => Pb][] = [
      [T.STRING, () => pb().len(1, r.pick(["__proto__", "10", "2", "a/b", "", "constructor", "-1", "4294967295"]))],
      [T.INT64, () => pb().vint(1, r.pick([0x3132333435363738n, 0x3031323334353637n, 0n, -1n, 0x2d31313131313131n]))],
      [T.BOOL, () => pb().vint(1, r.pick([0, 1, 7]))]
    ];
    for (let i = 0; i < 70; i++) {
      const m = pb();
      for (let k = r.below(6); k > 0; k--) {
        const idx = r.below(total);
        const [, kt] = keyTypes[Math.floor(idx / valueTypes.length)];
        const [, vt] = valueTypes[idx % valueTypes.length];
        const entry = pb();
        const special = specialKeys.find(([t]) => t === kt);
        const shape = r.below(8);
        if (shape !== 0) {
          if (special && r.bool()) entry.raw(special[1]().finish());
          else field(r, entry, 1, kt);
        }
        if (shape !== 1) {
          if (vt === T.MESSAGE) entry.len(2, pb().vint(1, r.range(-3, 3)).vint(2, r.u32()));
          else field(r, entry, 2, vt);
        }
        if (shape === 2) entry.vint(3, 1);
        if (shape === 3) entry.fixed32(1, 5);
        m.len(idx + 1, entry);
        if (r.below(5) === 0) m.len(idx + 1, entry);
      }
      s.data("m.mp.M", m.finish());
    }
    s.data("m.mp.M", pb().vint(1, 5).finish());
    s.data("m.mp.M", pb().len(1, [0x0a]).finish());
  });

  add("maps/invalid", (s) => {
    s.schema(
      file({
        name: "mi.proto",
        package: "m.mi",
        messageType: [
          {
            name: "FloatKey",
            nestedType: [{ name: "KEntry", options: { mapEntry: true }, field: [f("key", 1, T.FLOAT), f("value", 2, T.INT32)] }],
            field: [rep("k", 1, T.MESSAGE, { typeName: "KEntry" })]
          }
        ]
      })
    );
    s.data("m.mi.FloatKey", new Uint8Array(0));
    s.schema(
      file({
        name: "mi2.proto",
        package: "m.mi2",
        messageType: [
          {
            name: "MsgKey",
            nestedType: [{ name: "KEntry", options: { mapEntry: true }, field: [f("key", 1, T.MESSAGE), f("value", 2, T.INT32)] }],
            field: [rep("k", 1, T.MESSAGE, { typeName: "KEntry" })]
          }
        ]
      })
    );
    s.schema(
      file({
        name: "mi3.proto",
        package: "m.mi3",
        messageType: [
          {
            name: "NoValue",
            nestedType: [{ name: "KEntry", options: { mapEntry: true }, field: [f("key", 1, T.STRING)] }],
            field: [rep("k", 1, T.MESSAGE, { typeName: "KEntry" })]
          }
        ]
      })
    );
    s.schema(
      file({
        name: "mi4.proto",
        package: "m.mi4",
        messageType: [
          {
            name: "ProtoName",
            nestedType: [{ name: "Holder", field: [f("x", 1, T.INT32)] }],
            field: [rep("k", 1, T.MESSAGE, { typeName: "x.toString" }), rep("j", 2, T.MESSAGE, { typeName: "constructor" })]
          }
        ]
      })
    );
    s.schema(
      file({
        name: "mi5.proto",
        package: "m.mi5",
        messageType: [
          {
            name: "Lazy",
            nestedType: [
              { name: "VEntry", options: { mapEntry: true }, field: [f("key", 1, T.STRING), f("value", 2, T.MESSAGE, { typeName: "m.mi5.Later" })] },
              { name: "WEntry", options: { mapEntry: true }, field: [f("key", 1, T.INT32), f("value", 2, undefined, { typeName: "a b" })] }
            ],
            field: [rep("v", 1, T.MESSAGE, { typeName: "VEntry" }), rep("w", 2, T.MESSAGE, { typeName: "WEntry" })]
          },
          {
            name: "Lazy2",
            nestedType: [{ name: "VEntry", options: { mapEntry: true }, field: [f("key", 1, T.STRING), f("value", 2, T.MESSAGE, { typeName: "m.mi5.Later" })] }],
            field: [rep("v", 1, T.MESSAGE, { typeName: "VEntry" })]
          }
        ]
      })
    );
    s.data("m.mi5.Lazy", new Uint8Array(0));
    s.data("m.mi5.Lazy2", pb().len(1, pb().len(1, "k")).finish());
    s.schema(file({ name: "mi5b.proto", package: "m.mi5", messageType: [{ name: "Later", field: [f("y", 1, T.INT32)] }] }));
    s.data("m.mi5.Lazy2", pb().len(1, pb().len(1, "k").len(2, pb().vint(1, 4))).len(1, pb().len(1, "e")).finish());
  });

  add("oneofs", (s, r) => {
    s.schema(
      file({
        name: "o.proto",
        package: "m.o",
        syntax: "proto3",
        messageType: [
          { name: "In", field: [f("a", 1, T.INT32), msg("deep", 2, ".m.o.In")] },
          {
            name: "O",
            oneofDecl: [{ name: "zeta" }, { name: "seven" }, { name: "" }, { name: "_opt" }, { name: "3" }],
            field: [
              f("s", 1, T.STRING, { oneofIndex: 2 }),
              f("i", 2, T.INT32, { oneofIndex: 2 }),
              msg("m", 3, ".m.o.In", { oneofIndex: 2 }),
              f("a", 4, T.INT64, { oneofIndex: 1 }),
              rep("rr", 5, T.INT32, { oneofIndex: 1 }),
              f("plain", 6, T.INT32),
              f("opt", 7, T.DOUBLE, { oneofIndex: 4, proto3Optional: true }),
              f("third", 8, T.BOOL, { oneofIndex: 3 }),
              f("bare", 9, T.UINT32, { proto3Optional: true })
            ]
          },
          { name: "Digit", oneofDecl: [{ name: "zeta" }, { name: "7" }], field: [f("s", 1, T.STRING, { oneofIndex: 0 }), f("t", 2, T.INT32, { oneofIndex: 1 })] },
          { name: "Bad", oneofDecl: [{ name: "x" }], field: [f("y", 1, T.INT32, { oneofIndex: 3 })] }
        ]
      })
    );
    for (let i = 0; i < 40; i++) {
      const m = pb();
      for (let k = r.below(6); k > 0; k--) {
        switch (r.below(9)) {
          case 0:
            m.len(1, r.ascii(3));
            break;
          case 1:
            m.vint(2, r.range(-3, 3));
            break;
          case 2:
            m.len(3, r.bool() ? pb().vint(1, 1) : pb().len(2, pb().vint(1, 2)));
            break;
          case 3:
            m.vint(4, r.u64());
            break;
          case 4:
            if (r.bool()) m.vint(5, r.range(0, 9));
            else m.packed(5, pb().raw(Pb.varint(r.range(0, 9))).raw(Pb.varint(r.range(0, 9))));
            break;
          case 5:
            m.vint(6, r.range(0, 9));
            break;
          case 6:
            m.f64(7, r.double());
            break;
          case 7:
            m.vint(8, 1);
            break;
          case 8:
            m.vint(9, r.u32());
            break;
        }
      }
      s.data("m.o.O", m.finish());
    }
    s.data("m.o.O", pb().vint(4, 5).packed(5, pb().raw([1, 2])).finish());
    s.data("m.o.O", pb().packed(5, pb().raw([1])).vint(4, 5).vint(5, 3).finish());
    s.data("m.o.Digit", pb().len(1, "s").finish());
  });

  add("nesting", (s, r) => {
    s.schema(
      file({
        name: "n.proto",
        package: "m.n",
        messageType: [
          {
            name: "ProtobufNode",
            field: [f("v", 1, T.INT32), msg("child", 2, ".m.n.ProtobufNode"), rep("kids", 3, T.MESSAGE, { typeName: "ProtobufNode" }), msg("e", 4, ".m.n.Empty")],
            nestedType: [{ name: "Inner", field: [f("q", 1, T.STRING)], nestedType: [{ name: "Deeper", field: [f("z", 1, T.BOOL)] }] }]
          },
          { name: "Empty" },
          {
            name: "UsesNested",
            field: [msg("i", 1, "ProtobufNode.Inner"), msg("d", 2, ".m.n.ProtobufNode.Inner.Deeper"), rep("ds", 3, T.MESSAGE, { typeName: "ProtobufNode.Inner.Deeper" })]
          }
        ]
      })
    );
    const chain = (depth: number): Pb => (depth === 0 ? pb().vint(1, 1) : pb().len(2, chain(depth - 1)));
    s.data("m.n.ProtobufNode", chain(5).finish());
    s.data("m.n.ProtobufNode", chain(99).finish());
    s.data("m.n.ProtobufNode", chain(100).finish());
    s.data("m.n.ProtobufNode", chain(101).finish());
    s.data("m.n.ProtobufNode", pb().len(2, pb().vint(1, 1)).len(2, pb().vint(1, 2).len(3, pb())).len(2, pb().len(4, pb())).finish());
    s.data("m.n.ProtobufNode", pb().len(3, pb().len(2, pb())).len(3, pb().vint(1, 5)).len(3, pb().len(3, pb())).finish());
    s.data("m.n.Empty", pb().vint(1, 5).len(2, "x").finish());
    s.data("m.n.Empty", hex("0b 08 01 0c"));
    s.data("m.n.Empty", hex("0b 0c"));
    s.data("m.n.Empty", hex("5b 5c"));
    s.data("m.n.Empty", hex("00"));
    s.data("m.n.ProtobufNode", hex("0b 08 01 0c 08 05"));
    s.data("m.n.ProtobufNode", hex("1b 08 01 1c 08 05"));
    s.data("m.n.ProtobufNode", hex("1b 08 01 0c"));
    s.data("m.n.ProtobufNode", hex("0c"));
    s.data("m.n.ProtobufNode", pb().len(4, hex("0b 0c")).finish());
    s.data("m.n.UsesNested", pb().len(1, pb().len(1, "q")).len(2, pb().vint(1, 1)).len(3, pb()).len(3, pb().vint(1, 0)).finish());
    for (let i = 0; i < 20; i++) {
      const build = (d: number): Pb => {
        const m = pb();
        if (r.bool()) m.vint(1, r.range(-9, 9));
        if (d > 0 && r.bool()) m.len(2, build(d - 1));
        if (d > 0) for (let k = r.below(3); k > 0; k--) m.len(3, build(d - 1));
        if (r.below(4) === 0) m.len(4, pb());
        return m;
      };
      s.data("m.n.ProtobufNode", build(3).finish());
    }
    let groups = pb();
    for (let i = 0; i < 101; i++) groups.tag(5, 3);
    s.data("m.n.Empty", groups.finish());
    groups = pb();
    for (let i = 0; i < 100; i++) groups.tag(5, 3);
    for (let i = 0; i < 100; i++) groups.tag(5, 4);
    s.data("m.n.ProtobufNode", groups.finish());
  });

  add("names", (s, r) => {
    s.schema(
      file({
        name: "names.proto",
        package: "m.nm",
        messageType: [
          {
            name: "We-ird.Name$",
            field: [f("1", 1, T.INT32), f("b", 2, T.INT32), f("0", 3, T.STRING), f("constructor", 4, T.INT32), f("toString", 5, T.STRING), f("", 6, T.BOOL), f("__proto__", 7, T.INT32), f("a/b", 8, T.INT32), f("valueOf", 9, T.INT32)]
          },
          { name: "", field: [f("x", 1, T.INT32)] },
          { name: "", field: [msg("prev", 1, "Type0")] },
          { name: "ProtobufProtobufX", field: [msg("self", 1, "ProtobufProtobufX")] },
          { name: "Protobuf", field: [msg("p", 1, ".m.nm.Protobuf"), rep("ps", 2, T.MESSAGE, { typeName: "Protobuf" })] },
          { name: "HasOwn", field: [f("x", 1, T.INT32), f("hasOwnProperty", 2, T.INT32)] },
          { name: "HasOwnMsg", field: [rep("r", 1, T.INT32), msg("hasOwnProperty", 2, ".m.nm.HasOwnMsg")] },
          { name: "HasOwnRep", field: [rep("hasOwnProperty", 1, T.INT32)] },
          { name: "HasOwnRep2", field: [rep("hasOwnProperty", 1, T.INT32), f("z", 2, T.INT32)] }
        ],
        enumType: [{ name: "", value: [{ name: "A", number: 0 }] }],
        service: [{ name: "", method: [{ name: "", inputType: ".m.nm.HasOwn", outputType: "HasOwn" }, { name: "Call" }] }]
      })
    );
    for (let i = 0; i < 8; i++) {
      const m = pb();
      for (let k = 1; k <= 9; k++) if (r.bool()) field(r, m, k, [0, T.INT32, T.INT32, T.STRING, T.INT32, T.STRING, T.BOOL, T.INT32, T.INT32, T.INT32][k]);
      s.data("m.nm.WeirdName", m.finish());
    }
    s.data("m.nm.We-ird.Name$", new Uint8Array(0));
    s.data("m.nm.Type1", pb().len(1, pb().vint(1, 2)).finish());
    s.data("Type0", pb().vint(1, 2).finish());
    s.data("m.nm.ProtobufProtobufX", pb().len(1, pb().len(1, pb())).finish());
    s.data("m.nm.Protobuf", pb().len(1, pb()).len(2, pb()).len(2, pb().len(1, pb())).finish());
    s.data("m.nm.HasOwn", new Uint8Array(0));
    s.data("m.nm.HasOwn", pb().vint(2, 5).finish());
    s.data("m.nm.HasOwnMsg", new Uint8Array(0));
    s.data("m.nm.HasOwnMsg", pb().len(2, pb()).finish());
    s.data("m.nm.HasOwnRep", new Uint8Array(0));
    s.data("m.nm.HasOwnRep2", pb().vint(1, 3).finish());
    s.data("m.nm.HasOwnRep2", new Uint8Array(0));
    s.data("m.nm.Service0", new Uint8Array(0));
    s.data("m.nm.Service0.Method0", new Uint8Array(0));
    s.data("m.nm.Enum1", new Uint8Array(0));
  });

  add("lookups", (s) => {
    s.schema(file({ name: "l1.proto", package: "a.b", messageType: [{ name: "X", field: [f("v", 1, T.INT32)] }, { name: "Y", field: [msg("x", 1, "X"), msg("rx", 2, "b.X")] }] }));
    s.schema(file({ name: "l2.proto", messageType: [{ name: "X", field: [f("top", 1, T.STRING)] }, { name: "Root", field: [msg("x", 1, "a.b.X")] }] }));
    s.schema(
      file({
        name: "l3.proto",
        package: "c",
        messageType: [
          { name: "Z", field: [msg("x", 1, "X"), msg("bx", 2, "b.X"), msg("ax", 3, ".b.X"), msg("en", 4, "E")] },
          { name: "X", field: [f("inner", 1, T.BOOL)] }
        ],
        enumType: [{ name: "E", value: [{ name: "Q", number: 2 }] }],
        service: [{ name: "Svc", method: [{ name: "Do", inputType: "Z", outputType: "Z" }] }]
      })
    );
    const one = pb().len(1, pb().vint(1, 1).len(1, "s").vint(4, 1)).finish();
    for (const name of [
      "a.b.X",
      ".a.b.X",
      "X",
      "b.X",
      "a.b.Y",
      "Y",
      "c.Z",
      "Z",
      "c.X",
      "a",
      "a.b",
      "c.E",
      "E",
      "c.Svc",
      "c.Svc.Do",
      "",
      ".",
      "..",
      "a..b",
      "a.b.X.v",
      "Root",
      "x.y",
      "toString",
      "__proto__",
      "constructor",
      "b"
    ]) {
      s.data(name, one);
    }
  });

  add("descriptor-errors", (s) => {
    const good = file({ name: "g.proto", package: "m.g", messageType: [{ name: "G", field: [f("x", 1, T.INT32)] }] });
    s.data("m.g.G", pb().vint(1, 1).finish());
    s.schema(good);
    s.schema(good);
    s.schema(file({ name: "g2.proto", package: "m.g", messageType: [{ name: "G", field: [f("y", 1, T.INT32)] }] }));
    s.data("m.g.G", pb().vint(1, 2).finish());
    s.schema(file({ name: "g3.proto", package: "m.g", messageType: [{ name: "H", field: [f("x", 1, T.INT32)] }, { name: "G" }, { name: "I" }] }));
    s.data("m.g.H", pb().vint(1, 3).finish());
    s.data("m.g.I", pb().vint(1, 3).finish());
    s.schema(file({ name: "e1.proto", messageType: [{ name: "DupId", field: [f("a", 1, T.INT32), f("b", 1, T.INT32)] }] }));
    s.schema(file({ name: "e2.proto", messageType: [{ name: "DupName", field: [f("a", 1, T.INT32), f("a", 2, T.INT32)] }] }));
    s.schema(file({ name: "e3.proto", messageType: [{ name: "BadLabel", field: [f("a", 1, T.INT32, { label: 0 })] }] }));
    s.schema(file({ name: "e4.proto", messageType: [{ name: "BadType", field: [f("a", 1, 99)] }] }));
    s.schema(file({ name: "e5.proto", messageType: [{ name: "BadTypeName", field: [f("a", 1, T.MESSAGE, { typeName: "a b" })] }] }));
    s.schema(file({ name: "e6.proto", messageType: [{ name: "NegId", field: [f("a", -1, T.INT32)] }] }));
    s.schema(file({ name: "e7.proto", messageType: [{ name: "Group", field: [f("a", 1, T.GROUP)] }] }));
    s.schema(file({ name: "e8.proto", package: ".lead", messageType: [{ name: "A" }] }));
    s.schema(file({ name: "e9.proto", package: "m.g.G.sub", messageType: [{ name: "Inside", field: [f("q", 1, T.INT32)] }] }));
    s.data("m.g.G.sub.Inside", pb().vint(1, 9).finish());
    s.schema(file({ name: "e10.proto", package: "m.e.Color" }));
    s.schema(file({ name: "e11.proto", extension: [f("orphan", 5, T.INT32)] }));
    s.schema(file({ name: "e12.proto", messageType: [{ name: "BadExtendee", field: [f("a", 1, T.INT32, { extendee: "1x" })] }] }));
    let deep: any = { name: "D33" };
    for (let i = 32; i >= 0; i--) deep = { name: `D${i}`, nestedType: [deep] };
    s.schema(file({ name: "e13.proto", messageType: [deep] }));
    deep = { name: "N32" };
    for (let i = 31; i >= 0; i--) deep = { name: `N${i}`, nestedType: [deep] };
    s.schema(file({ name: "e14.proto", messageType: [deep] }));
    s.schema(file({ name: "e15.proto", messageType: [{ name: "Neg", oneofDecl: [{ name: "o" }], field: [f("a", 1, T.INT32, { oneofIndex: -1 })] }] }));
    s.schema(file({ name: "e16.proto", messageType: [{ name: "FieldVsOneof", oneofDecl: [{ name: "a" }], field: [f("a", 1, T.INT32)] }] }));
    s.schema(file({ name: "e17.proto", messageType: [{ name: "FieldVsNested", field: [f("Sub", 1, T.INT32)], nestedType: [{ name: "Sub" }] }] }));
    s.schema(file({ name: "e18.proto", messageType: [{ name: "BadMethod" }], service: [{ name: "S", method: [{ name: "m", inputType: "1bad" }] }] }));
    s.schema(file({ name: "e19.proto", service: [{ name: "S2", method: [{ name: "m" }, { name: "m" }] }] }));
    s.schema(file({ name: "e20.proto", package: "constructor", messageType: [{ name: "C" }] }));
    s.schema(file({ name: "e21.proto", package: "__proto__", messageType: [{ name: "P", field: [f("x", 1, T.INT32)] }] }));
    s.data("__proto__.P", pb().vint(1, 1).finish());
    s.data("P", pb().vint(1, 1).finish());
    s.schema(file({ name: "e22.proto", package: "m..g", messageType: [{ name: "Gap", field: [f("x", 1, T.INT32)] }] }));
    s.data("m..g.Gap", pb().vint(1, 1).finish());
    s.schema(file({ name: "e23.proto", package: "fresh", messageType: [{ name: "Q", field: [f("x", 1, T.INT32)] }] }));
    s.schema(file({ name: "e24.proto", messageType: [{ name: "fresh", field: [f("y", 1, T.INT32)] }] }));
    s.data("fresh.Q", pb().vint(1, 4).finish());
    s.data("fresh", pb().vint(1, 4).finish());
    s.schema(file({ name: "e25.proto", package: "fresh2", messageType: [{ name: "Q" }] }));
    s.schema(file({ name: "e26.proto", messageType: [{ name: "fresh2", field: [f("Q", 1, T.INT32)] }] }));
    s.data("fresh2", pb().vint(1, 4).finish());
    s.data("fresh2.Q", pb().vint(1, 4).finish());
    s.schema(file({ name: "e27.proto", enumType: [{ name: "fresh3" }], messageType: [{ name: "fresh3" }] }));
    s.schema(file({ name: "e28.proto", messageType: [{ name: "Reserved", field: [f("a", 5, T.INT32)], reservedRange: [{ start: 1, end: 10 }], reservedName: ["a"] }] }));
    s.data("Reserved", pb().vint(5, 1).finish());
    s.schema(file({ name: "e29.proto", messageType: [{ name: "IdZero", field: [f("z", 0, T.INT32), f("one", 1, T.INT32)] }] }));
    s.data("IdZero", hex("00 05 08 02"));
    s.data("IdZero", hex("02 00"));
    s.schema(file({ name: "e30.proto", messageType: [{ name: "BasicName", field: [f("d", 1, T.MESSAGE, { typeName: "double" }), f("u", 2, T.INT32, { typeName: "undefined" }), f("s", 3, T.MESSAGE, { typeName: "string" })] }] }));
    s.data("BasicName", pb().f64(1, 1.5).len(3, "str").finish());
    s.data("BasicName", pb().len(2, pb()).finish());
    s.data("BasicName", pb().vint(2, 1).finish());
    s.schema(file({ name: "e31.proto", messageType: [{ name: "MsgDefault", field: [msg("a", 1, "MsgDefault", { defaultValue: "0" }), msg("b", 2, "MsgDefault", { defaultValue: "x" })] }] }));
    s.data("MsgDefault", new Uint8Array(0));
    s.data("MsgDefault", pb().len(1, pb()).finish());
    s.schema(hex("0a ff 01"));
    s.schema(new Uint8Array(0));
    s.schema(hex("12 03 61 2e 62"));
    s.schema(hex("62 05 70 72 6f 74 6f"));
    s.schema(hex("42 04 3a 02 0a 00"));
    s.schema(hex("42 06 3a 04 12 02 08 01"));
    s.schema(hex("22 04 0a 02 41 42 08 01 22 02 0a 01"));
    s.schema(hex("22 06 0a 01 43 3a 01 ff"));
    s.schema(hex("0a 00 22 03 0a 01 44 22 03 0a 01 44"));
    s.data("D", new Uint8Array(0));
    s.data("AB", new Uint8Array(0));
  });

  add("extensions", (s) => {
    s.schema(
      file({
        name: "x1.proto",
        package: "m.x",
        messageType: [
          { name: "Base", field: [f("a", 1, T.INT32)], extensionRange: [{ start: 100, end: 200 }] },
          { name: "Holder", extension: [f("held", 101, T.STRING, { extendee: "Base" }), f("plain", 2, T.INT32)] }
        ],
        extension: [f("ext", 100, T.INT64, { extendee: ".m.x.Base" }), f("later", 102, T.INT32, { extendee: ".m.y.Later" })]
      })
    );
    s.data("m.x.Base", pb().vint(1, 1).vint(100, 5).len(101, "h").vint(102, 3).finish());
    s.data("m.x.Holder", pb().vint(2, 1).finish());
    s.data("m.x.Base", new Uint8Array(0));
    s.schema(file({ name: "x2.proto", package: "m.y", messageType: [{ name: "Later", field: [f("b", 1, T.INT32)] }] }));
    s.data("m.y.Later", pb().vint(1, 1).vint(102, 3).finish());
    s.schema(file({ name: "x3.proto", package: "m.z", enumType: [{ name: "En", value: [{ name: "A", number: 0 }] }], extension: [f("onEnum", 5, T.INT32, { extendee: "En" })] }));
    s.schema(file({ name: "x4.proto", package: "m.w", extension: [f("dup", 1, T.INT32, { extendee: ".m.x.Base" })] }));
    s.data("m.x.Base", pb().vint(1, 7).finish());
    s.schema(file({ name: "x5.proto", package: "m.v", extension: [f("ns", 5, T.INT32, { extendee: ".m" })] }));
    s.schema(file({ name: "x6.proto", package: "m.u", messageType: [{ name: "Rsv", field: [f("a", 1, T.INT32)], reservedRange: [{ start: 50, end: 60 }] }], extension: [f("rsv", 55, T.INT32, { extendee: "Rsv" })] }));
    s.data("m.u.Rsv", pb().vint(55, 1).finish());
  });

  add("schema-after-data", (s, r) => {
    const A = file({ name: "dep/a.proto", package: "m.dep", dependency: ["dep/b.proto"], messageType: [{ name: "A", field: [msg("b", 1, ".m.dep.B"), f("x", 2, T.INT32), rep("bs", 3, T.MESSAGE, { typeName: ".m.dep.B" })] }, { name: "A2", field: [f("x", 1, T.INT32), msg("b", 2, ".m.dep.B")] }] });
    const B = file({ name: "dep/b.proto", package: "m.dep", messageType: [{ name: "B", field: [f("y", 1, T.DOUBLE), msg("c", 2, ".m.dep.C")] }] });
    const C = file({ name: "dep/c.proto", package: "m.dep", messageType: [{ name: "C", field: [f("z", 1, T.STRING)] }] });
    s.data("m.dep.A", pb().vint(2, 1).finish());
    s.data("m.dep.B", pb().f64(1, 2).finish());
    s.schema(A);
    s.data("m.dep.A", pb().vint(2, 1).finish());
    s.data("m.dep.A2", pb().vint(1, 1).finish());
    s.schema(B);
    s.data("m.dep.A", pb().vint(2, 2).len(1, pb().f64(1, r.double())).finish());
    s.data("m.dep.A", pb().vint(2, 2).len(1, pb().len(2, pb().len(1, "c"))).finish());
    s.schema(C);
    s.data("m.dep.A", pb().len(3, pb().len(2, pb().len(1, "z"))).len(3, pb()).finish());
    const before = s.current();
    s.restore(before);
    s.data("m.dep.A", pb().len(1, pb().len(2, pb().len(1, "again"))).finish());
    s.restore([C, B, A]);
    s.restore([hex("0a ff"), A]);
    s.restore([]);
    s.data("m.dep.A", new Uint8Array(0));
    s.restore([B, file({ name: "dep/b.proto", package: "zzz" }), A, file({ name: "bad.proto", messageType: [{ name: "Q", field: [f("a", 1, T.INT32), f("a", 2, T.INT32)] }] }), C]);
    s.data("m.dep.A", new Uint8Array(0));
    s.data("Q", new Uint8Array(0));
  });

  add("control-word", (s) => {
    const CONTROL = (name: string, type: number) =>
      file({
        name: `control-${name}.proto`,
        package: name,
        messageType: [
          { name: "ProtobufControlData", field: [f("ControlWord", 1, type), f("MatchTime", 2, T.FLOAT)] },
          {
            name: "Wrap",
            nestedType: [{ name: "CEntry", options: { mapEntry: true }, field: [f("key", 1, T.STRING), f("value", 2, T.MESSAGE, { typeName: `.${name}.ProtobufControlData` })] }],
            field: [
              msg("one", 1, `.${name}.ProtobufControlData`),
              rep("many", 2, T.MESSAGE, { typeName: `.${name}.ProtobufControlData` }),
              rep("byKey", 3, T.MESSAGE, { typeName: "CEntry" }),
              msg("self", 4, `.${name}.Wrap`)
            ]
          }
        ]
      });
    s.schema(CONTROL("mrc.proto", T.UINT32));
    s.schema(CONTROL("other.proto", T.UINT32));
    const words = [0, 1, 0x2133, 0x0b00, 0x0d01, 0xffffffff, 0x7fffffff];
    for (const w of words) s.data("mrc.proto.ProtobufControlData", pb().vint(1, w).finish());
    s.data("other.proto.ProtobufControlData", pb().vint(1, 3).finish());
    s.data(
      "mrc.proto.Wrap",
      pb()
        .len(1, pb().vint(1, 0x31))
        .len(2, pb().vint(1, 0x102))
        .len(2, pb())
        .len(3, pb().len(1, "ControlWord").len(2, pb().vint(1, 1)))
        .len(3, pb().len(1, "k").len(2, pb().vint(1, 5)))
        .len(4, pb().len(4, pb().len(1, pb().vint(1, 0x2101))))
        .finish()
    );
    s.data("mrc.proto.Wrap", new Uint8Array(0));
    s.data("mrc.proto.Wrap", pb().len(3, pb().len(1, "one").len(2, pb().vint(1, 9))).finish());
    s.schema(
      file({
        name: "control-variants.proto",
        package: "variant",
        messageType: [
          { name: "Holder", field: [msg("c", 1, ".mrc.proto.ProtobufControlData"), f("ControlWord", 2, T.INT64)] }
        ]
      })
    );
    s.data("variant.Holder", pb().len(1, pb().vint(1, 0x2133)).vint(2, 5).finish());
    s.schema(file({ name: "control-dbl.proto", package: "mrc.proto2", messageType: [{ name: "ProtobufControlData", field: [f("ControlWord", 1, T.DOUBLE)] }] }));
  });

  add("utf8", (s) => {
    s.schema(file({ name: "u.proto", package: "m.u8", messageType: [{ name: "U", field: [f("s", 1, T.STRING), msg("n", 2, ".m.u8.U"), f("t", 3, T.STRING)] }] }));
    const samples = [
      "68 c3 a9 6c 6c 6f",
      "c3 28",
      "e2 82",
      "f0 9f 98 80",
      "f4 90 80 80",
      "f5 80 80 80",
      "ff fe",
      "80 81 41",
      "ed a0 80",
      "ed b0 80",
      "c0 80",
      "e0 80 80",
      "f0 80 80 80",
      "41 42 43 44 45 46 47 c3",
      "41 42 43 44 45 46 47 48 49 e2",
      "f0 9f",
      "ef bb bf 41",
      "c2 80",
      "c1 bf",
      "df bf",
      "e0 a0 80",
      "e0 9f bf",
      "ef bf bf",
      "f0 90 80 80",
      "f0 8f bf bf",
      "f4 8f bf bf",
      "f7 bf bf bf",
      "f8 88 80 80"
    ];
    for (const h of samples) {
      const bytes = hex(h);
      s.data("m.u8.U", pb().len(1, bytes).finish());
      s.data("m.u8.U", pb().len(1, bytes).vint(3, 1).finish());
      s.data("m.u8.U", pb().len(2, pb().len(1, bytes)).len(3, "after").finish());
    }
  });

  add("floats", (s) => {
    s.schema(file({ name: "fl.proto", package: "m.fl", messageType: [{ name: "F", field: [f("d", 1, T.DOUBLE), f("f", 2, T.FLOAT), rep("rd", 3, T.DOUBLE), rep("rf", 4, T.FLOAT)] }] }));
    const dbits = [0x7ff0000000000001n, 0x7ff8000000000000n, 0xfff8000000000001n, 0x7ff4000000000000n, 0x8000000000000000n, 0x0000000000000001n, 0x7ff0000000000000n];
    const fbits = [0x7f800001, 0x7fc00000, 0xffc00001, 0x7fa00000, 0x80000000, 0x00000001, 0x7f800000, 0xff800000];
    const elementD = (b: bigint) => ((b & 0x7ff0000000000000n) === 0x7ff0000000000000n && (b & 0xfffffffffffffn) !== 0n ? 0x7ff8000000000000n : b);
    const elementF = (b: number) => ((b & 0x7f800000) === 0x7f800000 && (b & 0x7fffff) !== 0 ? 0x7fc00000 : b);
    for (const b of dbits) s.data("m.fl.F", pb().f64Bits(1, b).packed(3, pb().raw(pb().f64Bits(1, elementD(b)).finish().slice(1))).finish());
    for (const b of fbits) s.data("m.fl.F", pb().fixed32(2, b).fixed32(4, elementF(b)).finish());
  });

  add("malformed", (s, r) => {
    s.schema(
      file({
        name: "mal.proto",
        package: "m.mal",
        messageType: [
          { name: "In", field: [f("a", 1, T.INT64), f("s", 2, T.STRING)] },
          {
            name: "Mal",
            field: [
              f("i32", 1, T.INT32),
              f("i64", 2, T.INT64),
              f("d", 3, T.DOUBLE),
              f("fl", 4, T.FLOAT),
              f("s", 5, T.STRING),
              f("by", 6, T.BYTES),
              f("b", 7, T.BOOL),
              f("f32", 8, T.FIXED32),
              f("f64", 9, T.FIXED64),
              msg("in", 10, ".m.mal.In"),
              rep("ri", 11, T.INT64),
              rep("rd", 12, T.DOUBLE),
              rep("ins", 13, T.MESSAGE, { typeName: ".m.mal.In" }),
              f("u32", 14, T.UINT32),
              f("s64", 15, T.SINT64)
            ]
          }
        ]
      })
    );
    const fixed = [
      "08",
      "08 80",
      "08 80 80 80 80 80 80 80 80 80 80 01",
      "08 ff ff ff ff ff ff ff ff ff 01",
      "10",
      "10 80",
      "10 80 80 80",
      "10 80 80 80 80",
      "10 80 80 80 80 80",
      "10 80 80 80 80 80 80 80 80 80 80",
      "10 ff ff ff ff ff ff ff ff ff 01",
      "10 ff ff ff ff ff ff ff ff ff 7f",
      "08 01 10 80 80 80 80 80 80 80 80 80 80",
      "19 00 00",
      "25 00",
      "2a 05 61",
      "2a 80",
      "2a ff ff ff ff 0f",
      "32 03 01",
      "38",
      "38 80 80 80 80 80 80 80 80 80 80",
      "45 01 02 03",
      "49 01 02 03 04 05 06 07",
      "52 03 08 01",
      "52 04 08 80 80 80",
      "52 02 12 05",
      "5a 03 01 02",
      "62 05 00 00 00 00 00",
      "6a 02 08",
      "6a 01 80",
      "70 80 80 80 80 10",
      "70 80 80 80 80 80 80 80 80 80 01",
      "78 80 80 80",
      "80",
      "80 80 80 80 80",
      "80 80 80 80 10",
      "80 80 80 80 80 80",
      "f8 ff ff ff 0f 01",
      "f8 ff ff ff 7f 01",
      "0e",
      "0f 01",
      "0c",
      "0b",
      "0b 0c",
      "0b 10 01 0c",
      "0b 1c",
      "0b 08 01",
      "07 01",
      "06 01",
      "00",
      "03",
      "04",
      "0a 01 01",
      "0a 05 01",
      "52 81 80 80 80 10 00",
      "52 ff ff ff ff 0f",
      "12 01",
      "1a 08 00 00 00 00 00 00 f0 3f 08"
    ];
    for (const h of fixed) s.data("m.mal.Mal", hex(h));
    const base = (): Uint8Array => {
      const m = pb();
      m.vint(1, r.range(-100, 100)).vint(2, r.u64()).f64(3, r.double()).f32(4, r.double()).len(5, r.ascii(6)).len(6, r.bytes(3));
      m.vint(7, 1).fixed32(8, r.u32()).fixed64(9, r.u64()).len(10, pb().vint(1, r.u64()).len(2, "in"));
      m.packed(11, pb().raw(Pb.varint(r.u64())).raw(Pb.varint(5))).len(13, pb().vint(1, 1)).vint(14, r.u32());
      return m.finish();
    };
    for (let i = 0; i < 60; i++) {
      const b = base();
      switch (r.below(4)) {
        case 0:
          s.data("m.mal.Mal", b.slice(0, r.below(b.length)));
          break;
        case 1: {
          const c = Uint8Array.from(b);
          for (let k = 1 + r.below(3); k > 0; k--) c[r.below(c.length)] = r.below(256);
          s.data("m.mal.Mal", c);
          break;
        }
        case 2: {
          const c = Uint8Array.from(b);
          c[r.below(c.length)] ^= 1 << r.below(8);
          s.data("m.mal.Mal", c);
          break;
        }
        default:
          s.data("m.mal.Mal", r.bytes(r.below(24)));
      }
    }
  });

  add("missing", (s) => {
    s.data("m.none.Nope", new Uint8Array(0));
    s.data("m.none.Nope", hex("08 01"));
    s.schema(file({ name: "none.proto", package: "m.none", messageType: [{ name: "Other" }] }));
    s.data("m.none.Other", hex("08 01"));
  });

  let n = 0;
  for (const [id, run] of cases) {
    const session = new Session(`matrix/${id}`);
    run(session, new Rng(seed + n++, `proto/${id}`));
  }
}

// ---------------------------------------------------------------------------------------------
// Fuzz: seeded random descriptors and messages built from them, plus mutated bytes.

type FuzzField = { name: string; number: number; type: number; label: number; typeName?: string; map?: [number, number, string?] };
type FuzzType = { name: string; fields: FuzzField[] };

const FUZZ_NAMES = ["a", "b", "x", "y", "value", "ControlWord", "id", "a_b", "Name", "data", "kids", "m", "length", "constructor", "toString", "z9"];
const FUZZ_DEFAULTS = ["0", "1", "-1", "2.5", "true", "abc", "1e3", "AAEC", "inf", "B", "9007199254740993"];

function fuzzSchema(r: Rng, n: number): { file: Uint8Array; pkg: string; types: FuzzType[]; enums: string[] } {
  const pkg = r.pick(["fz", "fz.a", "", "fz.b.c", "mrc.proto"]);
  const prefix = pkg ? `.${pkg}.` : ".";
  const typeNames = Array.from({ length: 1 + r.below(4) }, (_, i) =>
    i === 0 && r.below(3) === 0 ? "ProtobufControlData" : r.pick([`T${i}`, `ProtobufT${i}`, `t_${i}`])
  );
  const enums = r.below(3) === 0 ? [] : ["En"];
  const types: FuzzType[] = [];
  const messageType: object[] = [];
  for (const tn of typeNames) {
    const fields: FuzzField[] = [];
    const descriptorFields: object[] = [];
    const nested: object[] = [];
    const oneofs = r.below(3) === 0 ? [{ name: r.below(12) === 0 ? "1" : r.pick(["o", "choice"]) }] : [];
    for (let i = r.below(9); i > 0; i--) {
      const name =
        r.below(40) === 0 ? r.pick(["0", "7", "1e2", "2x"]) : r.below(8) === 0 ? r.pick(FUZZ_NAMES) : `${r.pick(FUZZ_NAMES)}${i}`;
      const number = r.below(25) === 0 ? r.range(0, 3) : r.below(20) === 0 ? 1 + r.below(20) : i + 20 * r.below(2);
      const kind = r.below(10);
      let type: number;
      let typeName: string | undefined;
      let label = r.below(4) === 0 ? 3 : r.below(12) === 0 ? 2 : 1;
      let map: [number, number, string?] | undefined;
      if (kind < 6) type = r.pick(SCALARS)[1];
      else if (kind === 6 && enums.length) {
        type = T.ENUM;
        typeName = r.bool() ? `${prefix}En` : "En";
      } else if (kind <= 8) {
        type = T.MESSAGE;
        const target = r.pick(typeNames);
        typeName = r.below(4) === 0 ? target : `${prefix}${target}`;
      } else {
        const keyType = r.pick(SCALARS.filter(([, t]) => t !== T.DOUBLE && t !== T.FLOAT && t !== T.BYTES))[1];
        const valueType = r.bool() ? r.pick(SCALARS)[1] : T.MESSAGE;
        const entry = `E${i}Entry`;
        const vtn = valueType === T.MESSAGE ? `${prefix}${r.pick(typeNames)}` : undefined;
        nested.push({ name: entry, options: { mapEntry: true }, field: [f("key", 1, keyType), f("value", 2, valueType, vtn ? { typeName: vtn } : {})] });
        type = T.MESSAGE;
        typeName = entry;
        label = 3;
        map = [keyType, valueType, vtn];
      }
      const opts: FieldOpts = { label, ...(typeName ? { typeName } : {}) };
      if (oneofs.length && label === 1 && r.below(3) === 0) opts.oneofIndex = 0;
      if (label === 3 && r.below(3) === 0) opts.packed = r.bool();
      if (label !== 3 && type !== T.MESSAGE && r.below(6) === 0) opts.defaultValue = r.pick(FUZZ_DEFAULTS);
      descriptorFields.push(f(name, number, type, opts));
      fields.push({ name, number, type, label, typeName, map });
    }
    messageType.push({ name: tn, field: descriptorFields, oneofDecl: oneofs, nestedType: nested });
    types.push({ name: tn, fields });
  }
  const enumType = enums.map((name) => ({ name, value: [{ name: "A", number: r.pick([0, 1]) }, { name: "B", number: 2 }] }));
  return { file: file({ name: `fuzz${n}.proto`, package: pkg || undefined, syntax: r.pick(["proto3", "proto2", ""]), messageType, enumType }), pkg, types, enums };
}

function fuzzMessage(r: Rng, types: FuzzType[], t: FuzzType, depth: number): Pb {
  const m = pb();
  for (let k = r.below(7); k > 0; k--) {
    if (t.fields.length === 0 || r.below(10) === 0) {
      field(r, m, 1 + r.below(30), r.pick(SCALARS)[1]);
      continue;
    }
    const fd = r.pick(t.fields);
    if (fd.map) {
      const entry = pb();
      if (r.below(5)) field(r, entry, 1, fd.map[0]);
      if (r.below(5)) {
        if (fd.map[1] === T.MESSAGE) {
          const target = types.find((x) => fd.map![2]?.endsWith(`.${x.name}`));
          entry.len(2, target && depth < 3 ? fuzzMessage(r, types, target, depth + 1) : pb());
        } else field(r, entry, 2, fd.map[1]);
      }
      m.len(fd.number, entry);
    } else if (fd.type === T.MESSAGE) {
      const target = types.find((x) => fd.typeName?.endsWith(x.name));
      m.len(fd.number, target && depth < 3 ? fuzzMessage(r, types, target, depth + 1) : pb());
    } else if (r.below(12) === 0) {
      m.tag(fd.number, r.pick([0, 1, 2, 5])).raw(value(r, r.pick([T.INT32, T.DOUBLE, T.FLOAT, T.STRING])).finish());
    } else if (fd.label === 3 && WIRE[fd.type] !== 2 && r.bool()) {
      const items = pb();
      for (let n = r.below(4); n > 0; n--) items.raw(value(r, fd.type, true).finish());
      m.packed(fd.number, items);
    } else field(r, m, fd.number, fd.type, fd.label === 3);
  }
  return m;
}

function mutate(r: Rng, bytes: Uint8Array): Uint8Array {
  switch (r.below(8)) {
    case 0:
      return bytes.slice(0, r.below(bytes.length + 1));
    case 1: {
      const c = Uint8Array.from(bytes);
      if (c.length) c[r.below(c.length)] = r.below(256);
      return c;
    }
    case 2: {
      const c = Uint8Array.from(bytes);
      if (c.length) c[r.below(c.length)] ^= 1 << r.below(8);
      return c;
    }
    default:
      return bytes;
  }
}

function fuzz(seed: number) {
  const rng = new Rng(seed, "proto/fuzz");
  for (let n = 0; n < 120; n++) {
    const s = new Session(`fuzz/${n}`);
    const schemas = Array.from({ length: 1 + rng.below(2) }, (_, i) => fuzzSchema(rng, n * 10 + i));
    const names = schemas.flatMap((sc) => sc.types.map((t) => [sc, t] as const));
    if (rng.below(3) === 0) {
      const [sc, t] = rng.pick(names);
      s.data(sc.pkg ? `${sc.pkg}.${t.name}` : t.name, fuzzMessage(rng, sc.types, t, 0).finish());
    }
    for (const sc of schemas) s.schema(rng.below(10) === 0 ? mutate(rng, sc.file) : sc.file);
    for (let k = 0; k < 18; k++) {
      const [sc, t] = rng.pick(names);
      const name = rng.below(8) === 0 ? t.name : sc.pkg ? `${sc.pkg}.${t.name}` : t.name;
      s.data(name, mutate(rng, fuzzMessage(rng, sc.types, t, 0).finish()));
    }
    if (rng.below(4) === 0) s.restore(s.current());
  }
}

// ---------------------------------------------------------------------------------------------

function replay(seed: number) {
  const decoder8 = new TextDecoder();
  const corpus = new Corpus(seed);
  protoMatrix(corpus);
  for (const c of corpus.cases) {
    const opsFile = c.files.find((file) => file.path.endsWith(".ops.jsonl"));
    if (opsFile === undefined) continue;
    const session = new Session(c.id);
    for (const line of decoder8.decode(opsFile.bytes).split("\n")) {
      if (!line.trim()) continue;
      const op = JSON.parse(line);
      if (op.fn !== "putProto") continue;
      const bytes = hexToBytes(op.args[2].b);
      if (op.args[3] === "FileDescriptorProto") session.schema(bytes);
      else session.data(op.args[3], bytes);
    }
  }
}

const [mode, a1] = process.argv.slice(2);
if (mode === "replay") replay(Number(a1));
else if (mode === "matrix") matrix(Number(a1));
else if (mode === "fuzz") fuzz(Number(a1));
else throw new Error("usage: genProto replay <seed> | matrix <seed> | fuzz <seed>");
