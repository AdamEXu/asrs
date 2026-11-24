import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readFixture } from "./dom/fixtureFile";

// spinButtons [<spec or fixture file>]
//   Trusted mouse input to the spin buttons of a number input in a real Chromium page, through the
//   DevTools protocol. Runs spinButtonsMain.mjs in an Electron main process and prints its
//   records. See oracle/README.md, "Spin button vectors".

const { fixture } = readFixture("spinButtons", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-spin-buttons-"));
let run;
try {
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(requestPath, JSON.stringify({ fixture, userData: path.join(scratch, "userData") }));
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  run = spawnSync(process.execPath, ["--lang=en-US", "--log-level=3", path.join(here, "spinButtonsMain.mjs"), requestPath], {
    env,
    encoding: "utf8",
    maxBuffer: 1 << 30,
    stdio: ["ignore", "pipe", "inherit"]
  });
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`spinButtons: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(run.stdout, () => process.exit(0));
