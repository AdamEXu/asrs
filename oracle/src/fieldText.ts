import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { UPSTREAM_DIR } from "./shims";
import { readFixture } from "./dom/fixtureFile";

// fieldText [<spec or fixture file>]
//   The pixels of focused inputs under upstream's stylesheets. Runs fieldTextMain.mjs in an Electron
//   main process per device scale and prints its records. See oracle/README.md, "Field text vectors".

const { fixture } = readFixture("fieldText", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const controls = fixture.controls.map((control: any) => ({
  ...control,
  css: control.css.map((name: string) => fs.readFileSync(path.join(UPSTREAM_DIR, "www", name), "utf8"))
}));
let out = "";
for (const scale of fixture.scales) {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-field-text-"));
  let run;
  try {
    const requestPath = path.join(scratch, "request.json");
    fs.writeFileSync(requestPath, JSON.stringify({ fixture: { ...fixture, controls }, scale, base: pathToFileURL(path.join(UPSTREAM_DIR, "www") + path.sep).href, page: path.join(scratch, "page.html"), userData: path.join(scratch, "userData") }));
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
    const gpu = ["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"];
    const platform = process.platform === "linux" ? ["--ozone-platform=headless", ...gpu] : [];
    run = spawnSync(
      process.execPath,
      [...platform, "--lang=en-US", "--log-level=3", "--force-color-profile=srgb", `--force-device-scale-factor=${scale}`, path.join(here, "fieldTextMain.mjs"), requestPath],
      { env, encoding: "utf8", maxBuffer: 1 << 30, stdio: ["ignore", "pipe", "inherit"] }
    );
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
  if (run.status !== 0) {
    process.stderr.write(`fieldText: Electron exited with ${run.status ?? run.signal}\n`);
    process.exit(1);
  }
  out += run.stdout;
}
process.stdout.write(out, () => process.exit(0));
