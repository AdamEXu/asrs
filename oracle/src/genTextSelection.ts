import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { UPSTREAM_DIR } from "./shims";
import { readFixture } from "./dom/fixtureFile";

// genTextSelection [<spec or fixture file>]
//   Trusted pointer input over pages with upstream's stylesheets in a real Chromium page: what is
//   selected after each operation and what a copy takes. Runs genTextSelectionMain.mjs in an
//   Electron main process and prints its records. See oracle/README.md, "Text selection vectors".

const { file, fixture } = readFixture("genTextSelection", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-text-selection-"));
let run;
try {
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  if (process.platform === "linux") {
    // The clipboard is the X server's.
    env.DISPLAY ??= ":0";
    env.XAUTHORITY ??= path.join(os.homedir(), ".Xauthority");
  }
  if (process.platform === "darwin") {
    const library = path.join(scratch, "pasteboard.dylib");
    const source = path.resolve(path.dirname(file), "../../../oracle/tools/pasteboard.m");
    const cc = spawnSync("cc", ["-dynamiclib", "-framework", "AppKit", "-o", library, source], { encoding: "utf8" });
    if (cc.status !== 0) throw new Error(`genTextSelection: cc: ${cc.stderr}`);
    env.DYLD_INSERT_LIBRARIES = library;
  }
  const css = (name: string) => fs.readFileSync(path.join(UPSTREAM_DIR, "www", name), "utf8");
  const pages = fixture.pages.map((page: any) => ({ ...page, css: page.css.map(css).join("\n") }));
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(requestPath, JSON.stringify({ fixture: { ...fixture, pages }, page: path.join(scratch, "page.html"), userData: path.join(scratch, "userData") }));
  run = spawnSync(
    process.execPath,
    ["--force-device-scale-factor=1", "--lang=en-US", "--log-level=3", path.join(here, "genTextSelectionMain.mjs"), requestPath],
    { env, encoding: "utf8", maxBuffer: 1 << 30, stdio: ["ignore", "pipe", "inherit"] }
  );
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`genTextSelection: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(run.stdout, () => process.exit(0));
