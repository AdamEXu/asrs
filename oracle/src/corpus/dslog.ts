import { ByteSource, Bytes, hex, toBytes, utf8 } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";

/** LabVIEW epoch (1904-01-01) seconds for 2024-03-16 19:20:00 UTC, a plausible match start. */
export const LV_MATCH_START = 3793634400n;

export interface LvTime {
  seconds: bigint;
  fraction: bigint;
}

export function lvTime(seconds: bigint, fraction = 0n): LvTime {
  return { seconds, fraction };
}

function header(version: number, time: LvTime): Bytes {
  return new Bytes().be(version, 4).be(time.seconds, 8).be(time.fraction, 8);
}

export type PdBlock = { kind: "rev"; canId?: number; bits: Uint8Array; extra: number[]; temp?: number } | { kind: "ctre"; canId?: number; bits: Uint8Array; extra?: number[] } | { kind: "none"; typeByte?: number };

export interface DsRecord {
  tripHalfMs: number;
  packetLoss: number;
  batteryRaw: number;
  cpuHalfPercent: number;
  mask: number;
  canHalfPercent: number;
  wifiDbHalf: number;
  wifiMbRaw: number;
  pd: PdBlock;
}

export const REV_PD_TYPE = 33;
export const CTRE_PD_TYPE = 25;

/** Packs 20 ten-bit channel values the way DSLogReader unpacks REV PDH currents (3 per 32 bits, LSB first). */
export function revBits(channels: number[]): Uint8Array {
  const bits = new Array<number>(27 * 8).fill(0);
  channels.slice(0, 20).forEach((value, i) => {
    const at = Math.floor(i / 3) * 32 + (i % 3) * 10;
    for (let b = 0; b < 10; b++) bits[at + b] = (value >> b) & 1;
  });
  return packBits(bits);
}

/** Packs 16 channel values at CTRE PDP positions (6 per 64 bits, 10-bit stride); the reader keeps only 8 bits. */
export function ctreBits(channels: number[]): Uint8Array {
  const bits = new Array<number>(21 * 8).fill(0);
  channels.slice(0, 16).forEach((value, i) => {
    const at = Math.floor(i / 6) * 64 + (i % 6) * 10;
    for (let b = 0; b < 10; b++) bits[at + b] = (value >> b) & 1;
  });
  return packBits(bits);
}

function packBits(bits: number[]): Uint8Array {
  const out = new Uint8Array(bits.length / 8);
  bits.forEach((bit, i) => (out[i >> 3] |= bit << (i & 7)));
  return out;
}

export function dslogRecord(r: DsRecord): Uint8Array {
  const b = new Bytes()
    .u8(r.tripHalfMs)
    .u8(r.packetLoss)
    .be(r.batteryRaw, 2)
    .u8(r.cpuHalfPercent)
    .u8(r.mask)
    .u8(r.canHalfPercent)
    .u8(r.wifiDbHalf)
    .be(r.wifiMbRaw, 2);
  switch (r.pd.kind) {
    case "rev":
      b.raw([0, 0, 0, REV_PD_TYPE]).u8(r.pd.canId ?? 1).raw(r.pd.bits).raw(r.pd.extra).u8(r.pd.temp ?? 0);
      break;
    case "ctre":
      b.raw([0, 0, 0, CTRE_PD_TYPE]).u8(r.pd.canId ?? 0).raw(r.pd.bits).raw(r.pd.extra ?? [0, 0, 0]);
      break;
    case "none":
      b.raw([0, 0, 0, r.pd.typeByte ?? 0]);
      break;
  }
  return b.finish();
}

export function dslog(time: LvTime, records: DsRecord[], version = 4): Uint8Array {
  const b = header(version, time);
  records.forEach((r) => b.raw(dslogRecord(r)));
  return b.finish();
}

export interface DsEvent {
  time: LvTime;
  text: ByteSource;
  length?: number;
}

export function dsevents(time: LvTime, events: DsEvent[], version = 4): Uint8Array {
  const b = header(version, time);
  events.forEach((e) => {
    const text = toBytes(e.text);
    b.be(e.time.seconds, 8).be(e.time.fraction, 8).be(e.length ?? text.length, 4).raw(text);
  });
  return b.finish();
}

export function randomRecord(rng: Rng, kind?: PdBlock["kind"]): DsRecord {
  const pdKind = kind ?? rng.pick(["rev", "ctre", "none"] as const);
  const pd: PdBlock =
    pdKind === "rev"
      ? { kind: "rev", bits: rng.bytes(27), extra: Array.from(rng.bytes(4)), temp: rng.below(256) }
      : pdKind === "ctre"
        ? { kind: "ctre", bits: rng.bytes(21), extra: Array.from(rng.bytes(3)) }
        : { kind: "none" };
  return {
    tripHalfMs: rng.below(256),
    packetLoss: rng.below(256),
    batteryRaw: rng.below(8) === 0 ? rng.below(65536) : 2800 + rng.below(700),
    cpuHalfPercent: rng.below(256),
    mask: rng.below(256),
    canHalfPercent: rng.below(256),
    wifiDbHalf: rng.below(256),
    wifiMbRaw: rng.below(65536),
    pd
  };
}

function quietRecord(mask: number, pd: PdBlock = { kind: "none" }): DsRecord {
  return { tripHalfMs: 10, packetLoss: 0, batteryRaw: 12 * 256, cpuHalfPercent: 50, mask, canHalfPercent: 20, wifiDbHalf: 60, wifiMbRaw: 512, pd };
}

const MATCH_EVENTS = [
  "<TagVersion>1 <time> 00.000 <message> FMS Connected:   Qualification - 12:1, Field Time: 24/3/16 19:20:3\n -- FRC Driver Station - Version 24.0",
  "<TagVersion>1 <time> 02.519 <count> 1 <flags> 0 <Code> 44004 <details> FRC:  The Driver Station has lost communication with the robot.  <location> Driver Station <stack> ",
  "<TagVersion>1 <time> 03.104 <message> Info Joystick 0: (Controller (Xbox One For Windows)) has 6 axes, 16 buttons, 1 POVs. ",
  "<TagVersion>1 <time> 17.000 <count> 2 <flags> 1 <Code> 1 <details> Warning at frc.robot.Robot.robotPeriodic(Robot.java:40): Loop time of 0.02s overrun\n <location> frc.robot.Robot.robotPeriodic(Robot.java:40) <stack> \tat edu.wpi.first.wpilibj.IterativeRobotBase.printLoopOverrunMessage",
  "<TagVersion>1 <time> 150.250 <message> Warning 44007 FRC:  Time since robot boot. ",
  "<TagVersion>1 <time> 151.000 <message>  ﻿  whitespace that String.prototype.trim removes 　"
];

function pairCases(corpus: Corpus) {
  const add = (id: string, about: string, log: Uint8Array | null, events: Uint8Array | null) => {
    const files = [];
    if (log !== null) files.push({ path: `dslog/${id}.dslog`, bytes: log });
    if (events !== null) files.push({ path: `dslog/${id}.dsevents`, bytes: events });
    corpus.add(`dslog/${id}`, about, files);
  };
  const start = lvTime(LV_MATCH_START, 0x8000000000000000n);
  const rng = corpus.rng("dslog/match");
  const records: DsRecord[] = [];
  for (let i = 0; i < 160 * 50; i++) {
    const enabled = i >= 500 && i < 500 + 150 * 50;
    const auto = enabled && i < 500 + 15 * 50;
    let mask = 0xff;
    if (enabled) mask &= ~(1 << 3) & ~(1 << 0);
    if (auto) mask &= ~(1 << 1);
    else if (enabled) mask &= ~(1 << 5) & ~(1 << 2);
    const r = randomRecord(rng, "rev");
    r.mask = mask;
    r.batteryRaw = 3000 + rng.below(300);
    r.packetLoss = rng.below(4);
    records.push(r);
  }
  const events: DsEvent[] = MATCH_EVENTS.map((text, i) => ({ time: lvTime(LV_MATCH_START + BigInt(i * 30), BigInt(i) << 60n), text }));
  add("match-pair", "a 160 s match: REV PDH every cycle, auto then teleop status bits, with a matching .dsevents file", dslog(start, records), dsevents(start, events));
  add("log-only", "a .dslog with no .dsevents beside it", dslog(start, records.slice(0, 200)), null);
  add("events-only", "a .dsevents with no .dslog beside it", null, dsevents(start, events));
  const later = lvTime(LV_MATCH_START + 7n, 0x4000000000000000n);
  add("events-offset", "the .dsevents header time differs from the .dslog: event timestamps are relative to their own header", dslog(start, records.slice(0, 500)), dsevents(later, events));
}

function recordCases(corpus: Corpus) {
  const add = (id: string, about: string, bytes: Uint8Array) => corpus.add(`dslog/${id}`, about, [{ path: `dslog/${id}.dslog`, bytes }]);
  const t0 = lvTime(LV_MATCH_START);
  add("masks", "every status mask byte 0..255 once", dslog(t0, Array.from({ length: 256 }, (_, m) => quietRecord(m))));

  const battery = [5000, 20 * 256, 20 * 256 + 1, 3000, 65535, 0, 21 * 256, 12 * 256 + 128].map((raw) => ({ ...quietRecord(0xff), batteryRaw: raw }));
  add("battery-spikes", "battery voltage above 20 V (including the first record, before any good value) repeats the last accepted value", dslog(t0, battery));

  const loss = [0, 1, 25, 26, 127, 128, 200, 255].map((v) => ({ ...quietRecord(0xff), packetLoss: v }));
  add("packet-loss-clamp", "signed packet loss byte times 4 percent, clamped to 0..1", dslog(t0, loss));

  const extremes = [0, 255].map((v) => ({
    ...quietRecord(v),
    tripHalfMs: v,
    cpuHalfPercent: v,
    canHalfPercent: v,
    wifiDbHalf: v,
    wifiMbRaw: v * 257
  }));
  add("byte-extremes", "trip time, CPU, CAN, wifi fields at 0 and 255", dslog(t0, extremes));

  const revChannels = Array.from({ length: 20 }, (_, i) => (i * 97 + 3) & 0x3ff);
  const rev = [
    quietRecord(0xff, { kind: "rev", bits: revBits(revChannels), extra: [0, 16, 255, 128], temp: 40 }),
    quietRecord(0xff, { kind: "rev", bits: revBits(Array(20).fill(0x3ff)), extra: [255, 255, 255, 255] }),
    quietRecord(0xff, { kind: "rev", bits: new Uint8Array(27).fill(0xff), extra: [1, 2, 3, 4], canId: 63 })
  ];
  add("rev-pdh", "REV PDH currents: 20 ten-bit channels at the 32-bit stride, 4 byte channels over 16, filler bits set", dslog(t0, rev));

  const ctreChannels = Array.from({ length: 16 }, (_, i) => (i * 61 + 7) & 0x3ff);
  const ctre = [
    quietRecord(0xff, { kind: "ctre", bits: ctreBits(ctreChannels) }),
    quietRecord(0xff, { kind: "ctre", bits: ctreBits(Array(16).fill(0x3ff)), extra: [9, 9, 9] }),
    quietRecord(0xff, { kind: "ctre", bits: new Uint8Array(21).fill(0xff) })
  ];
  add("ctre-pdp", "CTRE PDP currents: 10-bit channels of which the reader keeps the low 8 bits", dslog(t0, ctre));

  const mixed = [0, 1, 25, 33, 34, 255].map((typeByte, i) =>
    typeByte === 33
      ? quietRecord(i, { kind: "rev", bits: revBits(revChannels), extra: [1, 2, 3, 4] })
      : typeByte === 25
        ? quietRecord(i, { kind: "ctre", bits: ctreBits(ctreChannels) })
        : quietRecord(i, { kind: "none", typeByte })
  );
  add("pd-mixed", "the PD type byte changes per record: none, REV, CTRE, unknown ids", dslog(t0, mixed));

  const rng = corpus.rng("dslog/drift");
  add(
    "drift",
    "15000 cycles: timestamps accumulate += 0.02 in f64",
    dslog(lvTime(LV_MATCH_START), Array.from({ length: 15000 }, () => randomRecord(rng, "none")))
  );

  const times: [string, LvTime][] = [
    ["lv-zero", lvTime(0n)],
    ["lv-fraction-max", lvTime(LV_MATCH_START, 0xffffffffffffffffn)],
    ["lv-negative", lvTime(-1n, 1n)],
    ["lv-huge", lvTime(0x7fffffffffffffffn, 0x8000000000000001n)]
  ];
  for (const [name, time] of times) add(`header/${name}`, `header LabVIEW time ${time.seconds} + ${time.fraction}/2^64`, dslog(time, [quietRecord(0xff)]));
  add("header/version-3", "version 3 is unsupported: the worker reports failure", dslog(t0, [quietRecord(0xff)], 3));
  add("header/only", "a header with no records: the first read throws, is caught, and nothing is logged", dslog(t0, []));
  add("header/short", "a 12-byte file: the worker never reads the header time, the first record read throws, is caught, and nothing is logged", hex("00 00 00 04 00 00 00 00 e2 1e 9f 60"));
}

function truncationCases(corpus: Corpus) {
  const t0 = lvTime(LV_MATCH_START);
  const shapes: [string, PdBlock][] = [
    ["rev", { kind: "rev", bits: revBits(Array(20).fill(5)), extra: [1, 2, 3, 4] }],
    ["ctre", { kind: "ctre", bits: ctreBits(Array(16).fill(5)) }],
    ["none", { kind: "none" }]
  ];
  for (const [name, pd] of shapes) {
    const full = dslog(t0, [quietRecord(0xfe, pd), quietRecord(0xfd, pd)]);
    const last = full.length - dslogRecord(quietRecord(0xfd, pd)).length;
    for (let at = last + 1; at < full.length; at++) {
      corpus.add(`dslog/truncate/${name}/at-${at}`, `two ${name} records cut at byte ${at} of ${full.length}`, [
        { path: `dslog/truncate/${name}/at-${at}.dslog`, bytes: full.slice(0, at) }
      ]);
    }
  }
}

function eventCases(corpus: Corpus) {
  const add = (id: string, about: string, bytes: Uint8Array) => corpus.add(`dslog/events/${id}`, about, [{ path: `dslog/events/${id}.dsevents`, bytes }]);
  const t0 = lvTime(LV_MATCH_START, 0x1000000000000000n);
  const at = (i: number) => lvTime(LV_MATCH_START + BigInt(i), BigInt(i) * 0x0123456789abcdefn);
  const texts: string[] = [
    "",
    "plain text with no tags",
    "<TagVersion>1 <time> 00.000 <message> Hello",
    "<message> twice <message> removed",
    "<details> only details",
    "<TagVersion>1 tail with no next tag",
    "<stack> ends here",
    "x<Code>",
    "<time><time><count>",
    "<Code> 1 <Code> 2 <message> nested",
    "<TagVersion>1 <time> 1 <count> 1 <flags> 0 <Code> 0 <location> here <stack> there <message> last",
    "<MESSAGE> case matters <Message> ",
    "  \t\n\r\v\f padded   ﻿  ",
    "unicode é中😀 <message> emoji",
    "<location><stack>"
  ];
  add("tags", "tag stripping, including a final tag with no following '<' (slice(-1) keeps the last character)", dsevents(t0, texts.map((text, i) => ({ time: at(i), text }))));

  add(
    "bytes",
    "invalid UTF-8, a BOM and a NUL inside event text",
    dsevents(t0, [
      { time: at(0), text: hex("c3 28 41") },
      { time: at(1), text: hex("ef bb bf 42") },
      { time: at(2), text: hex("41 00 42") },
      { time: at(3), text: hex("ed a0 80") }
    ])
  );
  add(
    "length-overrun",
    "an event length past the end of the file: the text is clamped and the loop ends",
    dsevents(t0, [
      { time: at(0), text: "first" },
      { time: at(1), text: "second", length: 1000 }
    ])
  );
  add("before-header", "an event earlier than the header time has a negative timestamp", dsevents(t0, [{ time: lvTime(LV_MATCH_START - 10n), text: "early" }]));
  add("header-only", "no events after the header: the first read throws inside the worker, which never responds", dsevents(t0, []));
  add("version-5", "version 5 is unsupported: the worker reports failure", dsevents(t0, [{ time: at(0), text: "x" }], 5));
  add(
    "truncated-entry",
    "the last event stops inside its time field: the read throws inside the worker",
    new Bytes().raw(dsevents(t0, [{ time: at(0), text: "ok" }])).raw(hex("00 00 00 00 e2")).finish()
  );
  const many: DsEvent[] = [];
  for (let i = 0; i < 300; i++) many.push({ time: at(i), text: utf8(`<TagVersion>1 <time> ${i}.000 <message> event ${i} `) });
  add("many", "300 events with fractional LabVIEW times", dsevents(t0, many));
}

export function dslogMatrix(corpus: Corpus): void {
  pairCases(corpus);
  recordCases(corpus);
  truncationCases(corpus);
  eventCases(corpus);
  for (let i = 0; i < 3; i++) {
    const id = `dslog/random/${i}`;
    const rng = corpus.rng(id);
    const t = lvTime(LV_MATCH_START + BigInt(rng.below(1000000)), rng.u64());
    const records = Array.from({ length: 100 + rng.below(400) }, () => randomRecord(rng));
    const events = Array.from({ length: 1 + rng.below(20) }, (_, j) => ({ time: lvTime(t.seconds + BigInt(j), rng.u64()), text: rng.ascii(40) }));
    corpus.add(id, "seeded random records with mixed PD types and random events", [
      { path: `${id}.dslog`, bytes: dslog(t, records) },
      { path: `${id}.dsevents`, bytes: dsevents(t, events) }
    ]);
  }
}
