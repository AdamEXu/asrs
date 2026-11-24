import "./shims";
import { createHash } from "node:crypto";
import WorkerManager from "@upstream/hub/WorkerManager";
import Log from "@upstream/shared/log/Log";
import { resolveWorkerScript, setWorkerRouter } from "./shims";

// mcapReentry
//   The real export worker on sampled MCAP exports that outgrow the writer's 1 MiB chunk. First
//   {"arrayBufferMax"}, the longest ArrayBuffer this runtime allocates; then one record per case:
//   the log, the sample count (one per second from 0), the number of progress values and the
//   SHA-256 of their little-endian doubles, and the length and SHA-256 of the file, or null when
//   the worker rejected. A string field's value is "x" repeated `length` times.

type Field =
  | { key: string; start: number; type: "string" | "string[]"; length: number }
  | { key: string; start: number; type: "number"; value: number };

const text = (key: string, length: number, start = 0): Field => ({ key, start, type: "string", length });
const number = (key: string, value: number, start = 0): Field => ({ key, start, type: "number", value });

const BIG = 1_100_000;
const CASES: { name: string; samples: number; fields: Field[] }[] = [
  { name: "last sample fills the chunk", samples: 1, fields: [text("/A", BIG)] },
  { name: "one re-entry", samples: 2, fields: [text("/A", BIG)] },
  { name: "two re-entries", samples: 3, fields: [text("/A", BIG)] },
  { name: "many small messages after the bound", samples: 24_970, fields: [number("/A", 1)] },
  { name: "filled by the last sample, then a small field", samples: 1, fields: [text("/A", BIG), number("/B", 1)] },
  { name: "re-entered, then a small field", samples: 3, fields: [text("/A", BIG), number("/B", 1)] },
  { name: "filled by the last sample, then filled again", samples: 1, fields: [text("/A", BIG), text("/B", BIG)] },
  { name: "filled by the last sample, then re-entered", samples: 2, fields: [text("/A", 500_000), text("/B", 600_000)] },
  { name: "re-entered twice", samples: 2, fields: [text("/A", BIG), text("/B", BIG)] },
  { name: "re-entered, then a field without samples", samples: 2, fields: [text("/A", BIG), number("/B", 1, 9)] },
  { name: "filled twice, then a field without samples", samples: 1, fields: [text("/A", BIG), text("/B", BIG), number("/C", 1, 9)] },
  { name: "re-entered, then a field of no MCAP type", samples: 2, fields: [text("/A", BIG), { key: "/B", start: 0, type: "string[]", length: 4 }] },
  {
    name: "re-entered, then the undefined field",
    samples: 2,
    fields: [text("/A", BIG), { key: "/B", start: 0, type: "string[]", length: 4 }, text("undefined", 700_000)]
  },
  { name: "past the longest buffer", samples: 8, fields: [text("/A", 30_000_000)] }
];

let low = 1;
let high = 2 ** 33;
while (low < high) {
  const middle = Math.floor((low + high + 1) / 2);
  try {
    new ArrayBuffer(middle);
    low = middle;
  } catch {
    high = middle - 1;
  }
}
const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
out({ arrayBufferMax: low });

setWorkerRouter(null);
const script = (): string => {
  const worker = JSON.stringify(resolveWorkerScript("../bundles/exportWorker.js"));
  const source = `process.on("unhandledRejection", () => {});\nawait import(${worker});`;
  return `data:text/javascript,${encodeURIComponent(source)}`;
};

for (const { name, samples, fields } of CASES) {
  const log = new Log();
  for (const field of fields) {
    if (field.type === "number") log.putNumber(field.key, field.start, field.value);
    else if (field.type === "string") log.putString(field.key, field.start, "x".repeat(field.length));
    else log.putStringArray(field.key, field.start, ["x".repeat(field.length)]);
  }
  const serialized = log.toSerialized();
  serialized.timestampRange = [0, samples - 1];
  const options = { format: "mcap", samplingMode: "fixed", samplingPeriod: 1000, prefixes: "", includeGenerated: true };
  let posted = 0;
  const progress = createHash("sha256");
  const content: Uint8Array | null = await WorkerManager.request(script(), { options, log: serialized }, (value) => {
    posted++;
    progress.update(new Uint8Array(new Float64Array([value]).buffer));
  }).catch(() => null);
  out({
    name,
    samples,
    fields,
    progress: [posted, progress.digest("hex")],
    file: content === null ? null : [content.length, createHash("sha256").update(content).digest("hex")]
  });
}
process.exit(0);
