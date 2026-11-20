import { app, BrowserWindow } from "electron";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { encodePng } from "../src/canvas/png";
import { BOXES_SCRIPT, sceneHtml, textScenes } from "./scenes";

const USAGE = `usage: electron --force-device-scale-factor=<scale> --force-color-profile=srgb textMain.mjs <out dir> <scale>

Captures the DOM text scenes: scenes/<name>.json, ref/<name>@<scale>x.{png,json}.`;

const args = process.argv.slice(process.argv.findIndex((a) => /textMain\.mjs$/.test(a)) + 1);
const [outArg, scaleArg] = args;

function fail(message: string): never {
  console.error(message);
  process.exit(2);
}

function writeJson(file: string, value: unknown) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(value, null, 1) + "\n");
}

async function capture(out: string, scale: number) {
  for (const scene of textScenes()) {
    writeJson(path.join(out, "scenes", `${scene.name}.json`), scene);
    const html = path.join(out, "html", `${scene.name}.html`);
    fs.mkdirSync(path.dirname(html), { recursive: true });
    fs.writeFileSync(html, sceneHtml(scene));
    const win = new BrowserWindow({
      show: false,
      width: scene.width,
      height: scene.height,
      useContentSize: true,
      paintWhenInitiallyHidden: true,
      webPreferences: { backgroundThrottling: false }
    });
    await win.loadFile(html);
    await win.webContents.executeJavaScript(
      "document.fonts.ready.then(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))))"
    );
    const dom = await win.webContents.executeJavaScript(BOXES_SCRIPT);
    if (dom.devicePixelRatio !== scale) fail(`devicePixelRatio is ${dom.devicePixelRatio}, expected ${scale}`);
    const image = await win.webContents.capturePage();
    const size = image.getSize(scale);
    const bitmap = image.toBitmap({ scaleFactor: scale });
    const rgba = Buffer.alloc(bitmap.length);
    for (let i = 0; i < bitmap.length; i += 4) {
      rgba[i] = bitmap[i + 2];
      rgba[i + 1] = bitmap[i + 1];
      rgba[i + 2] = bitmap[i];
      rgba[i + 3] = bitmap[i + 3];
    }
    for (const patch of dom.patches as { color: string; rect: number[] }[]) {
      const x = Math.floor((patch.rect[0] + patch.rect[2] / 2) * scale);
      const y = Math.floor((patch.rect[1] + patch.rect[3] / 2) * scale);
      const i = (y * size.width + x) * 4;
      const got = "#" + [0, 1, 2].map((c) => rgba[i + c].toString(16).padStart(2, "0")).join("");
      if (got !== patch.color) fail(`colour patch ${patch.color} captured as ${got}; start Electron with --force-color-profile=srgb`);
    }
    const base = path.join(out, "ref", `${scene.name}@${scale}x`);
    fs.mkdirSync(path.dirname(base), { recursive: true });
    fs.writeFileSync(`${base}.png`, encodePng(size.width, size.height, rgba));
    writeJson(`${base}.json`, { scene: scene.name, scale, width: size.width, height: size.height, boxes: dom.boxes });
    win.destroy();
    console.error(`  ${scene.name}@${scale}x`);
  }
}

app.whenReady().then(async () => {
  try {
    if (outArg === undefined || (scaleArg !== "1" && scaleArg !== "2")) fail(USAGE);
    const out = path.resolve(outArg);
    writeJson(path.join(out, "info.json"), {
      electron: process.versions.electron,
      chrome: process.versions.chrome,
      platform: process.platform,
      arch: process.arch,
      osRelease: os.release()
    });
    await capture(out, Number(scaleArg));
  } catch (e) {
    console.error(e);
    process.exitCode = 1;
  }
  app.exit(process.exitCode ?? 0);
});
app.on("window-all-closed", () => {});
