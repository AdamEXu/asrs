import { app, BrowserWindow } from "electron";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { encodePng } from "../src/canvas/png";

const USAGE = "usage: electron field3dMain.mjs <scenes dir> <out dir> [<scene>...]";
const THEMES = ["dark", "light"];
const SCALES = [1, 2];

const args = process.argv.slice(process.argv.findIndex((a) => /field3dMain\.mjs$/.test(a)) + 1);
const [scenesDir, outDir, ...only] = args;
if (scenesDir === undefined || outDir === undefined) {
  console.error(USAGE);
  process.exit(2);
}
const here = path.dirname(fileURLToPath(import.meta.url));

app.commandLine.appendSwitch("lang", "en-US");

app.whenReady().then(async () => {
  try {
    const win = new BrowserWindow({
      show: false,
      width: 400,
      height: 300,
      paintWhenInitiallyHidden: true,
      webPreferences: { backgroundThrottling: false }
    });
    await win.loadFile(path.join(here, "field3dPage.html"));
    const names = fs
      .readdirSync(scenesDir)
      .filter((f) => f.endsWith(".json"))
      .map((f) => f.slice(0, -5))
      .filter((n) => only.length === 0 || only.includes(n))
      .sort();
    fs.mkdirSync(outDir, { recursive: true });
    const info: Record<string, unknown> = {};
    for (const name of names) {
      const spec = JSON.parse(fs.readFileSync(path.join(scenesDir, name + ".json"), "utf8"));
      for (const scale of SCALES) {
        for (const theme of THEMES) {
          const result = await win.webContents.executeJavaScript(
            `renderScene(${JSON.stringify(spec)}, ${theme === "dark"}, ${scale})`
          );
          const file = path.join(outDir, `${name}@${scale}x-${theme}.png`);
          fs.writeFileSync(file, encodePng(result.width, result.height, Buffer.from(result.rgba, "base64")));
          info.glRenderer = result.glRenderer;
          info.samples = result.samples;
        }
      }
      console.error(`  ${name}`);
    }
    info.electron = process.versions.electron;
    info.chrome = process.versions.chrome;
    info.platform = process.platform;
    info.arch = process.arch;
    fs.writeFileSync(path.join(outDir, "host.json"), JSON.stringify(info, null, 1) + "\n");
    app.exit(0);
  } catch (e) {
    console.error(e);
    app.exit(1);
  }
});
