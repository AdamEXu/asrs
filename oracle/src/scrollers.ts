import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { UPSTREAM_DIR } from "./shims";
import { readFixture } from "./dom/fixtureFile";

// scrollers [<spec or fixture file> [<shot dir>]]
//   How a real Chromium page scrolls for trusted keys, wheel events and scrollbar presses, and how
//   its scrollbars look. Runs scrollersMain.mjs in an Electron main process, once for each of the
//   fixture's "styles" and "scales", and prints its records. See oracle/README.md, "Scroller vectors".

const { fixture } = readFixture("scrollers", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const css = (fixture.css ?? []).map((name: string) => fs.readFileSync(path.join(UPSTREAM_DIR, "www", name), "utf8"));
let out = "";
const styles = process.platform === "darwin" ? fixture.styles ?? ["overlay"] : ["fluent"];
for (const style of styles) {
  for (const scale of fixture.scales ?? [1]) {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-scrollers-"));
    let run;
    try {
      const requestPath = path.join(scratch, "request.json");
      fs.writeFileSync(
        requestPath,
        JSON.stringify({ fixture: { ...fixture, css }, style, scale, page: path.join(scratch, "page.html"), shots: process.argv[3] ?? null, userData: path.join(scratch, "userData") })
      );
      const env = { ...process.env };
      delete env.ELECTRON_RUN_AS_NODE;
      env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
      // Rasterized as with a GPU, whatever the host's are and whether it has a display.
      const gpu = ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"];
      const platform = process.platform === "linux" ? ["--ozone-platform=headless", ...gpu] : [];
      // The argument domain of NSUserDefaults, which this process alone sees.
      const defaults = process.platform === "darwin" ? ["-AppleShowScrollBars", style === "classic" ? "Always" : "WhenScrolling", "-AppleScrollerPagingBehavior", fixture.jump ? "YES" : "NO"] : [];
      run = spawnSync(
        process.execPath,
        [...platform, "--lang=en-US", "--log-level=3", "--force-color-profile=srgb", `--force-device-scale-factor=${scale}`, path.join(here, "scrollersMain.mjs"), ...defaults, requestPath],
        { env, encoding: "utf8", maxBuffer: 1 << 30, stdio: ["ignore", "pipe", "inherit"] }
      );
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
    if (run.status !== 0) {
      process.stderr.write(`scrollers: Electron exited with ${run.status ?? run.signal}\n`);
      process.exit(1);
    }
    out += run.stdout;
  }
}
process.stdout.write(out, () => process.exit(0));
