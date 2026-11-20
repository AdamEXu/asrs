import "./shims";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";
import { run } from "./liveBaseRun";

// Emits LiveDataSource ground truth for asrs_live::LiveDataSourceBase as `oracle:liveBaseRun`
// fn records:
//   genLiveBase <count> <seed>
//     {"fn":"oracle:liveBaseRun","args":[<random>,<startMs>,[<op>...]],"ok":[<record>...]}
// The first <count> records are random scripts (see liveBaseRun.ts); the rest are fixed
// scripts for status transitions and the discard interval.

const [countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);

const STARTS = [1700000000000, 1700000000123, 0, 1000, 59999];
const ADDRESSES = ["127.0.0.1", "10.63.28.2", "172.27.0.1", ""];
const KEYS = ["a", "b", "/x/y", "NT:/SmartDashboard/v"];
const DISCARDS = [60, 300, 1200, -1, 0, 0.5, 5, 1e-3, NaN, -0, Infinity, -Infinity, -60, 1e21];
const CONSTS = [10, 0, -0, -5, 0.25, 59.5, 60, 61, 1e9, NaN, Infinity, -Infinity];
const ADVANCES = [0, 1, 15, 16, 17, 31, 32, 33, 50, 100, 999, 1000, 16 * 60, 59999, 61000, 300000];
const OFFSETS = [-2000, -1300, -301, -61, -60.5, -60, -59.999, -5, -1, -0.016, -0.001, 0, 0.5, 3];
const VALUES = [0, 1, -1, 2.5, NaN, Infinity, -0, 1e300];

function randomValues(r: Rng): number[] {
  return Array.from({ length: 32 }, () => r.unit());
}

function emit(random: number[], startMs: number, ops: unknown[][]) {
  const args = [toTree(random), toTree(startMs), toTree(ops)];
  const result = run(random, startMs, ops as any[][]);
  process.stdout.write(JSON.stringify({ fn: "oracle:liveBaseRun", args, ok: toTree(result) }) + "\n");
}

function makeCase(r: Rng): [number[], number, unknown[][]] {
  const startMs = r.pick(STARTS);
  let nowMs = startMs;
  const zeros = [0, 5, 100, 1300, 2000].map((back) => startMs / 1000 - back).concat([startMs / 1000 + 50]);
  let zero = r.pick(zeros);
  const ops: unknown[][] = [];
  const steps = r.range(5, 60);
  for (let i = 0; i < steps; i++) {
    const choice = r.below(100);
    if (choice < 6) {
      const enable = r.pick([true, false, undefined, undefined]);
      ops.push(["connect", r.pick(ADDRESSES), enable]);
    } else if (choice < 18) {
      ops.push(["setStatus", r.below(5)]);
    } else if (choice < 23) {
      ops.push(["newLog"]);
    } else if (choice < 25) {
      ops.push(["dropLog"]);
    } else if (choice < 45) {
      const base = r.below(6) === 0 ? r.pick(CONSTS) : nowMs / 1000 - zero;
      const timestamp = r.below(25) === 0 ? r.pick([NaN, Infinity, -Infinity, -0]) : base + r.pick(OFFSETS);
      ops.push(["put", r.pick(KEYS), timestamp, r.pick(VALUES)]);
    } else if (choice < 55) {
      if (r.below(3) === 0) {
        ops.push(["output", ["const", r.pick(CONSTS)]]);
      } else {
        if (r.below(4) === 0) zero = r.pick(zeros);
        ops.push(["output", ["wall", zero]]);
      }
    } else if (choice < 63) {
      const mode = r.below(6);
      ops.push(mode === 0 ? ["prefs", 0] : mode === 1 ? ["prefs", 1] : ["prefs", 2, r.pick(DISCARDS)]);
    } else if (choice < 85) {
      const ms = r.pick(ADVANCES);
      nowMs += ms;
      ops.push(["advance", ms]);
    } else if (choice < 88) {
      ops.push(["stop"]);
    } else if (choice < 92) {
      ops.push(["state"]);
    } else if (choice < 97) {
      ops.push(["range"]);
    } else {
      ops.push(["dump"]);
    }
  }
  ops.push(["state"], ["dump"]);
  return [randomValues(r), startMs, ops];
}

for (let i = 0; i < count; i++) {
  const [random, startMs, ops] = makeCase(new Rng(seed, `live/base/${i}`));
  emit(random, startMs, ops);
}

const fixed = new Rng(seed, "live/base/fixed");
const T0 = 1700000000000;
const ZERO = T0 / 1000 - 100;
const fill = (from: number, to: number): unknown[][] => {
  const out: unknown[][] = [];
  for (let t = from; t <= to; t += 10) out.push(["put", "a", t, t]);
  return out;
};

// Connect, go active and discard everything older than 60 s on the next 16 ms tick.
emit(randomValues(fixed), T0, [
  ["state"],
  ["prefs", 2, 60],
  ["connect", "127.0.0.1", undefined],
  ["state"],
  ["newLog"],
  ...fill(0, 100),
  ["setStatus", 2],
  ["output", ["wall", ZERO]],
  ["advance", 15],
  ["range"],
  ["advance", 1],
  ["range"],
  ["advance", 16],
  ["dump"],
  ["advance", 20000],
  ["dump"]
]);

// liveDiscard -1, a preferences object without liveDiscard and no preferences never discard;
// changing the preference takes effect on the next tick.
emit(randomValues(fixed), T0, [
  ["connect", "10.63.28.2", true],
  ["newLog"],
  ...fill(0, 100),
  ["setStatus", 2],
  ["output", ["wall", ZERO]],
  ["prefs", 2, -1],
  ["advance", 100],
  ["range"],
  ["prefs", 1],
  ["advance", 100],
  ["range"],
  ["prefs", 0],
  ["advance", 100],
  ["range"],
  ["prefs", 2, 5],
  ["advance", 16],
  ["range"],
  ["prefs", 2, 1200],
  ["advance", 16],
  ["dump"]
]);

// Any status other than Active drops the time supplier, so discarding stops until the next
// output, even while the status stays Connecting.
emit(randomValues(fixed), T0, [
  ["prefs", 2, 60],
  ["connect", "127.0.0.1", undefined],
  ["newLog"],
  ...fill(0, 100),
  ["output", ["wall", ZERO]],
  ["advance", 16],
  ["range"],
  ["setStatus", 2],
  ["output", ["wall", ZERO]],
  ["setStatus", 2],
  ["advance", 16],
  ["range"],
  ["setStatus", 1],
  ["put", "a", 30, 30],
  ["advance", 32],
  ["range"],
  ["output", ["wall", ZERO + 30]],
  ["advance", 16],
  ["dump"]
]);

// Stopped is final: no further status callbacks, the interval is cleared, output still runs.
emit(randomValues(fixed), T0, [
  ["prefs", 2, 60],
  ["connect", "127.0.0.1", undefined],
  ["setStatus", 2],
  ["newLog"],
  ...fill(0, 100),
  ["output", ["wall", ZERO]],
  ["stop"],
  ["state"],
  ["setStatus", 2],
  ["setStatus", 3],
  ["state"],
  ["output", ["wall", ZERO]],
  ["advance", 1000],
  ["range"],
  ["stop"],
  ["connect", "172.27.0.1", undefined],
  ["state"],
  ["output", ["wall", ZERO]],
  ["advance", 16],
  ["dump"]
]);

// A second connect leaks the first interval, which keeps discarding after stop.
emit(randomValues(fixed), T0, [
  ["prefs", 2, 60],
  ["connect", "127.0.0.1", undefined],
  ["advance", 5],
  ["connect", "127.0.0.1", undefined],
  ["newLog"],
  ...fill(0, 100),
  ["setStatus", 2],
  ["output", ["wall", ZERO]],
  ["stop"],
  ["output", ["wall", ZERO]],
  ["advance", 11],
  ["range"],
  ["advance", 5],
  ["dump"]
]);

// enableClearData = false (the RLOG source) never discards.
emit(randomValues(fixed), T0, [
  ["prefs", 2, 60],
  ["connect", "127.0.0.1", false],
  ["newLog"],
  ...fill(0, 100),
  ["setStatus", 2],
  ["output", ["wall", ZERO]],
  ["advance", 5000],
  ["dump"]
]);

// Math.max(0, minTime): a NaN supplier clears with NaN, a negative one clears before 0.
emit(randomValues(fixed), T0, [
  ["prefs", 2, 60],
  ["connect", "127.0.0.1", undefined],
  ["newLog"],
  ["put", "a", -5, 1],
  ["put", "a", 0, 2],
  ["put", "a", 3, 3],
  ["setStatus", 2],
  ["output", ["const", 10]],
  ["advance", 16],
  ["dump"],
  ["output", ["const", NaN]],
  ["advance", 16],
  ["dump"],
  ["output", ["const", 61]],
  ["advance", 16],
  ["dump"],
  ["prefs", 2, NaN],
  ["output", ["const", 100]],
  ["advance", 16],
  ["dump"]
]);

// No log, then a log that replaces the one handed to output.
emit(randomValues(fixed), T0, [
  ["prefs", 2, 60],
  ["connect", "127.0.0.1", undefined],
  ["setStatus", 2],
  ["output", ["wall", ZERO]],
  ["advance", 16],
  ["newLog"],
  ...fill(0, 100),
  ["advance", 16],
  ["range"],
  ["dropLog"],
  ["output", ["wall", ZERO]],
  ["advance", 16],
  ["dump"]
]);
