import "./environment";
import { app, BrowserWindow, dialog, net, screen, session } from "electron";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import * as os from "node:os";
import * as path from "node:path";
import { saveDisplayProfile, type DisplayProfile } from "./app/display";
import { GENERATED_LOGS } from "./app/logs";
import { busyReasons, domDump, hubReady, sendMainMessage } from "./app/page";
import { patchPageHtml } from "./app/patches";
import { actionKind, parseScenario, type Action, type GlMode, type Scale, type Theme } from "./app/scenario";
import { installTap } from "./app/tap";
import { encodePng } from "./canvas/png";

declare const __UPSTREAM_DIR__: string;

const USAGE = `usage: electron <switches> appMain.mjs <scenario.json> <out dir> --scale=<1|2> --theme=<light|dark>
         [--gl=<mode>] [--colour=srgb|display]

Starts upstream's bundles/main.js with an isolated userData directory and drives it through the
scenario with executeJavaScript and sendInputEvent. --gl picks the GL backend switches of a
scenario with "gl": true: native (macOS default), swiftshader (Linux and Windows default) or
d3d11-warp. --colour=srgb (default) has Chromium composite in sRGB; --colour=display keeps the
display's colour space and stores its ICC profile as display.icc. The driver names the Electron
switches it needs when they are missing. Results go to <out dir>: run.json, captures/, files/,
inputs/ and, with --colour=display, display.icc.`;

const WINDOW_ORIGIN = 16;
const RANDOM_SEED = 1;
const RUN_TIMEOUT_MS = 240_000;
const READY_TIMEOUT_MS = 90_000;
const IDLE_TIMEOUT_MS = 60_000;
const STABLE_TIMEOUT_MS = 30_000;
const STABLE_CAPTURES = 3;
const STABLE_GAP_MS = 120;
const SAVE_SETTLE_MS = 1500;
const STATE_SAVE_TIMEOUT_MS = 5000;
const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]", "::1"]);

function fail(message: string): never {
  console.error(message);
  process.exit(2);
}

const argv = process.argv.slice(process.argv.findIndex((a) => /appMain\.mjs$/.test(a)) + 1);
const positional = argv.filter((a) => !a.startsWith("--"));
const options = Object.fromEntries(argv.filter((a) => a.startsWith("--")).map((a) => a.slice(2).split("=", 2)));
if (positional.length !== 2) fail(USAGE);
const scenarioFile = path.resolve(positional[0]);
const out = path.resolve(positional[1]);
const scale = Number(options.scale) as Scale;
const theme = options.theme as Theme;
if (scale !== 1 && scale !== 2) fail(USAGE);
if (theme !== "light" && theme !== "dark") fail(USAGE);
const glMode = (options.gl ?? (process.platform === "darwin" ? "native" : "swiftshader")) as GlMode;
if (!["native", "swiftshader", "d3d11-warp"].includes(glMode)) fail(USAGE);
const colour = options.colour ?? "srgb";
if (colour !== "srgb" && colour !== "display") fail(USAGE);
const scenario = parseScenario(path.basename(scenarioFile, ".json"), fs.readFileSync(scenarioFile, "utf8"));
if (scenario.page === "app" && (scenario.window.width < 800 || scenario.window.height < 400)) {
  fail(`scenario ${scenario.name}: the hub window has a minimum size of 800x400`);
}

const switches: [string, string?][] = [
  ["force-device-scale-factor", String(scale)],
  ["lang", "en-US"],
  ["disable-accelerated-2d-canvas"],
  ["js-flags", `--random-seed=${RANDOM_SEED}`],
  // Only the 3D camera reads it: without it the default camera eases in over wall-clock frame
  // deltas, so the resting position differs in the last bits between runs.
  ["force-prefers-reduced-motion"],
  // Partial raster re-rasters only invalidated rects into old tiles, which leaves timing-dependent
  // seams (one pixel of a button border in 2 of 5 runs); a full raster is what a fresh paint gives.
  ["disable-partial-raster"]
];
if (colour === "srgb") switches.push(["force-color-profile", "srgb"]);
// GPU-less hosts need SwiftShader for WebGL. 2D and DOM launches go without it, so their raster
// path stays the one the canvas references were measured on.
if (scenario.gl && glMode !== "native") switches.push(["enable-unsafe-swiftshader"]);
if (scenario.gl && glMode === "d3d11-warp") switches.push(["use-angle", "d3d11-warp"]);
const switchText = switches.map(([n, v]) => (v === undefined ? `--${n}` : `--${n}=${v}`));
// Chromium reads the scale factor before this script runs, so every switch has to be on the
// real command line rather than appended here.
const missing = switches.filter(([n, v]) => !app.commandLine.hasSwitch(n) || app.commandLine.getSwitchValue(n) !== (v ?? ""));
const extra = ["enable-unsafe-swiftshader", "use-angle", "force-color-profile"].filter((n) => app.commandLine.hasSwitch(n) && !switches.some(([m]) => m === n));
if (missing.length > 0 || extra.length > 0) fail(`start Electron with exactly ${switchText.join(" ")} before appMain.mjs`);

fs.rmSync(out, { recursive: true, force: true });
const userData = path.join(out, "userData");
const capturesDir = path.join(out, "captures");
fs.mkdirSync(userData, { recursive: true });
fs.mkdirSync(capturesDir, { recursive: true });
app.setPath("userData", userData);

const seeds: Record<string, unknown> = {
  "prefs.json": { ...scenario.prefs, theme },
  "beta-2027.json": { welcomeComplete: true, surveyStatus: true },
  ...scenario.userData
};
for (const [file, value] of Object.entries(seeds)) {
  const seedFile = path.join(userData, file);
  fs.mkdirSync(path.dirname(seedFile), { recursive: true });
  fs.writeFileSync(seedFile, JSON.stringify(value));
}
const seedBytes = new Map(Object.keys(seeds).map((file) => [file, fs.readFileSync(path.join(userData, file))]));

type DialogRecord = { kind: string; options: unknown; response: unknown; scripted: boolean };
const dialogs: DialogRecord[] = [];
const dialogQueue: (number | string | null)[] = [];
const blockedRequests: string[] = [];
const errors: string[] = [];

function refuseNetwork(input: unknown): Promise<never> {
  const url = typeof input === "object" && input !== null && "url" in input ? String((input as Request).url) : String(input);
  blockedRequests.push(url);
  return Promise.reject(new TypeError("fetch failed"));
}
globalThis.fetch = refuseNetwork as typeof fetch;
net.fetch = refuseNetwork as typeof net.fetch;

function nextDialogResponse(kind: string, options: any): { value: number | string | null; scripted: boolean } {
  if (dialogQueue.length > 0) return { value: dialogQueue.shift()!, scripted: true };
  errors.push(`unscripted ${kind}: ${JSON.stringify(options)}`);
  return { value: null, scripted: false };
}

function messageBoxResponse(options: any, value: number | string | null): number {
  if (typeof value === "number") return value;
  if (typeof options.cancelId === "number") return options.cancelId;
  return Array.isArray(options.buttons) && options.buttons.length > 0 ? options.buttons.length - 1 : 0;
}

function dialogPath(value: number | string | null): string | null {
  return typeof value === "string" ? path.resolve(out, "files", "dialog", value) : null;
}

function scriptDialogs() {
  const optionsOf = (args: any[]) => (args[0] instanceof BrowserWindow ? args[1] : args[0]) ?? {};
  const messageBox = (kind: string, args: any[]) => {
    const options = optionsOf(args);
    const { value, scripted } = nextDialogResponse(kind, options);
    const response = messageBoxResponse(options, value);
    dialogs.push({ kind, options, response, scripted });
    return response;
  };
  const fileDialog = (kind: string, args: any[]) => {
    const options = optionsOf(args);
    const { value, scripted } = nextDialogResponse(kind, options);
    const file = dialogPath(value);
    if (file !== null) fs.mkdirSync(path.dirname(file), { recursive: true });
    dialogs.push({ kind, options, response: file, scripted });
    return file;
  };
  dialog.showMessageBox = (async (...args: any[]) => ({ response: messageBox("showMessageBox", args), checkboxChecked: false })) as any;
  dialog.showMessageBoxSync = ((...args: any[]) => messageBox("showMessageBoxSync", args)) as any;
  dialog.showErrorBox = (title: string, content: string) => {
    dialogs.push({ kind: "showErrorBox", options: { title, content }, response: null, scripted: false });
    errors.push(`error box: ${title}: ${content}`);
  };
  dialog.showOpenDialog = (async (...args: any[]) => {
    const file = fileDialog("showOpenDialog", args);
    return { canceled: file === null, filePaths: file === null ? [] : [file] };
  }) as any;
  dialog.showOpenDialogSync = ((...args: any[]) => {
    const file = fileDialog("showOpenDialogSync", args);
    return file === null ? undefined : [file];
  }) as any;
  dialog.showSaveDialog = (async (...args: any[]) => {
    const file = fileDialog("showSaveDialog", args);
    return { canceled: file === null, filePath: file ?? "" };
  }) as any;
  dialog.showSaveDialogSync = ((...args: any[]) => fileDialog("showSaveDialogSync", args) ?? undefined) as any;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until<T>(what: string, timeoutMs: number, probe: () => Promise<T | undefined | false>): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await probe();
    if (value !== undefined && value !== false) return value;
    if (Date.now() > deadline) throw new Error(`timed out after ${timeoutMs / 1000} s waiting for ${what}`);
    await sleep(50);
  }
}

let target: BrowserWindow | null = null;
const windows: BrowserWindow[] = [];

function placeWindow(win: BrowserWindow) {
  const area = screen.getPrimaryDisplay().workArea;
  win.setContentSize(scenario.window.width, scenario.window.height);
  win.setPosition(area.x + WINDOW_ORIGIN, area.y + WINDOW_ORIGIN);
}

app.on("browser-window-created", (_event, win) => {
  windows.push(win);
  // Unpackaged upstream opens DevTools docked, which halves the hub page. Windows stay inactive
  // and ignore the real pointer, which would otherwise move the hover state when it passes over
  // them; sendInputEvent goes straight to the renderer and is unaffected.
  win.webContents.openDevTools = () => {};
  win.show = () => win.showInactive();
  win.setIgnoreMouseEvents(true);
  win.webContents.on("did-start-navigation", (event: any) => {
    if (target === null && event.isMainFrame && /\/www\/hub\.html$/.test(new URL(event.url).pathname)) {
      target = win;
      placeWindow(win);
    }
  });
});

async function exec<T = unknown>(code: string): Promise<T> {
  return target!.webContents.executeJavaScript(code);
}

const call = (fn: Function, ...args: unknown[]) => `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(", ")})`;

async function nextFrames(count = 2) {
  await exec(`new Promise((r) => { let n = ${count}; const f = () => (--n <= 0 ? r() : requestAnimationFrame(f)); requestAnimationFrame(f); })`);
}

async function waitIdle() {
  let last: string[] = [];
  await until("the page to go idle", IDLE_TIMEOUT_MS, async () => {
    last = await exec<string[]>(call(busyReasons));
    return last.length === 0;
  }).catch((e) => {
    throw new Error(`${e.message} (busy: ${last.join(", ")})`);
  });
  await nextFrames();
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

type Rect = number[];

function maskedEqual(a: Buffer, b: Buffer, width: number, masks: Rect[]): boolean {
  if (a.length !== b.length) return false;
  if (masks.length === 0) return a.equals(b);
  const height = a.length / 4 / width;
  const inMask = new Uint8Array(width * height);
  for (const [x, y, w, h] of masks) {
    const x0 = Math.max(0, Math.floor(x * scale));
    const y0 = Math.max(0, Math.floor(y * scale));
    const x1 = Math.min(width, Math.ceil((x + w) * scale));
    const y1 = Math.min(height, Math.ceil((y + h) * scale));
    for (let yy = y0; yy < y1; yy++) inMask.fill(1, yy * width + x0, yy * width + x1);
  }
  for (let i = 0; i < inMask.length; i++) {
    if (inMask[i] === 1) continue;
    const o = i * 4;
    if (a[o] !== b[o] || a[o + 1] !== b[o + 1] || a[o + 2] !== b[o + 2] || a[o + 3] !== b[o + 3]) return false;
  }
  return true;
}

let displayProfile: DisplayProfile | null = null;
let displayId: number | null = null;

async function capture(name: string, probes: string[], maskSelectors: string[]) {
  await waitIdle();
  const win = target!;
  const display = screen.getDisplayMatching(win.getBounds());
  if (displayId === null) {
    displayId = display.id;
    displayProfile = colour === "srgb" ? { source: "forced-srgb" } : saveDisplayProfile(display.id, path.join(out, "display.icc"));
  } else if (display.id !== displayId) {
    throw new Error(`capture ${name}: the window moved to display ${display.id}`);
  }

  let dom: any = null;
  let rgba: Buffer | null = null;
  let size = { width: 0, height: 0 };
  let matches = 0;
  let attempts = 0;
  let glReads = -1;
  await until(`capture ${name} to be stable`, STABLE_TIMEOUT_MS, async () => {
    attempts++;
    const nextDom = await exec<any>(call(domDump, probes, maskSelectors));
    const image = await win.webContents.capturePage();
    const nextSize = image.getSize(scale);
    const next = bgraToRgba(image.toBitmap({ scaleFactor: scale }));
    const nextGlReads = await exec<number>("window.__asrsParity.glReads()");
    const masks = nextDom.masks.map((m: { rect: Rect }) => m.rect);
    const same =
      rgba !== null &&
      nextSize.width === size.width &&
      JSON.stringify(nextDom) === JSON.stringify(dom) &&
      nextGlReads === glReads &&
      maskedEqual(rgba, next, nextSize.width, masks);
    matches = same ? matches + 1 : 0;
    rgba = next;
    size = nextSize;
    dom = nextDom;
    glReads = nextGlReads;
    if (matches + 1 >= STABLE_CAPTURES) return true;
    await sleep(STABLE_GAP_MS);
    return false;
  });

  const expected = { width: scenario.window.width * scale, height: scenario.window.height * scale };
  if (size.width !== expected.width || size.height !== expected.height) {
    throw new Error(`capture ${name}: content is ${size.width}x${size.height}, expected ${expected.width}x${expected.height}`);
  }
  if (dom.devicePixelRatio !== scale) throw new Error(`capture ${name}: devicePixelRatio is ${dom.devicePixelRatio}, expected ${scale}`);
  if (dom.language !== "en-US") throw new Error(`capture ${name}: navigator.language is ${dom.language}`);
  if (dom.darkScheme !== (theme === "dark")) throw new Error(`capture ${name}: prefers-color-scheme does not match theme ${theme}`);

  const base = path.join(capturesDir, name);
  fs.writeFileSync(`${base}.png`, encodePng(size.width, size.height, rgba!));

  let hubState: string | undefined;
  let timelineRange: number[] | undefined;
  if (scenario.page === "app") {
    const saves = await exec<number>("window.__asrsParity.saves()");
    await until(`capture ${name}: a save-state after the capture`, STATE_SAVE_TIMEOUT_MS, async () => (await exec<number>("window.__asrsParity.saves()")) > saves);
    hubState = `${name}.hub-state.json`;
    fs.writeFileSync(path.join(capturesDir, hubState), await exec<string>("window.__asrsParity.savedState()"));
    timelineRange = await exec<number[]>("window.selection.getTimelineRange()");
  }

  const patches =
    scenario.page === "colour-patch"
      ? await exec("Array.from(document.querySelectorAll('.patch'), (el) => { const r = el.getBoundingClientRect(); return { rgb: el.dataset.rgb.split(',').map(Number), rect: [r.x, r.y, r.width, r.height] }; })")
      : undefined;
  const contexts: any[] = await exec("window.__asrsParity.contexts()");
  const canvases = [];
  const gl = [];
  const glFrames: Record<string, number> = {};
  for (const context of contexts) {
    if (!context.visible) continue;
    if (context.kind === "2d") {
      const pixels: any = await exec(`window.__asrsParity.canvasPixels(${context.index})`);
      if (pixels.width === 0 || pixels.height === 0) continue;
      const file = `${name}.canvas-${context.index}.png`;
      fs.writeFileSync(path.join(capturesDir, file), encodePng(pixels.width, pixels.height, Buffer.from(pixels.rgba)));
      canvases.push({ ...context, file, colorSpace: pixels.colorSpace });
    } else if (context.kind === "webgl" || context.kind === "webgl2") {
      if (!scenario.gl) continue;
      const pixels: any = await exec(`window.__asrsParity.glPixels(${context.index})`);
      if (pixels.rgba === null) throw new Error(`capture ${name}: WebGL canvas ${context.path} has not drawn since the page loaded`);
      const file = `${name}.gl-${context.index}.png`;
      fs.writeFileSync(path.join(capturesDir, file), encodePng(pixels.width, pixels.height, Buffer.from(pixels.rgba)));
      gl.push({
        ...context,
        file,
        width: pixels.width,
        height: pixels.height,
        mode: renderMode(name, pixels.attributes),
        attributes: pixels.attributes,
        renderer: pixels.renderer
      });
      glFrames[file] = pixels.frame;
    }
  }
  if (scenario.gl && gl.length === 0) throw new Error(`capture ${name}: the scenario is a 3D scenario but no WebGL canvas is visible`);

  fs.writeFileSync(
    `${base}.json`,
    JSON.stringify(
      {
        name,
        scale,
        theme,
        width: size.width,
        height: size.height,
        image: `${name}.png`,
        colourSpace: displayProfile!.source === "none" || displayProfile!.source === "forced-srgb" ? "srgb" : "display.icc",
        displayId,
        ...dom,
        canvases,
        gl,
        patches,
        hubState,
        timelineRange,
        timing: { stableAfter: attempts, glFrames, frame: await exec<number>("window.__asrsParity.frame()") }
      },
      null,
      1
    ) + "\n"
  );
  console.error(`  ${scenario.name} ${scale}x ${theme}: ${name} (${size.width}x${size.height}, ${canvases.length} canvases, ${gl.length} gl)`);
}

// Field3dRendererImpl creates its WebGL context with a powerPreference chosen by the render mode
// and nothing else, so the context's attributes name the mode the frame was drawn in.
const POWER_PREFERENCE_MODES: Record<string, string> = { default: "standard", "low-power": "low-power", "high-performance": "cinematic" };

function renderMode(name: string, attributes: any): string {
  const mode = POWER_PREFERENCE_MODES[attributes?.powerPreference];
  if (mode === undefined) throw new Error(`capture ${name}: WebGL powerPreference ${JSON.stringify(attributes?.powerPreference)} names no 3D render mode`);
  return mode;
}

function resolveLog(spec: string): string {
  const generated = /^app:([a-z0-9-]+)$/.exec(spec);
  if (generated === null) return path.resolve(spec);
  const make = GENERATED_LOGS[generated[1]];
  if (make === undefined) throw new Error(`no generated log ${spec}; known: ${Object.keys(GENERATED_LOGS).join(", ")}`);
  const file = path.join(out, "inputs", `${generated[1]}.wpilog`);
  if (!fs.existsSync(file)) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, make());
  }
  return file;
}

const MOUSE_TYPES = { move: ["mouseMove"], down: ["mouseDown"], up: ["mouseUp"], click: ["mouseDown", "mouseUp"] } as const;

async function perform(action: Action) {
  const contents = target!.webContents;
  if ("main" in action) {
    await exec(call(sendMainMessage, action.main, action.data));
  } else if ("open" in action) {
    await exec(call(sendMainMessage, "open-files", { files: action.open.map(resolveLog), merge: action.merge ?? false }));
  } else if ("eval" in action) {
    await exec(action.eval);
  } else if ("selection" in action) {
    const [method, ...args] = action.selection;
    await exec(`window.selection[${JSON.stringify(method)}](...${JSON.stringify(args)})`);
  } else if ("wait" in action) {
    await sleep(action.wait);
  } else if ("idle" in action) {
    await waitIdle();
  } else if ("mouse" in action) {
    for (const type of MOUSE_TYPES[action.mouse]) {
      contents.sendInputEvent({ type, x: action.x, y: action.y, button: action.button ?? "left", clickCount: action.clickCount ?? 1 } as any);
    }
  } else if ("wheel" in action) {
    const w = action.wheel;
    contents.sendInputEvent({
      type: "mouseWheel",
      x: w.x,
      y: w.y,
      deltaX: w.deltaX ?? 0,
      deltaY: w.deltaY ?? 0,
      wheelTicksX: w.wheelTicksX ?? 0,
      wheelTicksY: w.wheelTicksY ?? 0,
      hasPreciseScrollingDeltas: w.precise ?? false,
      canScroll: true
    } as any);
  } else if ("key" in action) {
    const modifiers = (action.modifiers ?? []) as any;
    contents.sendInputEvent({ type: "keyDown", keyCode: action.key, modifiers });
    if ([...action.key].length === 1) contents.sendInputEvent({ type: "char", keyCode: action.key, modifiers });
    contents.sendInputEvent({ type: "keyUp", keyCode: action.key, modifiers });
  } else if ("dialog" in action) {
    dialogQueue.push(action.dialog);
  } else if ("capture" in action) {
    await capture(action.capture, action.probes ?? [], action.masks ?? []);
  }
  if ("mouse" in action || "wheel" in action || "key" in action) await nextFrames();
}

function collectFiles() {
  const written: { file: string; bytes: number; seeded: boolean; changed: boolean }[] = [];
  const visit = (rel: string) => {
    const full = path.join(userData, rel);
    for (const entry of fs.readdirSync(full, { withFileTypes: true })) {
      const child = rel === "" ? entry.name : `${rel}/${entry.name}`;
      // Upstream writes top-level JSON files and the asset folders; everything else in userData
      // belongs to Chromium.
      if (rel === "" && entry.isDirectory() && !["userAssets", "autoAssets"].includes(entry.name)) continue;
      if (rel === "" && entry.isFile() && !entry.name.endsWith(".json")) continue;
      if (entry.isDirectory()) {
        visit(child);
        continue;
      }
      const bytes = fs.readFileSync(path.join(userData, child));
      const seed = seedBytes.get(child);
      const dest = path.join(out, "files", child);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.writeFileSync(dest, bytes);
      written.push({ file: child, bytes: bytes.length, seeded: seed !== undefined, changed: seed === undefined || !seed.equals(bytes) });
    }
  };
  visit("");
  return written;
}

async function openColourPatch() {
  const win = new BrowserWindow({
    show: false,
    frame: false,
    resizable: false,
    useContentSize: true,
    width: scenario.window.width,
    height: scenario.window.height,
    webPreferences: { backgroundThrottling: false }
  });
  target = win;
  placeWindow(win);
  win.showInactive();
  await win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(patchPageHtml()));
}

async function run() {
  const started = Date.now();
  if (scenario.page === "colour-patch") {
    await openColourPatch();
    await until("the tap", READY_TIMEOUT_MS, () => exec<boolean>("window.__asrsParity !== undefined"));
  } else {
    await until("the hub window", READY_TIMEOUT_MS, async () => target !== null && !target.webContents.isLoading() && target.isVisible() && (await exec<boolean>(call(hubReady))));
  }
  for (const [i, action] of scenario.actions.entries()) {
    try {
      await perform(action);
    } catch (e) {
      throw new Error(`action ${i} (${actionKind(action)}): ${(e as Error).message}`);
    }
  }
  await sleep(SAVE_SETTLE_MS);
  return Date.now() - started;
}

let finished = false;

function finish(elapsedMs: number | null, error: unknown) {
  if (finished) return;
  finished = true;
  if (error !== null) errors.push(String((error as Error).stack ?? error));
  const files = fs.existsSync(userData) ? collectFiles() : [];
  const record = {
    scenario: scenario.name,
    scenarioFile: path.relative(process.cwd(), scenarioFile).split(path.sep).join("/"),
    scale,
    theme,
    gl: scenario.gl ? glMode : null,
    colour,
    window: scenario.window,
    switches: switchText,
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    platform: process.platform,
    arch: process.arch,
    osRelease: os.release(),
    env: { TZ: process.env.TZ, LANG: process.env.LANG, LC_ALL: process.env.LC_ALL },
    displays: screen.getAllDisplays().map((d) => ({ id: d.id, bounds: d.bounds, workArea: d.workArea, scaleFactor: d.scaleFactor, colorSpace: d.colorSpace, colorDepth: d.colorDepth })),
    displayId,
    displayProfile,
    gpuFeatureStatus: app.getGPUFeatureStatus(),
    targetWindow: target === null || target.isDestroyed() ? null : { bounds: target.getBounds(), contentBounds: target.getContentBounds(), focused: target.isFocused(), title: target.getTitle() },
    windows: windows
      .filter((w) => !w.isDestroyed())
      .map((w) => {
        const url = w.webContents.getURL();
        return url.startsWith("data:") ? "data:" : url.split("/").slice(-2).join("/");
      }),
    elapsedMs,
    blockedRequests: [...new Set(blockedRequests)].sort(),
    dialogs,
    files,
    errors
  };
  fs.writeFileSync(path.join(out, "run.json"), JSON.stringify(record, null, 1) + "\n");
  if (errors.length > 0) console.error(`${scenario.name} ${scale}x ${theme}:\n  ${errors.join("\n  ")}`);
  app.exit(errors.length > 0 ? 1 : 0);
}

process.on("uncaughtException", (e) => {
  errors.push(`uncaught exception in the main process: ${e.stack ?? e}`);
});

app.whenReady().then(() => {
  session.defaultSession.webRequest.onBeforeRequest((details, respond) => {
    const url = new URL(details.url);
    if (/^(https?|wss?):$/.test(url.protocol) && !LOOPBACK.has(url.hostname)) {
      blockedRequests.push(details.url);
      respond({ cancel: true });
    } else {
      respond({});
    }
  });
  const preload = path.join(out, "tap-preload.js");
  fs.writeFileSync(preload, `require("electron").contextBridge.executeInMainWorld({ func: ${installTap.toString()}, args: [${scenario.gl}] });\n`);
  session.defaultSession.registerPreloadScript({ type: "frame", id: "asrs-parity-tap", filePath: preload });
  const timer = setTimeout(() => finish(null, new Error(`run did not finish within ${RUN_TIMEOUT_MS / 1000} s`)), RUN_TIMEOUT_MS);
  run().then(
    (elapsed) => {
      clearTimeout(timer);
      return finish(elapsed, null);
    },
    (e) => {
      clearTimeout(timer);
      return finish(null, e);
    }
  );
});

scriptDialogs();
if (scenario.page === "app") {
  createRequire(import.meta.url)(path.join(__UPSTREAM_DIR__, "bundles", "main.js"));
} else {
  app.on("window-all-closed", () => {});
}
