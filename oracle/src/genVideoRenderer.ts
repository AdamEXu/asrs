import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import VideoRenderer from "@upstream/shared/renderers/VideoRenderer";
import { encodePng } from "./canvas/png";
import { readFixture } from "./dom/fixtureFile";

// genVideoRenderer [<spec or fixture file>]
//   Upstream's VideoRenderer on a real Blink <img>, loading frame files from disk. Runs
//   genVideoRendererMain.mjs in an Electron main process and prints its records. See
//   oracle/README.md, "Video renderer vectors".

const { fixture } = readFixture("genVideoRenderer", process.argv[2]);
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-video-renderer-"));
let run;
try {
  for (const [name, [width, height]] of Object.entries(fixture.frames as { [name: string]: [number, number] })) {
    fs.writeFileSync(path.join(scratch, name), encodePng(width, height, Buffer.alloc(width * height * 4, 255)));
  }
  fs.writeFileSync(path.join(scratch, "corrupt.jpg"), "not an image");
  fs.writeFileSync(path.join(scratch, "page.html"), '<!doctype html><div id="root"><div class="video-container"><img /></div></div>');
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(
    requestPath,
    JSON.stringify({
      fixture,
      page: path.join(scratch, "page.html"),
      renderer: VideoRenderer.toString(),
      userData: path.join(scratch, "userData")
    })
  );
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  run = spawnSync(process.execPath, ["--lang=en-US", "--log-level=3", path.join(here, "genVideoRendererMain.mjs"), requestPath], {
    env,
    encoding: "utf8",
    maxBuffer: 1 << 30,
    stdio: ["ignore", "pipe", "inherit"]
  });
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`genVideoRenderer: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
process.stdout.write(run.stdout, () => process.exit(0));
