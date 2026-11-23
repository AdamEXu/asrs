import { setNow } from "../fakes/globals";
import * as fakeFs from "../fakes/mainFs";
import * as electronFake from "../fakes/mainElectron";
import { toTree } from "../cve1";
import { buildMainBundle } from "../update/bundle";

// `genMenus clicks <platform> <dev|packaged>`: main.ts's setupMenu handlers, with the real
// UpdateChecker and assetLoader (update/bundle.ts) against the `files` fakes. The generator keeps
// two hubs, a satellite and a preferences window open, focuses one of them (or none), calls the
// item's `click` as Electron would (item, focused window) and records what the handler did:
//   dialogs   showOpenDialog, showSaveDialog, showMessageBox and showMessageBoxSync calls, with
//             the parent window's id and the scripted reply; `icon` is "WINDOW_ICON" when it is
//             ElectronConstants' WINDOW_ICON
//   sent      messages posted on each window's port
//   popups    context menus: window id, position and item labels
//   created   windows opened: {id, file, bounds}, in order
//   focused   windows focused, closed, destroyed and moveTop: window ids, in order
//   external  shell.openExternal URLs; `paths` shell.openPath arguments; `clipboard` texts
//   files     the files written, as they are afterwards; null for a file that is gone
//   throw     the error name when the handler threw, `rejections` when a promise it made did
//   menu      the fields sendAllPreferences rewrites, read back from the template
// Other records set the world up: "config", "putFile", "removeFile", "setFailure", "satellite",
// "saveState", "domReady" and "close". "updatePreferences" is a hub's message, observed like a
// click.
// `checked` is the MenuItem's state the handler sees; Electron flips it before the click.

const T0 = Date.UTC(2026, 10, 21, 12, 34, 56);
const BETA_FILE = "/fake/userData/beta-2027.json";
const PREFS_FILE = "/fake/userData/prefs.json";
const CUSTOM_ASSETS = "/fake/custom assets";
const SAVED_LAYOUT = "/fake/out/layout.json";
const STUBS = {
  "main/electron/assetLoader": null,
  "main/electron/assetDownloader":
    "export function getAssetDownloadStatus() { return 'asset download status'; } export function startAssetDownloadLoop() {}",
  "main/electron/owletDownloadLoop":
    "export function getOwletDownloadStatus() { return 'owlet download status'; } export function startOwletDownloadLoop() {}"
};

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const flush = async (times = 20) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};
const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));

type Window = electronFake.FakeBrowserWindow;
type Reply = { kind: "open" | "save" | "box" | "boxSync"; value: any };
type Focus = "hub" | "satellite" | "preferences" | "none";

let focused: Window | null = null;
let windowIcon: unknown;
const loads = new Map<number, string>();
const replies: Reply[] = [];
const dialogs: any[] = [];
const popups: any[] = [];
const focusedIds: number[] = [];
const destroyedIds: number[] = [];
const movedIds: number[] = [];
const openedPaths: any[] = [];
const clipboard: any[] = [];
const rejections: string[] = [];

function describeOptions(options: any): any {
  const described: any = {};
  for (const key of Object.keys(options)) {
    described[key] = key === "icon" && options.icon === windowIcon ? "WINDOW_ICON" : options[key];
  }
  return described;
}

function showDialog(kind: Reply["kind"], args: any[]): any {
  const [parent, options] = args.length >= 2 ? [args[0], args[1]] : [null, args[0]];
  const reply = replies.shift();
  if (reply === undefined || reply.kind !== kind) throw new Error(`unscripted ${kind} dialog: ${JSON.stringify(options)}`);
  dialogs.push(toTree({ kind, window: parent === null ? null : parent.id, options: describeOptions(options), reply: reply.value }));
  return reply.value;
}

function install(constants: any): void {
  windowIcon = constants.WINDOW_ICON;
  const proto = electronFake.FakeBrowserWindow.prototype as any;
  electronFake.BrowserWindow.getFocusedWindow = () => focused;
  proto.loadFile = function (this: Window, file: string) {
    loads.set(this.id, file.split("/").pop()!);
  };
  proto.focus = function (this: Window) {
    focused = electronFake.BrowserWindow.fromId(this.id);
    focusedIds.push(this.id);
    this.emit("focus");
  };
  proto.moveTop = function (this: Window) {
    movedIds.push(this.id);
  };
  // Electron closes a window only after its page has unloaded, never inside close().
  const closeNow = proto.close;
  proto.close = function (this: Window) {
    setImmediate(() => closeNow.call(this));
  };
  const destroy = proto.destroy;
  proto.destroy = function (this: Window) {
    destroyedIds.push(this.id);
    destroy.call(this);
    this.emit("closed");
  };
  electronFake.Menu.prototype.popup = function (this: electronFake.Menu, options?: any) {
    popups.push({ window: options.window.id, x: options.x, y: options.y, labels: this.items.map((item) => item.options.label) });
  };
  electronFake.dialog.showOpenDialog = async (...args: any[]) => showDialog("open", args);
  electronFake.dialog.showSaveDialog = async (...args: any[]) => showDialog("save", args);
  electronFake.dialog.showMessageBox = async (...args: any[]) => showDialog("box", args);
  electronFake.dialog.showMessageBoxSync = (...args: any[]) => showDialog("boxSync", args);
  (electronFake.shell as any).openPath = async (path: any) => {
    openedPaths.push(path);
    return "";
  };
  electronFake.clipboard.writeText = (text: any) => {
    clipboard.push(text);
  };
  electronFake.net.fetch = (): any => Promise.reject(new TypeError("fetch failed"));
  process.on("unhandledRejection", (error: any) => {
    rejections.push(error?.name ?? String(error));
  });
}

function windowPort(window: Window): electronFake.FakePort | undefined {
  const posted = window.webContents.posted.filter((p) => p.channel === "port");
  const port1 = posted[posted.length - 1]?.ports?.[0];
  return electronFake.channels.find((c) => c.port1 === port1)?.port2;
}

const ports = new Map<electronFake.FakePort, number>();
function labelPorts(): void {
  for (const window of electronFake.windowList()) {
    const port = windowPort(window);
    if (port !== undefined) ports.set(port, window.id);
  }
}

const template = (): any[] => {
  const items = electronFake.menuTemplate();
  if (items === null) throw new Error("main.ts set no application menu");
  return items;
};

const itemAt = (path: number[]): any => path.slice(1).reduce((item, index) => item.submenu[index], template()[path[0]]);

function clickPaths(items: any[], prefix: number[] = []): number[][] {
  return items.flatMap((item, index) => {
    const path = [...prefix, index];
    return [...(typeof item.click === "function" ? [path] : []), ...(Array.isArray(item.submenu) ? clickPaths(item.submenu, path) : [])];
  });
}

const menuState = () => [
  itemAt([1, 2, 0]).label,
  itemAt([1, 3, 0]).label,
  itemAt([0, 7]).checked,
  itemAt([1, 7]).checked,
  itemAt([1, 8]).checked
];

function takeCreated(): any[] {
  return electronFake.takeCreated().map(({ id, bounds }) => ({ id, file: loads.get(id), bounds }));
}

function drain(): void {
  for (const list of [dialogs, popups, focusedIds, destroyedIds, movedIds, openedPaths, clipboard, rejections, electronFake.externalUrls]) {
    list.splice(0, list.length);
  }
  electronFake.takeClosed();
  electronFake.takeCreated();
  electronFake.sendLog.splice(0, electronFake.sendLog.length);
  fakeFs.takeWrites();
}

async function click(path: number[], window: Window | null, checked: boolean, script: Reply[] = []): Promise<void> {
  const item = itemAt(path);
  await observe({ op: "click", path, label: item.label, window: window?.id ?? null, checked }, window, script, () =>
    item.click({ ...item, checked }, window ?? undefined, {})
  );
}

async function observe(r: any, window: Window | null, script: Reply[], run: () => any): Promise<void> {
  labelPorts();
  drain();
  focused = window;
  window?.emit("focus");
  await flush();
  drain();
  replies.splice(0, replies.length, ...script);
  r.throw = null;
  try {
    run();
  } catch (error) {
    r.throw = (error as Error).name;
  }
  await flush();
  if (replies.length > 0) throw new Error(`${JSON.stringify(r)}: ${replies.length} scripted replies were not asked for`);
  labelPorts();
  const created = takeCreated();
  r.dialogs = [...dialogs];
  r.sent = toTree(
    electronFake.sendLog.map(({ port, message }) => {
      const to = ports.get(port);
      if (to === undefined) throw new Error(`${JSON.stringify(r)}: a message went to a port no window owns`);
      return { to, message };
    })
  );
  r.popups = toTree(popups);
  r.created = toTree(created);
  r.focused = [...focusedIds];
  r.closed = electronFake.takeClosed();
  r.destroyed = [...destroyedIds];
  r.moved = [...movedIds];
  r.external = [...electronFake.externalUrls];
  r.paths = toTree(openedPaths);
  r.clipboard = toTree(clipboard);
  const files = fakeFs.dumpFiles();
  const text = (name: string) => (files[name] === undefined ? null : Buffer.from(files[name]).toString("utf8"));
  r.files = toTree([...new Set(fakeFs.takeWrites())].map((name) => ({ name, text: text(name) })));
  r.rejections = [...rejections];
  r.menu = toTree(menuState());
  out(r);
  drain();
}

const byFile = (file: string): Window[] => electronFake.windowList().filter((w) => loads.get(w.id) === file);
const pathOf = (label: string): number[] => {
  const path = clickPaths(template()).find((p) => itemAt(p).label === label);
  if (path === undefined) throw new Error(`no ${label} item`);
  return path;
};

async function domReady(window: Window): Promise<void> {
  drain();
  window.webContents.emit("dom-ready");
  await flush();
  labelPorts();
  drain();
  out({ op: "domReady", window: window.id, menu: toTree(menuState()) });
}

async function close(window: Window): Promise<void> {
  window.close();
  await flush();
  drain();
  out({ op: "close", window: window.id });
}

function emit(window: Window, name: string, data: any): void {
  labelPorts();
  const port = windowPort(window);
  if (port === undefined) throw new Error("window has no port");
  port.emit("message", { data: { name, data } });
}

async function ensureWorld(): Promise<void> {
  while (byFile("hub.html").length < 2) {
    await click(pathOf("New Window"), byFile("hub.html")[0] ?? null, false);
    await domReady(byFile("hub.html").pop()!);
  }
  for (const extra of byFile("hub.html").slice(2)) await close(extra);
  while (byFile("satellite.html").length < 1) {
    const parent = byFile("hub.html")[0];
    drain();
    const data = { uuid: "satellite-uuid", type: 4 };
    emit(parent, "create-satellite", data);
    await flush();
    const [created] = takeCreated();
    out({ op: "satellite", parent: parent.id, uuid: data.uuid, window: toTree(created) });
    await domReady(byFile("satellite.html").pop()!);
  }
  for (const extra of byFile("satellite.html").slice(1)) await close(extra);
  for (const file of ["licenses.html", "download.html"]) for (const window of byFile(file)) await close(window);
  if (byFile("preferences.html").length === 0) await click(pathOf(platformSettings), byFile("hub.html")[0], false);
}

let platformSettings = "";

function putFile(name: string, text: string): void {
  fakeFs.putFile(name, utf8(text));
  out({ op: "putFile", name, text });
}

function removeFile(name: string): void {
  fakeFs.removeFile(name);
  out({ op: "removeFile", name });
}

function setFailure(name: string, kind: "read" | "write" | null): void {
  fakeFs.setFailure(name, kind);
  out({ op: "setFailure", name, kind });
}

const box = (value: number): Reply => ({ kind: "box", value: { response: value, checkboxChecked: false } });
const boxSync = (value: number): Reply => ({ kind: "boxSync", value });
const open = (...filePaths: string[]): Reply => ({ kind: "open", value: { canceled: filePaths.length === 0, filePaths } });
const save = (filePath: string | null): Reply => ({
  kind: "save",
  value: filePath === null ? { canceled: true, filePath: "" } : { canceled: false, filePath }
});

export async function runClicks(platform: string, packaged: boolean): Promise<void> {
  const mainBundle = await buildMainBundle(STUBS);
  const arch = platform === "darwin" ? "arm64" : "x64";
  Object.defineProperty(process, "platform", { value: platform });
  Object.defineProperty(process, "arch", { value: arch });
  platformSettings = platform === "darwin" ? "Settings..." : "Show Preferences...";
  setNow(T0);
  electronFake.setPackaged(packaged);
  (globalThis as any).__asrsMain = { electron: electronFake, fs: fakeFs };
  fakeFs.putFile(BETA_FILE, utf8('{"welcomeComplete":true,"surveyStatus":true}'));
  fakeFs.putDir("/fake/userData");
  fakeFs.putDir(CUSTOM_ASSETS);
  fakeFs.putDir(CUSTOM_ASSETS + "/Joystick_Broken");
  fakeFs.putFile(CUSTOM_ASSETS + "/Joystick_Broken/config.json", utf8("{}"));
  const bundle = await import(mainBundle);
  install(bundle.Constants);
  await bundle.load();
  electronFake.fireReady();
  await flush(60);
  const files = fakeFs.dumpFiles();
  out({
    op: "config",
    platform,
    packaged,
    arch,
    now: T0,
    assetStatus: "asset download status",
    owletStatus: "owlet download status",
    bundledAssets: bundle.Constants.BUNDLED_ASSETS,
    files: toTree(Object.keys(files).map((name) => ({ name, text: Buffer.from(files[name]).toString("utf8") }))),
    dirs: fakeFs.dumpDirs(),
    created: toTree(takeCreated()),
    menu: toTree(menuState())
  });
  await domReady(byFile("hub.html")[0]);
  await ensureWorld();

  const pick = (focus: Focus): Window | null => {
    if (focus === "none") return null;
    if (focus === "hub") return byFile("hub.html")[1];
    return byFile(focus + ".html")[0];
  };
  const run = async (label: string | number[], focus: Focus, script: Reply[] = [], checked = false) => {
    const path = typeof label === "string" ? pathOf(label) : label;
    await ensureWorld();
    await click(path, pick(focus), checked, script);
  };
  const focuses: Focus[] = ["hub", "satellite", "preferences", "none"];

  const scripts: { [label: string]: Partial<Record<Focus, Reply[][]>> } = {};
  const everywhere = (variants: Reply[][]) => Object.fromEntries(focuses.map((f, i) => [f, i === 0 ? variants : variants.slice(0, 1)]));
  scripts["About AdvantageScope"] = everywhere([[box(0)], [box(1)]]);
  scripts["Check for Updates..."] = everywhere([[box(0)]]);
  scripts["Use Custom Assets Folder"] = everywhere([[]]);
  scripts["Asset Download Status..."] = everywhere([[box(0)]]);
  scripts["Owlet Download Status..."] = everywhere([[box(0)]]);
  scripts["Open Log(s)..."] = { hub: [[open()], [open("/logs/a.wpilog")], [open("/logs/a.wpilog", "/logs/b.rlog")]] };
  scripts["Add New Log(s)..."] = scripts["Open Log(s)..."];
  scripts["Reset to DS Layout"] = { hub: [[boxSync(1)], [boxSync(0)]] };
  scripts["Export Layout..."] = everywhere([[save(null)]]);
  scripts["Import Layout..."] = everywhere([[open()]]);

  for (const path of clickPaths(template())) {
    const label = itemAt(path).label;
    for (const focus of focuses) {
      for (const script of scripts[label]?.[focus] ?? [[]]) await run(path, focus, script);
    }
    const repeated = [platformSettings, "Show Licenses...", "Download Logs..."];
    if (repeated.includes(label)) {
      await ensureWorld();
      if (label === platformSettings) await close(byFile("preferences.html")[0]);
      for (const focus of ["satellite", "hub", "none"] as Focus[]) await click(path, pick(focus), false);
      if (label === "Download Logs...") {
        await domReady(byFile("download.html")[0]);
        await click(pathOf("Use Systemcore USB Address"), pick("hub"), true);
        await click(path, pick("hub"), false);
      }
      if (platform !== "darwin") await click(pathOf("Bring All to Front"), pick("satellite"), false);
    }
  }

  const customAssets = "Use Custom Assets Folder";
  await run(customAssets, "hub", [open()], true);
  await run(customAssets, "none", [open(CUSTOM_ASSETS)], true);
  await run("Show Assets Folder", "hub");
  await run(customAssets, "hub", [open()], true);
  await run(customAssets, "satellite", [], false);
  await run("Show Assets Folder", "none");

  for (const label of ["Use Systemcore USB Address", "Use Systemcore Wi-Fi Address"]) {
    for (const checked of [true, false, true]) await run(label, "hub", [], checked);
  }
  await ensureWorld();
  for (const data of [
    { liveMode: "phoenix", systemcoreStaticAddress: "wifi" },
    { theme: "dark", userAssetsFolder: CUSTOM_ASSETS, liveMode: "bogus" },
    { hasScrolledTimeline: true },
    null
  ]) {
    const hub = byFile("hub.html")[0];
    await observe({ op: "updatePreferences", window: hub.id, data: toTree(data) }, hub, [], () => emit(hub, "update-preferences", data));
  }
  putFile(PREFS_FILE, '{"zzz":1,"theme":"dark","liveMode":"rlog","userAssetsFolder":null,"systemcoreStaticAddress":""}');
  await run("Use Systemcore USB Address", "none", [], true);
  await run([1, 3, 5], "hub");
  await run(customAssets, "hub", [open(CUSTOM_ASSETS)], true);
  await run(customAssets, "hub", [], false);

  const savedPrefs = Buffer.from(fakeFs.dumpFiles()[PREFS_FILE]).toString("utf8");
  for (const breakPrefs of [() => removeFile(PREFS_FILE), () => putFile(PREFS_FILE, "{")]) {
    await ensureWorld();
    breakPrefs();
    for (const path of [pathOf("Use Systemcore Wi-Fi Address"), [1, 2, 3], pathOf(customAssets), pathOf("Show Assets Folder")]) {
      await click(path, pick("hub"), true);
    }
    putFile(PREFS_FILE, savedPrefs);
  }
  setFailure(PREFS_FILE, "write");
  await click(pathOf("Use Systemcore Wi-Fi Address"), pick("hub"), true);
  await click(pathOf(customAssets), pick("hub"), false);
  setFailure(PREFS_FILE, null);

  await ensureWorld();
  const [hubA, hubB] = byFile("hub.html");
  const states: [Window, any][] = [
    [hubB, { sidebar: { width: 300, expanded: [] }, tabs: { selected: 0, tabs: [] } }],
    [hubA, { sidebar: { width: 12.5, expanded: ["/a"] }, tabs: { selected: 1, tabs: [1, "é🚀"] } }],
    [byFile("satellite.html")[0], { visualizer: null }]
  ];
  for (const [window, state] of states) {
    emit(window, "save-state", state);
    out({ op: "saveState", window: window.id, state: toTree(state) });
  }
  await run("Export Layout...", "hub", [save(SAVED_LAYOUT)]);
  await run("Export Layout...", "none", [save(SAVED_LAYOUT)]);
  setFailure(SAVED_LAYOUT, "write");
  await run("Export Layout...", "hub", [save(SAVED_LAYOUT)]);
  setFailure(SAVED_LAYOUT, null);

  const version = packaged ? electronFake.app.getVersion() : "dev";
  const bounds = (n: number) => ({ x: n, y: n + 1, width: 900 + n, height: 500 + n });
  const layout = (hubs: number[], satellites: number[], v: any = version) =>
    JSON.stringify({
      hubs: hubs.map((n) => ({ ...bounds(n), state: { hub: n } })),
      satellites: satellites.map((n) => ({ ...bounds(n), state: { satellite: n }, uuid: "uuid-" + n })),
      version: v
    });
  putFile("/fake/one.json", layout([10, 20], [30]));
  putFile("/fake/two.json", layout([40], [50, 60]));
  putFile("/fake/other.json", layout([70], [80], "other"));
  putFile("/fake/old.json", '{"layout":[]}');
  putFile("/fake/unknown.json", '{"layout":{},"version":1,"hubs":[],"satellites":{}}');
  putFile("/fake/array.json", "[]");
  putFile("/fake/number.json", "5");
  putFile("/fake/corrupt.json", "{");
  putFile("/fake/nullHub.json", JSON.stringify({ hubs: [null, { ...bounds(90), state: {} }], satellites: [], version }));
  putFile("/fake/nullSatellite.json", JSON.stringify({ hubs: [{ ...bounds(90), state: {} }], satellites: [null], version }));
  const imports: [string[], Reply[]][] = [
    [["/fake/old.json"], [box(0)]],
    [["/fake/unknown.json"], [box(0)]],
    [["/fake/array.json"], [box(0)]],
    [["/fake/number.json"], []],
    [["/fake/corrupt.json"], []],
    [["/fake/missing.json"], []],
    [["/fake/one.json"], []],
    [["/fake/one.json", "/fake/two.json", "/fake/other.json", "/fake/old.json"], []],
    [["/fake/one.json", "/fake/corrupt.json"], []],
    [["/fake/nullHub.json"], []],
    [["/fake/nullSatellite.json"], []],
    [["/fake/other.json"], packaged ? [boxSync(1)] : []],
    ...((packaged ? [[["/fake/other.json"], [boxSync(0)]]] : []) as [string[], Reply[]][])
  ];
  for (const [paths, script] of imports) {
    await run("Import Layout...", "hub", [open(...paths), ...script]);
    for (const window of [...byFile("hub.html"), ...byFile("satellite.html")]) {
      if (windowPort(window) === undefined || loads.get(window.id) === "hub.html") await domReady(window);
    }
  }
}
