import "./shims";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";
import { run } from "./logFieldRun";

// Emits LogField ground truth for asrs_log::log::LogField as `oracle:logFieldRun` fn records:
//   genLogField <count> <seed>
//     {"fn":"oracle:logFieldRun","args":[<type>,[<op>...]],"ok":[<result>...]}
// Each case is a random operation script (see logFieldRun.ts) over a field of a random type:
// puts of every type (mostly matching, some conflicting), getRange and the typed getters with
// every boundary shape, uuid cache reuse and startOffset, clearBeforeTime, type string changes
// that toggle child-log handling, and serialization round trips.

const [countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);

const NUMBERS = [0, -0, 1, 2, 1.5, -1, NaN, Infinity, -Infinity];
const STRINGS = ["", "a", "b", "log:", "\ud800", "é"];
const BYTES = [[], [1], [1, 2], [2], [0, 0, 0]];
const TYPE_STRINGS = [null, null, "log:wpilog", "log:", "struct:Pose2d", "raw", "Log:x", "int64[]"];
const OFFSETS = [-100, -3, -2, -1, 0, 1, 2, 3, 100];
const UUIDS = ["u1", "u2", "u3"];

function value(r: Rng, kind: number): unknown {
  const short = () => r.below(3);
  switch (kind) {
    case 0:
      return new Uint8Array(r.pick(BYTES));
    case 1:
      return r.bool();
    case 2:
      return r.pick(NUMBERS);
    case 3:
      return r.pick(STRINGS);
    case 4:
      return Array.from({ length: short() }, () => r.bool());
    case 5:
      return Array.from({ length: short() }, () => r.pick(NUMBERS));
    default:
      return Array.from({ length: short() }, () => r.pick(["", "a", "b"]));
  }
}

function makeCase(r: Rng): [number, unknown[][]] {
  const type = r.below(8);
  const exotic = r.below(5) === 0;
  const steps = r.range(5, 60);
  const ops: unknown[][] = [];
  const written: number[] = [];
  let cursor = r.range(-2, 5);

  const timestamp = (): number => {
    if (exotic && r.below(6) === 0) return r.pick([NaN, Infinity, -Infinity, -0]);
    switch (r.below(6)) {
      case 0:
      case 1:
        cursor += r.below(3);
        return cursor;
      case 2:
        return r.range(-2, 20);
      case 3:
        return r.range(-4, 40) / 2;
      case 4:
        return written.length > 0 ? r.pick(written) : cursor;
      default:
        return cursor - r.below(4);
    }
  };

  const bound = (): number => {
    switch (r.below(8)) {
      case 0:
      case 1:
      case 2:
        return written.length > 0 ? r.pick(written) : r.range(-2, 20);
      case 3:
        return (written.length > 0 ? r.pick(written) : 0) + r.pick([-0.5, 0.5, -1e-9, 1e-9]);
      case 4:
        return r.range(-4, 44) / 2;
      case 5:
        return r.pick([-Infinity, Infinity, -0, 0]);
      case 6:
        return exotic ? NaN : r.range(-5, 25);
      default:
        return cursor + r.range(-3, 3);
    }
  };

  for (let i = 0; i < steps; i++) {
    const choice = r.below(100);
    if (choice < 50) {
      const kind = r.below(8) === 0 ? r.below(7) : type === 7 ? r.below(7) : type;
      const t = timestamp();
      written.push(t);
      ops.push(["put", kind, t, value(r, kind)]);
    } else if (choice < 82) {
      const kind = r.below(3) === 0 ? -1 : r.below(6) === 0 ? r.below(7) : type === 7 ? -1 : type;
      const start = bound();
      const shape = r.below(3);
      const end = shape === 0 ? start : bound();
      const uuid = r.below(5) < 2 ? undefined : r.pick(UUIDS);
      const offset = r.below(5) < 3 ? undefined : r.pick(OFFSETS);
      ops.push(["get", kind, start, end, uuid, offset]);
    } else if (choice < 89) {
      ops.push(["clear", bound()]);
    } else if (choice < 93) {
      ops.push(["meta", r.pick(TYPE_STRINGS), r.pick(TYPE_STRINGS), r.pick(["", "{}", "unit"])]);
    } else if (choice < 95) {
      ops.push(["reload"]);
    } else {
      ops.push(["state"]);
    }
  }
  ops.push(["state"]);
  return [type, ops];
}

for (let i = 0; i < count; i++) {
  const r = new Rng(seed, `logField/${i}`);
  const [type, ops] = makeCase(r);
  const result = run(type, ops);
  process.stdout.write(
    JSON.stringify({ fn: "oracle:logFieldRun", args: [toTree(type), toTree(ops)], ok: toTree(result) }) + "\n"
  );
}
