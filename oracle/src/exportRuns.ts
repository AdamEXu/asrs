import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import WorkerManager from "@upstream/hub/WorkerManager";
import { toTree } from "./cve1";
import { ExportOptions, HistoricalSession } from "./historical/session";

// exportRuns <runs.json>
//   [{"file": <log, relative to the runs file>, "exports": [ExportOptions, ...]}, ...]
//   Decodes each file as `export` does and runs the real export worker once per options. One
//   record per run: the values it posted through WorkerManager's progress callback, in order, and
//   the bytes main would write, or null when the worker rejected.

const file = process.argv[2];
if (file === undefined) {
  process.stderr.write("usage: exportRuns <runs.json>\n");
  process.exit(2);
}
const runs: { file: string; exports: ExportOptions[] }[] = JSON.parse(fs.readFileSync(file, "utf8"));

let progress: number[] = [];
const request = WorkerManager.request;
WorkerManager.request = (script: string, payload: any) =>
  request.call(WorkerManager, script, payload, (value: number) => progress.push(value));

const lines: string[] = [];
for (const run of runs) {
  const session = await HistoricalSession.open(path.resolve(path.dirname(file), run.file));
  try {
    await session.loadEverything();
    for (const options of run.exports) {
      progress = [];
      const result = await session.export(options, 100000);
      if (result.outcome === "skipped") throw new Error(`${run.file}: skipped: ${result.reason}`);
      lines.push(
        JSON.stringify({
          file: run.file,
          options: toTree(options),
          progress: toTree(progress),
          bytes: toTree(result.outcome === "ok" ? result.bytes : null)
        })
      );
    }
  } finally {
    await session.close();
  }
}
process.stdout.write(lines.map((line) => line + "\n").join(""));
