import { app, BrowserWindow, screen } from "electron";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { encode } from "./cve1";
import { domScenes } from "./canvas/dom";
import { encodePng } from "./canvas/png";
import {
  allScenes,
  calibrationScenes,
  MEASURE_ALIGNS,
  MEASURE_BASELINES,
  measureFonts,
  measureStrings,
  type Scene
} from "./canvas/scenes";

const USAGE = `usage: electron canvasMain.mjs <mode> <out dir> [options]

modes:
  canvas [<scene>...]     replay display-list scenes on the CPU and GPU canvas
  measure                 TextMetrics of the label corpus under every font, align and baseline
  canvas-lut              replay the mask-gamma calibration scenes on the CPU canvas
  dom <scale>             capture the DOM text scenes at device scale 1 or 2; Electron must be
                          started with --force-device-scale-factor=<scale> and
                          --force-color-profile=srgb before the script path
  list                    print the scene names`;

const args = process.argv.slice(process.argv.findIndex((a) => /canvasMain\.mjs$/.test(a)) + 1);
const [mode, outArg, ...rest] = args;
const here = path.dirname(fileURLToPath(import.meta.url));

if (mode === "dom" && rest[0] !== "1" && rest[0] !== "2") fail(USAGE);

function fail(message: string): never {
  console.error(message);
  process.exit(2);
}

function writeJson(file: string, value: unknown) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(value, null, 1) + "\n");
}

function sceneJson(scene: Scene): string {
  return JSON.stringify(scene) + "\n";
}

async function page(width: number, height: number): Promise<BrowserWindow> {
  const win = new BrowserWindow({
    show: false,
    width,
    height,
    useContentSize: true,
    paintWhenInitiallyHidden: true,
    webPreferences: { backgroundThrottling: false }
  });
  return win;
}

async function replayScenes(win: BrowserWindow, out: string, scenes: Scene[], modes: string[]) {
  for (const scene of scenes) {
    const sceneFile = path.join(out, "scenes", `${scene.name}.json`);
    fs.mkdirSync(path.dirname(sceneFile), { recursive: true });
    fs.writeFileSync(sceneFile, sceneJson(scene));
    for (const m of modes) {
      const result = await win.webContents.executeJavaScript(`replay(${JSON.stringify(scene)}, ${m === "cpu"})`);
      const rgba = Buffer.from(result.rgba, "base64");
      const file = path.join(out, "ref", `${scene.name}.${m}.png`);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, encodePng(result.width, result.height, rgba));
    }
    console.error(`  ${scene.name}`);
  }
}

async function measure(win: BrowserWindow, out: string) {
  const call = `measure(${JSON.stringify(measureFonts())}, ${JSON.stringify(measureStrings())}, ${JSON.stringify(MEASURE_ALIGNS)}, ${JSON.stringify(MEASURE_BASELINES)})`;
  const rows = (await win.webContents.executeJavaScript(call)) as {
    font: string;
    resolvedFont: string;
    text: string;
    align: string;
    baseline: string;
    metrics: [string, number][];
  }[];
  const lines = rows.map(
    (r) =>
      `{"font":${JSON.stringify(r.font)},"resolvedFont":${JSON.stringify(r.resolvedFont)},"text":${JSON.stringify(r.text)},` +
      `"align":${JSON.stringify(r.align)},"baseline":${JSON.stringify(r.baseline)},"metrics":${encode(Object.fromEntries(r.metrics))}}`
  );
  fs.writeFileSync(path.join(out, "measure.jsonl"), lines.map((l) => l + "\n").join(""));
  console.error(`  measure: ${lines.length} records`);
}

async function dom(out: string, scale: string) {
  const dir = path.join(out, "dom");
  fs.mkdirSync(dir, { recursive: true });
  const displays = screen.getAllDisplays().map((d) => ({ scaleFactor: d.scaleFactor, colorSpace: d.colorSpace, colorDepth: d.colorDepth }));
  for (const scene of domScenes()) {
    const html = path.join(dir, `${scene.name}.html`);
    const script = `<script src="${pathToFileURL(path.join(here, "canvasPage.js")).href}"></script>`;
    fs.writeFileSync(html, scene.html.replace("</body>", `${script}</body>`));
    const win = await page(scene.width, scene.height);
    await win.loadFile(html);
    await win.webContents.executeJavaScript("document.fonts.ready.then(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))))");
    const boxes = await win.webContents.executeJavaScript("domBoxes()");
    if (boxes.devicePixelRatio !== Number(scale)) {
      throw new Error(`devicePixelRatio is ${boxes.devicePixelRatio}, expected ${scale}; pass --force-device-scale-factor=${scale}`);
    }
    const image = await win.webContents.capturePage();
    const size = image.getSize(Number(scale));
    const bitmap = image.toBitmap({ scaleFactor: Number(scale) });
    const rgba = Buffer.alloc(bitmap.length);
    for (let i = 0; i < bitmap.length; i += 4) {
      rgba[i] = bitmap[i + 2];
      rgba[i + 1] = bitmap[i + 1];
      rgba[i + 2] = bitmap[i];
      rgba[i + 3] = bitmap[i + 3];
    }
    for (const patch of boxes.patches as { color: string; rect: number[] }[]) {
      const x = Math.floor((patch.rect[0] + patch.rect[2] / 2) * Number(scale));
      const y = Math.floor((patch.rect[1] + patch.rect[3] / 2) * Number(scale));
      const i = (y * size.width + x) * 4;
      const got = "#" + [0, 1, 2].map((c) => rgba[i + c].toString(16).padStart(2, "0")).join("");
      if (got !== patch.color) {
        throw new Error(`colour patch ${patch.color} captured as ${got}; start Electron with --force-color-profile=srgb`);
      }
    }
    const base = path.join(dir, `${scene.name}@${scale}x`);
    fs.writeFileSync(`${base}.png`, encodePng(size.width, size.height, rgba));
    writeJson(`${base}.json`, { scene: scene.name, scale: Number(scale), width: size.width, height: size.height, displays, ...boxes });
    win.destroy();
    console.error(`  ${scene.name}@${scale}x`);
  }
}

function info(out: string) {
  writeJson(path.join(out, "info.json"), {
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    platform: process.platform,
    arch: process.arch,
    osRelease: os.release(),
    gpuFeatureStatus: app.getGPUFeatureStatus()
  });
}

async function main() {
  if (mode === "list") {
    for (const s of [...allScenes(), ...calibrationScenes()]) console.log(s.name);
    return;
  }
  if (outArg === undefined || mode === undefined) fail(USAGE);
  const out = path.resolve(outArg);
  if (mode === "dom") {
    await dom(out, rest[0]);
    return;
  }
  const win = await page(400, 300);
  await win.loadFile(path.join(here, "canvasPage.html"));
  if (mode === "canvas") {
    const scenes = allScenes().filter((s) => rest.length === 0 || rest.includes(s.name));
    const unknown = rest.filter((n) => !allScenes().some((s) => s.name === n));
    if (unknown.length > 0) fail(`unknown scenes: ${unknown.join(", ")}`);
    info(out);
    await replayScenes(win, out, scenes, ["cpu", "gpu"]);
  } else if (mode === "measure") {
    info(out);
    await measure(win, out);
  } else if (mode === "canvas-lut") {
    info(out);
    await replayScenes(win, out, calibrationScenes(), ["cpu"]);
  } else {
    fail(USAGE);
  }
  win.destroy();
}

app.whenReady().then(async () => {
  try {
    await main();
  } catch (e) {
    console.error(e);
    process.exitCode = 1;
  }
  app.exit(process.exitCode ?? 0);
});
app.on("window-all-closed", () => {});
