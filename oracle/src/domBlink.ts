import { UPSTREAM_DIR } from "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readFixture } from "./dom/fixtureFile";

// domBlink [<spec or fixture file>]
//   The `dom` harness with Blink as the DOM: the same fixture and page bundle (domPage.js) run in
//   an offscreen Electron BrowserWindow at the fixture's window size and device scale, driven by
//   domBlinkMain.mjs in a real Electron main process. Prints the same records as `dom`. See
//   oracle/README.md, "Blink backend".

const { fixture } = readFixture("domBlink", process.argv[2]);
const view = fixture.window ?? {};
const geometry = fixture.geometry ?? "scripted";
if (geometry !== "scripted" && geometry !== "real") {
  process.stderr.write(`domBlink: geometry must be "scripted" or "real", not ${JSON.stringify(geometry)}\n`);
  process.exit(2);
}
const scale = view.devicePixelRatio ?? 1;
if (scale !== 1 && scale !== 2) {
  process.stderr.write(`domBlink: devicePixelRatio must be 1 or 2, not ${JSON.stringify(scale)}\n`);
  process.exit(2);
}

const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-dom-blink-"));
let run;
try {
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(
    requestPath,
    JSON.stringify({
      fixture,
      upstream: UPSTREAM_DIR,
      page: path.join(here, "domPage.js"),
      boot: path.join(here, "domBlinkBoot.js"),
      zone: process.env.ASRS_ORACLE_ZONE,
      userData: path.join(scratch, "userData")
    })
  );
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  run = spawnSync(
    process.execPath,
    [`--force-device-scale-factor=${scale}`, "--lang=en-US", path.join(here, "domBlinkMain.mjs"), requestPath],
    { env, encoding: "utf8", maxBuffer: 1 << 30, stdio: ["ignore", "pipe", "inherit"] }
  );
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`domBlink: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(run.stdout, () => process.exit(0));
