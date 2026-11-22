import path from "node:path";
import { OracleWorker, resolveWorkerScript, terminatedWorkersExited } from "../shims";

// Runs one of upstream's `bundles/*.js` workers on a payload; `never` is a worker that never posts.

export type WorkerResult = { ok: any } | { never: string };

export async function runWorker(bundle: string, payload: unknown, timeoutMs = 120000): Promise<WorkerResult> {
  const script = resolveWorkerScript("../bundles/" + bundle);
  // A worker's relative URLs resolve against its own script URL, and Chromium fetches file:
  // URLs; Node's Request and fetch do neither, so the worker realm gets both.
  const loader =
    "data:text/javascript," +
    encodeURIComponent(`
import fs from "node:fs";
import { fileURLToPath } from "node:url";
const BASE = ${JSON.stringify(script)};
globalThis.ProgressEvent ??= class ProgressEvent extends Event {
  constructor(type, init = {}) {
    super(type);
    this.lengthComputable = init.lengthComputable ?? false;
    this.loaded = init.loaded ?? 0;
    this.total = init.total ?? 0;
  }
};
const RealRequest = globalThis.Request;
const realFetch = globalThis.fetch;
globalThis.Request = class extends RealRequest {
  constructor(input, init) { super(typeof input === "string" ? new URL(input, BASE).href : input, init); }
};
globalThis.fetch = async (input) => {
  const url = new URL(typeof input === "string" ? input : input.url, BASE);
  if (url.protocol === "data:") return realFetch(url.href);
  if (url.protocol !== "file:") throw new TypeError("Failed to fetch");
  let bytes;
  try { bytes = fs.readFileSync(fileURLToPath(url)); } catch { throw new TypeError("Failed to fetch"); }
  return new Response(bytes);
};
await import(BASE);
`);
  const worker = new OracleWorker(loader, { type: "module" });
  const result = await new Promise<WorkerResult>((resolve) => {
    const timer = setTimeout(() => resolve({ never: "timeout" }), timeoutMs);
    worker.onmessage = (event) => {
      clearTimeout(timer);
      resolve({ ok: event.data.payload });
    };
    worker.onerror = (event) => {
      clearTimeout(timer);
      resolve({ never: String(event.message) });
    };
    worker.postMessage({ id: 1, payload });
  });
  worker.terminate();
  await terminatedWorkersExited();
  return result;
}

export function encodePath(pathStr: string): string {
  return pathStr
    .split(path.sep)
    .map((component, index) => (index === 0 && component.endsWith(":") ? component : encodeURIComponent(component)))
    .join(path.sep);
}
