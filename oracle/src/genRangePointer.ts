import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { UPSTREAM_DIR } from "./shims";
import { readFixture } from "./dom/fixtureFile";

// genRangePointer [<spec or fixture file>]
//   Trusted pointer and keyboard input to the video timeline slider under upstream's hub.css in a
//   real Chromium page, through the DevTools protocol. Runs genRangePointerMain.mjs in an Electron
//   main process and prints its records. See oracle/README.md, "Range pointer vectors".

const { fixture } = readFixture("genRangePointer", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-range-pointer-"));
let run;
try {
  const css = ["global.css", "hub.css"].map((file) => fs.readFileSync(path.join(UPSTREAM_DIR, "www", file), "utf8")).join("\n");
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(requestPath, JSON.stringify({ fixture, css, page: path.join(scratch, "page.html"), userData: path.join(scratch, "userData") }));
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  run = spawnSync(
    process.execPath,
    [`--force-device-scale-factor=${fixture.scale ?? 1}`, "--lang=en-US", "--log-level=3", path.join(here, "genRangePointerMain.mjs"), requestPath],
    { env, encoding: "utf8", maxBuffer: 1 << 30, stdio: ["ignore", "pipe", "inherit"] }
  );
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`genRangePointer: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(run.stdout, () => process.exit(0));
