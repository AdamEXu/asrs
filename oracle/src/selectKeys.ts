import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readFixture } from "./dom/fixtureFile";

// selectKeys [<spec or fixture file>]
//   Trusted keyboard and mouse input to a closed <select> in a real Chromium page, through the
//   DevTools protocol so each key event carries the fixture's timestamp (type-ahead compares
//   event times). Runs selectKeysMain.mjs in an Electron main process and prints its records.
//   See oracle/README.md, "Select input vectors".

const { fixture } = readFixture("selectKeys", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-select-keys-"));
let run;
try {
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(requestPath, JSON.stringify({ fixture, userData: path.join(scratch, "userData") }));
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  run = spawnSync(process.execPath, ["--lang=en-US", "--log-level=3", path.join(here, "selectKeysMain.mjs"), requestPath], {
    env,
    encoding: "utf8",
    maxBuffer: 1 << 30,
    stdio: ["ignore", "pipe", "inherit"]
  });
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`selectKeys: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(run.stdout, () => process.exit(0));
