import "./shims";
import * as fs from "node:fs";
import * as path from "node:path";
import { buildCorpusA } from "./corpus/matrices";

const USAGE = "usage: corpusA --seed <u32> --out <dir>";

function option(name: string): string {
  const i = process.argv.indexOf(name);
  const value = i >= 0 ? process.argv[i + 1] : undefined;
  if (value === undefined) throw new Error(USAGE);
  return value;
}

const seed = Number(option("--seed"));
if (!Number.isInteger(seed) || seed < 0 || seed > 0xffffffff) throw new Error(`${USAGE}: bad seed`);
const out = path.resolve(option("--out"));

const corpus = buildCorpusA(seed);

const lines: string[] = [];
for (const c of [...corpus.cases].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))) {
  for (const file of c.files) {
    const target = path.join(out, file.path);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, file.bytes);
  }
  lines.push(JSON.stringify({ id: c.id, about: c.about, files: c.files.map((f) => f.path) }));
}
process.stdout.write(lines.map((l) => l + "\n").join(""));
