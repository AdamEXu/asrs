import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import CustomSchemas from "@upstream/hub/dataSources/schema/CustomSchemas";
import { WPILOGDecoder, WPILOGDecoderRecord } from "@upstream/hub/dataSources/wpilog/WPILOGDecoder";
import { CONTROL_ENTRY } from "@upstream/hub/dataSources/wpilog/WPILOGShared";
import Log from "@upstream/shared/log/Log";
import LoggableType from "@upstream/shared/log/LoggableType";
import { getStructuredTypeFromRaw, isChildLog, isSchema } from "@upstream/shared/log/LogKeyUtils";
import { Cve1, bitsToNumber, encode, fromTree, numberToBits, toTree } from "./cve1";
import { HistoricalSession } from "./historical/session";
import { dump } from "./logRun";
import { UPSTREAM_DIR } from "./shims";

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
//     when getRecordAtPosition returned null. `requests` is the field order
//     HistoricalDataSource.loadAllFields asks the worker for: schema, URCL aliases and
//     persistent fields first.
//
//     Corpus cases are cross-checked against a real HistoricalSession: the replay in
//     this harness uses upstream's CustomSchemas and must produce the same dump.

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

/** The order `HistoricalDataSource.updateFieldRequest(loadEverything)` requests fields in. */
function requestOrder(log: Log): string[] {
  const fields = log.getFieldKeys();
  const first = (field: string) =>
    isSchema(field) ||
    field.includes("URCL/Raw/Aliases") ||
    field.includes("URCL/Raw/Persistent") ||
    isChildLog(log.getWpilibType(field), log.getStructuredType(field));
  return [...fields.filter(first), ...fields.filter((f) => !first(f))];
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

interface Replayed {
  log: Log;
  requests: string[];
  dump: unknown;
}

/** Replays the worker's index + parseField sequence and returns the dump the hub observes. */
function replay(ops: unknown[][], types: { [key: string]: string }, records: { [key: string]: object[] }): Replayed {
  const log = new Log(false);
  applyOps(log, ops);
  // The worker drains changed fields and posts "initial" before any parseField
  // call; the hub keeps that serialized range/zero. Generated parents only reach
  // the hub through "fields" responses, applied in changed-field order.
  log.getChangedFields();
  const initial = log.toSerialized();
  const initialRange = initial.timestampRange === null ? null : [...initial.timestampRange];
  const initialZero = initial.timestampZero;
  const requests = requestOrder(log);
  const parents = new Set(initial.generatedParents);
  for (const key of requests) {
    const type = types[key];
    if (type === undefined) continue; // the worker warns and skips unknown keys
    if (!(key in records)) continue;
    parseRecords(log, key, type, records[key]);
    let hasRoot = false;
    log.getChangedFields().forEach((childKey) => {
      if (childKey === key) hasRoot = true;
      if (log.isGeneratedParent(childKey)) parents.add(childKey);
    });
    if (!hasRoot && log.getField(key) !== null && log.isGeneratedParent(key)) parents.add(key);
  }
  // Field values cross the worker's postMessage boundary; structured clone
  // canonicalizes NaN payloads to 0x7ff8000000000000.
  const d = dump(log) as {
    fields: unknown[];
    generatedParents: string[];
    timestampRange: number[] | null;
    timestampZero: number | null;
  };
  d.generatedParents = [...parents];
  d.timestampRange = initialRange;
  d.timestampZero = initialZero;
  return { log, requests, dump: canonicalize(d) };
}

function canonicalize(value: unknown): unknown {
  if (typeof value === "number") return Number.isNaN(value) ? NaN : value;
  if (value instanceof Uint8Array) return value;
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value !== null && typeof value === "object") {
    const out: { [key: string]: unknown } = {};
    for (const [k, v] of Object.entries(value)) out[k] = canonicalize(v);
    return out;
  }
  return value;
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
  const replayed = replay(extracted.ops, extracted.types, extracted.records);
  const session = await HistoricalSession.open(path.resolve(root, rel));
  try {
    if (!extracted.failed) await session.loadEverything();
    const sessionDump = dump(session.log);
    if (encode(toTree(replayed.dump)) !== encode(toTree(sessionDump))) {
      const a = replayed.dump as { keys: string[]; fields: unknown[] };
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
      throw new Error(`${id}: harness replay differs from the real worker`);
    }
    if (!extracted.failed) {
      out({ t: "case", id, ops: extracted.ops, requests: replayed.requests, types: extracted.types, records: extracted.records });
      out({ t: "dump", log: toTree(sessionDump) });
    } else {
      out({ t: "case", id, failed: true, ops: [], requests: [], types: {}, records: {} });
      out({ t: "dump", log: toTree(sessionDump) });
    }
  } finally {
    await session.close();
  }
}

interface GeneratedCase {
  id: string;
  entries: [name: string, type: string, metadata: string][];
  range: [number, number];
  records: { [key: string]: [us: number, bytes: Uint8Array][] };
}

function generatedCase(c: GeneratedCase): void {
  const ops: unknown[][] = [
    ["range", numberToBits(c.range[0] / 1e6), numberToBits(c.range[1] / 1e6)],
    ...c.entries.map(([name, type, metadata]) => [
      "field",
      name,
      BLANK_TYPE[type] ?? LoggableType.Raw,
      type,
      metadata,
      getStructuredTypeFromRaw(type)
    ])
  ];
  const records: { [key: string]: object[] } = {};
  for (const [key, recs] of Object.entries(c.records)) {
    records[key] = recs.map(([us, bytes]) => ({ t: numberToBits(us), v: toTree(bytes) }));
  }
  const types: { [key: string]: string } = {};
  for (const [name, type] of c.entries) types[name] = type;
  const replayed = replay(ops, types, records);
  out({ t: "case", id: c.id, generated: true, ops, requests: replayed.requests, types, records });
  out({ t: "dump", log: toTree(replayed.dump) });
}

const SECOND_US = 1_000_000;
const le = (bytes: number[]): Uint8Array => Uint8Array.from(bytes);
const u16le = (v: number) => [v & 0xff, (v >> 8) & 0xff];
const u32le = (v: number) => [v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, (v >> 24) & 0xff];

function persistentRecord(api: number, device: number, payload: number[]): Uint8Array {
  const data = new Uint8Array(8);
  data.set(u16le(((api << 6) | device) & 0xffff), 0);
  data.set(payload.slice(0, 6), 2);
  return data;
}

function periodicRecord(us: number, apiClass: number, index: number, device: number, payload: number[]): Uint8Array {
  const data = new Uint8Array(14);
  data.set(u32le(us), 0);
  data.set(u16le(((apiClass << 10) | (index << 6) | device) & 0xffff), 4);
  data.set(payload.slice(0, 8), 6);
  return data;
}

function generated(): GeneratedCase[] {
  const cases: GeneratedCase[] = [];
  const r3 = "URCLr3_periodic";
  const r2 = "URCLr2_periodic";
  const PERIODIC_CLASS = 47;
  const FIRMWARE_API = 0x98;
  const MODEL_API = 0x300 | 155;
  const FIRMWARE_API_R3 = (6 << 4) | 0;
  const firmwareR3 = (dev: number, major: number) => persistentRecord(FIRMWARE_API_R3, dev, [major, 0, 0, 0, 0, 0]);
  const firmwareR2 = (dev: number, major: number, minor: number, build: number) =>
    persistentRecord(FIRMWARE_API, dev, [major, minor, (build >> 8) & 0xff, build & 0xff, 0, 0]);
  const model = (dev: number, value: number, status = 0) =>
    persistentRecord(MODEL_API, dev, [value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff, (value >> 24) & 0xff, 0, status]);
  const cat = (...parts: Uint8Array[]) => Uint8Array.from(parts.flatMap((p) => [...p]));

  cases.push({
    id: "edge/r3/no-persistent",
    entries: [
      ["URCL/Raw/Periodic", r3, ""],
      ["URCL/Raw/Aliases", "URCLr3_aliases", ""]
    ],
    range: [SECOND_US, 2 * SECOND_US],
    records: {
      "URCL/Raw/Periodic": [[SECOND_US, cat(periodicRecord(10, PERIODIC_CLASS, 0, 1, [1, 2, 3, 4, 5, 6, 7, 8]))]],
      "URCL/Raw/Aliases": [[SECOND_US, le([0x7b, 0x7d])]]
    }
  });
  cases.push({
    id: "edge/r3/no-aliases",
    entries: [
      ["URCL/Raw/Periodic", r3, ""],
      ["URCL/Raw/Persistent", "URCLr3_persistent", ""]
    ],
    range: [0, SECOND_US],
    records: {
      "URCL/Raw/Persistent": [[0, firmwareR3(3, 26)]],
      "URCL/Raw/Periodic": [[SECOND_US, cat(periodicRecord(10, PERIODIC_CLASS, 0, 3, [9, 9, 9, 9, 9, 9, 9, 9]))]]
    }
  });
  cases.push({
    id: "edge/r3/alias-values",
    entries: [
      ["URCL/Raw/Periodic", r3, ""],
      ["URCL/Raw/Aliases", "URCLr3_aliases", ""],
      ["URCL/Raw/Persistent", "URCLr3_persistent", ""]
    ],
    range: [0, SECOND_US],
    records: {
      "URCL/Raw/Aliases": [[0, le([...new TextEncoder().encode('{"2":5,"3":[1,null],"4":null,"5":true,"name":"x"}')])]],
      "URCL/Raw/Persistent": [[0, cat(firmwareR3(2, 26), firmwareR3(3, 26), firmwareR3(4, 26), firmwareR3(5, 26), firmwareR3(9, 26))]],
      "URCL/Raw/Periodic": [
        [
          SECOND_US,
          cat(
            periodicRecord(10, PERIODIC_CLASS, 0, 2, [1, 0, 0, 0, 0, 0, 0, 0]),
            periodicRecord(11, PERIODIC_CLASS, 0, 3, [2, 0, 0, 0, 0, 0, 0, 0]),
            periodicRecord(12, PERIODIC_CLASS, 0, 4, [3, 0, 0, 0, 0, 0, 0, 0]),
            periodicRecord(13, PERIODIC_CLASS, 0, 5, [4, 0, 0, 0, 0, 0, 0, 0]),
            periodicRecord(14, PERIODIC_CLASS, 0, 9, [5, 0, 0, 0, 0, 0, 0, 0])
          )
        ]
      ]
    }
  });
  cases.push({
    id: "edge/r3/aliases-null",
    entries: [
      ["URCL/Raw/Periodic", r3, ""],
      ["URCL/Raw/Aliases", "URCLr3_aliases", ""],
      ["URCL/Raw/Persistent", "URCLr3_persistent", ""]
    ],
    range: [0, SECOND_US],
    records: {
      "URCL/Raw/Aliases": [[0, le([...new TextEncoder().encode("null")])]],
      "URCL/Raw/Persistent": [[0, firmwareR3(1, 26)]],
      "URCL/Raw/Periodic": [[SECOND_US, cat(periodicRecord(10, PERIODIC_CLASS, 0, 1, [0, 0, 0, 0, 0, 0, 0, 0]))]]
    }
  });
  cases.push({
    id: "edge/r3/short-periodic",
    entries: [
      ["URCL/Raw/Periodic", r3, ""],
      ["URCL/Raw/Persistent", "URCLr3_persistent", ""]
    ],
    range: [0, SECOND_US],
    records: {
      "URCL/Raw/Persistent": [[0, firmwareR3(1, 26)]],
      "URCL/Raw/Periodic": [
        [
          SECOND_US,
          cat(
            periodicRecord(10, PERIODIC_CLASS, 0, 1, [1, 2, 3, 4, 5, 6, 7, 8]),
            periodicRecord(20, PERIODIC_CLASS, 0, 1, [8, 7, 6, 5, 4, 3, 2, 1]).slice(0, 9),
            periodicRecord(30, PERIODIC_CLASS, 0, 1, [0, 0, 0, 0, 0, 0, 0, 0]).slice(0, 3)
          )
        ]
      ]
    }
  });
  cases.push({
    id: "edge/r3/non-periodic-key",
    entries: [["URCL/Raw", r3, ""]],
    range: [0, SECOND_US],
    records: {
      "URCL/Raw": [[SECOND_US, cat(periodicRecord(10, PERIODIC_CLASS, 0, 1, [1, 2, 3, 4, 5, 6, 7, 8]))]]
    }
  });
  cases.push({
    id: "edge/r2/missing-and-unknown-models",
    entries: [
      ["URCL/Raw/Periodic", r2, ""],
      ["URCL/Raw/Aliases", "URCLr2_aliases", ""],
      ["URCL/Raw/Persistent", "URCLr2_persistent", ""]
    ],
    range: [0, SECOND_US],
    records: {
      "URCL/Raw/Aliases": [[0, le([...new TextEncoder().encode('{"1":"Turret"}')])]],
      "URCL/Raw/Persistent": [
        [
          0,
          cat(
            firmwareR2(1, 25, 0, 1),
            firmwareR2(2, 25, 0, 1),
            model(2, 0x2159, 1),
            model(3, 0x2158),
            firmwareR2(4, 24, 1, 0xabcd),
            model(4, 0x9999)
          )
        ]
      ],
      "URCL/Raw/Periodic": [
        [
          SECOND_US,
          cat(
            periodicRecord(10, 6, 0, 1, [0, 64, 255, 255, 255, 255, 3, 0]),
            periodicRecord(11, 6, 1, 1, [0, 0, 128, 63, 30, 2, 1, 0]),
            periodicRecord(12, 6, 0, 2, [0, 64, 1, 0, 0, 0, 0, 0]),
            periodicRecord(13, 6, 0, 3, [0, 64, 0, 0, 0, 0, 0, 0]),
            periodicRecord(14, 6, 0, 4, [0, 64, 0, 0, 0, 0, 0, 0]),
            periodicRecord(15, 6, 0, 40, [0, 64, 0, 0, 0, 0, 0, 0]),
            periodicRecord(16, 6, 9, 1, [1, 2, 3, 4, 5, 6, 7, 8]),
            periodicRecord(17, 5, 0, 1, [1, 2, 3, 4, 5, 6, 7, 8])
          )
        ]
      ]
    }
  });
  {
    const persistent = cat(firmwareR2(1, 25, 0, 1), model(1, 0x2158));
    const frame = periodicRecord(10, 6, 0, 1, [0, 64, 255, 255, 255, 255, 3, 0]);
    cases.push({
      id: "edge/r1/periodic-overrun",
      entries: [["/URCL", "URCL", ""]],
      range: [SECOND_US, 2 * SECOND_US],
      records: {
        "/URCL": [
          [SECOND_US, Uint8Array.from([...u32le(1), ...u32le(1), ...persistent, ...frame])],
          [2 * SECOND_US, Uint8Array.from([...u32le(1), ...u32le(100), ...persistent, ...frame])]
        ]
      }
    });
    cases.push({
      id: "edge/r1/short-header",
      entries: [["/URCL", "URCL", ""]],
      range: [SECOND_US, SECOND_US],
      records: {
        "/URCL": [[SECOND_US, le([1, 2, 3])]]
      }
    });
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
for (const c of generated()) {
  generatedCase(c);
}
