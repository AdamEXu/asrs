import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { UPSTREAM_DIR } from "./shims";
import { readFixture } from "./dom/fixtureFile";

// genLiveNet [<spec or fixture file>]
//   `fetch`, `WebSocket`, `Response.text()`, `Response.json()` and timer order in a real Chromium
//   page with the hub's origin and content security policy. Runs genLiveNetMain.mjs in an Electron
//   main process and prints its records. See oracle/README.md, "Live network vectors".

const { fixture } = readFixture("genLiveNet", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-live-net-"));
const output = path.join(scratch, "records.jsonl");
let run;
let records = "";
try {
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  const hub = fs.readFileSync(path.join(UPSTREAM_DIR, "www", "hub.html"), "utf8");
  const policy = /http-equiv="Content-Security-Policy"\s+content="([^"]*)"/.exec(hub);
  if (policy === null) throw new Error("genLiveNet: hub.html has no content security policy");
  const page = path.join(scratch, "page.html");
  fs.writeFileSync(page, `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${policy[1]}"></head></html>`);
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(requestPath, JSON.stringify({ fixture, output, page, userData: path.join(scratch, "userData") }));
  run = spawnSync(
    process.execPath,
    [...(process.platform === "linux" ? ["--ozone-platform=headless"] : []), "--lang=en-US", "--log-level=3", "--no-proxy-server", path.join(here, "genLiveNetMain.mjs"), requestPath],
    { env, stdio: ["ignore", "ignore", "inherit"] }
  );
  if (run.status === 0) records = fs.readFileSync(output, "utf8");
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`genLiveNet: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(records, () => process.exit(0));
