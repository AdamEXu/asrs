import "./shims";
import { ExtData } from "@msgpack/msgpack";
import { fromTree, toTree } from "./cve1";
import { Corpus } from "./corpus/corpus";
import { jsonMatrix } from "./corpus/json";
import { Rng } from "./corpus/rng";
import { run } from "./logRun";

// Emits Log ground truth for asrs_log::log::Log as `oracle:logRun` fn records:
//   genLog <count> <seed> <corpusASeed>
//     {"fn":"oracle:logRun","args":[<enableTimestampSetCache>,[<op>...]],"ok":[<result>...]}
// The first <count> records are random scripts (see logRun.ts) over a key pool with
// integer-like keys, generated children, console and `.type` keys, merge and Phoenix prefixes;
// each ends with a dump. The rest replay corpus A's `.ops.jsonl` write sequences that need no
// struct, photon, proto or msgpack decoding, followed by a dump.
//
// Keys and JSON property names that Object.prototype already defines ("constructor",
// "__proto__", ...) are left out of the random scripts: upstream's `key in this.fields` and
// field tree lookups see the inherited property and throw or pollute the prototype.

const [countArg, seedArg, corpusSeedArg] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);
const corpusSeed = Number(corpusSeedArg);

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
  const steps = r.range(5, 70);
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
    } else if (choice < 95) {
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
