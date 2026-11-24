import descriptor from "protobufjs/ext/descriptor";
import { Bytes, ByteSource, hex, toBytes } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";
import { LogSpec } from "./wpilog";

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
  MESSAGE: 11,
  BYTES: 12,
  UINT32: 13,
  ENUM: 14,
  SFIXED32: 15,
  SFIXED64: 16,
  SINT32: 17,
  SINT64: 18
};

type FieldOpts = {
  repeated?: boolean;
  typeName?: string;
  oneofIndex?: number;
  defaultValue?: string;
  packed?: boolean;
  required?: boolean;
};

function f(name: string, number: number, type: number, opts: FieldOpts = {}) {
  return {
    name,
    number,
    type,
    label: opts.repeated ? 3 : opts.required ? 2 : 1,
    ...(opts.typeName ? { typeName: opts.typeName } : {}),
    ...(opts.oneofIndex !== undefined ? { oneofIndex: opts.oneofIndex } : {}),
    ...(opts.defaultValue !== undefined ? { defaultValue: opts.defaultValue } : {}),
    ...(opts.packed !== undefined ? { options: { packed: opts.packed } } : {})
  };
}

function fileDescriptor(object: object): Uint8Array {
  const fdp = (descriptor as any).FileDescriptorProto;
  return Uint8Array.from(fdp.encode(fdp.fromObject(object)).finish());
}

const W = ".wpi.proto.";
const doubles = (...names: string[]) => names.map((n, i) => f(n, i + 1, T.DOUBLE));

const GEOMETRY2D = fileDescriptor({
  name: "geometry2d.proto",
  package: "wpi.proto",
  syntax: "proto3",
  messageType: [
    { name: "ProtobufTranslation2d", field: doubles("x", "y") },
    { name: "ProtobufRotation2d", field: doubles("value") },
    {
      name: "ProtobufPose2d",
      field: [
        f("translation", 1, T.MESSAGE, { typeName: `${W}ProtobufTranslation2d` }),
        f("rotation", 2, T.MESSAGE, { typeName: `${W}ProtobufRotation2d` })
      ]
    },
    {
      name: "ProtobufTransform2d",
      field: [
        f("translation", 1, T.MESSAGE, { typeName: `${W}ProtobufTranslation2d` }),
        f("rotation", 2, T.MESSAGE, { typeName: `${W}ProtobufRotation2d` })
      ]
    },
    { name: "ProtobufTwist2d", field: doubles("dx", "dy", "dtheta") }
  ]
});

const GEOMETRY3D = fileDescriptor({
  name: "geometry3d.proto",
  package: "wpi.proto",
  syntax: "proto3",
  messageType: [
    { name: "ProtobufTranslation3d", field: doubles("x", "y", "z") },
    { name: "ProtobufQuaternion", field: doubles("w", "x", "y", "z") },
    { name: "ProtobufRotation3d", field: [f("q", 1, T.MESSAGE, { typeName: `${W}ProtobufQuaternion` })] },
    {
      name: "ProtobufPose3d",
      field: [
        f("translation", 1, T.MESSAGE, { typeName: `${W}ProtobufTranslation3d` }),
        f("rotation", 2, T.MESSAGE, { typeName: `${W}ProtobufRotation3d` })
      ]
    }
  ]
});

const KINEMATICS = fileDescriptor({
  name: "kinematics.proto",
  package: "wpi.proto",
  dependency: ["geometry2d.proto"],
  syntax: "proto3",
  messageType: [
    { name: "ProtobufChassisSpeeds", field: doubles("vx", "vy", "omega") },
    {
      name: "ProtobufSwerveModuleState",
      field: [f("speed", 1, T.DOUBLE), f("angle", 2, T.MESSAGE, { typeName: `${W}ProtobufRotation2d` })]
    }
  ]
});

const CONTROL = fileDescriptor({
  name: "control.proto",
  package: "mrc.proto",
  syntax: "proto3",
  messageType: [
    {
      name: "ProtobufControlData",
      field: [f("ControlWord", 1, T.UINT32), f("MatchTime", 2, T.FLOAT), f("GameData", 3, T.STRING)]
    },
    {
      name: "ProtobufDsState",
      field: [
        f("control", 1, T.MESSAGE, { typeName: ".mrc.proto.ProtobufControlData" }),
        f("history", 2, T.MESSAGE, { typeName: ".mrc.proto.ProtobufControlData", repeated: true }),
        f("event", 3, T.STRING)
      ]
    }
  ]
});

const TEST = ".asrs.test.";
const EVERYTHING_FILE = {
  name: "asrs/everything.proto",
  package: "asrs.test",
  syntax: "proto3",
  enumType: [
    {
      name: "Color",
      value: [
        { name: "RED", number: 0 },
        { name: "GREEN", number: 1 },
        { name: "BLUE", number: 2 }
      ]
    }
  ],
  messageType: [
    { name: "Inner", field: [f("v", 1, T.DOUBLE), f("label", 2, T.STRING)] },
    {
      name: "Everything",
      oneofDecl: [{ name: "choice" }],
      nestedType: [
        { name: "MEntry", options: { mapEntry: true }, field: [f("key", 1, T.STRING), f("value", 2, T.DOUBLE)] }
      ],
      field: [
        f("i32", 1, T.INT32),
        f("i64", 2, T.INT64),
        f("u32", 3, T.UINT32),
        f("u64", 4, T.UINT64),
        f("s32", 5, T.SINT32),
        f("s64", 6, T.SINT64),
        f("f32", 7, T.FIXED32),
        f("f64", 8, T.FIXED64),
        f("sf32", 9, T.SFIXED32),
        f("sf64", 10, T.SFIXED64),
        f("fl", 11, T.FLOAT),
        f("db", 12, T.DOUBLE),
        f("b", 13, T.BOOL),
        f("s", 14, T.STRING),
        f("by", 15, T.BYTES),
        f("color", 16, T.ENUM, { typeName: `${TEST}Color` }),
        f("rd", 17, T.DOUBLE, { repeated: true }),
        f("ri64", 18, T.INT64, { repeated: true }),
        f("inners", 19, T.MESSAGE, { repeated: true, typeName: `${TEST}Inner` }),
        f("inner", 20, T.MESSAGE, { typeName: `${TEST}Inner` }),
        f("name", 21, T.STRING, { oneofIndex: 0 }),
        f("number", 22, T.INT32, { oneofIndex: 0 }),
        f("nested", 23, T.MESSAGE, { oneofIndex: 0, typeName: `${TEST}Inner` }),
        f("m", 24, T.MESSAGE, { repeated: true, typeName: `${TEST}Everything.MEntry` }),
        f("rs", 25, T.STRING, { repeated: true }),
        f("snake_case_name", 26, T.INT32),
        f("rb", 27, T.BYTES, { repeated: true }),
        f("colors", 28, T.ENUM, { repeated: true, typeName: `${TEST}Color` })
      ]
    }
  ]
};
const EVERYTHING = fileDescriptor(EVERYTHING_FILE);

const LEGACY = fileDescriptor({
  name: "asrs/legacy.proto",
  package: "asrs.legacy",
  syntax: "proto2",
  enumType: [
    {
      name: "Mode",
      value: [
        { name: "A", number: 1 },
        { name: "B", number: 2 }
      ]
    }
  ],
  messageType: [
    {
      name: "Legacy",
      field: [
        f("a", 1, T.INT32, { defaultValue: "7" }),
        f("r", 2, T.STRING, { required: true }),
        f("mode", 3, T.ENUM, { typeName: ".asrs.legacy.Mode", defaultValue: "B" }),
        f("packed", 4, T.INT32, { repeated: true, packed: true }),
        f("unpacked", 5, T.INT32, { repeated: true }),
        f("big", 6, T.INT64, { defaultValue: "9007199254740993" }),
        f("ratio", 7, T.DOUBLE, { defaultValue: "inf" })
      ]
    }
  ]
});

class Pb {
  private b = new Bytes();

  static varintBytes(value: bigint | number): number[] {
    let v = BigInt.asUintN(64, BigInt(value));
    const out: number[] = [];
    do {
      const byte = Number(v & 0x7fn);
      v >>= 7n;
      out.push(v > 0n ? byte | 0x80 : byte);
    } while (v > 0n);
    return out;
  }

  tag(no: number, wire: number) {
    this.b.raw(Pb.varintBytes((no << 3) | wire));
    return this;
  }
  vint(no: number, v: bigint | number) {
    this.tag(no, 0).b.raw(Pb.varintBytes(v));
    return this;
  }
  sint(no: number, v: bigint) {
    return this.vint(no, (v << 1n) ^ (v >> 63n));
  }
  f64(no: number, x: number) {
    this.tag(no, 1).b.f64(x);
    return this;
  }
  f32(no: number, x: number) {
    this.tag(no, 5).b.f32(x);
    return this;
  }
  fixed64(no: number, v: bigint) {
    this.tag(no, 1).b.le(v, 8);
    return this;
  }
  fixed32(no: number, v: number) {
    this.tag(no, 5).b.le(v, 4);
    return this;
  }
  len(no: number, data: ByteSource | Pb) {
    const bytes = data instanceof Pb ? data.finish() : toBytes(data);
    this.tag(no, 2).b.raw(Pb.varintBytes(bytes.length)).raw(bytes);
    return this;
  }
  packedDoubles(no: number, values: number[]) {
    const inner = new Bytes();
    values.forEach((v) => inner.f64(v));
    return this.len(no, inner.finish());
  }
  packedVarints(no: number, values: (bigint | number)[]) {
    return this.len(
      no,
      values.flatMap((v) => Pb.varintBytes(v))
    );
  }
  raw(bytes: ByteSource) {
    this.b.raw(bytes);
    return this;
  }
  finish(): Uint8Array {
    return this.b.finish();
  }
}

const pb = () => new Pb();

class ProtoCase {
  readonly spec = new LogSpec();
  private ids = new Map<string, number>();
  private t = 1000;

  descriptor(typeName: string, file: Uint8Array, key = `/.schema/proto:${typeName}`) {
    return this.put(key, "proto:FileDescriptorProto", file);
  }

  put(key: string, type: string, bytes: ByteSource) {
    if (!this.ids.has(key)) this.ids.set(key, this.spec.entry(key, type));
    this.t += 20000;
    this.spec.put(this.ids.get(key)!, this.t, bytes);
    return this;
  }

  data(key: string, typeName: string, msg: Pb | ByteSource) {
    return this.put(key, `proto:${typeName}`, msg instanceof Pb ? msg.finish() : msg);
  }
}

const vec = (r: Rng, n: number) => {
  const m = pb();
  for (let i = 1; i <= n; i++) if (r.below(5) !== 0) m.f64(i, r.double());
  return m;
};

function everything(r: Rng): Pb {
  const m = pb();
  const maybe = (fn: () => void) => r.below(4) !== 0 && fn();
  maybe(() => m.vint(1, BigInt(r.pick([0, 1, -1, 2 ** 31 - 1, -(2 ** 31), r.range(-1000, 1000)]))));
  maybe(() => m.vint(2, r.pick([0n, -1n, 2n ** 53n + 1n, 2n ** 63n - 1n, -(2n ** 63n), r.u64()])));
  maybe(() => m.vint(3, r.pick([0, 2 ** 32 - 1, r.u32()])));
  maybe(() => m.vint(4, r.pick([2n ** 64n - 1n, 2n ** 53n, r.u64()])));
  maybe(() => m.sint(5, BigInt(r.range(-5000, 5000))));
  maybe(() => m.sint(6, r.pick([-(2n ** 63n), 2n ** 63n - 1n, BigInt.asIntN(64, r.u64())])));
  maybe(() => m.fixed32(7, r.u32()));
  maybe(() => m.fixed64(8, r.u64()));
  maybe(() => m.fixed32(9, r.u32()));
  maybe(() => m.fixed64(10, r.u64()));
  maybe(() => m.f32(11, r.double()));
  maybe(() => m.f64(12, r.double()));
  maybe(() => m.vint(13, r.pick([0, 1, 2])));
  maybe(() => m.len(14, r.below(4) === 0 ? hex("c3 28 ef bb bf ed a0 80") : r.ascii(10)));
  maybe(() => m.len(15, r.bytes(r.below(6))));
  maybe(() => m.vint(16, r.pick([0, 1, 2, 7, -1])));
  maybe(() => (r.bool() ? m.packedDoubles(17, [r.double(), r.double()]) : m.f64(17, r.double()).f64(17, r.double())));
  maybe(() => (r.bool() ? m.packedVarints(18, [r.u64(), -1n]) : m.vint(18, r.u64())));
  for (let i = r.below(3); i > 0; i--) m.len(19, pb().f64(1, r.double()).len(2, r.ascii(4)));
  maybe(() => m.len(20, r.bool() ? pb() : pb().f64(1, r.double())));
  const choices = r.below(4);
  if (choices & 1) m.len(21, r.ascii(5));
  if (choices & 2) r.bool() ? m.vint(22, r.range(-9, 9)) : m.len(23, pb().len(2, "nested"));
  for (let i = r.below(4); i > 0; i--)
    m.len(
      24,
      pb()
        .len(1, r.pick(["k", "10", "2", "__proto__", "a/b"]))
        .f64(2, r.double())
    );
  for (let i = r.below(3); i > 0; i--) m.len(25, r.ascii(3));
  maybe(() => m.vint(26, r.range(0, 99)));
  for (let i = r.below(3); i > 0; i--) m.len(27, r.bytes(r.below(4)));
  maybe(() => m.packedVarints(28, [0, 2, 5]));
  if (r.below(6) === 0) m.vint(99, 12345).len(100, "unknown");
  return m;
}

export function protoMatrix(corpus: Corpus) {
  {
    const id = "payload/proto/wpilib";
    const r = corpus.rng(id);
    const c = new ProtoCase();
    ["ProtobufTranslation2d", "ProtobufRotation2d", "ProtobufPose2d", "ProtobufTwist2d"].forEach((t) =>
      c.descriptor(`wpi.proto.${t}`, GEOMETRY2D)
    );
    ["ProtobufQuaternion", "ProtobufPose3d"].forEach((t) => c.descriptor(`wpi.proto.${t}`, GEOMETRY3D));
    ["ProtobufChassisSpeeds", "ProtobufSwerveModuleState"].forEach((t) => c.descriptor(`wpi.proto.${t}`, KINEMATICS));
    for (let i = 0; i < 8; i++) {
      c.data("/Proto/Pose2d", "wpi.proto.ProtobufPose2d", pb().len(1, vec(r, 2)).len(2, vec(r, 1)));
      c.data(
        "/Proto/Pose3d",
        "wpi.proto.ProtobufPose3d",
        pb()
          .len(1, vec(r, 3))
          .len(2, pb().len(1, vec(r, 4)))
      );
      c.data("/Proto/Twist2d", "wpi.proto.ProtobufTwist2d", vec(r, 3));
      c.data("/Proto/Module", "wpi.proto.ProtobufSwerveModuleState", pb().f64(1, r.double()).len(2, vec(r, 1)));
      c.data("/Proto/Speeds", "wpi.proto.ProtobufChassisSpeeds", vec(r, 3));
    }
    c.data("/Proto/Pose2d", "wpi.proto.ProtobufPose2d", new Uint8Array(0));
    c.data("/Proto/Pose2d", "wpi.proto.ProtobufPose2d", pb().len(1, vec(r, 2)).vint(15, 1).len(1, pb().f64(2, -0)));
    corpus.spec(id, "WPILib geometry and kinematics protobufs; every type carries the shared file descriptor", c.spec);
  }
  {
    const id = "payload/proto/dependency-order";
    const r = corpus.rng(id);
    const c = new ProtoCase();
    c.data("/Proto/Early", "wpi.proto.ProtobufChassisSpeeds", vec(r, 3));
    c.descriptor("wpi.proto.ProtobufSwerveModuleState", KINEMATICS);
    c.data("/Proto/Module", "wpi.proto.ProtobufSwerveModuleState", pb().f64(1, 1.5).len(2, pb().f64(1, 0.25)));
    c.data("/Proto/Speeds", "wpi.proto.ProtobufChassisSpeeds", vec(r, 3));
    c.descriptor("wpi.proto.ProtobufRotation2d", GEOMETRY2D);
    c.data("/Proto/Module", "wpi.proto.ProtobufSwerveModuleState", pb().f64(1, 2.5).len(2, pb().f64(1, 0.5)));
    corpus.spec(id, "data before any descriptor, and a file whose dependency descriptor arrives later", c.spec);
  }
  {
    const id = "payload/proto/control-data";
    const c = new ProtoCase();
    c.descriptor("mrc.proto.ProtobufControlData", CONTROL).descriptor("mrc.proto.ProtobufDsState", CONTROL);
    const words = [
      0, 1, 0b11, 0b101, 0b111, 0b1001, 0b110001, 0x100, 0x500, 0x600, 0xf00, 0x2000, 0x2fff, 0xffffffff, 0x80000001
    ];
    words.forEach((w, i) =>
      c.data(
        "/Proto/Control",
        "mrc.proto.ProtobufControlData",
        pb()
          .vint(1, w)
          .f32(2, 15 - i)
          .len(3, i % 2 ? "LRC" : "")
      )
    );
    c.data("/Proto/Control", "mrc.proto.ProtobufControlData", pb().f32(2, 1));
    const state = pb()
      .len(1, pb().vint(1, 0x2133))
      .len(2, pb().vint(1, 0x0101))
      .len(2, pb().vint(1, 0x0302))
      .len(2, pb())
      .len(3, "match");
    c.data("/Proto/DsState", "mrc.proto.ProtobufDsState", state).data(
      "/Proto/DsState",
      "mrc.proto.ProtobufDsState",
      pb().len(3, "empty")
    );
    corpus.spec(
      id,
      "mrc.proto.ProtobufControlData words over every mode, station and flag, direct, nested and repeated",
      c.spec
    );
  }
  {
    const id = "payload/proto/everything";
    const r = corpus.rng(id);
    const c = new ProtoCase();
    c.descriptor("asrs.test.Everything", EVERYTHING);
    for (let i = 0; i < 40; i++) c.data("/Proto/Everything", "asrs.test.Everything", everything(r));
    c.data("/Proto/Everything", "asrs.test.Everything", new Uint8Array(0));
    c.data("/Proto/Inner", "asrs.test.Inner", pb().f64(1, NaN).len(2, "x"));
    corpus.spec(
      id,
      "every scalar wire type, int64 extremes, enums, bytes, packed and unpacked repeats, oneof, map, unknown fields",
      c.spec
    );
  }
  {
    const id = "payload/proto/proto2";
    const c = new ProtoCase();
    c.descriptor("asrs.legacy.Legacy", LEGACY);
    c.data("/Proto/Legacy", "asrs.legacy.Legacy", pb().len(2, "req"));
    c.data(
      "/Proto/Legacy",
      "asrs.legacy.Legacy",
      pb().vint(1, -5).len(2, "r").vint(3, 1).packedVarints(4, [1, 2]).vint(5, 3).vint(5, 4)
    );
    c.data("/Proto/Legacy", "asrs.legacy.Legacy", pb().vint(1, 1).vint(4, 9).packedVarints(5, [7, 8]));
    c.data("/Proto/Legacy", "asrs.legacy.Legacy", pb().len(2, "r").vint(3, 9));
    corpus.spec(
      id,
      "proto2 defaults (int, enum, int64 past 2^53, inf), a missing required field, packed/unpacked mixing",
      c.spec
    );
  }
  {
    const id = "payload/proto/malformed";
    const c = new ProtoCase();
    c.descriptor("asrs.test.Everything", EVERYTHING);
    const bad = [
      "08",
      "08 ff ff ff ff ff ff ff ff ff ff 01",
      "72 05 61 62",
      "0e 01",
      "0f",
      "0b 08 01 0c",
      "0b 08 01",
      "00 01",
      "61 00 00 00 00 00 00 f0",
      "a2 01 02 09",
      "7a ff ff ff ff 0f",
      "62 06 61 62 63 64 65 66"
    ];
    bad.forEach((h) => c.data("/Proto/Bad", "asrs.test.Everything", hex(h)));
    corpus.spec(
      id,
      "truncated varints and fixed fields, overlong lengths, invalid and group wire types, field 0, wrong wire types",
      c.spec
    );
  }
  {
    const id = "payload/proto/descriptors";
    const c = new ProtoCase();
    const renamed = fileDescriptor({
      ...EVERYTHING_FILE,
      messageType: [{ name: "Inner", field: [f("other", 1, T.INT32)] }]
    });
    c.descriptor("asrs.test.Inner", EVERYTHING);
    c.descriptor("asrs.test.Inner", renamed, "/.schema/proto:asrs.test.InnerAgain");
    c.data("/Proto/Inner", "asrs.test.Inner", pb().f64(1, 2).len(2, "kept first"));
    c.descriptor("anon", fileDescriptor({ messageType: [{ name: "Anon", field: [f("x", 1, T.INT32)] }] }));
    c.data("/Proto/Anon", "Anon", pb().vint(1, 3));
    c.descriptor("garbage", hex("0a ff 01"));
    c.descriptor("empty", new Uint8Array(0));
    c.data("/Proto/Missing", "asrs.test.Nope", pb().vint(1, 1));
    c.put("/NotASchema/Descriptor", "proto:FileDescriptorProto", CONTROL);
    c.data("/Proto/Control", "mrc.proto.ProtobufControlData", pb().vint(1, 1));
    c.put("/Proto/Prefixed", "proto:.asrs.test.Inner", pb().f64(1, 4).finish());
    corpus.spec(
      id,
      "duplicate file names, anonymous package, garbage and empty descriptors, unknown types, non-schema descriptors",
      c.spec
    );
  }
  {
    const id = "payload/proto/dup-descriptor";
    const c = new ProtoCase();
    const anon = fileDescriptor({ messageType: [{ name: "Anon", field: [f("x", 1, T.INT32)] }] });
    c.descriptor("anon", anon);
    c.descriptor("anon", anon);
    c.data("/Proto/Anon", "Anon", pb().vint(1, 3));
    corpus.spec(
      id,
      "a nameless descriptor recorded twice: the second add throws in the hub's fromSerialized",
      c.spec
    );
  }
}
