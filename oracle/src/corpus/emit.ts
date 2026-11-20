import * as fs from "node:fs";
import * as path from "node:path";
import { Corpus } from "./corpus";

export function option(usage: string, name: string): string {
  const i = process.argv.indexOf(name);
  const value = i >= 0 ? process.argv[i + 1] : undefined;
  if (value === undefined) throw new Error(usage);
  return value;
}

export function seedOption(usage: string): number {
  const seed = Number(option(usage, "--seed"));
  if (!Number.isInteger(seed) || seed < 0 || seed > 0xffffffff) throw new Error(`${usage}: bad seed`);
  return seed;
}

/**
 * Writes every case under `out` and prints the case list the xtask manifest builder reads. Resolves once stdout has
 * taken the list: a pipe write is asynchronous, and exiting before it drains truncates the list past the pipe buffer.
 */
export function emitCorpus(corpus: Corpus, out: string): Promise<void> {
  const lines: string[] = [];
  for (const c of [...corpus.cases].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))) {
    for (const file of c.files) {
      const target = path.join(out, file.path);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(target, file.bytes);
    }
    lines.push(JSON.stringify({ id: c.id, about: c.about, files: c.files.map((f) => f.path) }));
  }
  const list = lines.map((l) => l + "\n").join("");
  return new Promise((resolve, reject) => process.stdout.write(list, (error) => (error ? reject(error) : resolve())));
}

/** Runs `fn` with console output from upstream code silenced (warnings about clamped CAN signals and the like). */
export function quietly<T>(fn: () => T): T {
  const saved = [console.log, console.warn, console.error];
  console.log = console.warn = console.error = () => {};
  try {
    return fn();
  } finally {
    [console.log, console.warn, console.error] = saved;
  }
}
