import "./shims";
import * as path from "node:path";
import { HistoricalSession } from "./historical/session";

const USAGE = `usage: decodeBench --startup | [--initial] <log file>

Opens the file as \`decode\` does and prints one JSON line: how long the initial load and the load
of every field took (wall clock), and what was loaded. --initial stops after the initial load.
--startup exits before opening anything, so its resource use can be subtracted.`;

const args = process.argv.slice(2);
if (args[0] === "--startup") process.exit(0);
const initialOnly = args[0] === "--initial";
const file = args[initialOnly ? 1 : 0];
if (file === undefined || file.startsWith("-")) {
  console.error(USAGE);
  process.exit(2);
}

const now = () => Number(process.hrtime.bigint()) / 1e6;
const start = now();
const session = await HistoricalSession.open(path.resolve(file));
const opened = now();
if (!initialOnly) await session.loadEverything();
const loaded = now();
const fields = Object.values(session.log.toSerialized().fields) as { timestamps: number[] }[];
const samples = fields.reduce((sum, field) => sum + field.timestamps.length, 0);
process.stdout.write(
  JSON.stringify({ initialMs: opened - start, fieldsMs: loaded - opened, fields: fields.length, samples }) + "\n"
);
await session.close();
