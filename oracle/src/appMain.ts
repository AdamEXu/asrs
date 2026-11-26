import "./environment";
import { app, BrowserWindow, clipboard, dialog, Menu, net, screen, session, type MenuItem } from "electron";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import { Socket } from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { clipFolder } from "./app/clips";
import { Desktop } from "./app/desktop";
import { saveDisplayProfile, type DisplayProfile } from "./app/display";
import { GENERATED_LOGS } from "./app/logs";
import { busyReasons, domDump, downloadShows, focused, hubReady, sendMainMessage } from "./app/page";
import { patchPageHtml } from "./app/patches";
import { placePoint } from "./app/points";
import { actionKind, parseScenario, type Action, type GlMode, type Scale, type Theme } from "./app/scenario";
import { installTap } from "./app/tap";
import { GRID_STEP, titleMap, titleMapArgs, type TitleMap } from "./app/tooltips";
import { encodePng } from "./canvas/png";

declare const __UPSTREAM_DIR__: string;

const USAGE = `usage: electron <switches> appMain.mjs <scenario.json> <out dir> --scale=<1|2> --theme=<light|dark>
         [--gl=<mode>] [--colour=srgb|display] [--robot=<port>] [--live=<port>] [--tooltips]

Starts upstream's bundles/main.js with an isolated userData directory and drives it through the
scenario with executeJavaScript and sendInputEvent. --gl picks the GL backend switches of a
scenario with "gl": true: native (macOS default), swiftshader (Linux and Windows default) or
d3d11-warp. --colour=srgb (default) has Chromium composite in sRGB; --colour=display keeps the
display's colour space and stores its ICC profile as display.icc. The driver names the Electron
switches it needs when they are missing. --robot is where a scenario's robotServer listens: main's
connections to port 22 of 127.0.0.1 go there. --live is the same for a scenario's live server and the
RLOG port. Results go to <out dir>: run.json, captures/, files/,
inputs/ and, with --colour=display, display.icc. --tooltips (macOS, with oracle/tools/tooltips.m
loaded) records the page's tooltips at each capture instead of its pixels.`;

const WINDOW_ORIGIN = 16;
const RANDOM_SEED = 1;
const RLOG_PORT = 5800;
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
const POINTER_SETTLE_MS = 700;
// The strip of the hub's title bar its page keeps clear for Electron's caption buttons (`right:
// 140px` of div.title-bar-text, the overlay's height).
const CAPTION = { width: 140, height: 38 };
const TOOLTIP_SETTLE_MS = 300;
const TOOLTIP_FRAME_MS = 20;
const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]", "::1"]);

function fail(message: string): never {
  console.error(message);
  process.exit(2);
}

const script = process.argv.findIndex((a) => /appMain\.mjs$/.test(a));
// macOS takes `-AppleShowScrollBars <style>`, the last two, as a default of this process.
const argv = process.argv.slice(script + 1).filter((a, i, all) => a !== "-AppleShowScrollBars" && all[i - 1] !== "-AppleShowScrollBars");
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
const livePort = Number(options.live);
const colour = options.colour ?? "srgb";
if (colour !== "srgb" && colour !== "display") fail(USAGE);
const tooltipLog = "tooltips" in options ? process.env.ASRS_TOOLTIP_LOG : undefined;
if ("tooltips" in options && tooltipLog === undefined) fail("--tooltips needs ASRS_TOOLTIP_LOG");
// What is thrown here would wait in Electron's own error box.
const scenario = (() => {
  try {
    return parseScenario(path.basename(scenarioFile, ".json"), fs.readFileSync(scenarioFile, "utf8"));
  } catch (e) {
    return fail((e as Error).message);
  }
})();
if (scenario.page === "app" && (scenario.window.width < 800 || scenario.window.height < 400)) {
  fail(`scenario ${scenario.name}: the hub window has a minimum size of 800x400`);
}

if (scenario.robot && !Number.isInteger(robotPort)) fail(`scenario ${scenario.name} needs --robot`);
if (scenario.live && !Number.isInteger(livePort)) fail(`scenario ${scenario.name} needs --live`);

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
  // `net.createConnection` passes its normalized arguments on as one array.
  const target = Array.isArray(args[0]) ? args[0][0] : args[0];
  if (scenario.live && target?.host === "127.0.0.1" && target.port === RLOG_PORT) target.port = livePort;
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
// The real input the page gets: what else the pointer does on its way is dropped.
let realInput: "none" | "all" | "wheel" = "none";

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
  win.webContents.on("before-mouse-event", (event, mouse) => {
    if (scriptedInput || realInput === "all" || (realInput === "wheel" && mouse.type === "mouseWheel")) return;
    event.preventDefault();
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

// toBitmap gives premultiplied colours, which the acrylic sidebar's half transparent ones show.
function straighten(rgba: Buffer) {
  for (let i = 0; i < rgba.length; i += 4) {
    const alpha = rgba[i + 3];
    if (alpha === 0 || alpha === 255) continue;
    for (let c = i; c < i + 3; c++) rgba[c] = Math.min(255, Math.floor((rgba[c] * 255 + (alpha >> 1)) / alpha));
  }
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

const desktop = process.platform === "win32" && scenario.page === "app" ? new Desktop() : null;

function contentOrigin(win: BrowserWindow) {
  return screen.dipToScreenRect(win, win.getContentBounds());
}

// Electron draws the caption buttons over the page, where capturePage does not see them.
async function drawCaptionButtons(rgba: Buffer, width: number) {
  if (desktop === null) return;
  target!.moveTop();
  const origin = contentOrigin(target!);
  const [w, h] = [CAPTION.width * scale, CAPTION.height * scale];
  const strip = bgraToRgba(await desktop.grab(origin.x + width - w, origin.y, w, h));
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const from = (y * w + x) * 4;
      const to = (y * width + width - w + x) * 4;
      strip.copy(rgba, to, from, from + 3);
      rgba[to + 3] = 255;
    }
  }
}

async function movePointer(action: Extract<Action, { pointer: string }>) {
  if (desktop === null) throw new Error("the real pointer is only driven on Windows");
  const origin = contentOrigin(target!);
  target!.setIgnoreMouseEvents(false);
  target!.moveTop();
  const kind = action.pointer;
  // The timeline zooms about a hovered time, which moves with every step of Chromium's animation.
  realInput = kind === "wheel" ? "wheel" : "all";
  try {
    await desktop.pointer(origin.x + Math.round(action.x * scale), origin.y + Math.round(action.y * scale), kind === "down" || kind === "up" ? kind : undefined);
    if (kind === "wheel") await desktop.wheel(action.unitsX ?? 0, action.unitsY ?? 0);
    await sleep(POINTER_SETTLE_MS);
  } finally {
    realInput = "none";
  }
}

async function activate(active: boolean) {
  if (desktop === null) throw new Error("windows are only activated on Windows");
  if (active) target!.focus();
  else target!.blur();
  await until(`the hub to be ${active ? "active" : "inactive"}`, READY_TIMEOUT_MS, async () => target!.isFocused() === active);
  await sleep(POINTER_SETTLE_MS);
}

let displayProfile: DisplayProfile | null = null;
let displayId: number | null = null;

const pointer: [number, number] = [0, 0];

// What AppKit got since the last call.
let tooltipBytes = 0;
function tooltipTexts(): string[] {
  const bytes = fs.existsSync(tooltipLog!) ? fs.readFileSync(tooltipLog!) : Buffer.alloc(0);
  const lines = bytes.subarray(tooltipBytes).toString("utf8").split("\n").slice(0, -1);
  tooltipBytes = bytes.length;
  return lines.map((line) => JSON.parse(line)[0]);
}

async function captureTooltips(name: string) {
  // The moves would be part of a drag.
  if (pressedButton !== null) return;
  const win = active();
  await waitIdle();
  const move = async (x: number, y: number) => {
    sendScripted(win.webContents, { type: "mouseMove", x, y });
    await nextFrames();
    await sleep(TOOLTIP_FRAME_MS);
    return tooltipTexts();
  };
  // Where no title is, and what the pointer hovers changes no layout.
  await move(0, 0);
  await sleep(TOOLTIP_SETTLE_MS);
  tooltipTexts();
  const map = await exec<TitleMap>(call(titleMap, ...titleMapArgs), win);
  const y = map.grid.findIndex((row) => row.includes("."));
  const bare = y < 0 ? null : [map.grid[y].indexOf("."), y].map((i) => (i + 0.5) * GRID_STEP);
  const titles = [];
  for (const { title, rect, point } of map.titles) {
    // Off every title first, so that the text is a change.
    if (bare !== null) await move(bare[0], bare[1]);
    const shown = await move(point[0], point[1]);
    const corner = await move(Math.ceil(rect[0]) + 2, Math.ceil(rect[1]) + 2);
    titles.push({ title, rect, point, shown, corner });
  }
  let left: string[] | undefined;
  if (titles.length > 0) {
    await move(titles[0].point[0], titles[0].point[1]);
    sendScripted(win.webContents, { type: "mouseLeave", x: -1, y: -1 } as any);
    await sleep(TOOLTIP_FRAME_MS * 5);
    left = tooltipTexts();
  }
  await move(pointer[0], pointer[1]);
  fs.writeFileSync(path.join(capturesDir, `${name}.tooltips.json`), JSON.stringify({ titles, left, gridStep: GRID_STEP, texts: map.texts, grid: map.grid }) + "\n");
  console.error(`  ${scenario.name}: ${name} (${titles.length} titles)`);
}

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

  // Whether a caret blinks on is a matter of time, and the port draws none in captures.
  const noCaret = await win.webContents.insertCSS("* { caret-color: transparent !important; }");
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
    if (process.platform === "win32") straighten(next);
    if (win === target) await drawCaptionButtons(next, nextSize.width);
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

  await win.webContents.removeInsertedCSS(noCaret);
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
  if (spec.startsWith("public:")) {
    const file = path.join(process.env.ASRS_CACHE!, "public-logs", spec.slice("public:".length));
    if (!fs.existsSync(file)) throw new Error(`${spec} is not fetched; run \`cargo xtask corpus fetch\``);
    return file;
  }
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

// Where each mouse and wheel action went and the size prompt actions left the prompt at, by index.
const points: Record<number, { x: number; y: number; hit: string | null } | { width: number; height: number }> = {};

async function place(index: number, x: number, y: number, on?: string, right?: number) {
  const win = active();
  const titleBar = scenario.titleBar && win === target && process.platform === "linux";
  const at = scenario.page === "app" ? await exec<{ x: number; y: number; hit: string | null }>(call(placePoint, x, y, titleBar, on ?? null, process.platform === "darwin" ? null : (right ?? null)), win) : { x, y, hit: null };
  if (process.platform === "darwin" && (at.x !== x || at.y !== y)) throw new Error(`[${x}, ${y}] does not hit ${on}`);
  points[index] = at;
  return at;
}

let pressedButton: string | null = null;
const MOUSE_TYPES = { move: ["mouseMove"], down: ["mouseDown"], up: ["mouseUp"], click: ["mouseDown", "mouseUp"], leave: ["mouseLeave"] } as const;

async function perform(action: Action, index: number) {
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
    const { x, y } = await place(index, action.x, action.y, action.on, action.right);
    pointer[0] = x;
    pointer[1] = y;
    for (const type of MOUSE_TYPES[action.mouse]) {
      // A real mouse reports the held button on its down and moves (PointerEvent.buttons).
      if (type === "mouseDown") pressedButton = button;
      if (type === "mouseUp") pressedButton = null;
      const modifiers = [...hostModifiers(action.modifiers), ...(pressedButton === null ? [] : [`${pressedButton}ButtonDown`])];
      sendScripted(contents, { type, x, y, button, clickCount: action.clickCount ?? 1, modifiers } as any);
    }
  } else if ("wheel" in action) {
    const w = action.wheel;
    const { x, y } = await place(index, w.x, w.y);
    sendScripted(contents, {
      type: "mouseWheel",
      x,
      y,
      deltaX: w.deltaX ?? 0,
      deltaY: w.deltaY ?? 0,
      wheelTicksX: w.wheelTicksX ?? 0,
      wheelTicksY: w.wheelTicksY ?? 0,
      hasPreciseScrollingDeltas: w.precise ?? false,
      canScroll: true
    } as any);
  } else if ("pointer" in action) {
    points[index] = { x: action.x, y: action.y, hit: null };
    await movePointer(action);
  } else if ("activate" in action) {
    await activate(action.activate);
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
    if (action.width !== undefined) {
      // A window manager keeps it at its least size, which on Linux is its content's.
      const [width, height] = prompt!.getContentSize();
      points[index] = { width, height };
    }
  } else if ("download" in action) {
    await until(`the download window to show ${JSON.stringify(action.download)}`, READY_TIMEOUT_MS, () => exec<boolean>(call(downloadShows, action.download), active()));
  } else if ("role" in action) {
    contents[action.role]();
    await nextFrames();
  } else if ("copy" in action) {
    await copy(contents, action.copy);
  } else if ("title" in action) {
    const matches = async () => ((await exec<string>("document.title")) === action.title) !== (action.not ?? false);
    await until(`the title ${action.not ? "not to be" : "to be"} ${JSON.stringify(action.title)}`, READY_TIMEOUT_MS, matches);
    await nextFrames();
  } else if ("focus" in action) {
    await emulateFocus(contents, action.focus);
  } else if ("traverse" in action) {
    await traverse(contents, action.traverse, action.count ?? 0, action.back ?? false);
  } else if ("hover" in action) {
    const folder = path.join(out, "files", "hover");
    fs.mkdirSync(folder, { recursive: true });
    const hovered = await exec("window.selection.getHoveredTime()");
    fs.writeFileSync(path.join(folder, `${action.hover}.json`), JSON.stringify(hovered) + "\n");
  } else if ("capture" in action) {
    if (tooltipLog === undefined) await capture(action.capture, action.probes ?? [], action.masks ?? []);
    else await captureTooltips(action.capture);
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
      await perform(action, i);
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
  desktop?.close();
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
    points,
    files,
    errors
  };
  fs.writeFileSync(path.join(out, "run.json"), JSON.stringify(record, null, 1) + "\n");
  if (errors.length > 0) console.error(`${scenario.name} ${scale}x ${theme}:\n  ${errors.join("\n  ")}`);
  // On Linux exiting past an open modal prompt crashes.
  for (const win of [...windows].reverse()) if (!win.isDestroyed()) win.destroy();
  setTimeout(() => app.exit(errors.length > 0 ? 1 : 0), 200);
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
