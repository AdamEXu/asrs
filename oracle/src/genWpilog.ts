import "./shims";
import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { WPILOGDecoder, WPILOGDecoderRecord } from "@upstream/hub/dataSources/wpilog/WPILOGDecoder";
import { CONTROL_ENTRY } from "@upstream/hub/dataSources/wpilog/WPILOGShared";
import { Cve1, numberToBits, toTree } from "./cve1";
import { buildCorpusA } from "./corpus/matrices";
import { UPSTREAM_DIR } from "./shims";

// Emits WPILOG reader/indexer ground truth for the asrs_formats::wpilog vectors.
//   genWpilog corpus <seed>
//     One line per .wpilog file in corpus A's case set (built in memory, same matrices
//     corpusA emits):
//     {"case":<id>,"bytes":{"b":"<hex>"},"header":{valid,version,extra},
//      "index":{min,max,records:[[entry,position]...]} | {"throw":<cve1>},
//      "records":[<record>...],"iter":{"ok":<count>}|{"throw":<cve1>}}
//     Each record is {p,e,t,s,c,d,g}: byte position, entry, timestamp bits, size
//     consumed, control info, raw payload hex and per-getter {"ok"|"throw"} results.
//   genWpilog public
//     One line per wpilog in parity/corpus/public.toml (files from the public-logs cache):
//     {"case":"public/<name>","in":{file,size,sha256},"header":{...},
//      "index":{min,max,count} | {"throw":<cve1>},
//      "blocks":"<16 hex per 512-record block>","sample":[{i,...record}...]}
//     Each block digest is FNV-1a 64 over the concatenated JSON.stringify of that
//     block's record objects {p,e,t,s,c,d} (no "g"); the sample holds the verbatim
//     object for control records, the first and last 64 records and every 8192nd.

const SAMPLE_EDGE = 64;
const SAMPLE_EVERY = 8192;
const BLOCK = 512;

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function bytesToHex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

function fnv1a64(text: string): string {
  let h = 0xcbf29ce484222325n;
  for (let i = 0; i < text.length; i++) {
    h ^= BigInt(text.charCodeAt(i));
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h.toString(16).padStart(16, "0");
}

function attempt(f: () => unknown): { ok: Cve1 } | { throw: Cve1 } {
  try {
    return { ok: toTree(f()) };
  } catch (e) {
    return { throw: toTree(e) };
  }
}

type IndexerModule = {
  _malloc(size: number): number;
  _run(buffer: number, bufferSize: number): number;
  HEAPF64: Float64Array;
  HEAPU8: Uint8Array;
  HEAPU32: Uint32Array;
};

let indexerInit: (() => Promise<IndexerModule>) | null = null;

// Upstream's wpilogIndexer.ts instantiates a fresh module for every run, and negative extra
// header sizes read the memory below the input buffer, so each file gets its own instance too.
async function indexer(): Promise<IndexerModule> {
  if (indexerInit === null) {
    const dir = path.join(UPSTREAM_DIR, "bundles");
    const { default: init } = await import(pathToFileURL(path.join(dir, "wpilogIndexer.js")).href);
    const wasmBinary = fs.readFileSync(path.join(dir, "wpilogIndexer.wasm"));
    indexerInit = async () => (await init({ wasmBinary })) as IndexerModule;
  }
  return indexerInit();
}

type IndexResult = { min: number; max: number; records: [number, number][] } | { throw: Cve1 };

// The same sequence upstream's wpilogIndexer.ts drives: malloc, copy in, _run,
// then read the output header and record list out of the wasm heap.
async function runIndexer(data: Uint8Array): Promise<IndexResult> {
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
      records.push([
        Module.HEAPU32[bufferOut / 4 + 5 + i * 2],
        Module.HEAPU32[bufferOut / 4 + 5 + i * 2 + 1]
      ]);
    }
    return { min, max, records };
  } catch (e) {
    return { throw: toTree(e) };
  }
}

function getterJson(record: WPILOGDecoderRecord): object {
  return {
    boolean: attempt(() => record.getBoolean()),
    integer: attempt(() => record.getInteger()),
    float: attempt(() => record.getFloat()),
    double: attempt(() => record.getDouble()),
    string: attempt(() => record.getString()),
    booleanArray: attempt(() => record.getBooleanArray()),
    integerArray: attempt(() => record.getIntegerArray()),
    floatArray: attempt(() => record.getFloatArray()),
    doubleArray: attempt(() => record.getDoubleArray()),
    stringArray: attempt(() => record.getStringArray())
  };
}

function controlJson(record: WPILOGDecoderRecord): Cve1 {
  if (!record.isControl()) return 0;
  if (record.isStart()) {
    try {
      const d = record.getStartData();
      return { start: { entry: d.entry, name: d.name, type: d.type, metadata: d.metadata } };
    } catch (e) {
      return { start: { throw: toTree(e) } };
    }
  }
  if (record.isFinish()) return { finish: record.getFinishEntry() };
  if (record.isSetMetadata()) {
    try {
      const d = record.getSetMetadataData();
      return { meta: { entry: d.entry, metadata: d.metadata } };
    } catch (e) {
      return { meta: { throw: toTree(e) } };
    }
  }
  return { type: record.getRaw()[0] ?? null };
}

function recordJson(
  record: WPILOGDecoderRecord,
  position: number,
  size: number,
  getters: boolean
): object {
  const r: Record<string, unknown> = {
    p: position,
    e: record.getEntry(),
    t: { n: numberToBits(record.getTimestamp()) },
    s: size,
    c: controlJson(record),
    d: { b: bytesToHex(record.getRaw()) }
  };
  if (getters) r.g = getterJson(record);
  return r;
}

function headerJson(decoder: WPILOGDecoder): object {
  return { valid: decoder.isValid(), version: decoder.getVersion(), extra: decoder.getExtraHeader() };
}

function iterJson(decoder: WPILOGDecoder): Cve1 {
  return attempt(() => {
    let count = 0;
    decoder.forEach(() => count++);
    return count;
  });
}

async function summarize(data: Uint8Array): Promise<Record<string, unknown>> {
  const decoder = new WPILOGDecoder(data);
  const record: Record<string, unknown> = { header: headerJson(decoder), iter: iterJson(decoder) };
  const indexed = await runIndexer(data);
  if ("throw" in indexed) {
    record.index = { throw: indexed.throw };
    return record;
  }
  const records: object[] = [];
  for (const [entry, position] of indexed.records) {
    const [record, size] = decoder.getRecordAtPosition(position);
    if (record === null) {
      records.push({ p: position, e: entry, null: true });
    } else {
      records.push(recordJson(record, position, size, true));
    }
  }
  record.index = {
    min: { n: numberToBits(indexed.min) },
    max: { n: numberToBits(indexed.max) },
    records: indexed.records
  };
  record.records = records;
  return record;
}

async function summarizePublic(data: Uint8Array): Promise<Record<string, unknown>> {
  const decoder = new WPILOGDecoder(data);
  const record: Record<string, unknown> = { header: headerJson(decoder) };
  const indexed = await runIndexer(data);
  if ("throw" in indexed) {
    record.index = { throw: indexed.throw };
    return record;
  }
  const blocks: string[] = [];
  let blockText = "";
  const sample: object[] = [];
  const total = indexed.records.length;
  for (let i = 0; i < total; i++) {
    const [entry, position] = indexed.records[i];
    const [record, size] = decoder.getRecordAtPosition(position);
    const json =
      record === null
        ? { p: position, e: entry, null: true }
        : recordJson(record, position, size, false);
    blockText += JSON.stringify(json);
    if (i % BLOCK === BLOCK - 1 || i === total - 1) {
      blocks.push(fnv1a64(blockText));
      blockText = "";
    }
    if (
      entry === CONTROL_ENTRY ||
      i < SAMPLE_EDGE ||
      i >= total - SAMPLE_EDGE ||
      i % SAMPLE_EVERY === 0
    ) {
      sample.push({ i, ...json });
    }
  }
  record.index = {
    min: { n: numberToBits(indexed.min) },
    max: { n: numberToBits(indexed.max) },
    count: total
  };
  record.blocks = blocks.join("");
  record.sample = sample;
  return record;
}

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

async function corpusMode(seedText: string) {
  const seed = Number(seedText);
  const corpus = buildCorpusA(seed);
  for (const c of [...corpus.cases].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))) {
    for (const file of c.files) {
      if (!file.path.endsWith(".wpilog")) continue;
      const line = await summarize(file.bytes);
      out({ case: c.id, bytes: { b: bytesToHex(file.bytes) }, ...line });
    }
  }
}

async function publicMode(root: string) {
  const dir = publicLogsDir(root);
  for (const log of publicLogs(root)) {
    if (log.format !== "wpilog") continue;
    const file = path.join(dir, log.file);
    const data = new Uint8Array(fs.readFileSync(file));
    const sha256 = createHash("sha256").update(data).digest("hex");
    if (sha256 !== log.sha256 || data.length !== log.size) {
      throw new Error(`${file}: sha256 or size differs from public.toml`);
    }
    const line = await summarizePublic(data);
    out({
      case: `public/${log.name}`,
      in: { file: log.file, size: log.size, sha256: log.sha256 },
      ...line
    });
  }
}

const USAGE = "usage: genWpilog corpus <seed> | public";
const [mode, arg] = process.argv.slice(2);
if (mode === "corpus" && arg !== undefined) {
  await corpusMode(arg);
} else if (mode === "public" && arg === undefined) {
  await publicMode(process.cwd());
} else {
  console.error(USAGE);
  process.exit(2);
}
