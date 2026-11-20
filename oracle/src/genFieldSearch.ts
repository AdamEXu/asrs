import "./shims";
import Fuse from "fuse.js";
import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import { WPILOGDecoder } from "@upstream/hub/dataSources/wpilog/WPILOGDecoder";
import Log from "@upstream/shared/log/Log";
import LoggableType from "@upstream/shared/log/LoggableType";
import { searchFields } from "@upstream/shared/log/LogUtil";
import { numberToBits } from "./cve1";
import { Rng } from "./corpus/rng";

// Emits ground truth for LogUtil.searchFields (Fuse.js 7.0.0), unit L10.
//   genFieldSearch <queries per set> <seed> <public log name>...
//     For each key set (the named public WPILOGs, then one synthetic set):
//       {"case":"keys/<set>","in":[name...],"source":{file,sha256}|null,"keys":[key...]}
//     where "in" is the createBlankField order and "keys" is log.getFieldKeys(); then one line
//     per query:
//       {"case":"query/<set>/<n>","q":<query>,"r":[index into keys...],"s":[score bits...]}
//     with the scores of the same results from a Fuse built with searchFields's options plus
//     includeScore. Two small sets, "empty" and "blank" (only whitespace keys), come last.
//     A public set holds the entry names of the log's start records in file order (first
//     occurrence), read with upstream's WPILOGDecoder from the public-logs cache.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const SEARCH_OPTIONS = { findAllMatches: true, ignoreLocation: true };

const PROTO_KEYS = new Set(Object.getOwnPropertyNames(Object.prototype));

interface PublicLog {
  name: string;
  file: string;
  sha256: string;
  size: number;
  format: string;
}

function publicLogs(root: string): PublicLog[] {
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

function publicLogsDir(root: string): string {
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

function entryNames(data: Uint8Array): string[] {
  const names: string[] = [];
  const seen = new Set<string>();
  new WPILOGDecoder(data).forEach((record) => {
    if (!record.isStart()) return;
    const name = record.getStartData().name;
    if (seen.has(name)) return;
    seen.add(name);
    names.push(name);
  });
  return names;
}

const UNICODE = [
  "é", "É", "ß", "ẞ", "İ", "ı", "Σ", "σ", "ς", "Ω", "ﬁ", "Å", "Å", "é", "ǅ", "Ǆ",
  "日本", "測試", "ｆｕｌｌ", "Ａ", "😀", "🚀", "\ud83d", "\ude00", "\u0000", " ", " ",
  "﻿", "​", "Ⅷ", "ⓐ", "Ⓐ", "𐐀", "𐐨", "ꭰ"
];

function syntheticKeys(rng: Rng): string[] {
  const keys = [
    "", " ", "  ", "\t", " ", "﻿", "\n",
    "0", "1", "12", "01", "-1", "4294967294", "4294967295", "1.5", "007",
    "a", "A", "ab", "a b", "a  b", " a", "a ", "a\tb",
    "/RealOutputs/Drive/Pose", "/realoutputs/drive/pose", "RealOutputs/Drive/Pose",
    "NT:/SmartDashboard/Auto Chooser/selected", "NT:/SmartDashboard/Field/Robot",
    "DS:/Dscomm/Console/ConsoleLine", "DS:/Dscomm/Console/ErrorInfo",
    "Log0/Drive/Pose", "/Log12/Drive/Pose",
    "My Field With Many Words In It", "word word word word word word word word word",
    "x.y.z", "x:y:z", "x_y_z", "x-y-z", "x|y|z", "x\\y\\z", "x/y/z",
    "Ünïcödé/Kéy", "ΣΙΣΥΦΟΣ/Σ", "İstanbul/İ", "straße/STRASSE", "ﬁeld/ﬂow",
    "日本語/キー/値", "emoji/😀/🚀", "lone/\ud83d/high", "lone/\ude00/low", "nul/\u0000/x",
    "ét́é", "été", "ÉTÉ", "ǅungla", "𐐀𐐨/deseret",
    "[]{}()*+?.^$|", "^prefix", "=exact", "!inverse", "'include", "suffix$", "\"quoted\"",
    "a".repeat(31), "a".repeat(32), "a".repeat(33), "ab".repeat(40),
    "/Very/Long/Key/" + "Segment/".repeat(20) + "Leaf",
    "spaced key " + "token ".repeat(30) + "end"
  ];
  const segments = [
    "Drive", "Module0", "Module1", "Turn", "Velocity", "Position", "Gyro", "Yaw", "Vision",
    "Camera", "Pose", "Odometry", "Elevator", "Arm", "Intake", "Shooter", "Flywheel", "RPM",
    "Voltage", "Current", "Temp", "Connected", "LoggedRobot", "SystemStats", "PowerDistribution",
    "Channel", "Alerts", "errors", "warnings", "CAN", "Utilization", "Setpoint", "Measured",
    "real outputs", "Auto Chooser", "selected", "active", "time"
  ];
  const separators = ["/", "/", "/", ".", ":", "_", "-", " ", "\\", "|"];
  const roots = ["", "/", "NT:/", "DS:", "/AdvantageKit/", "Log3/", "/RealOutputs/", "/Tuning/"];
  while (keys.length < 420) {
    let key = rng.pick(roots);
    const depth = rng.range(1, 6);
    for (let d = 0; d < depth; d++) {
      if (d > 0) key += rng.pick(separators);
      let segment = rng.pick(segments);
      if (rng.below(6) === 0) segment += rng.pick(UNICODE);
      if (rng.below(8) === 0) segment = rng.pick(UNICODE) + segment;
      if (rng.below(10) === 0) segment = segment.toUpperCase();
      if (rng.below(10) === 0) segment += String(rng.below(100));
      key += segment;
    }
    keys.push(key);
  }
  return keys.filter((k) => !PROTO_KEYS.has(k));
}

const SPECIAL = [
  " ", "  ", "\t", "/", "//", ".", "*", ".*", "\\", "^", "$", "=", "!", "'", "\"", "|", "(", ")",
  "[", "]", "{", "}", "?", "+", "-", "_", ":", "%", "#", "@", "&", "~", "`", ",", ";", "<", ">"
];

function mutate(rng: Rng, text: string, edits: number): string {
  let s = text;
  for (let e = 0; e < edits; e++) {
    const i = s.length === 0 ? 0 : rng.below(s.length);
    switch (rng.below(4)) {
      case 0:
        s = s.slice(0, i) + s.slice(i + 1);
        break;
      case 1:
        s = s.slice(0, i) + String.fromCharCode(97 + rng.below(26)) + s.slice(i);
        break;
      case 2:
        s = s.slice(0, i) + String.fromCharCode(97 + rng.below(26)) + s.slice(i + 1);
        break;
      default:
        if (i + 1 < s.length) s = s.slice(0, i) + s[i + 1] + s[i] + s.slice(i + 2);
    }
  }
  return s;
}

function randomCase(rng: Rng, text: string): string {
  switch (rng.below(4)) {
    case 0:
      return text.toUpperCase();
    case 1:
      return text.toLowerCase();
    case 2:
      return Array.from(text, (c) => (rng.bool() ? c.toUpperCase() : c)).join("");
    default:
      return text;
  }
}

function substring(rng: Rng, text: string): string {
  if (text.length === 0) return text;
  const start = rng.below(text.length);
  return text.slice(start, start + 1 + rng.below(Math.min(40, text.length - start)));
}

function segmentOf(rng: Rng, key: string): string {
  const parts = key.split(/[/:.]/).filter((p) => p.length > 0);
  return parts.length === 0 ? key : rng.pick(parts);
}

function query(rng: Rng, keys: string[], n: number): string {
  const key = keys.length === 0 ? "drive" : rng.pick(keys);
  const fixed = [
    "", "a", "A", "/", " ", "x", "zzzz", "pose", "Pose", "drive pose", "ı", "İ", "σ", "ς", "😀",
    "\ud83d", "\ude00", "😀".repeat(17), "a".repeat(31) + "😀" + "b", "İ".repeat(20), "a".repeat(32), "a".repeat(33), "a".repeat(64), "a".repeat(65), "q".repeat(200)
  ];
  if (n < fixed.length) return fixed[n];
  switch (rng.below(14)) {
    case 0:
      return randomCase(rng, key);
    case 1:
      return key.slice(0, 1 + rng.below(Math.max(1, key.length)));
    case 2:
      return substring(rng, key);
    case 3:
      return randomCase(rng, segmentOf(rng, key));
    case 4:
      return mutate(rng, segmentOf(rng, key), rng.range(1, 3));
    case 5:
      return mutate(rng, substring(rng, key), rng.range(1, 4));
    case 6: {
      let s = segmentOf(rng, key);
      for (let k = rng.range(1, 3); k > 0; k--) {
        const i = rng.below(s.length + 1);
        s = s.slice(0, i) + rng.pick(UNICODE) + s.slice(i);
      }
      return s;
    }
    case 7: {
      let s = "";
      const target = rng.pick([33, 40, 63, 64, 65, 96, 97, 120, 200, 300]);
      while (s.length < target) s += (s.length > 0 ? rng.pick(["/", " ", ""]) : "") + rng.pick(keys.length ? keys : ["x"]);
      return rng.bool() ? s.slice(0, target) : mutate(rng, s.slice(0, target), rng.range(1, 6));
    }
    case 8: {
      let s = "";
      for (let k = rng.range(1, 4); k > 0; k--) s += rng.pick(SPECIAL);
      return rng.bool() ? s : s + segmentOf(rng, key);
    }
    case 9:
      return segmentOf(rng, key) + " " + segmentOf(rng, rng.pick(keys.length ? keys : ["x"]));
    case 10: {
      let s = "";
      for (let k = rng.range(1, 8); k > 0; k--) {
        s += key.length > 0 && rng.below(3) > 0 ? key[rng.below(key.length)] : String.fromCharCode(32 + rng.below(95));
      }
      return s;
    }
    case 11: {
      let s = "";
      for (let k = rng.range(1, 6); k > 0; k--) s += rng.pick(UNICODE);
      return s;
    }
    case 12:
      return mutate(rng, key, rng.range(1, 8));
    default: {
      const s = substring(rng, key);
      return rng.bool() ? " " + s + " " : s.toLowerCase().replace(/[aeiou]/g, "");
    }
  }
}

function emitSet(
  name: string,
  names: string[],
  source: object | null,
  count: number,
  seed: number
) {
  for (const n of names) {
    if (PROTO_KEYS.has(n)) throw new Error(`${name}: key ${JSON.stringify(n)} collides with Object.prototype`);
  }
  const log = new Log();
  for (const n of names) log.createBlankField(n, LoggableType.Raw);
  const keys = log.getFieldKeys();
  out({ case: `keys/${name}`, in: names, source, keys });
  const index = new Map(keys.map((k, i) => [k, i]));
  const rng = new Rng(seed, name);
  for (let n = 0; n < count; n++) {
    const q = query(rng, keys, n);
    const items = searchFields(log, q);
    const scored =
      q.length === 0 ? [] : new Fuse(keys, { ...SEARCH_OPTIONS, includeScore: true }).search(q).slice(0, items.length);
    if (scored.length !== items.length || scored.some((m, i) => m.item !== items[i])) {
      throw new Error(`${name}/${n}: scored search disagrees with searchFields`);
    }
    out({ case: `query/${name}/${n}`, q, r: items.map((k) => index.get(k)!), s: scored.map((m) => numberToBits(m.score!)) });
  }
}

const [countArg, seedArg, ...names] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);
if (!Number.isInteger(count) || !Number.isFinite(seed) || names.length === 0) {
  console.error("usage: genFieldSearch <queries per set> <seed> <public log name>...");
  process.exit(2);
}
const root = process.cwd();
const logs = publicLogs(root);
for (const name of names) {
  const log = logs.find((l) => l.name === name && l.format === "wpilog");
  if (log === undefined) throw new Error(`${name}: no such wpilog in public.toml`);
  const file = path.join(publicLogsDir(root), log.file);
  const data = new Uint8Array(fs.readFileSync(file));
  const sha256 = createHash("sha256").update(data).digest("hex");
  if (sha256 !== log.sha256 || data.length !== log.size) {
    throw new Error(`${file}: sha256 or size differs from public.toml`);
  }
  emitSet(name, entryNames(data), { file: log.file, sha256 }, count, seed);
}
emitSet("synthetic", syntheticKeys(new Rng(seed, "synthetic-keys")), null, count, seed);
emitSet("empty", [], null, Math.min(count, 40), seed);
emitSet("blank", ["", " ", "\t", "\n", "\u00a0", "\u3000", "\ufeff", "\u2028"], null, Math.min(count, 40), seed);
