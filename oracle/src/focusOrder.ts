import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readFixture } from "./dom/fixtureFile";

// focusOrder [<spec or fixture file>]
//   What has the focus in upstream's real app after each Tab and Shift+Tab of the fixture's
//   scenarios. Runs appMain.mjs on each as `cargo xtask app run` does, at 1x in the light theme, and
//   prints what its `traverse` actions kept, after the logs the scenarios open. See oracle/README.md,
//   "Focus order vectors".

const { file, fixture } = readFixture("focusOrder", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-focus-order-"));
const lines: string[] = [];
const write = (record: object) => lines.push(JSON.stringify(record) + "\n");
// appMain.ts requires exactly these.
const SWITCHES = [
  "--force-device-scale-factor=1",
  "--lang=en-US",
  "--disable-accelerated-2d-canvas",
  "--js-flags=--random-seed=1",
  "--force-prefers-reduced-motion",
  "--disable-partial-raster",
  "--force-color-profile=srgb"
];

try {
  const logs = new Set<string>();
  for (const relative of fixture.scenarios as string[]) {
    const scenario = path.resolve(path.dirname(file), relative);
    const name = path.basename(scenario, ".json");
    const out = path.join(scratch, name);
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
    const platform = process.platform === "linux" ? ["--ozone-platform=headless"] : [];
    const args = [...platform, ...SWITCHES, path.join(here, "appMain.mjs"), scenario, out, "--scale=1", "--theme=light"];
    const run = spawnSync(process.execPath, args, { env, stdio: ["ignore", "inherit", "inherit"] });
    if (run.status !== 0) throw new Error(`${name}: Electron exited with ${run.status ?? run.signal}`);
    const inputs = path.join(out, "inputs");
    for (const log of fs.existsSync(inputs) ? fs.readdirSync(inputs).sort() : []) {
      if (!log.endsWith(".wpilog") || logs.has(log)) continue;
      logs.add(log);
      write({ log, bytes: fs.readFileSync(path.join(inputs, log)).toString("hex") });
    }
    for (const action of JSON.parse(fs.readFileSync(scenario, "utf8")).actions) {
      if (action.traverse === undefined) continue;
      const text = fs.readFileSync(path.join(out, "files", "focus", `${action.traverse}.json`), "utf8");
      text
        .trimEnd()
        .split("\n")
        .forEach((line, index) => write({ scenario: name, traverse: action.traverse, index, focused: JSON.parse(line) }));
    }
  }
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
process.stdout.write(lines.join(""), () => process.exit(0));
