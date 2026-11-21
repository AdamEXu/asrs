import { buildCanFramePayload, parseCanFrame } from "@upstream/hub/dataSources/schema/spark/can-spec-util";
import { sparkFramesSpec as spec2025 } from "@upstream/hub/dataSources/schema/spark/spark-frames-2025";
import { sparkFramesSpec as spec2026 } from "@upstream/hub/dataSources/schema/spark/spark-frames-2026";
import { Bytes, ByteSource, hex, toBytes } from "./bytes";
import { Corpus } from "./corpus";
import { quietly } from "./emit";
import { Rng } from "./rng";
import { WpilogBuilder } from "./wpilog";

const SECOND = 1000000;
const FIRMWARE_API = 0x98;
const MODEL_API = 0x300 | 155;
const LEGACY_PERIODIC_CLASS = 6;
const PERIODIC_CLASS = spec2026.periodicFrames.STATUS_0.apiClass;
export const SPARK_MAX = 0x2158;
export const SPARK_FLEX = 0x2159;

export const persistentId = (api: number, device: number) => ((api << 6) | device) & 0xffff;
export const periodicId = (apiClass: number, index: number, device: number) => ((apiClass << 10) | (index << 6) | device) & 0xffff;

export function persistentEntry(id: number, data: ByteSource): Uint8Array {
  const bytes = toBytes(data);
  const padded = new Uint8Array(6);
  padded.set(bytes.slice(0, 6));
  return new Bytes().le(id, 2).raw(padded).finish();
}

export function periodicEntry(ms: number, id: number, data: ByteSource): Uint8Array {
  const bytes = toBytes(data);
  const padded = new Uint8Array(8);
  padded.set(bytes.slice(0, 8));
  return new Bytes().le(ms, 4).le(id, 2).raw(padded).finish();
}

/** Legacy firmware frame: major, minor, big-endian build. */
export const legacyFirmware = (major: number, minor: number, build: number) => new Bytes().u8(major).u8(minor).be(build, 2).finish();
export const legacyModel = (model: number, status = 0) => new Bytes().le(model, 4).u8(0).u8(status).finish();

type Frame = (typeof spec2026.periodicFrames)[keyof typeof spec2026.periodicFrames];

/**
 * A STATUS frame payload made by decoding `seed` bytes with upstream's parseCanFrame and re-encoding the decoded
 * signals with buildCanFramePayload, so clamped signals come back in range; the seed bytes stand when encoding throws.
 */
export function statusFrame(frame: Frame, seed: Uint8Array): Uint8Array {
  return quietly(() => {
    try {
      const parsed = parseCanFrame(frame as any, { data: seed });
      return Uint8Array.from(buildCanFramePayload(frame as any, parsed as any));
    } catch {
      return seed;
    }
  });
}

const STATUS_2025 = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => (spec2025.periodicFrames as any)[`STATUS_${i}`] as Frame);
const STATUS_2026 = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => (spec2026.periodicFrames as any)[`STATUS_${i}`] as Frame);

function seedBytes(rng: Rng, n: number): Uint8Array {
  switch (n % 5) {
    case 0:
      return new Uint8Array(8);
    case 1:
      return new Uint8Array(8).fill(0xff);
    case 2:
      return hex("00 00 c0 7f 00 00 80 ff");
    default:
      return rng.bytes(8);
  }
}

class UrclLog {
  readonly b = new WpilogBuilder({ extra: "URCL corpus" });
  private nextId = 1;

  entry(name: string, type: string): number {
    const id = this.nextId++;
    this.b.start(0, id, name, type, "");
    return id;
  }

  put(id: number, us: number, data: ByteSource): this {
    this.b.record(id, us, toBytes(data));
    return this;
  }

  bytes(): Uint8Array {
    return this.b.bytes();
  }
}

function r3Cases(corpus: Corpus) {
  for (const root of ["NT:/URCL/", "URCL/"]) {
    const tag = root.startsWith("NT") ? "nt" : "direct";
    const rng = corpus.rng(`urcl/r3/${tag}`);
    const log = new UrclLog();
    const periodic = log.entry(root + "Raw/Periodic", "URCLr3_periodic");
    const aliases = log.entry(root + "Raw/Aliases", "URCLr3_aliases");
    const persistent = log.entry(root + "Raw/Persistent", "URCLr3_persistent");
    log.put(aliases, 1 * SECOND, '{"10":"Shooter","2":"Intake","name":"NotAnId","3":"Arm"}');
    const firmware = (dev: number, major: number) => persistentEntry(persistentId(FIRMWARE_API, dev), [major, 3, 0, 1, 0, 2]);
    log.put(persistent, 1 * SECOND, [
      ...firmware(2, 25),
      ...firmware(3, 26),
      ...firmware(10, 26),
      ...firmware(11, 24),
      ...firmware(12, 27),
      ...persistentEntry(persistentId(0x123, 4), [1, 2, 3])
    ]);
    for (let cycle = 0; cycle < 40; cycle++) {
      const us = 2 * SECOND + cycle * 20000;
      const chunks: Uint8Array[] = [];
      for (const dev of [2, 3, 10, 11, 12, 13]) {
        const specs = dev === 2 ? STATUS_2025 : STATUS_2026;
        for (let index = 0; index < 8; index++) {
          if ((cycle + index + dev) % 3 === 0) continue;
          chunks.push(periodicEntry(2000 + cycle * 20 + index, periodicId(PERIODIC_CLASS, index, dev), statusFrame(specs[index], seedBytes(rng, cycle + index))));
        }
      }
      if (cycle % 10 === 0) {
        chunks.push(periodicEntry(2000 + cycle * 20, periodicId(PERIODIC_CLASS, 8, 3), rng.bytes(8)));
        chunks.push(periodicEntry(2000 + cycle * 20, periodicId(PERIODIC_CLASS, 15, 3), rng.bytes(8)));
        chunks.push(periodicEntry(2000 + cycle * 20, periodicId(47, 0, 3), rng.bytes(8)));
      }
      log.put(periodic, us, chunks.flatMap((c) => [...c]));
      if (cycle === 20) log.put(aliases, us, '{"3":"ArmRenamed"}');
    }
    corpus.add(
      `urcl/r3/${tag}`,
      `URCL r3 under ${root}: firmware 24 to 27, 2025 and 2026 specs, aliases with integer-like keys renamed mid-log, unknown device, frames 8 and 15, another API class`,
      [{ path: `urcl/r3/${tag}.wpilog`, bytes: log.bytes() }]
    );
  }

  const edges = new UrclLog();
  const periodic = edges.entry("URCL/Raw/Periodic", "URCLr3_periodic");
  const aliases = edges.entry("URCL/Raw/Aliases", "URCLr3_aliases");
  const persistent = edges.entry("URCL/Raw/Persistent", "URCLr3_persistent");
  const frame0 = (applied: number[], dev = 1) => periodicEntry(10, periodicId(PERIODIC_CLASS, 0, dev), applied);
  edges.put(periodic, 1 * SECOND, frame0([0, 0, 0, 0, 0, 0, 0, 0]));
  edges.put(persistent, 2 * SECOND, persistentEntry(persistentId(FIRMWARE_API, 1), [26]));
  edges.put(periodic, 3 * SECOND, frame0([1, 2, 3, 4, 5, 6, 7, 8]));
  edges.put(aliases, 4 * SECOND, "not json");
  edges.put(periodic, 5 * SECOND, frame0([1, 2, 3, 4, 5, 6, 7, 8]));
  edges.put(aliases, 6 * SECOND, '{"1":""}');
  edges.put(periodic, 7 * SECOND, concatAll([frame0([9, 9, 9, 9, 9, 9, 9, 9]), hex("01 02 03")]));
  edges.put(periodic, 8 * SECOND, new Uint8Array());
  edges.put(persistent, 9 * SECOND, concatAll([persistentEntry(persistentId(FIRMWARE_API, 1), [26]), hex("98 26 1a")]));
  edges.put(periodic, 10 * SECOND, frame0([1, 1, 1, 1, 1, 1, 1, 1]));
  corpus.add(
    "urcl/r3/edges",
    "periodic data before any persistent record, invalid alias JSON, an empty alias, trailing partial entries in periodic and persistent values, an empty periodic value",
    [{ path: "urcl/r3/edges.wpilog", bytes: edges.bytes() }]
  );

  const derived = new UrclLog();
  const p = derived.entry("URCL/Raw/Periodic", "URCLr3_periodic");
  const ps = derived.entry("URCL/Raw/Persistent", "URCLr3_persistent");
  derived.put(ps, 0, persistentEntry(persistentId(FIRMWARE_API, 7), [26]));
  const rng = corpus.rng("urcl/r3/derived");
  for (let i = 0; i < 25; i++) {
    const s0 = statusFrame(STATUS_2026[0], rng.bytes(8));
    const s1 = statusFrame(STATUS_2026[1], i % 5 === 0 ? new Uint8Array(8) : rng.bytes(8));
    derived.put(p, SECOND + i * 20000, concatAll([periodicEntry(4000000000 + i, periodicId(PERIODIC_CLASS, 1, 7), s1), periodicEntry(i, periodicId(PERIODIC_CLASS, 0, 7), s0)]));
  }
  corpus.add(
    "urcl/r3/derived",
    "AppliedOutputVoltage and CurrentInput use the outer record timestamp while signals use u32 millisecond frame timestamps (some past 2^31)",
    [{ path: "urcl/r3/derived.wpilog", bytes: derived.bytes() }]
  );
}

function concatAll(parts: Uint8Array[]): Uint8Array {
  return Uint8Array.from(parts.flatMap((p) => [...p]));
}

function legacyPeriodicFrames(rng: Rng, cycle: number, dev: number): Uint8Array[] {
  const frames: Uint8Array[] = [];
  const order = cycle % 2 === 0 ? [1, 0, 2, 3, 4, 5, 6, 7] : [0, 1, 2, 3, 4, 5, 6, 7];
  for (const index of order) {
    let data: Uint8Array;
    switch (index) {
      case 0:
        data = new Bytes().le(rng.range(-32768, 32767), 2).le(rng.u32(), 2).le(rng.u32(), 2).u8(rng.below(256)).u8(0).finish();
        break;
      case 1:
        data = new Bytes().f32(rng.double()).u8(cycle % 4 === 0 ? 0 : rng.below(256)).raw(rng.bytes(3)).finish();
        break;
      case 2:
        data = new Bytes().f32(cycle % 3 === 0 ? -2.75 : rng.double()).raw(rng.bytes(4)).finish();
        break;
      default:
        data = rng.bytes(8);
    }
    frames.push(periodicEntry(1000 + cycle * 20 + index, periodicId(LEGACY_PERIODIC_CLASS, index, dev), data));
  }
  if (cycle % 7 === 0) frames.push(periodicEntry(1000 + cycle * 20, periodicId(LEGACY_PERIODIC_CLASS, 9, dev), rng.bytes(8)));
  return frames;
}

function legacyPersistent(): Uint8Array[] {
  return [
    persistentEntry(persistentId(FIRMWARE_API, 1), legacyFirmware(23, 1, 0x0102)),
    persistentEntry(persistentId(MODEL_API, 1), legacyModel(SPARK_MAX)),
    persistentEntry(persistentId(FIRMWARE_API, 2), legacyFirmware(24, 0, 0xfffe)),
    persistentEntry(persistentId(MODEL_API, 2), legacyModel(SPARK_FLEX)),
    persistentEntry(persistentId(FIRMWARE_API, 3), legacyFirmware(25, 2, 7)),
    persistentEntry(persistentId(MODEL_API, 3), legacyModel(SPARK_FLEX, 1)),
    persistentEntry(persistentId(FIRMWARE_API, 4), legacyFirmware(25, 0, 1)),
    persistentEntry(persistentId(MODEL_API, 4), legacyModel(0x1234)),
    persistentEntry(persistentId(FIRMWARE_API, 5), legacyFirmware(25, 0, 1)),
    persistentEntry(persistentId(MODEL_API, 6), legacyModel(SPARK_MAX))
  ];
}

function legacyCases(corpus: Corpus) {
  const devices = [1, 2, 3, 4, 5, 6, 9];
  {
    const rng = corpus.rng("urcl/r2");
    const log = new UrclLog();
    const periodic = log.entry("NT:/URCL/Raw/Periodic", "URCLr2_periodic");
    const aliases = log.entry("NT:/URCL/Raw/Aliases", "URCLr2_aliases");
    const persistent = log.entry("NT:/URCL/Raw/Persistent", "URCLr2_persistent");
    log.put(aliases, 0, '{"2":"Flywheel","1":"Turret"}');
    log.put(persistent, 0, concatAll(legacyPersistent()));
    for (let cycle = 0; cycle < 30; cycle++) {
      log.put(periodic, SECOND + cycle * 20000, concatAll(devices.flatMap((dev) => legacyPeriodicFrames(rng, cycle, dev))));
    }
    corpus.add(
      "urcl/r2/periodic",
      "URCL r2: Spark Max and Flex, firmware 23 to 25 (IAccum and Flex position fraction branches), failed and unknown model reads, devices missing firmware or model, frame 1 before and after frame 0",
      [{ path: "urcl/r2/periodic.wpilog", bytes: log.bytes() }]
    );
  }
  {
    const rng = corpus.rng("urcl/r1");
    const log = new UrclLog();
    const urcl = log.entry("/URCL", "URCL");
    for (let cycle = 0; cycle < 30; cycle++) {
      const persistent = legacyPersistent();
      const periodic = devices.flatMap((dev) => legacyPeriodicFrames(rng, cycle, dev));
      const value = new Bytes().le(persistent.length, 4).le(periodic.length, 4);
      persistent.forEach((p) => value.raw(p));
      periodic.forEach((p) => value.raw(p));
      log.put(urcl, SECOND + cycle * 20000, value.finish());
    }
    const short = new Bytes().le(1, 4).le(5, 4).raw(legacyPersistent()[0]).raw(periodicEntry(5, periodicId(6, 0, 1), [1, 2])).finish();
    log.put(urcl, 2 * SECOND, short);
    log.put(urcl, 3 * SECOND, hex("00 00 00"));
    corpus.add("urcl/r1/single-field", "URCL r1: persistent and periodic frames in one value, then counts that overrun the value and a value shorter than the header", [
      { path: "urcl/r1/single-field.wpilog", bytes: log.bytes() }
    ]);
  }
  {
    const rng = corpus.rng("urcl/r1/empty-key");
    const log = new UrclLog();
    const entries = [log.entry("", "URCL"), log.entry("/URCL", "URCL"), log.entry("Arm", "URCL")];
    for (let cycle = 0; cycle < 3; cycle++) {
      const persistent = legacyPersistent();
      const periodic = devices.flatMap((dev) => legacyPeriodicFrames(rng, cycle, dev));
      const value = new Bytes().le(persistent.length, 4).le(periodic.length, 4);
      persistent.forEach((p) => value.raw(p));
      periodic.forEach((p) => value.raw(p));
      for (const entry of entries) log.put(entry, SECOND + cycle * 20000, value.finish());
    }
    corpus.add(
      "urcl/r1/empty-key",
      "URCL r1 under the empty key, whose generated children start with \"/\" and so make every \"/\" key generated, beside \"/URCL\" and \"Arm\"",
      [{ path: "urcl/r1/empty-key.wpilog", bytes: log.bytes() }]
    );
  }
}

export function urclMatrix(corpus: Corpus): void {
  r3Cases(corpus);
  legacyCases(corpus);
  for (const [year, specs] of [
    ["2025", STATUS_2025],
    ["2026", STATUS_2026]
  ] as const) {
    const rng = corpus.rng(`urcl/frames/${year}`);
    const log = new UrclLog();
    const periodic = log.entry("URCL/Raw/Periodic", "URCLr3_periodic");
    const persistent = log.entry("URCL/Raw/Persistent", "URCLr3_persistent");
    log.put(persistent, 0, persistentEntry(persistentId(FIRMWARE_API, 20), [year === "2025" ? 25 : 26]));
    for (let i = 0; i < 200; i++) {
      const index = i % 8;
      log.put(periodic, SECOND + i * 1000, periodicEntry(i, periodicId(PERIODIC_CLASS, index, 20), statusFrame(specs[index], seedBytes(rng, i >> 3))));
    }
    corpus.add(`urcl/frames/${year}`, `200 STATUS_0..7 frames built with upstream's buildCanFramePayload for the ${year} spec`, [
      { path: `urcl/frames/${year}.wpilog`, bytes: log.bytes() }
    ]);
  }
}
