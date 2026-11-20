import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import * as readline from "node:readline";
import { ExportOptions, HistoricalSession } from "./historical/session";

const USAGE = `usage: historicalRun --root <corpus dir> --out <dir> --spec <spec.json>

Reads input paths (relative to --root) on stdin, one per line. For each input it writes
<out>/<input>/decode.jsonl (the \`decode\` dump) and one file per export in the spec's matrix,
<out>/<input>/export/<name>.<ext>, then prints one JSON line:
  {"input": "<input>", "outputs": [{"name", "outcome", "file"?}, ...]}
where outcome is "ok" (file written), "failed" (the export worker rejected) or "skipped" (fixed
sampling over too large a range). An input the oracle itself cannot handle prints
{"input", "error"} instead.`;

interface ExportMatrix {
  formats: ExportOptions["format"][];
  samplingModes: ExportOptions["samplingMode"][];
  samplingPeriod: number;
  includeGenerated: boolean[];
  prefixes: string[];
  fixedSampleLimit: number;
}

const EXTENSIONS: { [format: string]: string } = { "csv-table": "csv", "csv-list": "csv", wpilog: "wpilog", mcap: "mcap" };

function option(name: string): string {
  const i = process.argv.indexOf(name);
  const value = i >= 0 ? process.argv[i + 1] : undefined;
  if (value === undefined) {
    console.error(USAGE);
    process.exit(2);
  }
  return value;
}

function exportName(options: ExportOptions, prefixIndex: number): string {
  const generated = options.includeGenerated ? "generated" : "original";
  return `${options.format}.${options.samplingMode}.${generated}.p${prefixIndex}`;
}

const root = path.resolve(option("--root"));
const out = path.resolve(option("--out"));
const matrix: ExportMatrix = JSON.parse(fs.readFileSync(option("--spec"), "utf8")).export;

const inputs: string[] = [];
for await (const line of readline.createInterface({ input: process.stdin })) {
  if (line.trim().length > 0) inputs.push(line.trim());
}

for (const input of inputs) {
  const dir = path.join(out, input);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(path.join(dir, "export"), { recursive: true });
  let session: HistoricalSession | null = null;
  try {
    session = await HistoricalSession.open(path.join(root, input));
    const lines = session.dump("initial");
    await session.loadEverything();
    lines.push(...session.dump("full"));
    fs.writeFileSync(path.join(dir, "decode.jsonl"), lines.map((l) => l + "\n").join(""));
    const outputs: object[] = [{ name: "decode", outcome: "ok", file: "decode.jsonl" }];
    for (const format of matrix.formats) {
      for (const samplingMode of matrix.samplingModes) {
        for (const includeGenerated of matrix.includeGenerated) {
          for (let p = 0; p < matrix.prefixes.length; p++) {
            const options: ExportOptions = {
              format,
              samplingMode,
              samplingPeriod: matrix.samplingPeriod,
              prefixes: matrix.prefixes[p],
              includeGenerated
            };
            const name = exportName(options, p);
            const result = await session.export(options, matrix.fixedSampleLimit);
            if (result.outcome === "ok") {
              const file = `export/${name}.${EXTENSIONS[format]}`;
              fs.writeFileSync(path.join(dir, file), result.bytes);
              outputs.push({ name, outcome: "ok", file });
            } else {
              outputs.push({ name, outcome: result.outcome });
            }
          }
        }
      }
    }
    process.stdout.write(JSON.stringify({ input, outputs }) + "\n");
  } catch (error) {
    process.stdout.write(JSON.stringify({ input, error: String((error as Error)?.stack ?? error) }) + "\n");
  } finally {
    session?.close();
  }
}
