import "./shims";
import * as path from "node:path";
import { HistoricalSession } from "./historical/session";

const USAGE = `usage: decode <log file>

Opens the file with the real HistoricalDataSource and worker bundles and prints the canonical
Log dump (CVE-1 lines) twice: phase "initial" right after the worker's initial message, then
phase "full" after every field has been requested (see oracle/README.md).`;

const file = process.argv[2];
if (file === undefined || file.startsWith("-")) {
  console.error(USAGE);
  process.exit(2);
}

const session = await HistoricalSession.open(path.resolve(file));
try {
  const lines = session.dump("initial");
  await session.loadEverything();
  lines.push(...session.dump("full"));
  process.stdout.write(lines.map((l) => l + "\n").join(""));
} finally {
  session.close();
}
