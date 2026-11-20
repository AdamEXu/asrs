import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import { option, seedOption } from "./corpus/emit";
import { mutate } from "./corpus/fuzz";
import { Rng } from "./corpus/rng";

const USAGE = "usage: corpusFuzz --seed <u32> --count <n> --out <dir> --in <dir> [--in <dir>...]";
const EXTENSIONS = new Set([".wpilog", ".rlog", ".dslog", ".dsevents", ".log", ".csv"]);

function byName(a: fs.Dirent, b: fs.Dirent): number {
  return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
}

function listFiles(dir: string, out: string[] = []): string[] {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort(byName)) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) listFiles(full, out);
    else if (EXTENSIONS.has(path.extname(entry.name))) out.push(full);
  }
  return out;
}

const seed = seedOption(USAGE);
const count = Number(option(USAGE, "--count"));
if (!Number.isInteger(count) || count < 1) throw new Error(`${USAGE}: bad count`);
const out = path.resolve(option(USAGE, "--out"));
const inputs = process.argv.flatMap((arg, i) => (arg === "--in" && process.argv[i + 1] !== undefined ? [path.resolve(process.argv[i + 1])] : []));
if (inputs.length === 0) throw new Error(USAGE);

const byExtension = new Map<string, { rel: string; full: string }[]>();
for (const dir of inputs) {
  for (const full of listFiles(dir)) {
    const ext = path.extname(full);
    if (!byExtension.has(ext)) byExtension.set(ext, []);
    byExtension.get(ext)!.push({ rel: path.basename(dir) + "/" + path.relative(dir, full).split(path.sep).join("/"), full });
  }
}
const extensions = [...byExtension.keys()].sort();
const width = String(count - 1).length;
fs.mkdirSync(out, { recursive: true });
const lines: string[] = [];
for (let i = 0; i < count; i++) {
  const index = String(i).padStart(width, "0");
  const rng = new Rng(seed, `fuzz/${index}`);
  const pool = byExtension.get(rng.pick(extensions))!;
  const source = rng.pick(pool);
  const donor = rng.pick(pool);
  const { bytes, steps } = mutate(fs.readFileSync(source.full), rng, fs.readFileSync(donor.full), 8);
  const rel = `${index}${path.extname(source.full)}`;
  fs.writeFileSync(path.join(out, rel), bytes);
  lines.push(JSON.stringify({ id: `fuzz/${index}`, about: `${source.rel} mutated (donor ${donor.rel}): ${steps.join(" ")}`, files: [rel] }));
}
process.stdout.write(lines.map((l) => l + "\n").join(""));
