import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { UPSTREAM_DIR } from "./shims";
import { readFixture } from "./dom/fixtureFile";

// focusRings [<spec or fixture file>]
//   Which element of a real Chromium page with upstream's global.css takes its focus ring, after
//   trusted keyboard and mouse input and script's focus, in active and inactive windows. Runs
//   focusRingsMain.mjs in an Electron main process and prints its records. See oracle/README.md,
//   "Focus ring vectors".

const { fixture } = readFixture("focusRings", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-focus-rings-"));
let run;
try {
  const css = fixture.css.map((name: string) => fs.readFileSync(path.join(UPSTREAM_DIR, "www", name), "utf8"));
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(requestPath, JSON.stringify({ fixture: { ...fixture, css }, page: path.join(scratch, "page.html"), userData: path.join(scratch, "userData") }));
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  // A page in a window that is not shown needs no display.
  const platform = process.platform === "linux" ? ["--ozone-platform=headless"] : [];
  run = spawnSync(process.execPath, [...platform, "--lang=en-US", "--log-level=3", path.join(here, "focusRingsMain.mjs"), requestPath], {
    env,
    encoding: "utf8",
    maxBuffer: 1 << 30,
    stdio: ["ignore", "pipe", "inherit"]
  });
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`focusRings: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(run.stdout, () => process.exit(0));
