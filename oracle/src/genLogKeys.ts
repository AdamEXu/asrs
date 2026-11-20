import "./shims";
import { ExtData } from "@msgpack/msgpack";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";
import { run } from "./logRun";
import {
  applyKeyPrefix,
  extractMergePrefix,
  filterFieldByPrefixes,
  getChildLogExtension,
  getSchemaType,
  getStructuredTypeFromRaw,
  isChildLog,
  isMergePrefix,
  isSchema,
  removeMergePrefix,
  splitLogKey
} from "@upstream/shared/log/LogKeyUtils";
import { getLogValueText, logValuesEqual } from "@upstream/shared/log/LogUtil";

// Emits ground truth for the L09 key/value utils.
//   genLogKeys pure <count> <seed>
//     fn records for every pure LogKeyUtils export plus LogUtil.getLogValueText and
//     LogUtil.logValuesEqual, over a curated edge matrix followed by seeded random cases.
//   genLogKeys run <count> <seed>
//     oracle:logRun scripts that exercise the Log-taking helpers: findKey, getOrDefault,
//     getURCLKeys, LogFieldIterator and getLogValueText over stored values. Keys that
//     collide with Object.prototype members are kept out of writes and log-key lookups
//     (the L03 note: upstream's `key in this.fields` sees inherited properties), but do
//     appear in findKey search lists and the pure-fn coverage.

const [mode, countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const tree = toTree;
const attempt = <T,>(f: () => T): { ok: T } | { throw: unknown } => {
  try {
    return { ok: f() };
  } catch (e) {
    return { throw: e };
  }
};
const result = (r: { ok: unknown } | { throw: unknown }) =>
  "ok" in r ? { ok: tree(r.ok) } : { throw: tree(r.throw) };
const fncall = (fn: string, f: (...a: any[]) => unknown, args: unknown[]) => {
  const r = attempt(() => f(...args));
  out({ fn, args: args.map((a) => tree(a)), ...result(r) });
};

const PROTO_KEYS = ["constructor", "__proto__", "toString", "hasOwnProperty", "valueOf", "prototype"];
const KEYS = [
  "",
  "/",
  "//",
  "a",
  "a/b",
  "/a/b/c",
  "a/",
  "a//b",
  ":",
  "a:b",
  "a:b/c:d",
  "_",
  "a_b",
  "Phoenix6",
  "Phoenix6:x_y",
  "/Phoenix6/Talon_1/Pos",
  "Phoenix6_Talon_1",
  "xPhoenix6/a_b",
  "Log",
  "Log0",
  "Log007",
  "Log1",
  "Log1a",
  "Log12x",
  "xLog1",
  "/Log1",
  "/Log12/a/b",
  "//Log1/x",
  "Log1/Log2/a",
  "/Log1/Phoenix6/a_b",
  "/.schema/struct:Pose2d",
  ".schema/photonstruct:Bar",
  "/a/.schema/proto:wpi.proto.ProtobufPose2d",
  "/a/.schema/",
  "/.schema",
  ".schema/",
  "/.schema/x\ny",
  "a.schema/x",
  "é/ü",
  "😀/a",
  "a/😀",
  "\ud800",
  "a\ud800b",
  ".type",
  "m/.type",
  "0",
  "10",
  "constructor",
  "__proto__/x",
  "toString",
  "NT:/SmartDashboard/v",
  "DS:/Dscomm/Console/ErrorInfo",
  "log:x"
];
const TYPE_PAIRS: (string | null | undefined)[][] = [
  [null, null],
  [undefined, undefined],
  ["log:wpilog", null],
  ["log:WPILOG", null],
  ["log:", null],
  ["log", null],
  ["xlog:x", null],
  [null, "log:rlog"],
  [null, "log:RLOG"],
  ["struct:x", "log:dslog"],
  ["log:a", "log:b"],
  ["log:wpilog", "struct:x"],
  ["", ""],
  ["log:é", null]
];
const RAW_TYPES = [
  "struct:Pose2d",
  "struct:Pose2d[]",
  "struct:",
  "struct:[]",
  "struct:struct:x",
  "struct:x[]y",
  "Struct:x",
  "photonstruct:Pose3d",
  "photonstruct:",
  "photonstruct:x[]",
  "proto:wpi.proto.ProtobufPose2d",
  "proto:mrc.proto.ProtobufControlData",
  "proto:Foo",
  "proto:",
  "proto:a.b.",
  "proto:Protobuf",
  "proto:wpi.proto.ProtobufProtobuf",
  "proto:x.Protobu",
  "PROTO:x",
  "msgpack",
  "MessagePack",
  "MSGPACK",
  "msgpackx",
  "json",
  "JSON",
  "Json",
  "jsonx",
  "log:wpilog",
  "log:WPILOG",
  "log:",
  "log:alo:gb",
  "xlog:x",
  "",
  "x",
  "protobuf:x",
  "raw"
];
const PREFIXES = [
  "",
  "/",
  "a",
  "a/b",
  "/a",
  "a,",
  ",a",
  "a,b",
  "A,B",
  "a,,b",
  ",",
  ",,",
  "NT",
  "nt:/x",
  "NT:a,b",
  ".schema",
  "Phoenix6",
  "Phoenix6:Talon_1",
  "Phoenix6/Talon",
  "phoenix6:talon_1",
  "/Log1",
  "Log12",
  "é",
  "É/Ü",
  "0",
  "x/"
];

const NUMBERS = [0, -0, 1, -1, 2, 255, 256.7, -255.7, 0.5, NaN, Infinity, -Infinity, 1e21, 5e-324, 2 ** 53, -(2 ** 31)];
const STRINGS = [
  "",
  "a",
  "ab",
  "1",
  "1,2",
  "x",
  "true",
  "null",
  '"q"',
  "a\\b",
  "a\nb",
  "é",
  "😀",
  "\ud800",
  "\ud800a",
  "x".repeat(60)
];

function bytes(n: number, f: (i: number) => number): Uint8Array {
  const b = new Uint8Array(n);
  for (let i = 0; i < n; i++) b[i] = f(i) & 0xff;
  return b;
}

const VALUES: unknown[] = [
  null,
  undefined,
  true,
  false,
  ...NUMBERS,
  ...STRINGS,
  bytes(0, () => 0),
  bytes(1, () => 0xab),
  bytes(3, (i) => i),
  bytes(499, (i) => i * 7 + 1),
  bytes(500, (i) => i * 7 + 1),
  bytes(501, (i) => i * 7 + 1),
  bytes(600, (i) => 255 - i),
  [],
  [1],
  [1, 2, 3],
  [true, "x", 2.5, null, undefined],
  [NaN, Infinity, -0],
  [[1, 2], ["a"], []],
  [new Uint8Array([1, 2])],
  [{ a: 1 }, {}],
  [new Date(0), new Date(NaN)],
  Array.from({ length: 500 }, (_, i) => i),
  Array.from({ length: 501 }, (_, i) => i),
  Array.from({ length: 501 }, () => "x"),
  Array.from({ length: 502 }, (_, i) => [i]),
  [12n],
  ["é", "😀", "\ud800"],
  {},
  { length: 0 },
  { length: 2 },
  { a: 1, b: [2, 3] },
  { constructor: 1, toString: 2, hasOwnProperty: 3 },
  JSON.parse('{"__proto__":3}'),
  { length: 2, 0: 9, 1: 8 },
  { byteLength: 2, 0: 1, 1: 2 },
  { byteLength: "2", 0: 1, 1: 2 },
  new Date(0),
  new Date(NaN),
  12n,
  new ExtData(-1, new Uint8Array([1, 2, 3]))
];

function randomValue(r: Rng, depth: number): unknown {
  switch (r.below(depth > 2 ? 8 : 12)) {
    case 0:
      return r.pick([null, undefined, true, false]);
    case 1:
      return r.pick(NUMBERS);
    case 2:
      return r.pick(STRINGS);
    case 3:
      return r.below(3) === 0 ? new Date(r.range(0, 1e12)) : r.pick(STRINGS);
    case 4:
      return bytes(r.below(12), () => r.below(256));
    case 5:
      return bytes(r.below(4) === 0 ? r.range(495, 505) : r.below(600), () => r.below(256));
    case 6:
      return { [r.pick(NAMES)]: randomValue(r, depth + 1), length: r.pick([0, r.below(4)]) };
    case 7:
      return new ExtData(r.range(-2, 5), bytes(r.below(6), () => r.below(256)));
    case 8:
      return r.below(6) === 0 ? BigInt(r.below(300)) : r.pick(NUMBERS);
    default: {
      const n = r.pick([0, 1, r.below(6), r.below(8) === 0 ? r.range(498, 503) : r.below(30)]);
      return Array.from({ length: n }, () => randomValue(r, depth + 1));
    }
  }
}

// No Object.prototype names here: `randomValue` objects can become JSON field payloads, and
// a generated "constructor" child makes upstream's getFieldTree read the inherited property
// and throw (the L03 note).
const NAMES = ["a", "b", "0", "1", "x", "length", "byteLength", ".type", "é"];

const FNS: [string, (...a: any[]) => unknown][] = [
  ["shared/log/LogKeyUtils:isSchema", isSchema],
  ["shared/log/LogKeyUtils:getSchemaType", getSchemaType],
  ["shared/log/LogKeyUtils:isChildLog", isChildLog],
  ["shared/log/LogKeyUtils:getChildLogExtension", getChildLogExtension],
  ["shared/log/LogKeyUtils:isMergePrefix", isMergePrefix],
  ["shared/log/LogKeyUtils:extractMergePrefix", extractMergePrefix],
  ["shared/log/LogKeyUtils:removeMergePrefix", removeMergePrefix],
  ["shared/log/LogKeyUtils:getStructuredTypeFromRaw", getStructuredTypeFromRaw],
  ["shared/log/LogKeyUtils:applyKeyPrefix", applyKeyPrefix],
  ["shared/log/LogKeyUtils:splitLogKey", splitLogKey],
  ["shared/log/LogKeyUtils:filterFieldByPrefixes", filterFieldByPrefixes],
  ["shared/log/LogUtil:getLogValueText", getLogValueText],
  ["shared/log/LogUtil:logValuesEqual", logValuesEqual]
];
const FN: { [name: string]: (...a: any[]) => unknown } = Object.fromEntries(FNS);
const call = (name: string, args: unknown[]) => fncall(name, FN[name], args);

function pureMatrix() {
  for (const key of KEYS) {
    call("shared/log/LogKeyUtils:isSchema", [key]);
    call("shared/log/LogKeyUtils:getSchemaType", [key]);
    call("shared/log/LogKeyUtils:isMergePrefix", [key]);
    call("shared/log/LogKeyUtils:extractMergePrefix", [key]);
    call("shared/log/LogKeyUtils:removeMergePrefix", [key]);
    call("shared/log/LogKeyUtils:splitLogKey", [key]);
  }
  for (const [w, s] of TYPE_PAIRS) {
    call("shared/log/LogKeyUtils:isChildLog", [w, s]);
    call("shared/log/LogKeyUtils:getChildLogExtension", [w, s]);
  }
  for (const t of RAW_TYPES) call("shared/log/LogKeyUtils:getStructuredTypeFromRaw", [t]);
  for (const p of ["", "/", "a", "/Log1", "Log12", "x/", "é", "constructor"]) {
    for (const k of ["", "/", "a", "/a", "//a", "é", "__proto__", "0"]) {
      call("shared/log/LogKeyUtils:applyKeyPrefix", [p, k]);
    }
  }
  for (const prefixes of PREFIXES) {
    for (const schemas of [false, true]) {
      for (const nt of [false, true]) {
        call("shared/log/LogKeyUtils:filterFieldByPrefixes", [KEYS, prefixes, schemas, nt]);
      }
    }
  }
  for (const type of [0, 1, 2, 3, 4, 5, 6, 7, 8, -1]) {
    for (const value of VALUES) {
      call("shared/log/LogUtil:getLogValueText", [value, type]);
    }
  }
  const pairs: [unknown, unknown][] = [
    [null, null],
    [undefined, undefined],
    [null, undefined],
    [true, true],
    [true, false],
    [NaN, NaN],
    [-0, 0],
    [2, 2],
    ["a", "a"],
    ["a", "b"],
    [[], []],
    [[1], [1]],
    [[1, 2], [1, 3]],
    [[1, 2], [1]],
    [[1, 2], "ab"],
    ["ab", [97, 98]],
    [5, 5],
    [5, 6],
    [[1], undefined],
    [undefined, [1]],
    [null, []],
    [bytes(2, () => 1), bytes(2, () => 1)],
    [bytes(2, () => 1), bytes(2, () => 2)],
    [bytes(2, () => 1), bytes(3, () => 1)],
    [bytes(0, () => 0), bytes(0, () => 0)],
    [bytes(2, () => 1), [1, 1]],
    [bytes(2, () => 1), { byteLength: 2, 0: 1, 1: 1 }],
    [bytes(2, () => 1), { byteLength: 2, 0: 1, 1: 5 }],
    [{ byteLength: 2, 0: 1, 1: 2 }, bytes(2, (i) => i + 1)],
    [{ byteLength: "2", 0: 1, 1: 2 }, bytes(2, (i) => i + 1)],
    [{ byteLength: 0 }, bytes(0, () => 0)],
    ["ab", "ab"],
    ["ab", "cd"],
    [[12n], [12n]],
    [[12n], [13n]],
    [undefined, null]
  ];
  for (const type of [0, 1, 2, 3, 4, 5, 6, 7]) {
    for (const [a, b] of pairs) {
      call("shared/log/LogUtil:logValuesEqual", [type, a, b]);
    }
  }
}

function pureRandom(r: Rng) {
  const key = () => (r.below(6) === 0 ? r.pick(PROTO_KEYS) : r.pick(KEYS.concat(r.pick(PROTO_KEYS) + "/" + r.pick(KEYS))));
  const anyArg = (): unknown =>
    r.below(8) === 0 ? r.pick([null, undefined, 0, 1.5, true, {}, [], [1], 2n]) : key();
  const choice = r.below(13);
  switch (choice) {
    case 0:
      call("shared/log/LogKeyUtils:isSchema", [anyArg()]);
      call("shared/log/LogKeyUtils:getSchemaType", [anyArg()]);
      break;
    case 1:
      call("shared/log/LogKeyUtils:isMergePrefix", [anyArg()]);
      call("shared/log/LogKeyUtils:extractMergePrefix", [anyArg()]);
      call("shared/log/LogKeyUtils:removeMergePrefix", [anyArg()]);
      break;
    case 2:
      call("shared/log/LogKeyUtils:isChildLog", [anyArg(), anyArg()]);
      call("shared/log/LogKeyUtils:getChildLogExtension", [anyArg(), anyArg()]);
      break;
    case 3:
      call("shared/log/LogKeyUtils:getStructuredTypeFromRaw", [
        r.below(4) === 0 ? anyArg() : r.pick(RAW_TYPES)
      ]);
      break;
    case 4:
      call("shared/log/LogKeyUtils:applyKeyPrefix", [
        r.below(5) === 0 ? anyArg() : r.pick(PREFIXES),
        anyArg()
      ]);
      break;
    case 5:
      call("shared/log/LogKeyUtils:splitLogKey", [anyArg()]);
      break;
    case 6:
      call("shared/log/LogKeyUtils:filterFieldByPrefixes", [
        Array.from({ length: r.below(8) }, key),
        r.below(5) === 0 ? anyArg() : r.pick(PREFIXES),
        r.bool(),
        r.bool()
      ]);
      break;
    case 7:
    case 8:
      call("shared/log/LogUtil:getLogValueText", [
        randomValue(r, 0),
        r.pick([0, 1, 2, 3, 4, 5, 6, 7, 8, -1])
      ]);
      break;
    default:
      call("shared/log/LogUtil:logValuesEqual", [
        r.pick([0, 1, 2, 3, 4, 5, 6, 7, 8, -1]),
        randomValue(r, 1),
        randomValue(r, 1)
      ]);
  }
}

// --- run mode ---

const RUN_KEYS = [
  "a",
  "/a/b",
  "RobotState",
  "NT:/RobotState",
  "/Log1/a",
  "/Log12/a/b",
  "Log3",
  "/Log3/RobotState",
  "Phoenix6/Talon_1/Pos",
  "/Phoenix6/x_y",
  "Drive/left",
  "/.schema/struct:Pose2d",
  ".schema/photonstruct:Bar",
  ".type",
  "m/.type",
  "0",
  "10",
  "é/ü",
  "😀/a",
  "",
  "/",
  "x"
];
const SEARCHES = [
  "RobotState",
  "a",
  "/a",
  "a/b",
  "/Log1/a",
  "Phoenix6",
  "",
  "constructor",
  "__proto__",
  "missing"
];
const URCL_TYPES = [null, "URCL", "URCLx", "urcl", "xURCL", "log:wpilog", "struct:Pose2d", ""];
const LOGGABLE = [0, 1, 2, 3, 4, 5, 6, 7];

function runScalar(r: Rng, kind: number): unknown {
  switch (kind) {
    case 0:
      return bytes(r.pick([0, 1, 3, 499, 500, 501, 600, r.below(20)]), () => r.below(256));
    case 1:
      return r.bool();
    case 2:
      return r.pick(NUMBERS);
    default:
      return r.pick(STRINGS);
  }
}

function runArray(r: Rng, kind: number): unknown[] {
  const n = r.pick([0, 1, 2, 5, r.below(15), r.below(10) === 0 ? r.range(498, 503) : r.below(30)]);
  return Array.from({ length: n }, () => runScalar(r, kind));
}

function runCase(r: Rng): [boolean, unknown[][]] {
  const enableCache = r.below(5) !== 0;
  const steps = r.range(15, 60);
  const ops: unknown[][] = [];
  const written: string[] = [];
  let cursor = r.range(-2, 5);
  const pickKey = () => (written.length > 0 && r.below(3) === 0 ? r.pick(written) : r.pick(RUN_KEYS));
  const timestamp = () => {
    if (r.below(30) === 0) return r.pick([NaN, Infinity, -Infinity, -0]);
    cursor += r.range(-3, 6) / 2;
    return cursor;
  };
  const searchList = () =>
    Array.from({ length: r.below(6) }, () => r.pick(SEARCHES.concat(written.length ? [r.pick(written)] : "z")));

  for (let i = 0; i < steps; i++) {
    const choice = r.below(100);
    if (choice < 30) {
      const kind = r.below(8);
      const k = pickKey();
      written.push(k);
      const t = timestamp();
      if (kind < 4) {
        ops.push([["putRaw", "putBoolean", "putNumber", "putString"][kind], k, t, runScalar(r, kind)]);
      } else if (kind < 7) {
        ops.push(
          [["putBooleanArray", "putNumberArray", "putStringArray"][kind - 4], k, t, runArray(r, kind - 3)]
        );
      } else {
        const text = attempt(() => JSON.stringify(randomValue(r, 1)));
        ops.push(["putJSON", k, t, "ok" in text && text.ok !== undefined ? text.ok : "null"]);
      }
    } else if (choice < 36) {
      ops.push(["findKey", searchList()]);
    } else if (choice < 45) {
      const type = r.pick(LOGGABLE.concat([8, -1]));
      ops.push([
        "getOrDefault",
        pickKey(),
        type,
        r.below(4) === 0 ? r.pick([NaN, Infinity, -Infinity, -0, cursor]) : r.range(-4, 20) / 2,
        randomValue(r, 1),
        r.below(3) === 0 ? undefined : r.pick(["u1", "u2", ""])
      ]);
    } else if (choice < 50) {
      const k = pickKey();
      ops.push(["setWpilibType", k, r.pick(URCL_TYPES) ?? "URCL2"]);
      written.push(k);
    } else if (choice < 55) {
      ops.push(["urclKeys"]);
    } else if (choice < 65) {
      const times: number[] = [];
      let t = r.range(-4, 4);
      const n = r.below(8);
      for (let j = 0; j < n; j++) {
        if (r.below(6) === 0) times.push(r.pick([NaN, Infinity, -Infinity, -0]));
        times.push(t);
        t += r.below(3) === 0 ? r.range(-2, 1) : r.range(0, 4) / 2;
      }
      ops.push(["iterator", pickKey(), times]);
    } else if (choice < 75) {
      ops.push(["valueText", pickKey()]);
    } else if (choice < 80) {
      ops.push(["query", pickKey()]);
    } else if (choice < 85) {
      ops.push(["setStructuredType", pickKey(), r.pick([null, "struct:Pose2d", "proto:x", "log:wpilog"])]);
    } else if (choice < 90) {
      ops.push(["fieldTree", r.bool(), r.pick(["", "/", "a", "/Log1", "Phoenix6"])]);
    } else if (choice < 94) {
      ops.push(["deleteField", pickKey()]);
    } else {
      ops.push(["keys"]);
    }
  }
  ops.push(["findKey", ["RobotState", "a", "constructor", "missing"]]);
  ops.push(["urclKeys"]);
  ops.push(["dump"]);
  return [enableCache, ops];
}

if (mode === "pure") {
  pureMatrix();
  for (let i = 0; i < count; i++) pureRandom(new Rng(seed, `keys/${i}`));
} else if (mode === "run") {
  for (let i = 0; i < count; i++) {
    const [enableCache, ops] = runCase(new Rng(seed, `keyrun/${i}`));
    const args = [tree(enableCache), tree(ops)];
    const result = attempt(() => run(enableCache, ops as any[][]));
    out({ fn: "oracle:logRun", args, ...("ok" in result ? { ok: tree(result.ok) } : { throw: tree(result.throw) }) });
  }
} else {
  throw new Error(`genLogKeys: unknown mode ${mode}`);
}
