import "./shims";
import { ExtData, encode as msgpackEncode } from "@msgpack/msgpack";
import { fromTree, toTree } from "./cve1";
import { utf8 } from "./corpus/bytes";
import { Corpus } from "./corpus/corpus";
import { jsonMatrix } from "./corpus/json";
import { buildCorpusA } from "./corpus/matrices";
import { Rng } from "./corpus/rng";
import { WPILIB_STRUCTS, structSizes } from "./corpus/struct";
import { run } from "./logRun";
import { runScript, serveRealm } from "./logScript";

// Emits Log ground truth for asrs_log::log::Log as `oracle:logRun` fn records:
//   genLog <count> <seed> <corpusASeed>
//     {"fn":"oracle:logRun","args":[<enableTimestampSetCache>,[<op>...]],"ok":[<result>...]}
// The first <count> records are random scripts (see logRun.ts) over a key pool with
// integer-like keys, generated children, console and `.type` keys, merge and Phoenix prefixes;
// each ends with a dump. The rest replay corpus A's `.ops.jsonl` write sequences that need no
// struct, photon, proto or msgpack decoding, followed by a dump.
//
//   genLog writers <count> <seed> <corpusSeed>
//     {"fn":"oracle:logScript","args":[<realm random values>,[<op>...]],"ok":[<result>...]}
// Structured writers across a hub and a worker realm (see logScript.ts): the first <count>
// records are random scripts mixing struct, photonstruct, proto and msgpack writes with their
// schemas moved after the data, logs built in the worker realm, transferred and merged into the
// hub log under prefixes, and getUnit queries; then a few fixed scripts; then every corpus A
// `.ops.jsonl` write sequence in the worker realm, dumped there and again after a transfer to
// the hub realm.
//
// Keys and JSON property names that Object.prototype already defines ("constructor",
// "__proto__", ...) are left out of the random scripts: upstream's `key in this.fields` and
// field tree lookups see the inherited property and throw or pollute the prototype.

const BASE_KEYS = [
  "a",
  "b",
  "/a",
  "/a/b",
  "/a/b/c",
  "a/b",
  "0",
  "1",
  "2",
  "10",
  "01",
  "-1",
  "1.5",
  "4294967294",
  "4294967295",
  "x",
  "x/0",
  "x/1",
  "x/length",
  "/Log1/a",
  "/Log12/a/b",
  "Log3",
  "Phoenix6/Talon_1/Pos",
  "/Phoenix6/x_y",
  "/Log1/Phoenix6/a_b",
  "NT:/SmartDashboard/v",
  "DS:/Dscomm/Console/ConsoleLine",
  "DS:/Dscomm/Console/ErrorInfo",
  "/r/DS:/Dscomm/Console/ErrorInfo",
  "rDS:/Dscomm/Console/ConsoleLine",
  "q/DS:/Dscomm/Console/ProgramCrashInfo",
  "DS:/Dscomm/Console/Other",
  "m/.type",
  "/m/.type",
  ".type",
  "m",
  "/m",
  "j",
  "/j",
  "",
  "/",
  "//",
  "a/",
  "a//b",
  "é/ü",
  "\ud800",
  "\ud83d\ude00/a",
  "a/\ud83d\ude00",
  "/a/.schema/struct:Foo",
  ".schema/photonstruct:Bar"
];
const CHILDREN = ["0", "1", "2", "10", "length", "x", "y", "value", ".type", "", "a"];
const NAMES = ["a", "b", "0", "1", "2", "10", "01", "x/y", "", "é", "\ud83d\ude00", "length", ".type"];
const NUMBERS = [0, -0, 1, 2, 1.5, -1, 3, NaN, Infinity, -Infinity, 1e21, 5e-324];
const STRINGS = ["", "a", "b", "\ud800", "é", "Mechanism2d", "Console", "{}", "[1,2]"];
const TYPES = [null, "log:wpilog", "struct:Pose2d", "raw", "Log:x", "Console"];
const UUIDS = ["u1", "u2", "", "u3"];
const OFFSETS = [-100, -2, -1, 0, 1, 2, 100];
const PREFIXES = ["", "/", "a", "/a", "/a/", "x/", "Phoenix6", "/Log1", "m", "DS:"];
const GETTERS = [
  "getRange",
  "getRaw",
  "getBoolean",
  "getNumber",
  "getString",
  "getBooleanArray",
  "getNumberArray",
  "getStringArray"
];

function scalar(r: Rng, kind: number): unknown {
  switch (kind) {
    case 0:
      return new Uint8Array(r.pick([[], [1], [1, 2], [2], [0, 0, 0]]));
    case 1:
      return r.bool();
    case 2:
      return r.pick(NUMBERS);
    default:
      return r.pick(STRINGS);
  }
}

function array(r: Rng, kind: number): unknown[] {
  const length = r.pick([0, 1, 2, 2, 3, 5, 11]);
  const items = Array.from({ length }, () => scalar(r, kind));
  if (r.below(4) === 0 && length > 0) items.fill(items[0]);
  return items;
}

function unknown(r: Rng, depth: number): unknown {
  switch (r.below(depth > 2 ? 5 : 9)) {
    case 0:
      return r.below(4) === 0 ? r.pick([null, undefined, new Date(0), 12n]) : scalar(r, 0);
    case 1:
      return r.bool();
    case 2:
      return r.pick(NUMBERS);
    case 3:
      return r.pick(STRINGS);
    case 4:
      return r.below(3) === 0 ? new ExtData(r.range(-2, 5), new Uint8Array([r.below(4)])) : r.pick(NUMBERS);
    case 5:
    case 6: {
      const kind = r.below(5);
      return kind < 4 ? array(r, kind) : Array.from({ length: r.below(4) }, () => unknown(r, depth + 1));
    }
    default: {
      const object: { [id: string]: unknown } = {};
      for (let i = r.below(5); i > 0; i--) object[r.pick(NAMES)] = unknown(r, depth + 1);
      return object;
    }
  }
}

function jsonText(r: Rng): string {
  if (r.below(8) === 0) return r.pick(["", "{", "null", "1", '"s"', "[]", "{}", "nan"]);
  const value = unknown(r, 1);
  const text = JSON.stringify(value, (_k, v) =>
    v instanceof Uint8Array || v instanceof ExtData || typeof v === "bigint" ? [...String(v)] : v
  );
  return text ?? "null";
}

function makeCase(r: Rng): [boolean, unknown[][]] {
  const enableCache = r.below(5) !== 0;
  const steps = r.range(5, 50);
  const ops: unknown[][] = [];
  const keys = [...BASE_KEYS];
  let cursor = r.range(-2, 5);

  const key = (): string => {
    if (r.below(4) === 0) {
      const k = `${r.pick(keys)}/${r.pick(CHILDREN)}`;
      keys.push(k);
      return k;
    }
    return r.pick(keys);
  };
  const timestamp = (): number => {
    if (r.below(25) === 0) return r.pick([NaN, Infinity, -Infinity, -0]);
    switch (r.below(5)) {
      case 0:
      case 1:
        cursor += r.below(3);
        return cursor;
      case 2:
        return r.range(-4, 40) / 2;
      default:
        return cursor - r.below(4);
    }
  };
  const bound = (): number => {
    switch (r.below(5)) {
      case 0:
        return r.pick([-Infinity, Infinity, -0, 0, NaN]);
      case 1:
        return cursor + r.pick([-0.5, 0.5, 0]);
      default:
        return r.range(-4, 44) / 2;
    }
  };
  const written: string[] = [];
  const keyList = (): string[] =>
    Array.from({ length: r.range(0, 5) }, () => (written.length > 0 && r.below(3) !== 0 ? r.pick(written) : key()));
  const keyLists: string[][] = [];
  const reusedKeyList = (): string[] => {
    if (keyLists.length === 0 || r.below(4) === 0) keyLists.push(keyList());
    return r.pick(keyLists);
  };

  for (let i = 0; i < steps; i++) {
    const choice = r.below(100);
    if (choice < 40) {
      const kind = r.below(8);
      const k = written.length > 0 && r.below(3) === 0 ? r.pick(written) : key();
      written.push(k);
      const t = timestamp();
      switch (kind) {
        case 0:
        case 1:
        case 2:
        case 3:
          ops.push([["putRaw", "putBoolean", "putNumber", "putString"][kind], k, t, scalar(r, kind)]);
          break;
        case 4: {
          const which = r.below(3);
          ops.push([["putBooleanArray", "putNumberArray", "putStringArray"][which], k, t, array(r, which + 1)]);
          break;
        }
        case 5:
          ops.push(["putJSON", k, t, jsonText(r)]);
          break;
        case 6:
          ops.push(["putUnknownStruct", k, t, unknown(r, 0), r.bool()]);
          break;
        default:
          if (r.bool()) {
            ops.push(["putPose", k, t, [r.pick(NUMBERS), r.pick(NUMBERS)], r.pick(NUMBERS)]);
          } else {
            const n = r.below(4);
            ops.push(["putTranslationArray", k, t, Array.from({ length: n }, () => [r.pick(NUMBERS), r.pick(NUMBERS)])]);
          }
      }
    } else if (choice < 52) {
      const uuid = r.below(3) === 0 ? undefined : r.pick(UUIDS);
      const offset = r.below(2) === 0 ? undefined : r.pick(OFFSETS);
      const start = bound();
      const end = r.below(3) === 0 ? start : bound();
      ops.push(["get", r.pick(GETTERS), key(), start, end, uuid, offset]);
    } else if (choice < 64) {
      ops.push(["getTimestamps", reusedKeyList(), r.below(5) === 0 ? null : r.pick(UUIDS)]);
    } else if (choice < 67) {
      ops.push(["getChangedFields"]);
    } else if (choice < 71) {
      ops.push(["query", key()]);
    } else if (choice < 73) {
      ops.push(["keys"]);
    } else if (choice < 76) {
      ops.push(["fieldTree", r.bool(), r.pick(PREFIXES)]);
    } else if (choice < 78) {
      ops.push(["range"]);
    } else if (choice < 79) {
      ops.push(["displayOffset", r.pick([null, "start-zero", "original"])]);
    } else if (choice < 82) {
      ops.push(["clearBeforeTime", bound(), r.below(4) !== 0]);
    } else if (choice < 83) {
      ops.push(["updateRangeWithTimestamp", timestamp()]);
    } else if (choice < 86) {
      ops.push(["createBlankField", key(), r.below(8)]);
    } else if (choice < 88) {
      const listed = keyLists.flat();
      ops.push(["deleteField", listed.length > 0 && r.bool() ? r.pick(listed) : key()]);
    } else if (choice < 90) {
      const type = r.below(8);
      const n = r.below(4);
      let t = r.range(-2, 4);
      const timestamps = Array.from({ length: n }, () => (t += r.range(1, 3)));
      if (n > 0 && r.below(4) === 0) timestamps[0] = r.pick([-0, 0, NaN]);
      const values = Array.from({ length: n }, () =>
        type === 7 ? undefined : type < 4 ? scalar(r, type) : array(r, type - 3)
      );
      ops.push([
        "setField",
        key(),
        {
          type,
          timestamps,
          values,
          structuredType: r.pick(TYPES),
          wpilibType: r.pick(TYPES),
          metadataString: r.pick(["", "{}"]),
          stripingReference: r.bool(),
          typeWarning: r.bool()
        }
      ]);
    } else if (choice < 93) {
      const which = r.below(4);
      const k = key();
      if (which === 0) ops.push(["setStructuredType", k, r.pick(TYPES)]);
      else if (which === 1) ops.push(["setWpilibType", k, r.pick(TYPES) ?? "int64"]);
      else if (which === 2) ops.push(["setMetadataString", k, r.pick(["", "{}", "unit"])]);
      else ops.push(["setGeneratedParent", k]);
    } else if (choice < 97) {
      ops.push(["reload"]);
    } else {
      ops.push(["dump"]);
    }
  }
  ops.push(["getChangedFields"]);
  ops.push(["dump"]);
  return [enableCache, ops];
}

function emit(enableCache: boolean, ops: unknown[][]) {
  const args = [toTree(enableCache), toTree(ops)];
  const result = run(enableCache, ops as any[][]);
  process.stdout.write(JSON.stringify({ fn: "oracle:logRun", args, ok: toTree(result) }) + "\n");
}

function core(count: number, seed: number, corpusSeed: number) {
  for (let i = 0; i < count; i++) {
    const [enableCache, ops] = makeCase(new Rng(seed, `log/${i}`));
    emit(enableCache, ops);
  }

  emit(true, [
    ["putNumber", "a", 0, 1],
    ["putNumber", "b", -0, 2],
    ["putNumber", "c", NaN, 3],
    ["putNumber", "c", NaN, 4],
    ["putNumber", "d", 1, 5],
    ["getTimestamps", ["a", "b", "c", "d"], null],
    ["getTimestamps", ["b", "a"], "u1"],
    ["putNumber", "a", -0, 1],
    ["putNumber", "b", 2, 2],
    ["getTimestamps", ["b", "a"], "u1"],
    ["getTimestamps", ["b", "a"], "u1"],
    ["deleteField", "a"],
    ["getTimestamps", ["b", "a"], "u1"],
    ["clearBeforeTime", NaN, true],
    ["range"],
    ["getTimestamps", ["d", "c"], ""],
    ["dump"]
  ]);

  emit(true, [
    ["putNumber", "n1", 1, 1],
    ["putNumber", "n2", 2, 2],
    ["putNumber", "n3", 4, 3],
    ["getTimestamps", ["n1", "n2"], ""],
    ["getTimestamps", ["n1", "n2"], "u1"],
    ["getTimestamps", ["n1", "n2", "n3"], "u2"],
    ["putNumber", "n1", 3, 1],
    ["getTimestamps", ["n1", "n2"], ""],
    ["getTimestamps", ["n1", "n2"], "u1"],
    ["deleteField", "n3"],
    ["getTimestamps", ["n1", "n2", "n3"], "u2"],
    ["dump"]
  ]);

  const SUPPORTED = new Set(["putRaw", "putJSON"]);
  const corpus = new Corpus(corpusSeed);
  jsonMatrix(corpus);
  for (const c of [...corpus.cases].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))) {
    const file = c.files.find((f) => f.path.endsWith(".ops.jsonl"));
    if (file === undefined) continue;
    const lines = new TextDecoder().decode(file.bytes).split("\n").filter((l) => l.length > 0);
    const calls = lines.map((l) => {
      const call = JSON.parse(l) as { fn: string; args: unknown[] };
      return { fn: call.fn, args: call.args.map(fromTree) };
    });
    if (!calls.every((call) => SUPPORTED.has(call.fn))) continue;
    emit(true, [...calls.map((call) => [call.fn, ...call.args]), ["getChangedFields"], ["dump"]]);
  }
}

type Op = any[];

function corpusOps(c: { files: { path: string; bytes: Uint8Array }[] }): Op[] | null {
  const file = c.files.find((f) => f.path.endsWith(".ops.jsonl"));
  if (file === undefined) return null;
  const lines = new TextDecoder().decode(file.bytes).split("\n").filter((l) => l.length > 0);
  return lines.map((l) => {
    const call = JSON.parse(l) as { fn: string; args: unknown[] };
    return [call.fn, ...call.args.map(fromTree)];
  });
}

function isSchemaOp(op: Op): boolean {
  return (
    (op[0] === "putRaw" && /(?:^|\/)\.schema\//.test(op[1])) ||
    (op[0] === "putProto" && op[4] === "FileDescriptorProto")
  );
}

const STRUCT_SCHEMAS: [string, string][] = [
  ...WPILIB_STRUCTS,
  ["Mixed", "int8 a;bool b;enum {x=1,y=2} uint8 e;int32 bits:4;uint32 more:3;float f;char name[3]"],
  ["Nested", "Mixed m[2];Pose2d pose;double DistanceMeters"],
  ["Empty", ""]
];
const STRUCT_SIZE = structSizes(STRUCT_SCHEMAS.filter(([name]) => name !== "Empty"));
const BIG_GEOMETRY = ["Translation2d", "Pose2d", "Transform2d", "Translation3d", "Pose3d", "Transform3d"];
const SCHEMA_KEY_PREFIXES = ["/.schema/", ".schema/", "NT:/.schema/", "/Robot/.schema/"];
const DATA_KEYS = ["/Pose", "Pose", "/Robot/Pose", "/Arr", "NT:/Vision/Result", "/Mixed", "0", "1", "/m", "/a/b", "/Odometry/Robot"];
const UNIT_KEYS = [
  "DriveDistanceMeters",
  "arm/AngleDegrees",
  "/Shooter/speedRPM",
  "Voltage",
  "volts",
  "motor_temp_celsius",
  "x",
  "élanMeters",
  "\ud800Meters",
  "Flywheel/VelocityRadPerSec",
  "Battery Volts",
  "/Robot/Pose/length"
];
const UNIT_METADATA = [
  "",
  "{}",
  '{"unit":"meters"}',
  '{"units":"Degrees"}',
  '{"Unit":"rpm","units":"volts"}',
  '{"unit":5}',
  '{"unit":"meters","Units":null}',
  '{"unit":"meters","Units":"seconds"}',
  "null",
  "[1]",
  '"meters"',
  '{"unit":""}',
  '{"unit":"METERS PER SECOND"}',
  '{"unit":"İnches"}',
  '{"unit":"furlongs"}',
  "unit: degrees",
  "Unit=Volts\nfoo",
  "foo\nunits meters per second",
  "unitless",
  "UNIT ms\nunit: rotations",
  '{"units":"m/s"}',
  '{"unit":"meters","Units":5}',
  '{"Units":"volts","unit":[]}'
];
const ARRAY_UNIT_KEYS = ["WheelPositionsRadians", "/Drive/VelocitiesMetersPerSec", "Currents"];
const MERGE_PREFIXES = ["", "/Log1", "Log2", "/Replay", "/", "NT:", "/a/", "/Log1/Phoenix6"];

function timestampSource(r: Rng): () => number {
  let cursor = r.range(0, 4);
  return () => {
    switch (r.below(6)) {
      case 0:
        return cursor;
      case 1:
        return cursor - r.below(3);
      default:
        cursor += r.range(1, 3) / 2;
        return cursor;
    }
  };
}

function structPayload(r: Rng, name: string, isArray: boolean): Uint8Array {
  const size = STRUCT_SIZE.get(name) ?? 8;
  if (BIG_GEOMETRY.includes(name) && isArray && r.below(4) === 0) {
    return r.bytes(r.pick([5000, 5001, size * 313, size * 312]));
  }
  if (r.below(8) === 0) return r.bytes(r.pick([0, 1, size - 1, size + 3]));
  return r.bytes(isArray ? size * r.range(0, 3) : size);
}

function structBlock(r: Rng, t: () => number): Op[] {
  const ops: Op[] = [];
  const names = Array.from({ length: r.range(1, 4) }, () => r.pick(STRUCT_SCHEMAS)[0]);
  for (const name of names) {
    const isArray = r.below(3) === 0;
    if (name === "Empty" && isArray) continue;
    ops.push(["putStruct", r.pick(DATA_KEYS), t(), structPayload(r, name, isArray), name, isArray]);
  }
  for (const [name, text] of STRUCT_SCHEMAS) {
    if (r.below(3) !== 0) ops.push(["putRaw", r.pick(SCHEMA_KEY_PREFIXES) + "struct:" + name, t(), utf8(text)]);
  }
  if (r.below(4) === 0) {
    ops.push(["putNumber", r.pick(DATA_KEYS), t(), 1]);
  }
  return ops;
}

function sameLengthKeys(r: Rng, length: number, n: number): string[] {
  const keys = new Set<string>();
  while (keys.size < n) {
    let k = "";
    for (let i = 0; i < length; i++) k += String.fromCharCode(r.range(0x61, 0x7a));
    keys.add(k);
  }
  return [...keys];
}

function msgpackBlock(r: Rng, t: () => number, corpusPayloads: Uint8Array[]): Op[] {
  const ops: Op[] = [];
  for (let i = r.range(1, 4); i > 0; i--) {
    const key = r.pick(["/mp", "mp", "/Robot/State", "/m"]);
    switch (r.below(5)) {
      case 0: {
        const object: { [k: string]: unknown } = {};
        for (const k of sameLengthKeys(r, r.range(2, 3), r.range(14, 24))) object[k] = r.below(100);
        ops.push(["putMsgpack", key, t(), msgpackEncode(object)]);
        break;
      }
      case 1: {
        const [name, text] = r.pick(STRUCT_SCHEMAS);
        const value = { ".schema": { ["struct:" + name]: utf8(text) }, n: r.below(9), raw: r.bytes(2) };
        ops.push(["putMsgpack", key, t(), msgpackEncode(value)]);
        break;
      }
      case 2:
        ops.push(["putMsgpack", key, t(), r.pick([new Uint8Array([0xc0]), new Uint8Array([0xc1]), r.bytes(3), new Uint8Array([])])]);
        break;
      case 3: {
        // {"\xc3": n}: utf8DecodeJs reads the value byte as the key's continuation byte, so the
        // cached key string depends on the payload that first cached it, until a full bucket
        // evicts it at a Math.random index.
        const malformed = () => Uint8Array.from([0x81, 0xa1, 0xc3, r.below(64)]);
        const letters = shuffle(r, Array.from({ length: 26 }, (_, i) => 0x61 + i)).slice(0, r.range(15, 26));
        const filler = [0xde, 0, letters.length, ...letters.flatMap((k) => [0xa1, k, 0])];
        ops.push(["putMsgpack", key, t(), malformed()]);
        ops.push(["putMsgpack", key, t(), Uint8Array.from(filler)]);
        ops.push(["putMsgpack", key, t(), malformed()]);
        break;
      }
      default:
        if (corpusPayloads.length > 0) ops.push(["putMsgpack", key, t(), r.pick(corpusPayloads)]);
    }
  }
  return ops;
}

function pbField(field: number, wire: number, payload: number[]): number[] {
  return wire === 2 ? [(field << 3) | 2, payload.length, ...payload] : [(field << 3) | wire, ...payload];
}

function pbText(field: number, text: string): number[] {
  return pbField(field, 2, [...utf8(text)]);
}

/** A FileDescriptorProto whose messages have only `double` fields. */
function protoFile(file: string, pkg: string, messages: [string, string[]][]): Uint8Array {
  const message = ([name, fields]: [string, string[]]) =>
    pbField(4, 2, [
      ...pbText(1, name),
      ...fields.flatMap((field, i) =>
        pbField(2, 2, [...pbText(1, field), ...pbField(3, 0, [i + 1]), ...pbField(4, 0, [1]), ...pbField(5, 0, [1])])
      )
    ]);
  return Uint8Array.from([...pbText(1, file), ...pbText(2, pkg), ...messages.flatMap(message)]);
}

const PROTO_FILES = [
  protoFile("l08/point.proto", "l08", [["ProtobufPoint", ["x", "y"]]]),
  protoFile("l08/speed.proto", "l08", [["Speed", ["VelocityMetersPerSec", "omega"]]]),
  protoFile("l08/dup.proto", "l08", [["ProtobufPoint", ["z"]]]),
  protoFile("l08/point.proto", "l08", [["Other", ["w"]]])
];

function protoBlock(r: Rng, t: () => number): Op[] {
  const ops: Op[] = [];
  for (let i = r.range(1, 4); i > 0; i--) {
    const type = r.pick(["l08.ProtobufPoint", "l08.Speed", "l08.Missing", "ProtobufPoint"]);
    const doubles = Array.from({ length: r.range(0, 2) }, (_, k) => {
      const value = new Uint8Array(8);
      new DataView(value.buffer).setFloat64(0, r.pick([1.5, -0, NaN, 1e300, r.range(-9, 9)]), true);
      return [((k + 1) << 3) | 1, ...value];
    });
    ops.push(["putProto", r.pick(["/Proto", "/Robot/Point", "Speeds"]), t(), Uint8Array.from(doubles.flat()), type]);
  }
  for (const file of PROTO_FILES) {
    if (r.below(3) !== 0) ops.push(["putProto", r.pick(["/.schema/proto:l08", "NT:/.schema/proto:x"]), t(), file, "FileDescriptorProto"]);
  }
  return ops;
}

function unitBlock(r: Rng, t: () => number): Op[] {
  const ops: Op[] = [];
  const keys = Array.from({ length: r.range(2, 6) }, () => r.pick(UNIT_KEYS));
  for (const key of keys) {
    ops.push(["putNumber", key, t(), r.below(10)]);
    if (r.bool()) ops.push(["getUnit", key]);
    if (r.bool()) ops.push(["setMetadataString", key, r.pick(UNIT_METADATA)]);
    if (r.bool()) ops.push(["getUnit", key]);
  }
  if (r.bool()) {
    const key = r.pick(ARRAY_UNIT_KEYS);
    ops.push(["putNumberArray", key, t(), [1, 2]]);
    ops.push(["getUnit", key + "/0"], ["getUnit", key + "/length"]);
    if (r.bool()) ops.push(["setStructuredType", key, r.pick(["Translation2d", "ChassisSpeeds", null])]);
    ops.push(["getUnit", key + "/1"]);
  }
  if (r.bool()) {
    const key = r.pick(["/Odometry/Robot", "Pose", "/Speeds"]);
    const name = r.pick(["Pose2d", "Pose3d", "ChassisSpeeds", "Translation2d"]);
    ops.push(["putStruct", key, t(), structPayload(r, name, false), name, false]);
    const child = r.pick(["/translation/x", "/rotation/value", "/vx", "/omega", "/x", "/translation/z", ""]);
    ops.push(["putNumber", key + child, t(), 3], ["getUnit", key + child]);
  }
  if (r.below(3) === 0) {
    const key = r.pick(keys);
    ops.push([r.pick(["deleteField", "setMetadataString"]), key, r.pick(UNIT_METADATA)], ["getUnit", key]);
  }
  return ops;
}

function shuffle<T>(r: Rng, items: T[]): T[] {
  for (let i = items.length - 1; i > 0; i--) {
    const j = r.below(i + 1);
    [items[i], items[j]] = [items[j], items[i]];
  }
  return items;
}

function reorder(r: Rng, items: Op[]): Op[] {
  switch (r.below(4)) {
    case 0:
      return items;
    case 1:
      return [...items.filter((op) => !isSchemaOp(op)), ...items.filter(isSchemaOp)];
    case 2:
      return [...items.filter((op) => !isSchemaOp(op)), ...items.filter(isSchemaOp).reverse()];
    default:
      return shuffle(r, items);
  }
}

function writersCase(r: Rng, corpusCases: Op[][], msgpackPayloads: Uint8Array[]): Op[] {
  const t = timestampSource(r);
  let items: Op[] = [];
  for (let k = r.range(1, 3); k > 0; k--) {
    switch (r.below(6)) {
      case 0:
      case 1:
        items.push(...r.pick(corpusCases));
        break;
      case 5:
        items.push(...protoBlock(r, t));
        break;
      case 2:
        items.push(...structBlock(r, t));
        break;
      case 3:
        items.push(...msgpackBlock(r, t, msgpackPayloads));
        break;
      default:
        items.push(...unitBlock(r, t));
    }
  }
  items = reorder(r, items);

  const ops: Op[] = [
    ["new", "h", "hub", r.below(4) !== 0],
    ["new", "w", "worker", true]
  ];
  const workerShare = r.below(4);
  const merges = new Set(Array.from({ length: r.below(3) }, () => r.below(items.length + 1)));
  const queryKeys = [...DATA_KEYS, ...UNIT_KEYS];
  let generation = 0;
  const mergeWorker = () => {
    ops.push(["transfer", "w", "s", "hub"]);
    if (r.below(6) === 0) ops.push(["op", "s", "dumpAll"]);
    ops.push(["merge", "h", "s", r.pick(MERGE_PREFIXES)]);
    ops.push(["new", "w", "worker", r.below(5) !== 0]);
    generation++;
  };
  const workerMerge = r.below(4) === 0 ? r.below(items.length + 1) : -1;
  let side = false;
  items.forEach((item, i) => {
    if (merges.has(i)) mergeWorker();
    if (i === workerMerge) {
      ops.push(["new", "w2", "worker", r.bool()]);
      side = true;
    }
    if (side && r.below(6) === 0) {
      ops.push(["merge", "w", "w2", r.pick(MERGE_PREFIXES)]);
      side = false;
    }
    let target = r.below(4) < workerShare ? "w" : "h";
    if (side && r.bool()) target = "w2";
    ops.push(["op", target, ...item]);
    const k = typeof item[1] === "string" ? item[1] : r.pick(queryKeys);
    switch (r.below(30)) {
      case 0:
        ops.push(["op", target, "getUnit", k]);
        break;
      case 1:
        ops.push(["op", target, "query", k]);
        break;
      case 2:
        ops.push(["op", target, "reload"]);
        break;
      case 3:
        ops.push(["op", target, "dumpAll"]);
        break;
    }
  });
  if (side) ops.push(["merge", "w", "w2", r.pick(MERGE_PREFIXES)]);
  if (generation === 0 || r.bool()) mergeWorker();
  ops.push(["op", "h", "getChangedFields"], ["op", "h", "units"], ["op", "h", "dumpAll"]);
  return ops;
}

const FIXED_SCRIPTS: Op[][] = [
  [
    ["new", "h", "hub", true],
    ...UNIT_METADATA.flatMap((metadata, i) => [
      ["op", "h", "putNumber", `/u${i}`, 1, i],
      ["op", "h", "setMetadataString", `/u${i}`, metadata],
      ["op", "h", "putNumber", `/u${i}Inches`, 1, i],
      ["op", "h", "setMetadataString", `/u${i}Inches`, metadata]
    ]),
    ["op", "h", "units"]
  ],
  [
    ["new", "h", "hub", true],
    ["new", "w", "worker", true],
    ["op", "h", "putNumber", "/Arm/Angle", 1, 1],
    ["op", "h", "putNumber", "/Pose/x", 1, 1],
    ["op", "h", "getUnit", "/Arm/Angle"],
    ["op", "h", "getUnit", "/Pose/x"],
    ["op", "h", "putRaw", "/.schema/struct:Translation2d", 1, utf8("double x;double y")],
    ["op", "h", "putStruct", "/Pose", 2, new Uint8Array(16), "Translation2d", false],
    ["op", "h", "getUnit", "/Pose/x"],
    ["op", "w", "putNumber", "/Arm/Angle", 1, 2],
    ["op", "w", "setMetadataString", "/Arm/Angle", '{"unit":"degrees"}'],
    ["op", "w", "getUnit", "/Arm/Angle"],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", ""],
    ["op", "h", "getUnit", "/Arm/Angle"],
    ["op", "h", "setMetadataString", "/Arm/Angle", '{"unit":"degrees"}'],
    ["op", "h", "getUnit", "/Arm/Angle"],
    ["op", "h", "deleteField", "/Pose/x"],
    ["op", "h", "putNumber", "/Pose/x", 3, 1],
    ["op", "h", "units"]
  ],
  [
    ["new", "h", "hub", true],
    ["new", "w", "worker", true],
    ["op", "w", "putProto", "/P", 1, Uint8Array.from([9, 0, 0, 0, 0, 0, 0, 248, 63]), "l08.ProtobufPoint"],
    ["op", "w", "putProto", "/.schema/proto:a", 1, PROTO_FILES[0], "FileDescriptorProto"],
    ["op", "w", "putProto", "/.schema/proto:b", 1, PROTO_FILES[2], "FileDescriptorProto"],
    ["op", "w", "putProto", "/.schema/proto:c", 1, PROTO_FILES[3], "FileDescriptorProto"],
    ["op", "w", "dumpAll"],
    ["op", "h", "putProto", "/.schema/proto:s", 1, PROTO_FILES[1], "FileDescriptorProto"],
    ["op", "h", "putProto", "/S", 1, Uint8Array.from([9, 0, 0, 0, 0, 0, 0, 248, 63]), "l08.Speed"],
    ["transfer", "w", "s", "hub"],
    ["op", "s", "dumpAll"],
    ["new", "w", "worker", true],
    ["op", "w", "putProto", "/.schema/proto:b", 1, PROTO_FILES[2], "FileDescriptorProto"],
    ["op", "w", "putProto", "/Q", 2, Uint8Array.from([9, 0, 0, 0, 0, 0, 0, 248, 63]), "l08.ProtobufPoint"],
    ["transfer", "w", "s2", "hub"],
    ["op", "h", "putProto", "/.schema/proto:a", 2, PROTO_FILES[0], "FileDescriptorProto"],
    ["merge", "h", "s2", "/Log1"],
    ["op", "h", "dumpAll"],
    ["op", "h", "reload"],
    ["op", "h", "dumpAll"]
  ],
  [
    ["new", "h", "hub", true],
    ["op", "h", "putStruct", "/Big", 1, new Uint8Array(5001), "Pose2d", true],
    ["op", "h", "putStruct", "/Edge", 1, new Uint8Array(5000), "Translation2d", true],
    ["op", "h", "putStruct", "/BigSwerve", 1, new Uint8Array(5016), "ChassisSpeeds", true],
    ["op", "h", "putStruct", "/BigOne", 1, new Uint8Array(5008), "Pose3d", false],
    ...WPILIB_STRUCTS.map(([name, text]) => ["op", "h", "putRaw", "/.schema/struct:" + name, 2, utf8(text)]),
    ["op", "h", "putStruct", "/Big", 3, new Uint8Array(5001), "Pose2d", true],
    ["op", "h", "putStruct", "/Edge", 3, new Uint8Array(5000), "Translation2d", true],
    ["op", "h", "putStruct", "/Small", 3, new Uint8Array(4992), "Translation2d", true],
    ["op", "h", "putStruct", "/BigSwerve", 3, new Uint8Array(5016), "ChassisSpeeds", true],
    ["op", "h", "units"],
    ["op", "h", "dumpAll"]
  ],
  [
    ["new", "h", "hub", true],
    ["new", "w", "worker", true],
    ["op", "w", "putStruct", "/Pose", 1, new Uint8Array(24), "Pose2d", false],
    ["op", "w", "putRaw", "/.schema/struct:Pose2d", 1, utf8("Translation2d translation;Rotation2d rotation")],
    ["op", "w", "putRaw", "/.schema/struct:Rotation2d", 1, utf8("double value")],
    ["op", "w", "dumpAll"],
    ["op", "h", "putStruct", "/Pose", 0.5, new Uint8Array(24), "Pose2d", false],
    ["op", "h", "putRaw", "/.schema/struct:Translation2d", 0.5, utf8("double x;double y")],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", "/Log1"],
    ["op", "h", "dumpAll"],
    ["op", "h", "putRaw", "/.schema/struct:Unrelated", 2, utf8("double z")],
    ["op", "h", "units"],
    ["op", "h", "dumpAll"]
  ],
  [
    ["new", "h", "hub", true],
    ["new", "w", "worker", true],
    ["op", "w", "putNumber", "", 1, 1],
    ["op", "w", "putNumber", "/", 2, 2],
    ["op", "w", "putNumber", "a", -0, 3],
    ["op", "w", "putNumberArray", "arr", 3, [1, 2]],
    ["op", "h", "putNumber", "/P/a", 5, 1],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", "/P"],
    ["new", "w", "worker", true],
    ["op", "w", "putNumber", "a", -0, 3],
    ["op", "w", "putNumber", "", 4, 4],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", ""],
    ["op", "h", "dumpAll"]
  ],
  [
    ["new", "h", "hub", true],
    ["new", "w", "worker", true],
    ["op", "h", "putNumber", "a", 1, 1],
    ["op", "w", "putNumber", "b", NaN, 2],
    ["op", "w", "putNumber", "c", 3, 2],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", "/x"],
    ["op", "h", "range"],
    ["op", "h", "dumpAll"],
    ["new", "h2", "hub", true],
    ["new", "w", "worker", true],
    ["op", "h2", "putNumber", "a", 1, 1],
    ["op", "h2", "updateRangeWithTimestamp", -0],
    ["op", "w", "putNumber", "b", 0, 2],
    ["op", "w", "putNumber", "c", NaN, 2],
    ["transfer", "w", "s", "hub"],
    ["merge", "h2", "s", "/x"],
    ["op", "h2", "dumpAll"]
  ],
  [
    ["new", "h", "hub", true],
    ["new", "w", "worker", true],
    ["op", "w", "putNumber", "/Battery", 0, 12],
    ["op", "w", "putBoolean", "/DriverStation/Enabled", 0, false],
    ["op", "w", "putBoolean", "/DriverStation/Autonomous", 0, false],
    ["op", "w", "putBoolean", "/DriverStation/Enabled", 10, true],
    ["op", "w", "putBoolean", "/DriverStation/Autonomous", 10, true],
    ["op", "w", "putBoolean", "/DriverStation/Autonomous", 25, false],
    ["op", "w", "putBoolean", "/DriverStation/Enabled", 160, false],
    ["op", "w", "putNumber", "/Battery", 170, 11.5],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", "/Log0"],
    ["new", "w", "worker", true],
    ["op", "w", "putNumber", "/Battery", 3, 12],
    ["op", "w", "putBoolean", "/DriverStation/Enabled", 3, false],
    ["op", "w", "putBoolean", "/DriverStation/Autonomous", 3, false],
    ["op", "w", "putBoolean", "/DriverStation/Enabled", 17.25, true],
    ["op", "w", "putBoolean", "/DriverStation/Autonomous", 17.25, true],
    ["op", "w", "putBoolean", "/DriverStation/Autonomous", 32.2, false],
    ["op", "w", "putBoolean", "/DriverStation/Enabled", 167.3, false],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", "/Log1"],
    ["new", "w", "worker", false],
    ["op", "w", "putBoolean", "DS:enabled", 40, false],
    ["op", "w", "putBoolean", "DS:enabled", 51.5, true],
    ["op", "w", "putBoolean", "DS:enabled", 201.4, false],
    ["op", "w", "putNumber", "/Other", 60, 1],
    ["transfer", "w", "s", "hub"],
    ["merge", "h", "s", "/Log2"],
    ["op", "h", "range"],
    ["op", "h", "dumpAll"]
  ]
];

async function writers(count: number, seed: number, corpusSeed: number) {
  const corpus = buildCorpusA(corpusSeed);
  const cases = [...corpus.cases].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  const replays: [string, Op[]][] = [];
  for (const c of cases) {
    const ops = corpusOps(c);
    if (ops !== null) replays.push([c.id, ops]);
  }
  const small = replays.map(([, ops]) => ops).filter((ops) => ops.length <= 40);
  const msgpackPayloads = small.flat().filter((op) => op[0] === "putMsgpack").map((op) => op[3] as Uint8Array);

  const emitScript = async (ops: Op[], realmSeed: number): Promise<boolean> => {
    const realms = [...new Set(ops.flatMap((op) => (op[0] === "new" ? [op[2]] : op[0] === "transfer" ? [op[3]] : [])))];
    const result = await runScript(realms, ops, realmSeed);
    if (result.never) return false;
    const args = [toTree(result.realms), toTree(ops)];
    process.stdout.write(JSON.stringify({ fn: "oracle:logScript", args, ok: result.results }) + "\n");
    return true;
  };

  for (let i = 0; i < count; i++) {
    for (let attempt = 0; ; attempt++) {
      const r = new Rng(seed, `writers/${i}/${attempt}`);
      if (await emitScript(writersCase(r, small, msgpackPayloads), seed + i)) break;
    }
  }
  for (const ops of FIXED_SCRIPTS) await emitScript(ops, seed);
  for (const [id, ops] of replays) {
    const script: Op[] = [
      ["new", "w", "worker", true],
      ...ops.map((op) => ["op", "w", ...op]),
      ["op", "w", "getChangedFields"],
      ["op", "w", "dumpAll"],
      ["transfer", "w", "h", "hub"],
      ["op", "h", "units"],
      ["op", "h", "dumpAll"]
    ];
    if (!(await emitScript(script, seed))) process.stderr.write(`genLog: ${id} never completes upstream, skipped\n`);
  }
}

if (!serveRealm()) {
  const argv = process.argv.slice(2);
  if (argv[0] === "writers") {
    await writers(Number(argv[1]), Number(argv[2]), Number(argv[3]));
  } else {
    core(Number(argv[0]), Number(argv[1]), Number(argv[2]));
  }
}
