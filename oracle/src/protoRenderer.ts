import { UPSTREAM_DIR } from "./shims";
import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { encode } from "./cve1";
import { Corpus } from "./corpus/corpus";
import { protoMatrix } from "./corpus/proto";
import { runWpilogWorker } from "./renderer/workerCheck";

const USAGE = `usage: protoRenderer --seed <u32>

Decodes corpus A's protobuf cases (payload/proto/*) with upstream's real wpilogWorker bundle
twice: in a web worker of a hidden Electron BrowserWindow (the real renderer runtime) and in
the oracle's worker_threads shim. Prints one CVE-1 line per case with the renderer's initial
and fields messages, after a header line. Exits non-zero, printing the first differing case,
if the two runtimes disagree.`;

const i = process.argv.indexOf("--seed");
const seed = i >= 0 ? Number(process.argv[i + 1]) : NaN;
if (!Number.isInteger(seed) || seed < 0 || seed > 0xffffffff) {
  console.error(USAGE);
  process.exit(2);
}

const corpus = new Corpus(seed);
protoMatrix(corpus);
const cases = corpus.cases.map((c) => ({
  id: c.id,
  bytes: c.files.find((f) => f.path.endsWith(".wpilog"))!.bytes
}));
const workerUrl = pathToFileURL(path.join(UPSTREAM_DIR, "bundles", "wpilogWorker.js")).href;

const harness: string[] = [];
for (const c of cases) {
  harness.push(encode({ case: c.id, responses: await runWpilogWorker(workerUrl, c.bytes) }));
}

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-renderer-"));
let renderer: string[];
let pageEnvironment: string;
try {
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(
    requestPath,
    JSON.stringify({ workerUrl, cases: cases.map((c) => ({ id: c.id, base64: Buffer.from(c.bytes).toString("base64") })) })
  );
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  const main = path.join(path.dirname(fileURLToPath(import.meta.url)), "rendererMain.mjs");
  const run = spawnSync(process.execPath, [main, requestPath], { env, encoding: "utf8", maxBuffer: 1 << 30 });
  if (run.status !== 0) {
    console.error(run.stderr);
    throw new Error(`Electron renderer run exited with ${run.status ?? run.signal}`);
  }
  [pageEnvironment, ...renderer] = run.stdout.split(/\r?\n/).filter((l) => l.length > 0);
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}

if (pageEnvironment !== encode({ processType: "undefined" })) {
  throw new Error(`the renderer page has Node globals: ${pageEnvironment}`);
}
if (renderer.length !== harness.length) throw new Error(`renderer returned ${renderer.length} of ${harness.length} cases`);
for (let k = 0; k < harness.length; k++) {
  if (renderer[k] !== harness[k]) {
    console.error(`case ${cases[k].id}: renderer and harness differ\nrenderer: ${renderer[k]}\nharness:  ${harness[k]}`);
    process.exit(1);
  }
}

const header = encode({
  bundle: "bundles/wpilogWorker.js",
  runtime: "Electron renderer, module web worker, no Node integration",
  electron: process.versions.electron,
  harnessIdentical: true
});
process.stdout.write([header, ...renderer].map((l) => l + "\n").join(""));
