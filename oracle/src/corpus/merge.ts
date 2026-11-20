import { Corpus } from "./corpus";
import { dslog, lvTime, LV_MATCH_START, randomRecord } from "./dslog";
import { Rng } from "./rng";
import { WpilogBuilder, payload } from "./wpilog";

const SECOND = 1000000;

/** One robot-state timeline: [seconds, enabled, autonomous, test] transitions. */
type Timeline = [number, boolean, boolean, boolean?][];

interface Side {
  timeline: Timeline;
  span: [number, number];
  shift?: number;
  keys?: "akit" | "akit-nt" | "akit-test" | "datalog" | "fms" | "robot-mode" | "none";
  modeInfo?: boolean;
}

function wpilogSide(side: Side): Uint8Array {
  const shift = side.shift ?? 0;
  const us = (s: number) => Math.round((s + shift) * SECOND);
  const b = new WpilogBuilder({ extra: "merge corpus" });
  const keys = side.keys ?? "akit";
  const [enabledKey, autoKey] =
    keys === "akit-nt"
      ? ["NT:/AdvantageKit/DriverStation/Enabled", "NT:/AdvantageKit/DriverStation/Autonomous"]
      : keys === "datalog"
        ? ["DS:enabled", "DS:autonomous"]
        : ["/DriverStation/Enabled", "/DriverStation/Autonomous"];
  b.start(us(side.span[0]), 1, "/Battery", "double", "");
  b.record(1, us(side.span[0]), payload.double(12.5));
  b.record(1, us(side.span[1]), payload.double(12.1));
  if (keys === "fms") {
    b.start(us(side.span[0]), 2, "NT:/FMSInfo/FMSControlData", "int64", "");
    for (const [t, enabled, auto, test] of side.timeline) {
      b.record(2, us(t), payload.int64((enabled ? 1 : 0) | (auto ? 2 : 0) | (test ? 4 : 0) | 0x20));
    }
  } else if (keys === "robot-mode") {
    b.start(us(side.span[0]), 2, "/DriverStation/Enabled", "boolean", "");
    b.start(us(side.span[0]), 3, "/DriverStation/RobotMode", "string", "");
    for (const [t, enabled, auto, test] of side.timeline) {
      b.record(2, us(t), payload.boolean(enabled));
      b.record(3, us(t), payload.string(auto ? "Autonomous" : test ? "TEST" : "teleop"));
    }
  } else if (keys !== "none") {
    b.start(us(side.span[0]), 2, enabledKey, "boolean", "");
    if (side.modeInfo !== false) b.start(us(side.span[0]), 3, autoKey, "boolean", "");
    if (keys === "akit-test") b.start(us(side.span[0]), 4, "/DriverStation/Test", "boolean", "");
    for (const [t, enabled, auto, test] of side.timeline) {
      b.record(2, us(t), payload.boolean(enabled));
      if (side.modeInfo !== false) b.record(3, us(t), payload.boolean(auto));
      if (keys === "akit-test") b.record(4, us(t), payload.boolean(test === true));
    }
  }
  return b.bytes();
}

/** A DS log whose status bits follow the timeline, one record per 20 ms from 0 to `span[1]`. */
function dslogSide(side: Side): Uint8Array {
  const records = [];
  const rng = new Rng(1, "merge-dslog");
  for (let i = 0; i * 0.02 <= side.span[1]; i++) {
    const t = i * 0.02;
    const state = side.timeline.filter(([at]) => at <= t).pop() ?? [0, false, false];
    let mask = 0xff;
    if (state[1]) mask &= ~(1 << 3) & ~1;
    if (state[1] && state[2]) mask &= ~(1 << 1);
    if (state[1] && !state[2]) mask &= ~(1 << 5) & ~(1 << 2);
    const r = randomRecord(rng, "none");
    r.mask = mask;
    r.batteryRaw = 3000;
    records.push(r);
  }
  return dslog(lvTime(LV_MATCH_START), records);
}

const MATCH: Timeline = [
  [10, false, false],
  [20, true, true],
  [35, false, true],
  [38, true, false],
  [173, false, false]
];

const MATCH_WITH_TEST: Timeline = [
  [10, false, false, false],
  [20, true, true, false],
  [35, false, true, false],
  [38, true, false, false],
  [100, true, false, true],
  [120, false, false, false],
  [125, true, false, false],
  [173, false, false, false]
];

function shifted(timeline: Timeline, by: number): Timeline {
  return timeline.map(([t, ...state]) => [t + by, ...state]);
}

function jittered(timeline: Timeline, rng: Rng, amount: number): Timeline {
  return timeline.map(([t, ...state]) => [t + (rng.unit() * 2 - 1) * amount, ...state]);
}

export function mergeMatrix(corpus: Corpus): void {
  const pair = (id: string, about: string, target: Side, source: Side, sourceFormat: "wpilog" | "dslog" = "wpilog") =>
    corpus.add(`merge/${id}`, about, [
      { path: `merge/${id}/target.wpilog`, bytes: wpilogSide(target) },
      { path: `merge/${id}/source.${sourceFormat}`, bytes: sourceFormat === "wpilog" ? wpilogSide(source) : dslogSide(source) }
    ]);
  const span: [number, number] = [0, 180];

  pair("no-state", "neither log has an enabled key: offset falls back to the difference of range starts", { timeline: [], span, keys: "none" }, { timeline: [], span: [5, 100], keys: "none" });
  pair("never-enabled", "the source is never enabled: fallback offset", { timeline: MATCH, span }, { timeline: [[0, false, false]], span: [3, 90] });
  pair("shifted", "the same match shifted by 12.345 s", { timeline: MATCH, span }, { timeline: shifted(MATCH, -12.345), span: [0, 170] });
  pair("shifted-epoch", "a source on an epoch time base, about 1.7e9 s away", { timeline: MATCH, span }, { timeline: MATCH, span, shift: 1710616800.125 });
  pair(
    "jittered",
    "both logs see the transitions with up to 60 ms disagreement: consensus cluster refinement",
    { timeline: jittered(MATCH, corpus.rng("merge/jitter-a"), 0.06), span },
    { timeline: jittered(shifted(MATCH, 4), corpus.rng("merge/jitter-b"), 0.06), span: [0, 185] }
  );
  pair(
    "outliers",
    "one transition far off the others: shortest half and adaptive inlier radius",
    { timeline: MATCH, span },
    { timeline: shifted(MATCH, 2).map(([t, e, a], i) => [i === 2 ? t + 0.7 : i === 3 ? t - 0.3 : t, e, a]), span }
  );
  pair("one-transition", "a single rising edge on each side", { timeline: [[0, false, false], [30, true, false]], span: [0, 60] }, { timeline: [[0, false, false], [21.5, true, false]], span: [0, 60] });
  pair(
    "two-edges-wide",
    "two matched edges disagree by 0.4 s: the rising edge wins",
    { timeline: [[0, false, false], [20, true, false], [40, false, false]], span: [0, 60] },
    { timeline: [[0, false, false], [15, true, false], [35.4, false, false]], span: [0, 60] }
  );
  pair(
    "two-edges-narrow",
    "two matched edges disagree by 0.2 s: their mean is used",
    { timeline: [[0, false, false], [20, true, false], [40, false, false]], span: [0, 60] },
    { timeline: [[0, false, false], [15, true, false], [35.2, false, false]], span: [0, 60] }
  );
  pair(
    "mode-mismatch",
    "both logs have mode info but disagree about auto vs teleop",
    { timeline: MATCH, span },
    { timeline: shifted(MATCH, 1).map(([t, e, a]) => [t, e, !a]), span }
  );
  pair("one-mode-info", "only the target has an autonomous key", { timeline: MATCH, span }, { timeline: shifted(MATCH, -3), span, modeInfo: false });
  pair(
    "starts-enabled",
    "the source starts already enabled less than 5 s into its range",
    { timeline: MATCH, span },
    { timeline: [[2, true, false], [130, false, false]], span: [0, 150] }
  );
  pair(
    "starts-enabled-late",
    "the source's first sample is enabled more than 5 s into its range: a disabled range is prepended",
    { timeline: MATCH, span },
    { timeline: [[8, true, false], [140, false, false]], span: [0, 150] }
  );
  pair("no-overlap", "a target teleop pulse against a source in auto for its whole range: no transition pairs, only interval-boundary candidates, scored as a mode mismatch", { timeline: [[0, false, false], [1, true, false], [2, false, false]], span: [0, 3] }, { timeline: [[0, true, true]], span: [0, 1] });
  pair("ties", "two identical enable pulses: equal scores tie-break toward rising edges", { timeline: [[0, false, false], [10, true, false], [20, false, false], [30, true, false], [40, false, false]], span: [0, 50] }, { timeline: [[0, false, false], [5, true, false], [15, false, false]], span: [0, 20] });
  pair("nt-keys", "NT-logged AdvantageKit keys on the source", { timeline: MATCH, span }, { timeline: shifted(MATCH, 6), span, keys: "akit-nt" });
  pair("datalog-keys", "plain WPILib DataLog DS: keys on the source", { timeline: MATCH, span }, { timeline: shifted(MATCH, -7.5), span, keys: "datalog" });
  pair(
    "fms-control-word",
    "an NT:/FMSInfo/FMSControlData int64 control word (enabled, auto and test bits) is the source's only state key",
    { timeline: MATCH, span },
    { timeline: shifted(MATCH_WITH_TEST, 3), span, keys: "fms" }
  );
  pair(
    "robot-mode",
    "post-2027 /DriverStation/RobotMode strings in mixed case (Autonomous, TEST, teleop) on both sides, with a test-mode period",
    { timeline: MATCH_WITH_TEST, span, keys: "robot-mode" },
    { timeline: shifted(MATCH_WITH_TEST, -5.5), span, keys: "robot-mode" }
  );
  pair(
    "test-mode",
    "pre-2027 /DriverStation/Test on both sides: teleop-to-test transitions pair only with each other",
    { timeline: MATCH_WITH_TEST, span, keys: "akit-test" },
    { timeline: shifted(MATCH_WITH_TEST, 9.25), span, keys: "akit-test" }
  );
  pair("dslog-source", "a Driver Station log merged into an AdvantageKit log", { timeline: MATCH, span }, { timeline: shifted(MATCH, -8), span: [0, 175] }, "dslog");

  for (let i = 0; i < 6; i++) {
    const rng = corpus.rng(`merge/random-${i}`);
    const make = (): Timeline => {
      const out: Timeline = [[0, false, false]];
      let t = rng.unit() * 10;
      for (let k = 0; k < 2 + rng.below(8); k++) {
        t += 0.05 + rng.unit() * 30;
        out.push([t, rng.bool(), rng.bool()]);
      }
      return out;
    };
    const target = make();
    const source = rng.bool() ? jittered(shifted(target, rng.unit() * 40 - 20), rng, 0.2) : make();
    pair(`random-${i}`, "seeded random enable timelines, half of them a jittered shift of the target", { timeline: target, span: [0, 200] }, { timeline: source, span: [0, 220] });
  }
}
