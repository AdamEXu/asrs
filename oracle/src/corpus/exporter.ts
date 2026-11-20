import Log from "@upstream/shared/log/Log";
import type ExportOptions from "@upstream/shared/ExportOptions";

let nextId = 1;

/** Runs upstream's real export worker bundle on a log; resolves to null when the worker rejects. */
export function exportLog(log: Log, options: ExportOptions): Promise<string | Uint8Array | null> {
  return new Promise((resolve, reject) => {
    const worker = new Worker("../bundles/exportWorker.js", { type: "module" });
    const id = nextId++;
    worker.onerror = (e: any) => {
      worker.terminate();
      reject(new Error(`exportWorker: ${e.message}`));
    };
    worker.onmessage = (e: MessageEvent) => {
      const data = e.data as { id: number; payload?: string | Uint8Array; progress?: number };
      if (data.id !== id || "progress" in data) return;
      worker.terminate();
      resolve(data.payload === undefined ? null : data.payload);
    };
    worker.postMessage({ id, payload: { options, log: log.toSerialized() } });
  });
}

export function exportOptions(overrides: Partial<ExportOptions>): ExportOptions {
  return { format: "csv-table", samplingMode: "changes", samplingPeriod: 20, prefixes: "", includeGenerated: true, ...overrides };
}

export async function exportBytes(log: Log, options: Partial<ExportOptions>): Promise<Uint8Array> {
  const result = await exportLog(log, exportOptions(options));
  if (result === null) throw new Error(`export rejected: ${JSON.stringify(options)}`);
  return typeof result === "string" ? new TextEncoder().encode(result) : Uint8Array.from(result);
}
