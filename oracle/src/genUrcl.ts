import "./shims";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import CustomSchemas from "@upstream/hub/dataSources/schema/CustomSchemas";
import { WPILOGDecoder, WPILOGDecoderRecord } from "@upstream/hub/dataSources/wpilog/WPILOGDecoder";
import { CONTROL_ENTRY } from "@upstream/hub/dataSources/wpilog/WPILOGShared";
import Log from "@upstream/shared/log/Log";
import LogField from "@upstream/shared/log/LogField";
import LoggableType from "@upstream/shared/log/LoggableType";
import { sparkFramesSpec as spec2025 } from "@upstream/hub/dataSources/schema/spark/spark-frames-2025";
import { sparkFramesSpec as spec2026 } from "@upstream/hub/dataSources/schema/spark/spark-frames-2026";
import { getStructuredTypeFromRaw } from "@upstream/shared/log/LogKeyUtils";
import { AUTONOMOUS_KEYS, ENABLED_KEYS, PROGRAM_START_TIME_KEYS, UTILITY_KEYS } from "@upstream/shared/log/RobotState";
import { Cve1, bitsToNumber, encode, fromTree, numberToBits, toTree } from "./cve1";
import { HistoricalSession } from "./historical/session";
import { dump } from "./logRun";
import { UPSTREAM_DIR } from "./shims";
import { ByteSource, Bytes, concat, hex, toBytes, utf8 } from "./corpus/bytes";
import {
  SPARK_FLEX,
  SPARK_MAX,
  legacyFirmware,
  legacyModel,
  periodicEntry,
  periodicId,
  persistentEntry,
  persistentId,
  statusFrame
} from "./corpus/urcl";
import { WpilogBuilder } from "./corpus/wpilog";

// Emits replay vectors for the URCL custom schemas (URCLSchema.ts, URCLSchemaLegacy.ts).
//
//   genUrcl <corpus-dir>
//     meta line, then two lines per case:
//       {"t":"case","id":<case-id>,"ops":[[op,...]...],"requests":[key,...],
//        "types":{key:type},"records":{key:[<record>...]}}
//       {"t":"dump","log":<logRun dump>}
//     `ops` mirrors the worker's index-time field calls: ["range",min,max] (as
//     updateRangeWithTimestamp), ["field",name,type,metadata,structuredType|null],
//     ["meta",name,metadata], ["del",name]. Each record is {"t":<ts bits>,"v":<cve1>}
//     where `v` is what the worker's type switch decodes (raw bytes for the custom
//     schema branch), {"t":...,"throw":<cve1>} when the getter threw, or {"null":true}
//     when getRecordAtPosition returned null. `requests` lists the parseField keys the real
//     HistoricalDataSource posted to its worker, in order.
//
//     Every case, corpus or generated here, is a WPILOG file opened in a real
//     HistoricalSession, whose hub Log is the dump. The replay of the extracted ops,
//     records and requests must produce the same dump, or generation fails.

interface IndexerModule {
  _malloc(size: number): number;
  _run(buffer: number, bufferSize: number): number;
  HEAPF64: Float64Array;
  HEAPU8: Uint8Array;
  HEAPU32: Uint32Array;
}

let indexerModule: IndexerModule | null = null;

async function indexer(): Promise<IndexerModule> {
  if (indexerModule === null) {
    const dir = path.join(UPSTREAM_DIR, "bundles");
    const { default: init } = await import(pathToFileURL(path.join(dir, "wpilogIndexer.js")).href);
    indexerModule = (await init({
      wasmBinary: fs.readFileSync(path.join(dir, "wpilogIndexer.wasm"))
    })) as IndexerModule;
  }
  return indexerModule;
}

interface Indexed {
  min: number;
  max: number;
  records: [number, number][];
}

async function runIndexer(data: Uint8Array): Promise<Indexed | { failed: true }> {
  const Module = await indexer();
  const bufferIn = Module._malloc(data.length);
  Module.HEAPU8.set(data, bufferIn);
  try {
    const bufferOut = Module._run(bufferIn, data.length);
    const min = Module.HEAPF64[bufferOut / 8];
    const max = Module.HEAPF64[bufferOut / 8 + 1];
    const recordCount = Module.HEAPU32[bufferOut / 4 + 4];
    const records: [number, number][] = [];
    for (let i = 0; i < recordCount; i++) {
      records.push([Module.HEAPU32[bufferOut / 4 + 5 + i * 2], Module.HEAPU32[bufferOut / 4 + 5 + i * 2 + 1]]);
    }
    return { min, max, records };
  } catch {
    return { failed: true };
  }
}

const BLANK_TYPE: { [type: string]: LoggableType } = {
  boolean: LoggableType.Boolean,
  int: LoggableType.Number,
  int64: LoggableType.Number,
  float: LoggableType.Number,
  double: LoggableType.Number,
  string: LoggableType.String,
  json: LoggableType.String,
  "boolean[]": LoggableType.BooleanArray,
  "int[]": LoggableType.NumberArray,
  "int64[]": LoggableType.NumberArray,
  "float[]": LoggableType.NumberArray,
  "double[]": LoggableType.NumberArray,
  "string[]": LoggableType.StringArray
};

/** The value `parseField`'s type switch decodes out of `record`. */
function decodedValue(type: string, record: WPILOGDecoderRecord): unknown {
  switch (type) {
    case "boolean":
      return record.getBoolean();
    case "int":
    case "int64":
      return record.getInteger();
    case "float":
      return record.getFloat();
    case "double":
      return record.getDouble();
    case "string":
    case "json":
      return record.getString();
    case "boolean[]":
      return record.getBooleanArray();
    case "int[]":
    case "int64[]":
      return record.getIntegerArray();
    case "float[]":
      return record.getFloatArray();
    case "double[]":
      return record.getDoubleArray();
    case "string[]":
      return record.getStringArray();
    default:
      return record.getRaw();
  }
}

interface Extracted {
  failed: boolean;
  ops: unknown[][];
  types: { [key: string]: string };
  records: { [key: string]: object[] };
}

/** The bookkeeping wpilogWorker.ts performs while indexing, resolved into ops and per-key record lists. */
async function extract(data: Uint8Array): Promise<Extracted> {
  const indexed = await runIndexer(data);
  if ("failed" in indexed) return { failed: true, ops: [], types: {}, records: {} };
  const decoder = new WPILOGDecoder(data);
  const entryIds: { [id: number]: string } = {};
  const entryTypes: { [name: string]: string } = {};
  const entryStartTimes: { [id: number]: number } = {};
  const dataRecordPositions: { [name: string]: number[] } = {};
  const ops: unknown[][] = [
    ["range", numberToBits(indexed.min), numberToBits(indexed.max)]
  ];
  for (const [entry, position] of indexed.records) {
    try {
      if (entry === CONTROL_ENTRY) {
        const record = decoder.getRecordAtPosition(position)[0];
        if (record === null || record === undefined) continue;
        if (record.isStart()) {
          const startData = record.getStartData();
          if (!(startData.name in dataRecordPositions) || entryTypes[startData.name] !== startData.type) {
            dataRecordPositions[startData.name] = [];
          }
          entryIds[startData.entry] = startData.name;
          entryTypes[startData.name] = startData.type;
          entryStartTimes[startData.entry] = record.getTimestamp();
          ops.push([
            "field",
            startData.name,
            BLANK_TYPE[startData.type] ?? LoggableType.Raw,
            startData.type,
            startData.metadata,
            getStructuredTypeFromRaw(startData.type)
          ]);
        } else if (record.isFinish()) {
          const finishEntry = record.getFinishEntry();
          if (record.getTimestamp() - entryStartTimes[finishEntry] < 1e6) {
            ops.push(["del", entryIds[finishEntry]]);
            delete entryIds[finishEntry];
            delete entryStartTimes[finishEntry];
          }
        } else if (record.isSetMetadata()) {
          const metadata = record.getSetMetadataData();
          if (metadata.entry in entryIds) {
            ops.push(["meta", entryIds[metadata.entry], metadata.metadata]);
          }
        }
      } else if (entry in entryIds) {
        const key = entryIds[entry];
        if (key in dataRecordPositions) {
          dataRecordPositions[key].push(position);
        }
      }
    } catch {
      return { failed: true, ops: [], types: {}, records: {} };
    }
  }
  const records: { [key: string]: object[] } = {};
  for (const [key, positions] of Object.entries(dataRecordPositions)) {
    records[key] = positions.map((position) => {
      try {
        const [record] = decoder.getRecordAtPosition(position);
        if (record === null) return { null: true };
        return {
          t: numberToBits(record.getTimestamp()),
          v: toTree(decodedValue(entryTypes[key], record))
        };
      } catch (e) {
        return { throw: toTree(e) };
      }
    });
  }
  return { failed: false, ops, types: entryTypes, records };
}

function applyOps(log: Log, ops: unknown[][]): void {
  for (const op of ops) {
    switch (op[0]) {
      case "range":
        log.updateRangeWithTimestamp(bitsToNumber(op[1] as string));
        log.updateRangeWithTimestamp(bitsToNumber(op[2] as string));
        break;
      case "field": {
        const [_, name, blank, type, metadata, structured] = op as [
          unknown,
          string,
          LoggableType,
          string,
          string,
          string | null
        ];
        log.createBlankField(name, blank);
        if (structured !== null) log.setStructuredType(name, structured);
        log.setWpilibType(name, type);
        log.setMetadataString(name, metadata);
        break;
      }
      case "meta":
        log.setMetadataString(op[1] as string, op[2] as string);
        break;
      case "del":
        log.deleteField(op[1] as string);
        break;
      default:
        throw new Error(`unknown op ${op[0]}`);
    }
  }
}

/** wpilogWorker's `parseField`: the type switch plus the CustomSchemas hook, over one key's records. */
function parseRecords(log: Log, key: string, type: string, records: object[]): void {
  for (const record of records) {
    const r = record as { null?: boolean; t?: string; v?: Cve1; throw?: Cve1 };
    if (r.null === true || r.throw !== undefined) continue;
    const timestamp = bitsToNumber(r.t!) / 1000000.0;
    const v = fromTree(r.v);
    try {
      switch (type) {
        case "boolean":
          log.putBoolean(key, timestamp, v as boolean);
          break;
        case "int":
        case "int64":
        case "float":
        case "double":
          log.putNumber(key, timestamp, v as number);
          break;
        case "string":
          log.putString(key, timestamp, v as string);
          break;
        case "boolean[]":
          log.putBooleanArray(key, timestamp, v as boolean[]);
          break;
        case "int[]":
        case "int64[]":
        case "float[]":
        case "double[]":
          log.putNumberArray(key, timestamp, v as number[]);
          break;
        case "string[]":
          log.putStringArray(key, timestamp, v as string[]);
          break;
        case "json":
          log.putJSON(key, timestamp, v as string);
          break;
        case "msgpack":
          (log as any).putMsgpack(key, timestamp, v);
          break;
        default:
          if (type.startsWith("struct:") || type.startsWith("photon:") || type.startsWith("proto:")) {
            throw new Error(`unsupported type ${type}`);
          }
          log.putRaw(key, timestamp, v as Uint8Array);
          if (CustomSchemas.has(type)) {
            try {
              CustomSchemas.get(type)!(log, key, timestamp, v as Uint8Array);
            } catch {}
            log.setGeneratedParent(key);
          }
      }
    } catch {}
  }
}


/**
 * Replays the worker's index and parseField sequence on a worker-side Log and moves its
 * "initial" and "fields" responses through structuredClone into a hub-side Log, as
 * HistoricalDataSource does, and returns the hub's dump.
 */
function replay(
  ops: unknown[][],
  types: { [key: string]: string },
  records: { [key: string]: object[] },
  requests: string[]
): unknown {
  const worker = new Log(false);
  applyOps(worker, ops);
  worker.getChangedFields();
  const hub = new Log();
  hub.mergeWith(Log.fromSerialized(structuredClone(worker.toSerialized())), "");
  for (let key of requests) {
    if (!(key in types) && key.startsWith("/")) key = key.slice(1);
    const type = types[key];
    if (type === undefined) continue;
    if (key in records) parseRecords(worker, key, type, records[key]);
    const fields = worker.getChangedFields();
    if (!fields.has(key) && worker.getField(key) !== null) fields.add(key);
    fields.forEach((field) => {
      hub.setField(field, LogField.fromSerialized(structuredClone(worker.getField(field)!.toSerialized())));
      if (worker.isGeneratedParent(field)) hub.setGeneratedParent(field);
    });
  }
  return dump(hub);
}

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function listFiles(dir: string, base: string, list: string[] = []): string[] {
  if (!fs.existsSync(dir)) return list;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) listFiles(full, path.join(base, entry.name), list);
    else if (entry.name.endsWith(".wpilog")) list.push(path.join(base, entry.name));
  }
  return list;
}

async function corpusCase(root: string, rel: string): Promise<void> {
  const data = new Uint8Array(fs.readFileSync(path.join(root, rel)));
  const id = rel.slice(0, -".wpilog".length);
  const extracted = await extract(data);
  const session = await HistoricalSession.open(path.resolve(root, rel));
  try {
    const requests: string[] = [];
    const worker = (session as any).source.worker;
    if (worker !== null) {
      const post = worker.postMessage.bind(worker);
      worker.postMessage = (message: any, transfer?: any) => {
        if (message.type === "parseField") requests.push(message.key);
        post(message, transfer);
      };
    }
    if (!extracted.failed) await session.loadEverything();
    const sessionDump = dump(session.log);
    const replayed = replay(extracted.ops, extracted.types, extracted.records, requests);
    if (encode(toTree(replayed)) !== encode(toTree(sessionDump))) {
      // wpilogWorker.ts parses an entry during "start" when an eager key equals it or starts
      // with it plus "/", which every eager key does for the empty key. The replay does not
      // model that; parity decode and parity historical cover these files through the real
      // source.
      const eagerKeys = [...ENABLED_KEYS, ...AUTONOMOUS_KEYS, ...UTILITY_KEYS, ...PROGRAM_START_TIME_KEYS];
      if (Object.keys(extracted.types).some((key) => eagerKeys.some((e) => e === key || e.startsWith(key + "/")))) {
        console.error(`${id}: skipped, an entry is parsed eagerly`);
        return;
      }
      const a = replayed as { keys: string[]; fields: unknown[] };
      const b = sessionDump as { keys: string[]; fields: unknown[] };
      for (const key of new Set([...a.keys, ...b.keys])) {
        const fa = a.fields[a.keys.indexOf(key)];
        const fb = b.fields[b.keys.indexOf(key)];
        if (encode(toTree(fa)) !== encode(toTree(fb)))
          console.error(`${id} field ${key}:\n  replay  ${JSON.stringify(toTree(fa))}\n  session ${JSON.stringify(toTree(fb))}`);
      }
      for (const scalar of ["generatedParents", "timestampRange", "timestampZero"] as const) {
        const fa = JSON.stringify((a as any)[scalar]);
        const fb = JSON.stringify((b as any)[scalar]);
        if (fa !== fb) console.error(`${id} ${scalar}:\n  replay  ${fa}\n  session ${fb}`);
      }
      const ka = JSON.stringify(a.keys);
      const kb = JSON.stringify(b.keys);
      if (ka !== kb) console.error(`${id} keys:\n  replay  ${ka}\n  session ${kb}`);
      console.error(`${id} requests: ${JSON.stringify(requests)}`);
      throw new Error(`${id}: harness replay differs from the real worker`);
    }
    if (!extracted.failed) {
      out({ t: "case", id, ops: extracted.ops, requests, types: extracted.types, records: extracted.records });
      out({ t: "dump", log: toTree(sessionDump) });
    } else {
      out({ t: "case", id, failed: true, ops: [], requests: [], types: {}, records: {} });
      out({ t: "dump", log: toTree(sessionDump) });
    }
  } finally {
    await session.close();
  }
}

const SECOND = 1_000_000;
const FIRMWARE_API = 0x98;
const MODEL_API = 0x300 | 155;
const LEGACY_CLASS = 6;
const PERIODIC_CLASS = spec2026.periodicFrames.STATUS_0.apiClass;
const STATUS_2025 = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => (spec2025.periodicFrames as any)[`STATUS_${i}`]);
const STATUS_2026 = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => (spec2026.periodicFrames as any)[`STATUS_${i}`]);
const R1 = "URCL";
const R2 = "URCLr2_periodic";
const R3 = "URCLr3_periodic";

class GenLog {
  private b = new WpilogBuilder({ extra: "genUrcl" });
  private ids = new Map<string, number>();

  entry(name: string, type: string, metadata = ""): this {
    const id = this.ids.size + 1;
    this.b.start(0, id, name, type, metadata);
    this.ids.set(name, id);
    return this;
  }

  put(name: string, us: number, data: ByteSource): this {
    this.b.record(this.ids.get(name)!, us, toBytes(data));
    return this;
  }

  bytes(): Uint8Array {
    return this.b.bytes();
  }
}

const r3Root = (root: string, aliases = true) => {
  const log = new GenLog().entry(root + "Raw/Periodic", R3);
  if (aliases) log.entry(root + "Raw/Aliases", "URCLr3_aliases");
  return log.entry(root + "Raw/Persistent", "URCLr3_persistent");
};
const r2Root = (root: string) =>
  new GenLog()
    .entry(root + "Raw/Periodic", R2)
    .entry(root + "Raw/Aliases", "URCLr2_aliases")
    .entry(root + "Raw/Persistent", "URCLr2_persistent");
const firmware3 = (dev: number, major: number) => persistentEntry(persistentId(FIRMWARE_API, dev), [major, 1, 0, 2, 0, 3]);
const firmware2 = (dev: number, major: number, minor = 0, build = 1) =>
  persistentEntry(persistentId(FIRMWARE_API, dev), legacyFirmware(major, minor, build));
const model2 = (dev: number, model: number, status = 0) => persistentEntry(persistentId(MODEL_API, dev), legacyModel(model, status));
const status = (ms: number, index: number, dev: number, data: ByteSource) =>
  periodicEntry(ms, periodicId(PERIODIC_CLASS, index, dev), data);
const legacy = (ms: number, index: number, dev: number, data: ByteSource) => periodicEntry(ms, periodicId(LEGACY_CLASS, index, dev), data);
const r1Value = (persistent: Uint8Array[], periodic: Uint8Array[], counts?: [number, number]) =>
  concat(new Bytes().le(counts?.[0] ?? persistent.length, 4).le(counts?.[1] ?? periodic.length, 4).finish(), ...persistent, ...periodic);
const json = (text: string) => utf8(text);
const f32Bits = (bits: number) => new Bytes().le(bits, 4).finish();

/** Deterministic byte patterns: all clear, all set, NaN-bearing, then an LCG stream. */
function pattern(n: number, seed: number): Uint8Array {
  if (n % 4 === 0) return new Uint8Array(8);
  if (n % 4 === 1) return new Uint8Array(8).fill(0xff);
  if (n % 4 === 2) return hex("01 00 c0 7f 00 00 80 ff");
  const out = new Uint8Array(8);
  let x = (seed * 2654435761 + n * 40503) >>> 0;
  for (let i = 0; i < 8; i++) {
    x = (Math.imul(x, 1664525) + 1013904223) >>> 0;
    out[i] = x >>> 24;
  }
  return out;
}

function generated(): [string, Uint8Array][] {
  const cases: [string, Uint8Array][] = [];
  const add = (id: string, log: GenLog) => cases.push([id, log.bytes()]);

  {
    const log = r3Root("URCL/");
    const devs = [0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 63];
    log.put("URCL/Raw/Aliases", 0, json('{"2":5,"3":[1,null,[2,"x"],{}],"4":null,"5":true,"6":{"a":1},"7":"","8":"Arm/Sub","9":-0,"10":1e21,"11":[],"63":"Last"}'));
    log.put("URCL/Raw/Persistent", 0, concat(...devs.map((d) => firmware3(d, 26))));
    log.put("URCL/Raw/Periodic", SECOND, concat(...devs.map((d, i) => status(i, i % 8, d, statusFrame(STATUS_2026[i % 8], pattern(i, 1))))));
    add("r3/alias-values", log);
  }
  for (const [id, aliases] of [
    ["r3/alias-tostring-throws", '{"3":{"toString":1}}'],
    ["r3/alias-nested-tostring-throws", '{"3":[1,{"toString":"x"}]}'],
    ["r3/alias-valueof-object", '{"3":{"valueOf":1}}']
  ]) {
    const log = r3Root("URCL/");
    log.put("URCL/Raw/Aliases", 0, json(aliases));
    log.put("URCL/Raw/Persistent", 0, concat(firmware3(2, 26), firmware3(3, 26), firmware3(4, 25)));
    log.put("URCL/Raw/Periodic", SECOND, concat(status(1, 0, 2, pattern(3, 2)), status(2, 0, 3, pattern(3, 3)), status(3, 0, 4, pattern(3, 4))));
    add(id, log);
  }
  {
    const log = r3Root("URCL/");
    log.put("URCL/Raw/Persistent", 0, concat(firmware3(0, 26), firmware3(1, 26), firmware3(2, 26), firmware3(10, 26)));
    const frames = concat(status(1, 0, 0, pattern(3, 5)), status(2, 0, 1, pattern(3, 6)), status(3, 0, 2, pattern(3, 7)), status(4, 0, 10, pattern(3, 8)));
    const aliasSets: ByteSource[] = [
      '["Zero","One"]',
      '"ab"',
      "5",
      "null",
      "[]",
      concat(hex("ef bb bf"), '{"1":"Bom"}'),
      concat('{"1":"', hex("ff fe c3"), '"}'),
      '{"1":"A","01":"B","-1":"C","64":"D","1.5":"E","2":"Dup","10":"Dup"}',
      '{"__proto__":"P","constructor":"C","2":"Two"}',
      '{"1":"First","1":"Second"}',
      "  {}  ",
      "{}x"
    ];
    aliasSets.forEach((aliases, i) => {
      log.put("URCL/Raw/Aliases", (i + 1) * SECOND, aliases);
      log.put("URCL/Raw/Periodic", (i + 1) * SECOND, frames);
    });
    log.put("URCL/Raw/Periodic", (aliasSets.length + 1) * SECOND - 1, frames);
    add("r3/alias-shapes", log);
  }
  {
    const log = new GenLog()
      .entry("URCL/Raw/Periodic", R3)
      .entry("URCL/Raw/Aliases", "string")
      .entry("URCL/Raw/Persistent", "URCLr3_persistent");
    log.put("URCL/Raw/Aliases", 0, '{"1":"Ignored"}');
    log.put("URCL/Raw/Persistent", 0, firmware3(1, 26));
    log.put("URCL/Raw/Periodic", SECOND, status(1, 0, 1, pattern(3, 9)));
    add("r3/aliases-wrong-type", log);
  }
  {
    const log = r3Root("URCL/");
    const devs = [0, 1, 2, 3, 4, 5, 6, 7];
    const majors = [0, 23, 24, 25, 26, 27, 128, 255];
    log.put("URCL/Raw/Persistent", 0, concat(...devs.map((d, i) => firmware3(d, majors[i]))));
    for (let cycle = 0; cycle < 4; cycle++) {
      const chunks: Uint8Array[] = [];
      for (const dev of devs) {
        const specs = majors[dev] >= 26 ? STATUS_2026 : STATUS_2025;
        for (let index = 0; index < 16; index++) {
          const seed = pattern(cycle + index, dev * 16 + index);
          chunks.push(status(cycle * 100 + index, index, dev, index < 8 ? statusFrame(specs[index], seed) : seed));
        }
        chunks.push(periodicEntry(cycle * 100 + 50, periodicId(LEGACY_CLASS, 0, dev), pattern(cycle, dev)));
        chunks.push(periodicEntry(cycle * 100 + 51, periodicId(PERIODIC_CLASS + 1, 0, dev), pattern(cycle, dev)));
      }
      log.put("URCL/Raw/Periodic", SECOND + cycle * 20000, concat(...chunks));
    }
    add("r3/firmware-years-all-frames", log);
  }
  for (const [year, specs, major] of [
    ["2025", STATUS_2025, 25],
    ["2026", STATUS_2026, 26]
  ] as const) {
    const log = r3Root("URCL/", false);
    log.put("URCL/Raw/Persistent", 0, firmware3(5, major));
    for (let index = 0; index < 8; index++) {
      const frames: Uint8Array[] = [];
      for (let n = 0; n < 24; n++) frames.push(status(index * 100 + n, index, 5, statusFrame(specs[index], pattern(n, index * 31 + n))));
      for (let n = 0; n < 4; n++) frames.push(status(index * 100 + 50 + n, index, 5, pattern(n, index)));
      log.put("URCL/Raw/Periodic", SECOND + index * 1000, concat(...frames));
    }
    add(`r3/signals-${year}`, log);
  }
  {
    const log = r3Root("URCL/", false);
    log.put("URCL/Raw/Persistent", 0, concat(firmware3(1, 25), firmware3(1, 26), firmware3(2, 26), firmware3(2, 24)));
    log.put("URCL/Raw/Periodic", SECOND, concat(status(1, 0, 1, pattern(3, 10)), status(2, 0, 2, pattern(3, 11))));
    log.put("URCL/Raw/Persistent", 2 * SECOND, concat(firmware3(1, 26), hex("98 26")));
    log.put("URCL/Raw/Periodic", 2 * SECOND, status(3, 0, 1, pattern(3, 12)));
    log.put("URCL/Raw/Persistent", 3 * SECOND, concat(firmware3(1, 26), hex("98 26 1a")));
    log.put("URCL/Raw/Periodic", 3 * SECOND, status(4, 0, 1, pattern(3, 13)));
    log.put("URCL/Raw/Persistent", 4 * SECOND, concat(firmware3(1, 26), hex("98")));
    log.put("URCL/Raw/Periodic", 4 * SECOND, status(5, 0, 1, pattern(3, 14)));
    log.put("URCL/Raw/Persistent", 5 * SECOND, new Uint8Array());
    log.put("URCL/Raw/Periodic", 5 * SECOND, status(6, 0, 1, pattern(3, 15)));
    add("r3/persistent-variants", log);
  }
  {
    const log = r3Root("URCL/", false);
    log.put("URCL/Raw/Persistent", 0, firmware3(1, 26));
    const full = status(7, 1, 1, statusFrame(STATUS_2026[1], pattern(3, 16)));
    for (let length = 0; length <= 14; length++) log.put("URCL/Raw/Periodic", SECOND + length * 1000, concat(full, full.slice(0, length)));
    add("r3/truncated-frames", log);
  }
  {
    const log = new GenLog()
      .entry("Raw/Periodic", R3)
      .entry("Raw/Persistent", "URCLr3_persistent")
      .entry("A/URCL/Raw/Periodic", R3)
      .entry("A/URCL/Raw/Persistent", "URCLr3_persistent")
      .entry("A/URCL/Raw/Aliases", "URCLr3_aliases")
      .entry("B/URCL/Raw/Periodic", R3)
      .entry("B/URCL/Raw/Persistent", "URCLr3_persistent")
      .entry("FooRaw/Periodic", R3)
      .entry("FooRaw/Persistent", "URCLr3_persistent")
      .entry("C/URCL/Raw/Periodic/", R3)
      .entry("C/URCL/Raw/Persistent", "URCLr3_persistent");
    for (const root of ["", "A/URCL/", "B/URCL/", "Foo", "C/URCL/"]) {
      log.put(root + "Raw/Persistent", 0, concat(firmware3(1, 26), firmware3(2, root === "B/URCL/" ? 25 : 26)));
      log.put(root === "C/URCL/" ? "C/URCL/Raw/Periodic/" : root + "Raw/Periodic", SECOND, concat(status(1, 0, 1, pattern(3, 17)), status(2, 1, 2, pattern(3, 18))));
    }
    log.put("A/URCL/Raw/Aliases", 0, '{"1":"Shared","2":"Shared"}');
    add("r3/roots", log);
  }
  {
    const log = new GenLog()
      .entry("URCL/Spark-1/AppliedOutput", "string")
      .entry("URCL/Spark-1", "double")
      .entry("URCL/Spark-1/Firmware", "boolean")
      .entry("URCL/Spark-1/PeriodicFrame/0", "int64")
      .entry("URCL/Spark-1/Fault/Other", "raw", '{"description":"existing"}')
      .entry("URCL/Raw/Persistent", "URCLr3_persistent")
      .entry("URCL/Raw/Periodic", R3);
    log.put("URCL/Spark-1/AppliedOutput", 0, "text");
    log.put("URCL/Spark-1", 0, new Bytes().f64(1.5).finish());
    log.put("URCL/Spark-1/Firmware", 0, [1]);
    log.put("URCL/Spark-1/PeriodicFrame/0", 0, new Bytes().le(7, 8).finish());
    log.put("URCL/Raw/Persistent", 0, firmware3(1, 26));
    log.put("URCL/Raw/Periodic", SECOND, concat(status(1, 0, 1, pattern(1, 19)), status(2, 1, 1, pattern(3, 20))));
    add("r3/key-collisions", log);
  }

  {
    const log = r2Root("URCL/");
    const devs = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 63];
    log.put("URCL/Raw/Aliases", 0, '{"5":"Five","6":null,"7":{"toString":1}}');
    log.put(
      "URCL/Raw/Persistent",
      0,
      concat(
        model2(0, SPARK_MAX),
        firmware2(0, 25, 1, 0x0203),
        firmware2(1, 23, 0, 7),
        model2(1, SPARK_FLEX),
        firmware2(2, 24, 0, 0xffff),
        model2(2, SPARK_FLEX),
        firmware2(3, 24),
        model2(3, SPARK_MAX),
        firmware2(4, 25),
        model2(4, SPARK_FLEX, 1),
        firmware2(5, 25),
        model2(5, 0x9999),
        firmware2(6, 25),
        model2(6, SPARK_FLEX, 0xff),
        firmware2(8, 25),
        model2(9, SPARK_MAX),
        firmware2(63, 22, 255, 0x8000),
        model2(63, SPARK_FLEX)
      )
    );
    for (let cycle = 0; cycle < 3; cycle++) {
      const chunks: Uint8Array[] = [];
      for (const dev of devs) {
        for (let index = 0; index < 10; index++) chunks.push(legacy(cycle * 100 + index, index, dev, pattern(cycle + index, dev * 10 + index)));
        chunks.push(periodicEntry(cycle * 100 + 60, periodicId(LEGACY_CLASS - 1, 0, dev), pattern(cycle, dev)));
      }
      log.put("URCL/Raw/Periodic", SECOND + cycle * 20000, concat(...chunks));
    }
    add("r2/models", log);
  }
  {
    const log = r2Root("URCL/");
    log.put("URCL/Raw/Persistent", 0, concat(firmware2(1, 24), model2(1, SPARK_FLEX), firmware2(2, 23), model2(2, SPARK_MAX)));
    const frames: Uint8Array[] = [];
    let ms = 0;
    const both = (index: number, data: ByteSource) => {
      frames.push(legacy(ms++, index, 1, data));
      frames.push(legacy(ms++, index, 2, data));
    };
    both(0, [0x01, 0x80, 0xff, 0xff, 0x00, 0x00, 0x33, 0]);
    both(1, concat(new Bytes().f32(1500).finish(), [0, 0x80, 0x0c, 0]));
    both(0, [0xff, 0x7f, 0x00, 0x00, 0xff, 0xff, 0x00, 0]);
    both(1, concat(f32Bits(0x7fa00001), [40, 0x00, 0xf8, 0]));
    for (const position of [-2.75, -0.5, 0, 0.25, 3.5, -1e9, NaN, Infinity, -Infinity]) {
      both(2, concat(new Bytes().f32(position).finish(), [0x00, 0x80, 0x12, 0x34]));
    }
    both(2, concat(f32Bits(0xffc00007), f32Bits(0x7f800001)));
    for (const raw of [0, 0x3ff, 0x200, 0x155]) {
      both(3, concat([raw & 0xff, (raw >> 8) & 0x03 | 0xfc, 0xff, 0x7f], new Bytes().f32(-0).finish()));
      both(3, concat([raw & 0xff, (raw >> 8) & 0x03, 0x00, 0x80], new Bytes().f32(12.5).finish()));
    }
    both(4, concat(new Bytes().f32(-100).finish(), new Bytes().f32(7.25).finish()));
    both(5, concat(new Bytes().f32(0.5).finish(), [0xff, 0xff, 0, 0]));
    both(6, concat(new Bytes().f32(60).finish(), [0x34, 0x12, 0, 0]));
    both(7, f32Bits(0x7f800000));
    log.put("URCL/Raw/Periodic", SECOND, concat(...frames));
    add("r2/frame-values", log);
  }
  {
    const log = r2Root("URCL/");
    log.put("URCL/Raw/Persistent", 0, concat(firmware2(1, 25), model2(1, SPARK_FLEX)));
    for (let index = 0; index < 8; index++) {
      const full = legacy(index, index, 1, pattern(3, 21 + index));
      for (const length of [6, 7, 8, 9, 10, 11, 12, 13]) log.put("URCL/Raw/Periodic", SECOND + index * 1000 + length, full.slice(0, length));
    }
    log.put("URCL/Raw/Persistent", 2 * SECOND, concat(firmware2(1, 25), model2(1, SPARK_FLEX), hex("98 26 19")));
    log.put("URCL/Raw/Periodic", 2 * SECOND, legacy(1, 0, 1, pattern(3, 30)));
    log.put("URCL/Raw/Persistent", 3 * SECOND, concat(firmware2(1, 25), model2(1, SPARK_FLEX).slice(0, 7)));
    log.put("URCL/Raw/Periodic", 3 * SECOND, legacy(1, 0, 1, pattern(3, 31)));
    add("r2/truncated", log);
  }

  {
    const log = new GenLog().entry("/URCL", R1).entry("URCL", R1).entry("a/b", R1, '{"source":"URCL"}');
    const persistent = [firmware2(1, 25, 2, 3), model2(1, SPARK_FLEX), model2(2, SPARK_MAX), firmware2(2, 23), firmware2(3, 25), model2(4, SPARK_MAX), model2(63, 0x1111, 0), firmware2(63, 25)];
    const periodic = [0, 2, 3, 4, 63].flatMap((dev) => [0, 1, 2, 3, 4, 5, 6, 7, 9].map((index) => legacy(dev * 10 + index, index, dev, pattern(index + dev, dev))));
    log.put("/URCL", SECOND, r1Value(persistent, periodic));
    log.put("/URCL", 2 * SECOND, r1Value([], []));
    log.put("/URCL", 3 * SECOND, r1Value(persistent, []));
    log.put("/URCL", 4 * SECOND, r1Value([], periodic));
    log.put("/URCL", 5 * SECOND, r1Value(persistent, periodic, [persistent.length, 3]));
    log.put("/URCL", 6 * SECOND, concat(r1Value(persistent, periodic), [1, 2, 3]));
    log.put("/URCL", 7 * SECOND, r1Value(persistent, periodic, [0xffffffff, 0]));
    log.put("/URCL", 8 * SECOND, r1Value(persistent, periodic, [persistent.length, 0xffffffff]));
    log.put("/URCL", 9 * SECOND, r1Value([firmware2(1, 25).slice(0, 3)], [], [1, 0]));
    log.put("/URCL", 10 * SECOND, concat(r1Value([], [], [0, 0]).slice(0, 7)));
    log.put("URCL", SECOND, r1Value(persistent, periodic.slice(0, 9)));
    log.put("a/b", SECOND, r1Value([firmware2(1, 24), model2(1, SPARK_FLEX)], [legacy(5, 2, 1, pattern(3, 32))]));
    add("r1/field-sets", log);
  }
  return cases;
}

const USAGE = "usage: genUrcl <corpus-dir>";
const dir = process.argv[2];
if (dir === undefined) {
  console.error(USAGE);
  process.exit(2);
}
out({ t: "meta", generator: "urcl" });
for (const rel of listFiles(path.join(dir, "urcl"), "urcl")) {
  await corpusCase(dir, rel);
}
for (const rel of listFiles(path.join(dir, "fuzz/urcl"), "fuzz/urcl")) {
  await corpusCase(dir, rel);
}
const generatedDir = fs.mkdtempSync(path.join(os.tmpdir(), "genUrcl-"));
try {
  for (const [id, bytes] of generated()) {
    const rel = path.join("generated", id + ".wpilog");
    fs.mkdirSync(path.dirname(path.join(generatedDir, rel)), { recursive: true });
    fs.writeFileSync(path.join(generatedDir, rel), bytes);
    await corpusCase(generatedDir, rel);
  }
} finally {
  fs.rmSync(generatedDir, { recursive: true, force: true });
}
