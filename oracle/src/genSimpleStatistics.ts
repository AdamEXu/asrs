import "./shims";
import { MessageChannel, Worker, isMainThread, receiveMessageOnPort, workerData } from "node:worker_threads";
import type { MessagePort } from "node:worker_threads";
import * as ss from "simple-statistics";
import { fromTree, toTree } from "./cve1";

// Seeded ground truth for the simple-statistics 7.8.7 subset StatisticsController uses
// (asrs_tabs::simple_statistics), over arrays the fn spec cannot carry: sizes past quickselect's
// 600-element Floyd-Rivest window, duplicate runs, NaN, undefined and coerced elements. Lines:
//   {"a":<id>,"x":[<CVE-1 elements>]}                            defines array <id>
//   {"a":<id>,"fn":<name>,"p":<CVE-1>?,"ok":<CVE-1>|"throw":<CVE-1>|"never":true}
// "p" is the second argument where the function takes one. Probabilities include values that
// coerce (null, "0", true) and ones out of range, whose select writes named properties such as
// arr["-1"] that quantileSorted then reads. Functions whose results go through the C library's
// pow (sampleSkewness, sumNthPowerDeviations with n !== 2) differ by platform and stay in the
// per-platform fn spec. A quantile whose probability makes quickselect spin forever (k is NaN,
// or -Infinity pins the partition's i) runs in a worker that gets the call once it is up; no
// reply within the timeout is recorded as "never".

const NEVER_TIMEOUT_MS = 1000;

const ARRAY_FNS = [
  "mean",
  "sum",
  "minSorted",
  "maxSorted",
  "medianSorted",
  "modeSorted",
  "harmonicMean",
  "rootMeanSquare",
  "sampleVariance",
  "sampleStandardDeviation",
  "median",
  "medianAbsoluteDeviation",
  "interquartileRange",
  "logAverage"
] as const;

const call = (name: string, args: unknown[]): unknown => (ss as any)[name](...args);

function outcome(name: string, args: unknown[]): { ok: unknown } | { throw: unknown } {
  try {
    return { ok: call(name, args) };
  } catch (e) {
    return { throw: e };
  }
}

if (!isMainThread && workerData?.simpleStatistics === true) {
  const port = workerData.port as MessagePort;
  const signal = workerData.signal as Int32Array;
  Atomics.store(signal, 1, 1);
  Atomics.notify(signal, 1);
  port.on("message", (msg: { name: string; args: unknown }) => {
    const result = outcome(msg.name, fromTree(msg.args) as unknown[]);
    port.postMessage("ok" in result ? { ok: toTree(result.ok) } : { throw: toTree(result.throw) });
    Atomics.store(signal, 0, 1);
    Atomics.notify(signal, 0);
  });
} else {
  await main();
}

async function guarded(name: string, args: unknown[]): Promise<Record<string, unknown>> {
  const signal = new Int32Array(new SharedArrayBuffer(8));
  const { port1, port2 } = new MessageChannel();
  const worker = new Worker(new URL(import.meta.url), {
    workerData: { simpleStatistics: true, port: port2, signal },
    transferList: [port2]
  });
  try {
    if (Atomics.wait(signal, 1, 0, 120000) === "timed-out") throw new Error("worker did not start");
    port1.postMessage({ name, args: toTree(args) });
    const deadline = Date.now() + NEVER_TIMEOUT_MS;
    for (;;) {
      const waited = Atomics.wait(signal, 0, 0, Math.max(1, deadline - Date.now()));
      const reply = receiveMessageOnPort(port1);
      if (reply !== undefined) return reply.message;
      if (waited === "timed-out" || Date.now() >= deadline) return { never: true };
    }
  } finally {
    port1.close();
    await worker.terminate();
  }
}

async function main() {
  let seed = 0x5717_0787;
  const rng = () => {
    seed = (seed + 0x6d2b79f5) >>> 0;
    let t = seed;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const below = (n: number) => Math.floor(rng() * n);
  const pick = <T>(items: readonly T[]): T => items[below(items.length)];

  const SPECIALS = [NaN, -0, 0, Infinity, -Infinity, 1e-300, -1e-300, 5e-324, 1e308, -1e308, 0.5, 1, -1, 2 ** 53];
  // Fresh objects per element: CVE-1 cannot carry shared references, and `===` sees them.
  const MIXED: (() => unknown)[] = [1, 2.5, -3, "2", "abc", "", " 4 ", "1e3", true, false, null, undefined].map(
    (v) => () => v
  );
  MIXED.push(
    () => [5],
    () => [],
    () => ({})
  );
  const uniform = () => rng() * 200 - 100;
  const kinds: Record<string, (n: number) => unknown[]> = {
    uniform: (n) => Array.from({ length: n }, uniform),
    ints: (n) => Array.from({ length: n }, () => below(10) - 3),
    sorted: (n) => Array.from({ length: n }, uniform).sort((a, b) => a - b),
    reversed: (n) => Array.from({ length: n }, uniform).sort((a, b) => b - a),
    equal: (n) => Array.from({ length: n }, () => 7.25),
    positive: (n) => Array.from({ length: n }, () => rng() * 1000 + 1e-9),
    nan: (n) => Array.from({ length: n }, () => (rng() < 0.08 ? NaN : uniform())),
    undef: (n) => Array.from({ length: n }, () => (rng() < 0.08 ? undefined : uniform())),
    special: (n) => Array.from({ length: n }, () => pick(SPECIALS)),
    magnitude: (n) => Array.from({ length: n }, () => (rng() - 0.5) * 10 ** (below(40) - 20)),
    mixed: (n) => Array.from({ length: n }, () => pick(MIXED)())
  };
  const SMALL = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 16, 31, 32, 33, 64, 100, 255];
  const LARGE = [599, 600, 601, 602, 603, 1000, 1201, 2500];
  const LARGE_KINDS = ["uniform", "ints", "sorted", "reversed", "nan", "undef", "special"];

  const arrays: unknown[][] = [];
  for (const kind of Object.keys(kinds)) {
    for (const n of SMALL) arrays.push(kinds[kind](n));
    if (LARGE_KINDS.includes(kind)) for (const n of LARGE) arrays.push(kinds[kind](n));
  }

  const PROBABILITIES: unknown[] = [
    0,
    -0,
    1,
    0.01,
    0.05,
    0.1,
    0.25,
    0.5,
    0.75,
    0.9,
    0.95,
    0.99,
    1 / 3,
    2 / 3,
    1e-17,
    1 - 2 ** -53,
    1.5,
    -0.1,
    Infinity,
    "0.5",
    "1",
    "0",
    "",
    true,
    false,
    null,
    [0.25, 0.5, 0.75],
    [0.9, 0.1, 0.5, 0.5],
    [0, 1],
    [],
    [0.5, 1.5]
  ];
  const NEVER_PROBABILITIES: unknown[] = [NaN, undefined, "abc", -Infinity, [0.5, NaN], [NaN, 0.25, 0.75], [-Infinity]];

  const lines: string[] = [];
  const emit = (value: object) => lines.push(JSON.stringify(value));
  const record = (id: number, name: string, p: unknown[], result: Record<string, unknown>) =>
    emit({ a: id, fn: name, ...(p.length > 0 ? { p: toTree(p[0]) } : {}), ...result });

  let id = 0;
  const define = (x: unknown[]) => {
    emit({ a: id, x: toTree(x) });
    return id++;
  };
  let variant = 0;
  for (const x of arrays) {
    const variants = [x];
    if (x.every((v) => typeof v === "number" || v === undefined)) variants.push(x.slice().sort((a: any, b: any) => a - b));
    for (const v of variants) {
      const a = define(v);
      for (const name of ARRAY_FNS) {
        const r = outcome(name, [v]);
        record(a, name, [], "ok" in r ? { ok: toTree(r.ok) } : { throw: toTree(r.throw) });
      }
      const nth = outcome("sumNthPowerDeviations", [v, 2]);
      record(a, "sumNthPowerDeviations", [2], "ok" in nth ? { ok: toTree(nth.ok) } : { throw: toTree(nth.throw) });
      const ps = [...PROBABILITIES, rng(), v.length > 0 ? below(v.length + 1) / v.length : 0.5];
      for (const name of ["quantile", "quantileSorted"]) {
        for (const p of ps) {
          const r = outcome(name, [v, p]);
          record(a, name, [p], "ok" in r ? { ok: toTree(r.ok) } : { throw: toTree(r.throw) });
        }
      }
      if (variant++ % 16 === 0 || v.length === 601 || v.length === 2500) {
        const count = NEVER_PROBABILITIES.length;
        for (const p of [NEVER_PROBABILITIES[variant % count], NEVER_PROBABILITIES[(variant + 3) % count]]) {
          record(a, "quantile", [p], await guarded("quantile", [v, p]));
          const r = outcome("quantileSorted", [v, p]);
          record(a, "quantileSorted", [p], "ok" in r ? { ok: toTree(r.ok) } : { throw: toTree(r.throw) });
        }
      }
    }
  }
  process.stdout.write(lines.join("\n") + "\n");
}
