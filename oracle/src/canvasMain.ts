import { app, BrowserWindow, nativeTheme, screen } from "electron";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { encode, toTree } from "./cve1";
import { domScenes } from "./canvas/dom";
import { encodePng } from "./canvas/png";
import { uiHtml, uiScenes } from "./canvas/ui";
import { heatmapFixtures } from "./heatmap/fixtures";
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
  heatmap                 run the upstream Heatmap.update fixtures on a real page and record
                          the setData arguments, store contents and CPU-canvas pixels
  dom <scale>             capture the DOM text scenes at device scale 1 or 2; Electron must be
                          started with --force-device-scale-factor=<scale> and
                          --force-color-profile=srgb before the script path
  ui <scale>              capture the UI theme scenes (upstream global.css, symbols, filter chains)
                          at device scale 1 or 2, with the same Electron flags as dom
  list                    print the scene names`;

const args = process.argv.slice(process.argv.findIndex((a) => /canvasMain\.mjs$/.test(a)) + 1);
const [mode, outArg, ...rest] = args;
const here = path.dirname(fileURLToPath(import.meta.url));

if ((mode === "dom" || mode === "ui") && rest[0] !== "1" && rest[0] !== "2") fail(USAGE);

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

async function captureRgba(win: BrowserWindow, scale: number) {
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
  const at = (x: number, y: number) => {
    const i = (Math.floor(y * scale) * size.width + Math.floor(x * scale)) * 4;
    return [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]];
  };
  return { size, rgba, at };
}

async function ui(out: string, scaleArg: string) {
  const scale = Number(scaleArg);
  const upstream = path.join(here, "..", "upstream");
  for (const scene of uiScenes(upstream)) {
    writeJson(path.join(out, "scenes", `${scene.name}.json`), scene);
    nativeTheme.themeSource = scene.dark ? "dark" : "light";
    const html = path.join(out, "html", `${scene.name}.html`);
    fs.mkdirSync(path.dirname(html), { recursive: true });
    fs.writeFileSync(html, uiHtml(scene, upstream));
    const win = await page(scene.width, scene.height);
    await win.loadFile(html);
    const state = await win.webContents.executeJavaScript(
      "Promise.all(Array.from(document.images).map((i) => i.decode())).then(() => document.fonts.ready)" +
        ".then(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))))" +
        ".then(() => ({ dpr: window.devicePixelRatio, dark: matchMedia('(prefers-color-scheme: dark)').matches, " +
        "broken: Array.from(document.images).filter((i) => i.naturalWidth === 0).map((i) => i.src) }))"
    );
    if (state.dpr !== scale) throw new Error(`devicePixelRatio is ${state.dpr}, expected ${scale}`);
    if (state.dark !== scene.dark) throw new Error(`${scene.name}: prefers-color-scheme did not follow nativeTheme`);
    if (state.broken.length > 0) throw new Error(`images failed to load: ${state.broken.join(", ")}`);
    if (scene.pointer !== null) {
      const { x, y, down } = scene.pointer;
      win.webContents.sendInputEvent({ type: "mouseMove", x, y });
      if (down) win.webContents.sendInputEvent({ type: "mouseDown", x, y, button: "left", clickCount: 1 });
      const pseudo = down ? ":active" : ":hover";
      const matched = await win.webContents.executeJavaScript(
        `new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))).then(() => document.querySelector("button${pseudo}") !== null)`
      );
      if (!matched) throw new Error(`${scene.name}: no button matches ${pseudo} after the synthetic pointer event`);
    }
    const shot = await captureRgba(win, scale);
    for (const p of scene.patches) {
      const got = "#" + shot.at(p.x + 10, p.y + 10).slice(0, 3).map((c) => c.toString(16).padStart(2, "0")).join("");
      if (got !== p.color) throw new Error(`colour patch ${p.color} captured as ${got}; start Electron with --force-color-profile=srgb`);
    }
    const probes = scene.probes.map((p) => ({ chain: p.chain, input: p.input, opacity: p.opacity, rgba: shot.at(p.x + p.size / 2, p.y + p.size / 2) }));
    const buttons = (scene.pointer === null ? [] : scene.buttons).map((b) => shot.at(b.x + b.size / 2, b.y + 1));
    const base = path.join(out, "ref", `${scene.name}@${scale}x`);
    fs.mkdirSync(path.dirname(base), { recursive: true });
    fs.writeFileSync(`${base}.png`, encodePng(shot.size.width, shot.size.height, shot.rgba));
    writeJson(`${base}.json`, { scene: scene.name, scale, width: shot.size.width, height: shot.size.height, probes, buttons });
    if (scene.pointer?.down) win.webContents.sendInputEvent({ type: "mouseUp", x: scene.pointer.x, y: scene.pointer.y, button: "left", clickCount: 1 });
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

interface PageError {
  name: string;
  message: string;
}

interface HeatmapResult {
  created: boolean;
  setData: unknown;
  store: unknown;
  error: PageError | null;
  canvas: [number, number] | null;
  readError: PageError | null;
  rgba: string | null;
  shadow: string | null;
}

function errorTree(error: PageError | null): unknown {
  if (error === null) return null;
  return toTree(Object.assign(new Error(error.message), { name: error.name }));
}

async function heatmap(win: BrowserWindow, out: string) {
  const dir = path.join(out, "heatmap");
  for (const [index, fixture] of heatmapFixtures().entries()) {
    if (fixture.nodeOnly) continue;
    const results: HeatmapResult[] = await win.webContents.executeJavaScript(`heatmapRun(${index})`);
    writeJson(path.join(dir, `${fixture.name}.fixture.json`), {
      name: fixture.name,
      updates: fixture.updates.map((u) => ({
        translations: toTree(u.translations),
        pixelDimensions: toTree(u.pixelDimensions),
        fieldDimensions: toTree(u.fieldDimensions)
      }))
    });
    writeJson(path.join(dir, `${fixture.name}.expected.json`), {
      name: fixture.name,
      updates: results.map((r) => ({
        created: r.created,
        setData: toTree(r.setData),
        store: toTree(r.store),
        throw: errorTree(r.error),
        canvas: r.canvas,
        readError: errorTree(r.readError)
      }))
    });
    for (const [i, r] of results.entries()) {
      if (r.rgba !== null && r.canvas !== null) {
        fs.writeFileSync(
          path.join(dir, `${fixture.name}.${i}.png`),
          encodePng(r.canvas[0], r.canvas[1], Buffer.from(r.rgba, "base64"))
        );
      }
      if (r.shadow !== null && r.canvas !== null) {
        fs.writeFileSync(path.join(dir, `${fixture.name}.${i}.shadow`), Buffer.from(r.shadow, "base64"));
      }
    }
    console.error(`  ${fixture.name}`);
  }
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
  if (mode === "ui") {
    info(out);
    await ui(out, rest[0]);
    return;
  }
  const win = await page(400, 300);
  if (mode === "heatmap") {
    info(out);
    await win.loadFile(path.join(here, "heatmapPage.html"));
    await heatmap(win, out);
    win.destroy();
    return;
  }
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
