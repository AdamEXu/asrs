import { install as installGlobals } from "./fakes/globals";
import * as fakeFs from "./fakes/mainFs";
import * as electronFake from "./fakes/mainElectron";
import { toTree } from "./cve1";

// Ground truth for asrs-services' window model: upstream main.ts, unmodified, against the fake
// `electron` of the `files` harness. Every window main.ts constructs is recorded with the
// options it passed to `new BrowserWindow` (minus `webPreferences`), reached through the real
// triggers: startup, hub port messages, popup and application menu clicks. `order` steps make
// main.ts broadcast preferences and record which windows receive them, in order, which is how
// its most-recently-focused hub list becomes observable.
//
//   genWindows <scenario> <platform>
//     fresh   no saved state: default hub, beta welcome, every secondary window, MRU moves
//     saved   startup from a saved state file with hubs and satellites

const [scenario, platform] = process.argv.slice(2);
if ((scenario !== "fresh" && scenario !== "saved") || !["darwin", "win32", "linux"].includes(platform)) {
  console.error("usage: genWindows <fresh|saved> <darwin|win32|linux>");
  process.exit(2);
}
installGlobals();
Object.defineProperty(process, "platform", { value: platform });

const Constants = await import("@upstream/main/electron/ElectronConstants");
await import("@upstream/main/electron/main");

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const flush = async (times = 8) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};

const label = (item: any): string | undefined => (item instanceof electronFake.MenuItem ? item.label : item?.label);
const submenuOf = (item: any): any[] | undefined => {
  const submenu = item instanceof electronFake.MenuItem ? item.options?.submenu : item?.submenu;
  return submenu instanceof electronFake.Menu ? submenu.items : submenu;
};
function findItem(items: any[], text: string): any {
  for (const item of items) {
    if (label(item) === text) return item;
    const submenu = submenuOf(item);
    if (Array.isArray(submenu)) {
      const found = findItem(submenu, text);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}
function click(item: any, baseWindow: any) {
  if (item === undefined) throw new Error("menu item not found");
  (item instanceof electronFake.MenuItem ? item.options?.click : item?.click)?.call(item, item, baseWindow);
}

function windowOf(id: number): any {
  const window = electronFake.FakeBrowserWindow.fromId(id);
  if (window === null) throw new Error(`window ${id} is gone`);
  return window;
}

function optionsTree(options: any) {
  const copy: any = {};
  for (const key of Object.keys(options ?? {})) {
    if (key === "webPreferences") continue;
    copy[key] = key === "parent" ? (options.parent?.id ?? null) : options[key];
  }
  return toTree(copy);
}

function portWindowIds(): Map<electronFake.FakePort, number> {
  const ids = new Map<electronFake.FakePort, number>();
  for (const window of electronFake.windowList()) {
    for (const posted of window.webContents?.posted ?? []) {
      if (posted.channel !== "port") continue;
      const channel = electronFake.channels.find((c) => c.port1 === posted.ports?.[0]);
      if (channel !== undefined) ids.set(channel.port2, window.id);
    }
  }
  return ids;
}

let step = 0;
async function record(op: string, args: unknown, run: () => unknown | Promise<unknown>) {
  const r: any = { step: step++, op };
  if (args !== undefined) r.args = toTree(args);
  r.result = toTree(await run());
  await flush();
  r.created = electronFake.takeCreated().map((c) => ({
    id: c.id,
    options: optionsTree(electronFake.FakeBrowserWindow.fromId(c.id)?.options)
  }));
  r.closed = electronFake.takeClosed();
  r.focused = electronFake.FakeBrowserWindow.getFocusedWindow()?.id ?? null;
  electronFake.takeSent();
  out(r);
}

const ready = () =>
  record("ready", undefined, async () => {
    electronFake.fireReady();
    await flush(12);
    return null;
  });
const domReady = (id: number) =>
  record("domReady", { window: id }, () => {
    windowOf(id).webContents.emit("dom-ready");
    return null;
  });
const setBounds = (id: number, bounds: electronFake.Rect) =>
  record("setBounds", { window: id, bounds }, () => {
    windowOf(id).setBounds(bounds);
    return null;
  });
const focus = (id: number) =>
  record("focus", { window: id }, () => {
    windowOf(id).focus();
    return null;
  });
const close = (id: number) =>
  record("close", { window: id }, () => {
    windowOf(id).close();
    return null;
  });
const menu = (text: string, baseWindow: number) =>
  record("menu", { label: text, window: baseWindow }, () => {
    click(findItem(electronFake.menuTemplate() ?? [], text), electronFake.FakeBrowserWindow.fromId(baseWindow) ?? undefined);
    return null;
  });

function sendHub(id: number, name: string, data: unknown) {
  const port = [...portWindowIds()].find(([, windowId]) => windowId === id)?.[0];
  if (port === undefined) throw new Error(`window ${id} has no port`);
  port.emit("message", { data: { name, data } });
}
const message = (id: number, name: string, data: unknown) =>
  record("message", { window: id, name, data }, () => {
    sendHub(id, name, data);
    return null;
  });
const popupClick = (id: number, name: string, data: unknown, text: string) =>
  record("popupClick", { window: id, name, data, label: text }, async () => {
    sendHub(id, name, data);
    await flush();
    const popped = electronFake.popped[electronFake.popped.length - 1];
    click(findItem(popped.menu.items, text), popped.window);
    return null;
  });
const order = (from: number) =>
  record("order", { window: from }, async () => {
    electronFake.takeSent();
    electronFake.sendLog.splice(0, electronFake.sendLog.length);
    sendHub(from, "update-preferences", {});
    await flush();
    const ids = portWindowIds();
    return electronFake.sendLog
      .filter((entry) => entry.message?.name === "set-preferences")
      .map((entry) => ids.get(entry.port) ?? null);
  });

const lastCreated = () => Math.max(...electronFake.windowList().map((w) => w.id));

out({ op: "config", platform, scenario, stateFile: Constants.STATE_FILENAME });

if (scenario === "fresh") {
  await ready();
  const a = lastCreated();
  await domReady(a);
  const welcome = lastCreated();
  await domReady(welcome);
  await close(welcome);
  await setBounds(a, { x: 101, y: 77, width: 1001, height: 603 });
  await order(a);
  await focus(a);
  await menu("New Window", a);
  const b = lastCreated();
  await focus(b);
  await order(a);
  await menu("New Window", b);
  const c = lastCreated();
  await order(a);
  await focus(a);
  await order(a);
  await focus(c);
  await order(a);
  await close(b);
  await order(a);

  await message(a, "create-satellite", { uuid: "sat-1", type: 1 });
  await domReady(lastCreated());
  await message(c, "create-satellite", { uuid: "sat-1", type: 1 });
  await domReady(lastCreated());
  await message(a, "create-satellite", { uuid: "sat-2", type: 2 });
  await domReady(lastCreated());
  await order(a);
  await message(a, "source-list-help", { title: "Help", items: [] });
  await popupClick(a, "ask-rename-tab", { index: 0, name: "Line Graph" }, "Rename...");
  await message(a, "prompt-export", { incompleteWarning: false, supportsAkit: true, path: null });
  const axis = {
    legend: "left",
    lockedRange: [0, 1],
    autoUnitGroup: "none",
    autoUnitSelected: null,
    autoUnitDefault: null,
    unitConversion: { autoTarget: null, preset: null },
    filter: 0,
    config: {},
    rect: { x: 0, y: 0, width: 100, height: 30 }
  };
  await popupClick(a, "ask-edit-axis", axis, "Edit Range...");
  await popupClick(a, "ask-edit-axis", axis, "Edit Conversion...");
  await popupClick(
    a,
    "ask-3d-camera",
    { position: [10, 10], options: [], selectedIndex: -1, fov: 60, isFTC: false },
    "Set FOV..."
  );
  await menu(platform === "darwin" ? "Settings..." : "Show Preferences...", a);
  const prefs = lastCreated();
  await menu(platform === "darwin" ? "Settings..." : "Show Preferences...", c);
  await menu("Show Licenses...", a);
  await menu("Show Licenses...", a);
  await menu("Download Logs...", c);
  const download = lastCreated();
  await domReady(download);
  await menu("Download Logs...", a);
  await order(a);
  await focus(prefs);
  await menu("New Window", a);
  await order(a);
  await close(download);
  await menu("Download Logs...", a);
  await domReady(lastCreated());
  await order(a);
} else {
  const state = {
    hubs: [
      { x: 40, y: 50, width: 1200, height: 700, state: { saved: 1 } },
      { x: 5000, y: 50, width: 900, height: 600, state: { saved: 2 } },
      { x: 300, y: 200, width: 1100, height: 650, state: { saved: 3 } }
    ],
    satellites: [
      { x: 10, y: 10, width: 640, height: 480, state: { tab: 1 }, uuid: "sat-a" },
      { x: -4000, y: 10, width: 640, height: 480, state: { tab: 2 }, uuid: "sat-b" }
    ]
  };
  const text = JSON.stringify(state);
  out({ op: "putState", file: Constants.STATE_FILENAME, text });
  fakeFs.putFile(Constants.STATE_FILENAME, new Uint8Array(Buffer.from(text, "utf8")));
  await ready();
  const ids = electronFake.windowList().map((w) => w.id);
  for (const id of ids) await domReady(id);
  await order(ids[0]);
  await focus(ids[2]);
  await order(ids[0]);
}
