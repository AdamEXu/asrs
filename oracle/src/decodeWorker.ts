import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import { HistoricalDataSource } from "@upstream/hub/dataSources/HistoricalDataSource";
import {
  AKIT_TIMESTAMP_KEYS,
  ALLIANCE_KEYS,
  AUTONOMOUS_KEYS,
  DRIVER_STATION_KEYS,
  ENABLED_KEYS,
  EVENT_KEYS,
  MATCH_NUMBER_KEYS,
  MATCH_TYPE_KEYS,
  SYSTEM_TIME_KEYS,
  UTILITY_KEYS
} from "@upstream/shared/log/RobotState";
import { encode } from "./cve1";
import { dumpLog } from "./historical/dump";
import { OracleWorker, terminatedWorkersExited } from "./shims";
import { realSetImmediate } from "./historical/timers";

const USAGE = `usage: decodeWorker <log file>

Runs the real worker bundle for the file's extension on its own, the way HistoricalDataSource
drives it, and prints every message it posts (CVE-1 lines, see oracle/README.md):
  {"request": "start"} then the worker's messages for start;
  if the initial log is partial, one {"request": "parseField", "key"} per requested key, each
  followed by its messages. The keys are the initial log's fields in order, the keys
  HistoricalDataSource always requests, "/" + the first field, a missing key and the first
  field again.`;

const MISSING_KEY = "/asrs-oracle/missing";

const ALWAYS_REQUESTED = [
  ...ENABLED_KEYS,
  ...AUTONOMOUS_KEYS,
  ...UTILITY_KEYS,
  ...ALLIANCE_KEYS,
  ...DRIVER_STATION_KEYS,
  ...SYSTEM_TIME_KEYS,
  ...AKIT_TIMESTAMP_KEYS,
  ...EVENT_KEYS,
  ...MATCH_TYPE_KEYS,
  ...MATCH_NUMBER_KEYS
];

/** Synchronous, so the lines before a worker that never answers are already out. */
function write(lines: string[]): void {
  const bytes = Buffer.from(lines.map((l) => l + "\n").join(""), "utf8");
  let offset = 0;
  while (offset < bytes.length) {
    try {
      offset += fs.writeSync(1, bytes, offset);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EAGAIN") throw error;
    }
  }
}

function readOrNull(file: string): Uint8Array | null {
  try {
    return new Uint8Array(fs.readFileSync(file));
  } catch {
    return null;
  }
}

function messageLines(message: any): string[] {
  switch (message.type) {
    case "progress":
      return [encode({ type: "progress", value: message.value })];
    case "initial":
      return [
        encode({ type: "initial", isPartial: message.isPartial }),
        ...dumpLog(message.log, { phase: "worker", status: "", loadingFields: [] })
      ];
    case "failed":
      return [encode({ type: "failed" })];
    case "fields":
      return [
        encode({ type: "fields", count: message.fields.length }),
        ...message.fields.map((field: any) =>
          encode({
            key: field.key,
            generatedParent: field.generatedParent,
            type: field.data.type,
            timestamps: field.data.timestamps,
            values: field.data.values,
            structuredType: field.data.structuredType,
            wpilibType: field.data.wpilibType,
            metadataString: field.data.metadataString,
            stripingReference: field.data.stripingReference,
            typeWarning: field.data.typeWarning
          })
        )
      ];
    default:
      throw new Error(`unexpected worker message ${JSON.stringify(message.type)}`);
  }
}

async function idle(worker: OracleWorker): Promise<void> {
  while (worker.busy) await new Promise((resolve) => realSetImmediate(resolve));
}

let file = process.argv[2];
if (file === undefined || file.startsWith("-")) {
  console.error(USAGE);
  process.exit(2);
}
file = path.resolve(file);
if (file.endsWith(".dsevents")) file = file.slice(0, -8) + "dslog";

let workerName: string | null = null;
for (const [extension, name] of Object.entries((new HistoricalDataSource() as any).WORKER_NAMES as object)) {
  if (file.endsWith(extension)) workerName = name;
}
if (workerName === null) {
  console.error(`${file}: no worker for this extension`);
  process.exit(2);
}
const files = file.endsWith(".dslog")
  ? [readOrNull(file), readOrNull(file.slice(0, -5) + "dsevents")]
  : [readOrNull(file)];

const worker = new OracleWorker("../bundles/" + workerName, { type: "module" });
let initial: any = null;
worker.onmessage = (event) => {
  if (event.data.type === "initial") initial = event.data;
  write(messageLines(event.data));
};
worker.onerror = () => write([encode({ type: "threw" })]);

write([encode({ request: "start" })]);
worker.postMessage(
  { type: "start", data: files },
  files.map((array) => (array === null ? new ArrayBuffer(0) : array.buffer))
);
await idle(worker);

if (initial !== null && initial.isPartial) {
  const keys = Object.keys(initial.log.fields);
  const requests = [...keys, ...ALWAYS_REQUESTED];
  if (keys.length > 0) requests.push("/" + keys[0]);
  requests.push(MISSING_KEY);
  if (keys.length > 0) requests.push(keys[0]);
  for (const key of requests) {
    write([encode({ request: "parseField", key })]);
    worker.postMessage({ type: "parseField", key });
    await idle(worker);
  }
}
worker.terminate();
await terminatedWorkersExited();
