import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import { ExportOptions, HistoricalSession } from "./historical/session";

const USAGE = `usage: export <log file> --options <ExportOptions JSON> [--out <file>] [--fixed-limit <n>]

Decodes the file like \`decode\`, then runs the real export worker and writes the exact bytes
main would write to --out (default stdout). Exit status 0 when the worker produced output,
3 when it rejected (the hub shows "Failed to export data"), 4 when fixed-period sampling was
skipped because the range needs more than --fixed-limit samples (default 100000).`;

function option(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const file = process.argv[2];
const optionsText = option("--options");
if (file === undefined || file.startsWith("-") || optionsText === undefined) {
  console.error(USAGE);
  process.exit(2);
}
const options: ExportOptions = JSON.parse(optionsText);
const out = option("--out");
const limit = Number(option("--fixed-limit") ?? 100000);

const session = await HistoricalSession.open(path.resolve(file));
let code = 0;
try {
  await session.loadEverything();
  const result = await session.export(options, limit);
  if (result.outcome === "ok") {
    if (out === undefined) process.stdout.write(result.bytes);
    else fs.writeFileSync(out, result.bytes);
  } else {
    console.error(`export ${result.outcome}${result.outcome === "skipped" ? `: ${result.reason}` : ""}`);
    code = result.outcome === "failed" ? 3 : 4;
  }
} finally {
  session.close();
}
process.exitCode = code;
