import "../src/environment";
import { app, BrowserWindow, nativeTheme, screen } from "electron";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import * as os from "node:os";
import * as path from "node:path";
import { encodePng } from "../src/canvas/png";

declare const __UPSTREAM_DIR__: string;

const USAGE = `usage: electron [--force-device-scale-factor=<n>] hostMain.mjs <mode> <out dir> [options]

modes:
  info                    Electron, GPU and display facts for the host description
  colour-patch <scale>    capture 512 known sRGB patches with capturePage at device scale 1 or 2
                          and require every patch within 1 level per channel of its source
  launch                  start the real upstream app with an isolated userData directory, wait
                          for the hub window to load, capture it and quit`;

const PATCH = 16;
const COLUMNS = 32;
const LAUNCH_TIMEOUT_MS = 90_000;
const SETTLE_MS = 3000;

const args = process.argv.slice(process.argv.findIndex((a) => /hostMain\.mjs$/.test(a)) + 1);
const [mode, outArg, ...rest] = args;

function fail(message: string): never {
  console.error(message);
  process.exit(2);
}

function writeJson(file: string, value: unknown) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(value, null, 1) + "\n");
}

function displays() {
  return screen.getAllDisplays().map((d) => ({
    size: d.size,
    scaleFactor: d.scaleFactor,
    colorSpace: d.colorSpace,
    colorDepth: d.colorDepth,
    depthPerComponent: d.depthPerComponent
  }));
}

async function info(out: string) {
  const gpuInfo = await app.getGPUInfo("complete");
  writeJson(path.join(out, "electron.json"), {
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    platform: process.platform,
    arch: process.arch,
    osRelease: os.release(),
    osVersion: os.version(),
    cpus: os.cpus().length,
    cpuModel: os.cpus()[0]?.model,
    memoryBytes: os.totalmem(),
    darkTheme: nativeTheme.shouldUseDarkColors,
    displays: displays(),
    gpuFeatureStatus: app.getGPUFeatureStatus(),
    gpuInfo
  });
}

function patchColours(): [number, number, number][] {
  const colours: [number, number, number][] = [];
  const seen = new Set<string>();
  const add = (c: [number, number, number]) => {
    const key = c.join(",");
    if (!seen.has(key)) {
      seen.add(key);
      colours.push(c);
    }
  };
  for (let i = 0; i < 8; i++) add([i & 1 ? 255 : 0, i & 2 ? 255 : 0, i & 4 ? 255 : 0]);
  for (const hex of ["2b66a2", "e5b31b", "af2437", "80588e", "e48b32", "c0b487", "858584", "3b875a", "d993aa", "5f4528", "ff8c00"]) {
    add([0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16)) as [number, number, number]);
  }
  for (let v = 0; v < 256; v++) add([v, v, v]);
  const steps = [0, 51, 102, 153, 204, 255];
  for (const r of steps) for (const g of steps) for (const b of steps) add([r, g, b]);
  for (let i = 0; colours.length < 512; i++) {
    const v = (i * 37 + 11) % 256;
    add([v, (v * 7 + 90) % 256, (v * 13 + 200) % 256]);
  }
  return colours;
}

function bgraToRgba(bitmap: Buffer): Buffer {
  const rgba = Buffer.alloc(bitmap.length);
  for (let i = 0; i < bitmap.length; i += 4) {
    rgba[i] = bitmap[i + 2];
    rgba[i + 1] = bitmap[i + 1];
    rgba[i + 2] = bitmap[i];
    rgba[i + 3] = bitmap[i + 3];
  }
  return rgba;
}

async function colourPatch(out: string, scale: number) {
  const colours = patchColours();
  const rows = Math.ceil(colours.length / COLUMNS);
  const width = COLUMNS * PATCH;
  const height = rows * PATCH;
  const cells = colours
    .map(([r, g, b], i) => {
      const x = (i % COLUMNS) * PATCH;
      const y = Math.floor(i / COLUMNS) * PATCH;
      return `<div style="position:absolute;left:${x}px;top:${y}px;width:${PATCH}px;height:${PATCH}px;background:rgb(${r},${g},${b})"></div>`;
    })
    .join("");
  const html = `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;overflow:hidden;background:#000}</style></head><body>${cells}</body></html>`;
  const win = new BrowserWindow({
    show: false,
    x: 0,
    y: 0,
    width,
    height,
    useContentSize: true,
    frame: false,
    resizable: false,
    webPreferences: { backgroundThrottling: false }
  });
  win.showInactive();
  await win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(html));
  await win.webContents.executeJavaScript("new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))");
  const dpr = await win.webContents.executeJavaScript("window.devicePixelRatio");
  if (dpr !== scale) throw new Error(`devicePixelRatio is ${dpr}, expected ${scale}; pass --force-device-scale-factor=${scale}`);
  const image = await win.webContents.capturePage();
  const size = image.getSize(scale);
  if (size.width !== width * scale || size.height !== height * scale) {
    throw new Error(`capture is ${size.width}x${size.height}, expected ${width * scale}x${height * scale}`);
  }
  const rgba = bgraToRgba(image.toBitmap({ scaleFactor: scale }));
  let exact = 0;
  let maxDelta = 0;
  const failures: { source: number[]; captured: number[] }[] = [];
  colours.forEach((source, i) => {
    let worst = 0;
    let captured: number[] = [];
    for (let dy = 0; dy < PATCH * scale; dy++) {
      for (let dx = 0; dx < PATCH * scale; dx++) {
        const x = (i % COLUMNS) * PATCH * scale + dx;
        const y = Math.floor(i / COLUMNS) * PATCH * scale + dy;
        const at = (y * size.width + x) * 4;
        const px = [rgba[at], rgba[at + 1], rgba[at + 2]];
        const delta = Math.max(...px.map((v, c) => Math.abs(v - source[c])));
        if (delta >= worst) {
          worst = delta;
          captured = px;
        }
      }
    }
    if (worst === 0) exact++;
    maxDelta = Math.max(maxDelta, worst);
    if (worst > 1) failures.push({ source, captured });
  });
  const base = path.join(out, `colour-patch@${scale}x`);
  fs.mkdirSync(out, { recursive: true });
  fs.writeFileSync(`${base}.png`, encodePng(size.width, size.height, rgba));
  writeJson(`${base}.json`, {
    scale,
    width: size.width,
    height: size.height,
    patches: colours.length,
    exact,
    maxDelta,
    failures,
    switches: process.argv.filter((a) => a.startsWith("--")),
    displays: displays()
  });
  win.destroy();
  console.error(`colour-patch@${scale}x: ${exact}/${colours.length} exact, max delta ${maxDelta}, ${failures.length} over 1 level`);
  if (failures.length > 0) process.exitCode = 1;
}

function launch(out: string) {
  const userData = path.join(out, "launch-userdata");
  fs.rmSync(userData, { recursive: true, force: true });
  fs.mkdirSync(userData, { recursive: true });
  app.setPath("userData", userData);
  const timer = setTimeout(() => {
    console.error(`launch: no hub window finished loading within ${LAUNCH_TIMEOUT_MS / 1000} s`);
    app.exit(1);
  }, LAUNCH_TIMEOUT_MS);
  let captured = false;
  process.on("uncaughtException", (e) => {
    console.error("launch: uncaught exception in the main process:", e);
    app.exit(1);
  });
  app.on("browser-window-created", (_event, win) => {
    // Upstream opens DevTools when unpackaged; docked, they shrink the hub page to half its width.
    win.webContents.on("devtools-opened", () => win.webContents.closeDevTools());
    win.webContents.once("did-finish-load", async () => {
      const url = win.webContents.getURL();
      if (captured || !url.endsWith("/www/hub.html")) return;
      captured = true;
      await new Promise((r) => setTimeout(r, SETTLE_MS));
      try {
        const image = await win.webContents.capturePage();
        const scale = screen.getDisplayMatching(win.getBounds()).scaleFactor;
        const size = image.getSize(scale);
        fs.writeFileSync(path.join(out, "launch.png"), encodePng(size.width, size.height, bgraToRgba(image.toBitmap({ scaleFactor: scale }))));
        writeJson(path.join(out, "launch.json"), {
          page: new URL(url).pathname.split("/").slice(-2).join("/"),
          title: win.getTitle(),
          bounds: win.getBounds(),
          contentBounds: win.getContentBounds(),
          scaleFactor: scale,
          capture: size,
          windows: BrowserWindow.getAllWindows().length
        });
        console.error(`launch: hub window loaded, ${size.width}x${size.height} captured`);
      } catch (e) {
        console.error(e);
        process.exitCode = 1;
      }
      clearTimeout(timer);
      app.exit(process.exitCode ?? 0);
    });
  });
  createRequire(import.meta.url)(path.join(__UPSTREAM_DIR__, "bundles", "main.js"));
}

if (outArg === undefined || mode === undefined) fail(USAGE);
const out = path.resolve(outArg);

if (mode === "launch") {
  launch(out);
} else {
  app.whenReady().then(async () => {
    try {
      if (mode === "info") await info(out);
      else if (mode === "colour-patch") {
        const scale = Number(rest[0]);
        if (scale !== 1 && scale !== 2) fail(USAGE);
        await colourPatch(out, scale);
      } else fail(USAGE);
    } catch (e) {
      console.error(e);
      process.exitCode = 1;
    }
    app.exit(process.exitCode ?? 0);
  });
  app.on("window-all-closed", () => {});
}
