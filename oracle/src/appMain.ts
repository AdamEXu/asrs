import "./environment";
import { app, BrowserWindow, clipboard, dialog, Menu, net, screen, session, type MenuItem } from "electron";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import { Socket } from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { clipFolder } from "./app/clips";
import { saveDisplayProfile, type DisplayProfile } from "./app/display";
import { GENERATED_LOGS } from "./app/logs";
import { busyReasons, domDump, downloadShows, focused, hubReady, sendMainMessage } from "./app/page";
import { patchPageHtml } from "./app/patches";
import { actionKind, parseScenario, type Action, type GlMode, type Scale, type Theme } from "./app/scenario";
import { installTap } from "./app/tap";
import { encodePng } from "./canvas/png";

declare const __UPSTREAM_DIR__: string;

const USAGE = `usage: electron <switches> appMain.mjs <scenario.json> <out dir> --scale=<1|2> --theme=<light|dark>
         [--gl=<mode>] [--colour=srgb|display] [--robot=<port>]

Starts upstream's bundles/main.js with an isolated userData directory and drives it through the
scenario with executeJavaScript and sendInputEvent. --gl picks the GL backend switches of a
scenario with "gl": true: native (macOS default), swiftshader (Linux and Windows default) or
d3d11-warp. --colour=srgb (default) has Chromium composite in sRGB; --colour=display keeps the
display's colour space and stores its ICC profile as display.icc. The driver names the Electron
switches it needs when they are missing. --robot is where a scenario's robotServer listens: main's
connections to port 22 of 127.0.0.1 go there. Results go to <out dir>: run.json, captures/, files/,
inputs/ and, with --colour=display, display.icc.`;

const WINDOW_ORIGIN = 16;
const RANDOM_SEED = 1;
const RUN_TIMEOUT_MS = 480_000;
const READY_TIMEOUT_MS = 90_000;
const IDLE_TIMEOUT_MS = 60_000;
const STABLE_TIMEOUT_MS = 30_000;
const STABLE_CAPTURES = 3;
const STABLE_GAP_MS = 120;
const SAVE_SETTLE_MS = 1500;
const STATE_SAVE_TIMEOUT_MS = 5000;
const MENU_TIMEOUT_MS = 10_000;
const PROMPT_SETTLE_MS = 300;
const COPY_SETTLE_MS = 200;
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
const robotPort = Number(options.robot);
const colour = options.colour ?? "srgb";
if (colour !== "srgb" && colour !== "display") fail(USAGE);
const scenario = parseScenario(path.basename(scenarioFile, ".json"), fs.readFileSync(scenarioFile, "utf8"));
if (scenario.page === "app" && (scenario.window.width < 800 || scenario.window.height < 400)) {
  fail(`scenario ${scenario.name}: the hub window has a minimum size of 800x400`);
}

if (scenario.robot && !Number.isInteger(robotPort)) fail(`scenario ${scenario.name} needs --robot`);

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

// `{"$file": path}` seeds a binary file from parity/scenarios.
function seedContents(value: unknown): string | Buffer {
  const file = (value as { $file?: string } | null)?.$file;
  return file === undefined ? JSON.stringify(value) : fs.readFileSync(path.join(path.dirname(scenarioFile), file));
}

const seeds: Record<string, unknown> = {
  "prefs.json": { ...scenario.prefs, theme },
  "beta-2027.json": { welcomeComplete: true, surveyStatus: true },
  ...scenario.userData
};
for (const [file, value] of Object.entries(seeds)) {
  const seedFile = path.join(userData, file);
  fs.mkdirSync(path.dirname(seedFile), { recursive: true });
  fs.writeFileSync(seedFile, seedContents(value));
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

const connect = Socket.prototype.connect;
Socket.prototype.connect = function (this: Socket, ...args: any[]) {
  const target = args[0];
  if (scenario.robot && target?.host === "127.0.0.1" && target.port === 22) return connect.call(this, { ...target, port: robotPort });
  return (connect as any).apply(this, args);
} as any;

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
    if (file !== null) fs.mkdirSync(String(value).endsWith("/") ? file : path.dirname(file), { recursive: true });
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
// The prompt window a "prompt" action waited for: input and captures go to it until it closes.
let prompt: BrowserWindow | null = null;
let scriptedInput = false;

function active(): BrowserWindow {
  if (prompt !== null && prompt.isDestroyed()) prompt = null;
  return prompt ?? target!;
}

// Popping a menu up natively would block on a menu no script can click, so menus are kept for a
// "menu" action instead.
let popped: { menu: Menu; window: BrowserWindow } | null = null;
Menu.prototype.popup = function (this: Menu, options?: Electron.PopupOptions) {
  popped = { menu: this, window: (options?.window as BrowserWindow | undefined) ?? target! };
};

async function clickMenu(labels: string[]) {
  const { menu, window } = await until("a menu to pop up", MENU_TIMEOUT_MS, async () => popped ?? false);
  popped = null;
  let items: MenuItem[] = menu.items;
  for (const [i, label] of labels.entries()) {
    const item = items.find((it) => it.label === label);
    if (item === undefined) throw new Error(`no menu item ${JSON.stringify(label)} among ${JSON.stringify(items.map((it) => it.label))}`);
    if (i === labels.length - 1) {
      if (!item.enabled) throw new Error(`menu item ${JSON.stringify(label)} is disabled`);
      (item.click as any)({}, window, window.webContents);
    } else {
      items = item.submenu!.items;
    }
  }
}

function clickAppMenu(labels: string[]) {
  const found: MenuItem[] = [];
  const visit = (items: MenuItem[]) => {
    for (const item of items) {
      if (item.submenu) visit(item.submenu.items);
      else if (labels.includes(item.label)) found.push(item);
    }
  };
  visit(Menu.getApplicationMenu()!.items);
  if (found.length !== 1) throw new Error(`${found.length} application menu items are labelled ${JSON.stringify(labels)}`);
  (found[0].click as any)({}, target, target!.webContents);
}

function hostModifiers(names: string[] = []): string[] {
  return names.map((name) => (name !== "commandOrControl" ? name : process.platform === "darwin" ? "meta" : "control"));
}

function isSatellite(win: BrowserWindow): boolean {
  return new URL(win.webContents.getURL() || "about:blank").pathname.endsWith("/www/satellite.html");
}

async function switchPrompt(name: string | null, index: number, width?: number, height?: number) {
  if (name === "hub") {
    if (prompt !== null && !prompt.isDestroyed() && !isSatellite(prompt)) throw new Error("the prompt is still open");
    prompt = null;
    return;
  }
  if (name === null) {
    await until("the prompt to close", READY_TIMEOUT_MS, async () => prompt === null || prompt.isDestroyed());
    prompt = null;
    return;
  }
  prompt = await until(`the ${name} prompt`, READY_TIMEOUT_MS, async () => {
    const win = windows.filter((w) => !w.isDestroyed() && new URL(w.webContents.getURL() || "about:blank").pathname.endsWith(`/www/${name}.html`))[index];
    if (win === undefined || win.webContents.isLoading() || !win.isVisible()) return false;
    return win;
  });
  if (width !== undefined && height !== undefined) prompt.setContentSize(width, height);
  await sleep(PROMPT_SETTLE_MS);
}

function sendScripted(contents: Electron.WebContents, input: Parameters<Electron.WebContents["sendInputEvent"]>[0]) {
  scriptedInput = true;
  try {
    contents.sendInputEvent(input);
  } finally {
    scriptedInput = false;
  }
}
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
  // A modal prompt's show() begins a sheet on its parent, which activates nothing;
  // showInactive() never shows it.
  if (!win.isModal()) win.show = () => win.showInactive();
  // macOS can activate the app all the same, as it launches, and a focused page
  // draws focus rings, which the port, captured inactive, does not.
  if (process.platform === "darwin") {
    win.setFocusable(false);
    win.on("focus", () => errors.push(`a window took the focus during ${performing}`));
  }
  win.setIgnoreMouseEvents(true);
  // macOS still hands the page a move from the physical cursor when the window appears under
  // it, so every mouse event the scenario did not send is dropped.
  win.webContents.on("before-mouse-event", (event) => {
    if (!scriptedInput) event.preventDefault();
  });
  win.webContents.on("did-start-navigation", (event: any) => {
    if (target === null && event.isMainFrame && /\/www\/hub\.html$/.test(new URL(event.url).pathname)) {
      target = win;
      placeWindow(win);
    }
    // Windows of other apps can cover a satellite, which hides its page and stops its frames.
    if (event.isMainFrame && /\/www\/satellite\.html$/.test(new URL(event.url).pathname)) win.webContents.setBackgroundThrottling(false);
  });
});

async function exec<T = unknown>(code: string, win: BrowserWindow = target!): Promise<T> {
  return win.webContents.executeJavaScript(code);
}

const call = (fn: Function, ...args: unknown[]) => `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(", ")})`;

async function nextFrames(count = 2) {
  const win = active();
  let onClosed = () => {};
  const closed = new Promise<void>((resolve) => win.once("closed", (onClosed = resolve)));
  const frames = exec(`new Promise((r) => { let n = ${count}; const f = () => (--n <= 0 ? r() : requestAnimationFrame(f)); requestAnimationFrame(f); })`, win).catch((e) => {
    if (!win.isDestroyed()) throw e;
  });
  // A prompt that input closed never answers.
  await Promise.race(win === target ? [frames] : [frames, closed]);
  if (!win.isDestroyed()) win.off("closed", onClosed);
}

async function waitIdle() {
  let last: string[] = [];
  await until("the page to go idle", IDLE_TIMEOUT_MS, async () => {
    last = await exec<string[]>(call(busyReasons));
    const win = active();
    if (win !== target && isSatellite(win)) last = last.concat(await exec<string[]>(call(busyReasons), win));
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
  const win = active();
  const onPage = <T = unknown>(code: string) => exec<T>(code, win);
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
    const nextDom = await onPage<any>(call(domDump, probes, maskSelectors));
    const image = await win.webContents.capturePage();
    const nextSize = image.getSize(scale);
    const next = bgraToRgba(image.toBitmap({ scaleFactor: scale }));
    const nextGlReads = await onPage<number>("window.__asrsParity.glReads()");
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

  const content = win === target ? scenario.window : { width: win.getContentSize()[0], height: win.getContentSize()[1] };
  const expected = { width: content.width * scale, height: content.height * scale };
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
    // A satellite's capture has the state the satellite saved.
    const page = win !== target && isSatellite(win) ? win : target!;
    const saves = await exec<number>("window.__asrsParity.saves()", page);
    await until(`capture ${name}: a save-state after the capture`, STATE_SAVE_TIMEOUT_MS, async () => (await exec<number>("window.__asrsParity.saves()", page)) > saves);
    hubState = `${name}.hub-state.json`;
    fs.writeFileSync(path.join(capturesDir, hubState), await exec<string>("window.__asrsParity.savedState()", page));
    timelineRange = await exec<number[]>("window.selection.getTimelineRange()");
  }

  const patches =
    scenario.page === "colour-patch"
      ? await exec("Array.from(document.querySelectorAll('.patch'), (el) => { const r = el.getBoundingClientRect(); return { rgb: el.dataset.rgb.split(',').map(Number), rect: [r.x, r.y, r.width, r.height] }; })")
      : undefined;
  const contexts: any[] = await onPage("window.__asrsParity.contexts()");
  const canvases = [];
  const gl = [];
  const glFrames: Record<string, number> = {};
  for (const context of contexts) {
    if (!context.visible) continue;
    if (context.kind === "2d") {
      const pixels: any = await onPage(`window.__asrsParity.canvasPixels(${context.index})`);
      if (pixels.width === 0 || pixels.height === 0) continue;
      const file = `${name}.canvas-${context.index}.png`;
      fs.writeFileSync(path.join(capturesDir, file), encodePng(pixels.width, pixels.height, Buffer.from(pixels.rgba)));
      canvases.push({ ...context, file, colorSpace: pixels.colorSpace });
    } else if (context.kind === "webgl" || context.kind === "webgl2") {
      if (!scenario.gl) continue;
      const pixels: any = await onPage(`window.__asrsParity.glPixels(${context.index})`);
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
  if (scenario.gl && win === target && gl.length === 0) throw new Error(`capture ${name}: the scenario is a 3D scenario but no WebGL canvas is visible`);

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
        timing: { stableAfter: attempts, glFrames, frame: await onPage<number>("window.__asrsParity.frame()") }
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

// `{"$clip": name}` is the folder of a generated clip.
function resolveClips(value: unknown): unknown {
  if (typeof value !== "object" || value === null) return value;
  if (Array.isArray(value)) return value.map(resolveClips);
  const clip = (value as { $clip?: string }).$clip;
  if (clip !== undefined) return clipFolder(path.join(out, "inputs"), clip);
  return Object.fromEntries(Object.entries(value).map(([key, v]) => [key, resolveClips(v)]));
}

// What the window's copy leaves on an emptied clipboard.
async function copy(contents: Electron.WebContents, name: string) {
  if (process.platform === "darwin" && process.env.ASRS_PRIVATE_PASTEBOARD === undefined) throw new Error("a copy needs xtask's private pasteboard");
  clipboard.clear();
  contents.copy();
  await nextFrames();
  await sleep(COPY_SETTLE_MS);
  const folder = path.join(out, "files", "clipboard");
  fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(folder, `${name}.txt`), clipboard.readText());
}

async function emulateFocus(contents: Electron.WebContents, enabled: boolean) {
  if (!contents.debugger.isAttached()) contents.debugger.attach("1.3");
  await contents.debugger.sendCommand("Emulation.setFocusEmulationEnabled", { enabled });
}

// Presses Tab `count` times and keeps what had the focus after each, or what has it now.
async function traverse(contents: Electron.WebContents, name: string, count: number, back: boolean) {
  const win = active();
  const records = [];
  for (let i = 0; i < Math.max(count, 1); i++) {
    if (count > 0) {
      const modifiers = back ? ["shift" as const] : [];
      contents.sendInputEvent({ type: "keyDown", keyCode: "Tab", modifiers });
      contents.sendInputEvent({ type: "keyUp", keyCode: "Tab", modifiers });
      await nextFrames(3);
    }
    records.push(await exec(call(focused), win));
  }
  const folder = path.join(out, "files", "focus");
  fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(folder, `${name}.json`), records.map((record) => JSON.stringify(record) + "\n").join(""));
}

let pressedButton: string | null = null;
const MOUSE_TYPES = { move: ["mouseMove"], down: ["mouseDown"], up: ["mouseUp"], click: ["mouseDown", "mouseUp"] } as const;

async function perform(action: Action) {
  const contents = active().webContents;
  if ("main" in action) {
    await exec(call(sendMainMessage, action.main, resolveClips(action.data)));
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
    const button = action.button ?? "left";
    for (const type of MOUSE_TYPES[action.mouse]) {
      // A real mouse reports the held button on its down and moves (PointerEvent.buttons).
      if (type === "mouseDown") pressedButton = button;
      if (type === "mouseUp") pressedButton = null;
      const modifiers = [...hostModifiers(action.modifiers), ...(pressedButton === null ? [] : [`${pressedButton}ButtonDown`])];
      sendScripted(contents, { type, x: action.x, y: action.y, button, clickCount: action.clickCount ?? 1, modifiers } as any);
    }
  } else if ("wheel" in action) {
    const w = action.wheel;
    sendScripted(contents, {
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
    const modifiers = hostModifiers(action.modifiers) as any;
    const send = (type: "keyDown" | "char" | "keyUp", keyCode = action.key) => {
      if (!contents.isDestroyed()) contents.sendInputEvent({ type, keyCode, modifiers });
    };
    send("keyDown");
    if (action.key === "Enter") send("char", "\r");
    else if ([...action.key].length === 1) send("char");
    send("keyUp");
  } else if ("dialog" in action) {
    dialogQueue.push(action.dialog);
  } else if ("menu" in action) {
    await clickMenu(action.menu);
  } else if ("appMenu" in action) {
    clickAppMenu(action.appMenu);
  } else if ("prompt" in action) {
    await switchPrompt(action.prompt, action.index ?? 0, action.width, action.height);
  } else if ("download" in action) {
    await until(`the download window to show ${JSON.stringify(action.download)}`, READY_TIMEOUT_MS, () => exec<boolean>(call(downloadShows, action.download), active()));
  } else if ("role" in action) {
    contents[action.role]();
    await nextFrames();
  } else if ("copy" in action) {
    await copy(contents, action.copy);
  } else if ("focus" in action) {
    await emulateFocus(contents, action.focus);
  } else if ("traverse" in action) {
    await traverse(contents, action.traverse, action.count ?? 0, action.back ?? false);
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

let performing = "startup";

async function run() {
  const started = Date.now();
  if (scenario.page === "colour-patch") {
    await openColourPatch();
    await until("the tap", READY_TIMEOUT_MS, () => exec<boolean>("window.__asrsParity !== undefined"));
  } else {
    await until("the hub window", READY_TIMEOUT_MS, async () => target !== null && !target.webContents.isLoading() && target.isVisible() && (await exec<boolean>(call(hubReady))));
  }
  for (const [i, action] of scenario.actions.entries()) {
    performing = `action ${i} (${actionKind(action)})`;
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
  const timer = setTimeout(() => finish(null, new Error(`run did not finish within ${RUN_TIMEOUT_MS / 1000} s (in ${performing})`)), RUN_TIMEOUT_MS);
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

// Upstream's alpha expires on 2027-01-09, after which every hub opens a dialog. Only that date
// moves, not main's clock: the clock is in what runs record, like the save dialogs' file names.
function requireMain() {
  const RealDate = Date;
  let moved = 0;
  globalThis.Date = new Proxy(RealDate, {
    construct(target, args, newTarget) {
      const expiration = String(args) === "2027,0,9";
      if (expiration) moved++;
      return Reflect.construct(target, expiration ? [8.64e15] : args, newTarget);
    }
  });
  try {
    createRequire(import.meta.url)(path.join(__UPSTREAM_DIR__, "bundles", "main.js"));
  } finally {
    globalThis.Date = RealDate;
  }
  if (moved !== 1) fail(`upstream's main built its expiration date ${moved} times, expected once`);
}

scriptDialogs();
if (scenario.page === "app") {
  requireMain();
} else {
  app.on("window-all-closed", () => {});
}
