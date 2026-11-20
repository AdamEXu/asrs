import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import { removeMergePrefix } from "@upstream/shared/log/LogKeyUtils";
import { AUTONOMOUS_KEYS, ENABLED_KEYS, UTILITY_KEYS } from "@upstream/shared/log/RobotState";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";
import { HistoricalSession } from "./historical/session";
import { run } from "./lazyHelpers/mergeCalculatorRun";
import { runWithLog } from "./logRun";

// Emits `oracle:mergeCalculatorRun` ground truth for asrs_log::log::merge_calculator:
//   genMergeCalculator <count> <seed>
//     {"fn":"oracle:mergeCalculatorRun","args":[<name>,<target>,<source>],"ok":<offset>}
//   <target> and <source> are the logs' toSerialized() dumps, so the run rebuilds
//   both through Log.fromSerialized before calling the real getMergeOffset.
// Pairs come from corpus B's merge/* cases (full serialized logs, decoded by the
// real historical pipeline), the public.toml merge pair (reduced serialized logs,
// see below) and scripted Log writes covering the constants' edges.

const [countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);
const root = process.cwd();

const out = (name: string, target: any, source: any) => {
  const result = run(name, target, source);
  process.stdout.write(
    JSON.stringify({
      fn: "oracle:mergeCalculatorRun",
      args: [toTree(name), toTree(target), toTree(source)],
      ok: toTree(result)
    }) + "\n"
  );
};

async function decode(file: string): Promise<any> {
  const session = await HistoricalSession.open(path.resolve(file));
  try {
    await session.loadEverything();
    return session.log.toSerialized();
  } finally {
    await session.close();
  }
}

// Corpus B merge pairs: full serialized logs.
async function corpusPairs() {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(root, "parity/corpus/synthetic/b.manifest.json"), "utf8")
  );
  const dir = path.join(root, "target/parity/corpus/synthetic/b");
  for (const c of manifest.list) {
    if (!c.id.startsWith("merge/")) continue;
    const files = c.files.map((f: { path: string }) => f.path).sort();
    const [targetFile, sourceFile] = [
      files.find((f: string) => f.endsWith("/target.wpilog")),
      files.find((f: string) => !f.endsWith("/target.wpilog"))
    ];
    const target = await decode(path.join(dir, targetFile));
    const source = await decode(path.join(dir, sourceFile));
    out(c.id, target, source);
  }
}

// getMergeOffset reads the timestamp range plus the fields findKey picks from the
// ENABLED/AUTONOMOUS/UTILITY lists, so a serialized log reduced to those candidate
// fields answers identically. The public logs are far too large to embed whole.
const STATE_KEYS = new Set([...ENABLED_KEYS, ...AUTONOMOUS_KEYS, ...UTILITY_KEYS]);

function isCandidate(key: string): boolean {
  const unmerged = removeMergePrefix(key);
  if (STATE_KEYS.has(unmerged)) return true;
  return unmerged.startsWith("/") && STATE_KEYS.has(unmerged.slice(1));
}

function reduced(serialized: any): any {
  const fields: { [key: string]: any } = {};
  for (const [key, field] of Object.entries(serialized.fields)) {
    if (isCandidate(key)) fields[key] = field;
  }
  return { ...serialized, fields };
}

interface PublicLog {
  name: string;
  file: string;
  sha256: string;
  size: number;
  merge?: string;
}

function publicLogs(): PublicLog[] {
  const text = fs.readFileSync(path.join(root, "parity/corpus/public.toml"), "utf8");
  const logs: PublicLog[] = [];
  let current: { [key: string]: string | number } | null = null;
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "[[log]]") {
      current = {};
      logs.push(current as unknown as PublicLog);
    } else if (current !== null) {
      const m = /^([a-z0-9_]+) = ("(?:[^"\\]|\\.)*"|-?[0-9]+)$/.exec(trimmed);
      if (m !== null) {
        current[m[1]] = m[2].startsWith('"') ? (JSON.parse(m[2]) as string) : Number(m[2]);
      }
    }
  }
  return logs;
}

function publicLogsDir(): string {
  let cache = process.env.ASRS_CACHE;
  if (cache === undefined) {
    const target = path.join(root, "target");
    try {
      cache = path.join(path.dirname(fs.realpathSync(target)), "cache");
    } catch {
      cache = path.join(target, "cache");
    }
  }
  return path.join(cache, "public-logs");
}

async function publicPairs() {
  const dir = publicLogsDir();
  const groups = new Map<string, PublicLog[]>();
  for (const log of publicLogs()) {
    if (log.merge === undefined) continue;
    const group = groups.get(log.merge) ?? [];
    group.push(log);
    groups.set(log.merge, group);
  }
  for (const [merge, logs] of [...groups.entries()].sort()) {
    const a = reduced(await decode(path.join(dir, logs[0].file)));
    const b = reduced(await decode(path.join(dir, logs[1].file)));
    out(`public/${merge}`, a, b);
    out(`public/${merge}-reversed`, b, a);
  }
}

// Scripted cases over the constants' edges: gap threshold, inlier tolerance,
// two-edge span, cluster span, inlier radius clamps, merge-prefixed keys, blank
// and never-enabled logs, NaN and duplicate timestamps.
function scriptedCases() {
  const build = (ops: unknown[][]) => runWithLog(true, ops).log.toSerialized();
  const emit = (name: string, targetOps: unknown[][], sourceOps: unknown[][]) =>
    out(name, build(targetOps), build(sourceOps));

  const ENABLED = "/DriverStation/Enabled";
  const AUTO = "/DriverStation/Autonomous";
  const TEST = "/DriverStation/Test";

  const pulse = (key: string, times: [number, boolean][]): unknown[][] =>
    times.map(([t, v]) => ["putBoolean", key, t, v]);
  const timeline = (
    transitions: [number, boolean, boolean?][],
    key = ENABLED,
    autoKey: string | null = AUTO,
    extra: unknown[][] = []
  ): unknown[][] => {
    const ops: unknown[][] = [["putNumber", "/Battery", transitions[0][0], 12]];
    for (const [t, enabled, auto] of transitions) {
      ops.push(["putBoolean", key, t, enabled]);
      if (autoKey !== null && auto !== undefined) ops.push(["putBoolean", autoKey, t, auto]);
    }
    return ops.concat(extra);
  };

  emit("empty/empty", [], []);
  emit("empty/enabled", [], timeline([[0, false], [10, true], [20, false]]));
  emit("blank/enabled", [["createBlankField", ENABLED, 1]], timeline([[0, true]]));
  emit(
    "never-enabled/both",
    timeline([[0, false], [50, false]]),
    timeline([[5, false], [60, false]])
  );
  emit(
    "merged-prefix",
    timeline([[0, false], [10, true, true], [30, true, false], [50, false]]),
    timeline([[2, false], [12, true, true], [32, true, false], [52, false]], "/Log1" + ENABLED, "/Log1" + AUTO)
  );
  emit(
    "gap-threshold/at",
    timeline([[0, false], [10, true], [40, false]]),
    timeline([[5, true, false], [60, false]])
  );
  emit(
    "gap-threshold/over",
    timeline([[0, false], [10, true], [40, false]]),
    timeline([[5.0000000001, true, false], [60, false]])
  );
  emit(
    "inlier-tolerance/inside",
    timeline([[0, false], [20, true, true], [40, true, false], [60, false]]),
    timeline([[0, false], [19, true, true], [41, true, false], [60, false]])
  );
  emit(
    "two-edge/exact-span",
    timeline([[0, false], [20, true], [40, false]]),
    timeline([[0, false], [15, true], [40.25, false]])
  );
  emit(
    "two-edge/over-span",
    timeline([[0, false], [20, true], [40, false]]),
    timeline([[0, false], [15, true], [40.2500001, false]])
  );
  emit(
    "cluster/at-limit",
    timeline([[0, false], [10, true], [20, false], [30, true], [40, false], [50, true], [60, false]]),
    timeline([[0, false], [9, true], [19.9, false], [29.95, true], [40.1, false], [50.05, true], [60, false]])
  );
  emit(
    "duplicate-timestamps",
    [["putBoolean", ENABLED, 0, false], ["putBoolean", ENABLED, 0, true], ["putBoolean", ENABLED, 30, false]],
    [["putBoolean", ENABLED, 4, false], ["putBoolean", ENABLED, 4, true], ["putBoolean", ENABLED, 34, false]]
  );
  emit(
    "utility-both-sides",
    timeline([[0, false], [10, true, false], [25, false]], ENABLED, AUTO, [
      ["putBoolean", TEST, 15, true],
      ["putBoolean", TEST, 22, false]
    ]),
    timeline([[3, false], [13, true, false], [28, false]], ENABLED, AUTO, [
      ["putBoolean", TEST, 18, true],
      ["putBoolean", TEST, 25, false]
    ])
  );
  emit(
    "mode-info-one-side",
    timeline([[0, false], [10, true, true], [30, true, false], [50, false]]),
    timeline([[4, false], [14, true], [44, false]], ENABLED, null)
  );
  emit(
    "unrelated-fields",
    [["putNumber", "a", 0, 1], ["putNumber", "b", 5, 2]],
    timeline([[2, false], [12, true], [22, false]])
  );
}

// Seeded random enable/auto/utility timelines, half of them shifted and jittered
// copies of the other side, covering the refinement branches the corpus misses.
function randomCases() {
  const KEYS = [
    ["/DriverStation/Enabled", "/DriverStation/Autonomous", "/DriverStation/Test"],
    ["DS:enabled", "DS:autonomous", "DS:test"],
    ["NT:/AdvantageKit/DriverStation/Enabled", "NT:/AdvantageKit/DriverStation/Autonomous", "NT:/AdvantageKit/DriverStation/Test"],
    ["NT:/DriverStation/ControlWord/enabled", "DS:controlWord/robotMode", "DS:controlWord/robotMode"]
  ];
  const MODES = ["autonomous", "teleop", "test", "disabled"];
  for (let i = 0; i < count; i++) {
    const r = new Rng(seed, `merge/${i}`);
    const [enabledKey, autoKey, testKey] = r.pick(KEYS);
    const stringModes = enabledKey.includes("ControlWord") && autoKey.includes("robotMode");
    const timelineOps = (r: Rng): { ops: unknown[][]; transitions: [number, number][] } => {
      const ops: unknown[][] = [["putNumber", "/Battery", 0, 12]];
      const transitions: [number, number][] = [];
      let t = r.unit() * 5;
      let enabled = r.below(4) !== 0 ? false : r.bool();
      ops.push([stringModes ? "putString" : "putBoolean", enabledKey, t, stringModes ? (enabled ? "teleop" : "disabled") : enabled]);
      const n = 1 + r.below(10);
      for (let k = 0; k < n; k++) {
        t += 0.05 + r.unit() * 40;
        enabled = r.bool();
        const auto = r.bool();
        const test = !auto && r.below(5) === 0;
        ops.push(["putBoolean", enabledKey, t, enabled]);
        if (stringModes) {
          ops.push(["putString", autoKey, t, enabled ? (auto ? "autonomous" : test ? "test" : "teleop") : r.pick(MODES)]);
        } else {
          if (r.below(5) > 0) ops.push(["putBoolean", autoKey, t, auto]);
          if (r.below(6) === 0) ops.push(["putBoolean", testKey, t, test]);
        }
        transitions.push([t, enabled ? 1 : 0]);
      }
      return { ops, transitions };
    };
    const shiftOps = (ops: unknown[][], shift: number, jitter: number): unknown[][] =>
      ops.map((op) => {
        const [name, key, t, v] = op as [string, string, number, unknown];
        const j = name.startsWith("put") && key !== "/Battery" ? (r.unit() * 2 - 1) * jitter : 0;
        return [name, key, t + shift + j, v];
      });
    const target = timelineOps(r);
    const source =
      r.below(3) === 0
        ? { ops: shiftOps(target.ops, r.unit() * 60 - 30, r.pick([0, 0.05, 0.15, 0.5, 1.2])), transitions: [] }
        : timelineOps(r);
    const cache = r.bool();
    const t = runWithLog(cache, target.ops).log.toSerialized();
    const s = runWithLog(cache, source.ops).log.toSerialized();
    out(`random/${i}`, t, s);
  }
}

async function main() {
  await corpusPairs();
  await publicPairs();
  scriptedCases();
  randomCases();
}

await main();
