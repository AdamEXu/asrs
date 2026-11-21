import { install as installGlobals, intervalCount, setNow, tickInterval } from "./fakes/globals";
import * as fakeFs from "./fakes/mainFs";
import * as electronFake from "./fakes/mainElectron";
import { toTree } from "./cve1";

// Ground truth for asrs-services' persistence layer: upstream's own main-process code,
// unmodified, running against a fake `fs` and `electron`. Each emitted record carries the
// operation, its in-memory result, and the fake filesystem afterwards so the port can replay
// and compare bytes.
//
//   genFiles state <count> <seed> [packaged]
//     StateTracker scenarios: state-file reads with and without reset, renderer/satellite
//     tracking, save-interval writes.
//   genFiles flows <count> <seed> [packaged]
//     main.ts's file flows reached through real code: `ready` startup, port `emit`s
//     (save-state, save-type-memory), popup menu clicks (recent units, layout
//     export/import, DS layout restore), dom-ready sends, interval ticks.
//   genFiles beta <count> <seed>
//     betaUtil's getState / welcome / expiry / survey-schedule calls over a fake clock.
//
// `main.ts` is bundled as strict ESM here but sloppy CJS in the real app, so records never
// put a primitive window element where a bounds reset would assign to it.

const [mode, countArg, seedArg, variant] = process.argv.slice(2);
installGlobals();
electronFake.setPackaged(variant === "packaged");
Object.defineProperty(process, "platform", { value: "darwin" });

await import("@upstream/main/electron/main");
const StateTracker = (await import("@upstream/main/electron/StateTracker")).default;
const betaUtil = await import("@upstream/main/electron/betaUtil");
const DEFAULT_DS_LAYOUT = (await import("@upstream/main/lite/dsLayout")).default;
const { formatDate } = await import("@upstream/shared/util");
const Constants = await import("@upstream/main/electron/ElectronConstants");

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const flush = async (times = 8) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};

// Tracker id 0 is the app's own instance from main.ts module load; `newTracker` ids start at 1.
const trackers: any[] = [null];
const trackerIntervals: number[] = [0];

let caseIndex = 0;
let stepIndex = 0;

async function record(op: string, args: unknown, run: () => unknown | Promise<unknown>) {
  const r: any = { case: caseIndex, step: stepIndex++, op };
  if (args !== undefined) r.args = toTree(args);
  try {
    r.result = toTree(await run());
  } catch (error) {
    r.throw = (error as Error).name;
    if (r.throw === "TypeError") process.stderr.write(`${op} case ${caseIndex}: ${(error as Error).stack}\n`);
  }
  await flush();
  r.created = electronFake.takeCreated().map((c) => c.id);
  r.closed = electronFake.takeClosed();
  const sentAll = electronFake.takeSent();
  r.sent = sentAll
    .filter((m) => m?.name === "restore-type-memory" || m?.name === "restore-state")
    .map((m) => ({ name: m.name, data: toTree(m.data) }));
  r.writes = fakeFs.takeWrites();
  r.files = toTree(fakeFs.dumpFiles());
  out(r);
  return r;
}

const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));
const bytesOf = (value: Uint8Array) => value;

async function opPutFile(args: any) {
  await record("putFile", args, () => {
    fakeFs.putFile(args.name, args.data);
    return null;
  });
}
async function opRemoveFile(args: any) {
  await record("removeFile", args, () => {
    fakeFs.removeFile(args.name);
    return null;
  });
}
async function opSetDisplays(displays: any[]) {
  await record("setDisplays", { displays }, () => {
    electronFake.setDisplays(displays);
    return null;
  });
}
async function opSetWindow(id: number, bounds: any) {
  await record("setWindow", { id, bounds }, () => {
    electronFake.setWindowBounds(id, bounds);
    return null;
  });
}
async function opSetNow(ms: number) {
  await record("setNow", { ms }, () => {
    setNow(ms);
    return null;
  });
}
async function opNewTracker() {
  await record("newTracker", undefined, () => {
    const tracker = new StateTracker();
    trackers.push(tracker);
    trackerIntervals.push(intervalCount() - 1);
    return trackers.length - 1;
  });
}
async function opGetSaved(index: number, allowReset: boolean) {
  await record("getSaved", { tracker: index, allowReset }, () =>
    trackers[index].getSavedApplicationState(allowReset)
  );
}
async function opGetCurrent(index: number) {
  await record("getCurrent", { tracker: index }, () => trackers[index].getCurrentApplicationState());
}
async function opSaveRendererState(index: number, id: number, state: unknown) {
  await record("saveRendererState", { tracker: index, id, state }, () => {
    trackers[index].saveRendererState({ id }, state);
    return null;
  });
}
async function opGetRendererState(index: number, id: number) {
  await record("getRendererState", { tracker: index, id }, () =>
    trackers[index].getRendererState({ id })
  );
}
async function opSaveSatelliteIds(index: number, windows: unknown) {
  await record("saveSatelliteIds", { tracker: index, windows }, () => {
    trackers[index].saveSatelliteIds(windows);
    return null;
  });
}
async function opTick(tracker: number) {
  await record("tickTracker", { tracker }, () => {
    if (trackerIntervals[tracker] !== undefined) tickInterval(trackerIntervals[tracker]);
    return null;
  });
}
async function opReady() {
  await record("ready", undefined, async () => {
    electronFake.fireReady();
    await flush(12);
    return null;
  });
}
async function opDomReady(windowId: number) {
  await record("domReady", { window: windowId }, () => {
    electronFake.FakeBrowserWindow.fromId(windowId)?.webContents.emit("dom-ready");
    return null;
  });
}
function channelForWindow(windowId: number): number | null {
  const window = electronFake.FakeBrowserWindow.fromId(windowId);
  const posted = window?.webContents.posted.filter((p) => p.channel === "port") ?? [];
  const port1 = posted[posted.length - 1]?.ports?.[0];
  const index = electronFake.channels.findIndex((c) => c.port1 === port1);
  return index < 0 ? null : index;
}
async function opEmit(windowId: number, name: string, data: unknown) {
  await record("emit", { window: windowId, name, data }, () => {
    const channel = channelForWindow(windowId);
    if (channel === null) return null;
    electronFake.channels[channel].port2.emit("message", { data: { name, data } });
    return null;
  });
}
async function opMenuClick(path: number[], semantic: any) {
  return await record(semantic.op, semantic.args, () => {
    const template = electronFake.menuTemplate();
    if (template === null) return "no-menu";
    const item = electronFake.menuItemAt(template, path);
    const baseWindow = electronFake.FakeBrowserWindow.fromId(currentWindowId) ?? undefined;
    (item instanceof electronFake.MenuItem ? item.options?.click : item?.click)?.call(item, item, baseWindow);
    return null;
  });
}
async function opDialog(kind: string, value: unknown) {
  electronFake.queueDialog(kind as any, value);
}

function findMenuIndex(items: any[], label: string): number {
  return items.findIndex((item) => (item instanceof electronFake.MenuItem ? item.label : item?.label) === label);
}

const fileMenuPath = (label: string): number[] | null => {
  const template = electronFake.menuTemplate();
  if (template === null) return null;
  for (let i = 0; i < template.length; i++) {
    const submenu = (template[i] as any)?.submenu;
    if (!Array.isArray(submenu)) continue;
    const j = findMenuIndex(submenu, label);
    if (j >= 0) return [i, j];
  }
  return null;
};

const STATE_NAME = Constants.STATE_FILENAME;
const TYPE_MEMORY_NAME = Constants.TYPE_MEMORY_FILENAME;
const RECENT_UNITS_NAME = Constants.RECENT_UNITS_FILENAME;
const PREFS_NAME = Constants.PREFS_FILENAME;
const BETA_NAME = "/fake/userData/beta-2027.json";

out({
  op: "config",
  result: toTree({
    userData: "/fake/userData",
    temp: "/fake/temp",
    packaged: variant === "packaged",
    appVersion: Constants.APP_VERSION,
    prefsFile: PREFS_NAME,
    stateFile: STATE_NAME,
    typeMemoryFile: TYPE_MEMORY_NAME,
    recentUnitsFile: RECENT_UNITS_NAME,
    betaFile: BETA_NAME
  })
});

let seed = Number(seedArg) >>> 0 || 0x2545f491;
const next = () => {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return seed >>> 0;
};
const below = (n: number) => next() % n;
const pick = <T>(items: readonly T[]): T => items[below(items.length)];

const j = (text: string) => utf8(text);

// ---------- state mode ----------

const STATE_FILES: (Uint8Array | null)[] = [
  null,
  j('{"hubs":[],"satellites":[]}'),
  j('{"hubs":[{"x":0,"y":0,"width":1100,"height":650,"state":null}],"satellites":[]}'),
  j('{"hubs":[{"x":0,"y":0,"width":1100,"height":650,"state":{"sidebar":{"width":250,"expanded":[]},"tabs":{"selected":0,"tabs":[]}}}],"satellites":[]}'),
  j('{"hubs":[{"x":100,"y":100,"width":800,"height":500,"state":1}],"satellites":[{"x":50,"y":50,"width":900,"height":500,"state":2,"uuid":"sat-1"}]}'),
  j('{"hubs":[{"x":5000,"y":0,"width":1100,"height":650,"state":null}],"satellites":[]}'),
  j('{"hubs":[{"x":1800,"y":0,"width":400,"height":300,"state":null}],"satellites":[{"x":-500,"y":0,"width":900,"height":500,"state":null,"uuid":"u"}]}'),
  j('{"hubs":[{"x":0,"y":0,"width":1100,"height":650,"state":null}],"satellites":[{"x":0,"y":0,"width":900,"height":500,"state":null,"uuid":"s"}]}'),
  j('{"satellites":[],"hubs":[{"x":0,"y":0,"width":1100,"height":650,"state":null}]}'),
  j('{"hubs":[{"x":"5","y":0,"width":100,"height":100,"state":null}],"satellites":[]}'),
  j('{"hubs":[{"x":null,"y":0,"width":100,"height":100,"state":null}],"satellites":[]}'),
  j('{"hubs":[{"y":0,"width":100,"height":100,"state":null}],"satellites":[]}'),
  j('{"hubs":[{}],"satellites":[]}'),
  j('{"hubs":[[]],"satellites":[]}'),
  j('{"hubs":[null],"satellites":[]}'),
  j('{"hubs":[{"x":0,"y":0,"width":1e400,"height":100,"state":null}],"satellites":[]}'),
  j('{"hubs":{},"satellites":{}}'),
  j('{"hubs":"ab","satellites":[]}'),
  j('{"hubs":5,"satellites":[]}'),
  j('{"hubs":null,"satellites":[]}'),
  j("{}"),
  j("[]"),
  j("[1,2]"),
  j("null"),
  j("5"),
  j('"x"'),
  j("true"),
  j('{"hubs":'),
  j(""),
  j('{"hubs":[],"satellites":[],}'),
  utf8("﻿" + '{"hubs":[{"x":0,"y":0,"width":1100,"height":650,"state":null}],"satellites":[]}'),
  utf8("﻿﻿" + '{"hubs":[],"satellites":[]}'),
  new Uint8Array([0x7b, 0x22, 0x68, 0x75, 0x62, 0x73, 0x22, 0x3a, 0x5b, 0x7b, 0x22, 0x78, 0x22, 0x3a, 0x30, 0xff, 0x7d, 0x5d, 0x7d]),
  j('{"hubs":[{"x":0,"y":0,"width":1100,"height":650,"state":"héllo 😀"}],"satellites":[]}'),
  j('{"hubs":[{"x":-0,"y":0,"width":1100,"height":650,"state":null}],"satellites":[]}'),
  j('{"hubs":[{"x":0,"y":0,"width":1100,"height":650,"state":{"nested":[1,{"a":[2]}]}}],"satellites":[]}')
];

const DISPLAYS_SETS: any[][] = [
  [{ x: 0, y: 0, width: 1920, height: 1080 }],
  [
    { x: 0, y: 0, width: 1920, height: 1080 },
    { x: 1920, y: 0, width: 1920, height: 1080 }
  ],
  [
    { x: -2560, y: 0, width: 2560, height: 1440 },
    { x: 0, y: 0, width: 1920, height: 1080 }
  ],
  [{ x: 100, y: 50, width: 1921, height: 1081 }],
  []
];

const WINDOW_IDS = [1, 2, 3, 5, 10, 42, 4294967294, 4294967295, 0, -1, 1e21, NaN];
const BOUNDS_POOL = [
  { x: 0, y: 0, width: 1100, height: 650 },
  { x: -100, y: -50, width: 900, height: 500 },
  { x: 3000, y: 2000, width: 100, height: 100 },
  { x: 1919, y: 1079, width: 1, height: 1 },
  { x: 0.5, y: 0.25, width: 1100, height: 650 }
];
const STATE_VALUES: unknown[] = [
  null,
  { sidebar: { width: 300, expanded: ["/a"] }, tabs: { selected: 0, tabs: [] } },
  { a: 1, b: [1, 2, { c: "x" }] },
  "plain string",
  42,
  [1, 2, 3],
  { nested: { deep: { deeper: true } } }
];

async function stateCase(file: Uint8Array | null, displays: any[], scripted: boolean) {
  caseIndex++;
  stepIndex = 0;
  await opSetDisplays(displays);
  if (file !== null) await opPutFile({ name: STATE_NAME, data: file });
  const trackerId = trackers.length;
  await opNewTracker();
  if (scripted) {
    for (let i = 0; i < 3; i++) await opGetSaved(trackerId, true);
    await opGetSaved(trackerId, false);
  }
}

async function stateMode(count: number) {
  // File read cases: each file through fresh trackers, reset and non-reset reads.
  for (const file of STATE_FILES) {
    for (const displays of DISPLAYS_SETS) {
      await stateCase(file, displays, true);
    }
  }

  // Corrupted backup under a non-.json ending would slice differently; keep the real name.
  caseIndex++;
  stepIndex = 0;
  await opPutFile({ name: STATE_NAME, data: j('{"hubs":') });
  await opNewTracker();
  await opNewTracker();
  await opGetSaved(1, false);
  await opGetSaved(1, true);

  // Window id ordering and classification.
  caseIndex++;
  stepIndex = 0;
  await opSetDisplays([{ x: 0, y: 0, width: 1920, height: 1080 }]);
  const t = trackers.length;
  await opNewTracker();
  for (const id of WINDOW_IDS) {
    await opSetWindow(id, pick(BOUNDS_POOL));
    await opSaveRendererState(t, id, pick(STATE_VALUES));
  }
  await opGetCurrent(t);
  await opSetWindow(10, null);
  await opSetWindow(1e21, null);
  await opGetCurrent(t);
  await opSaveSatelliteIds(t, {
    "uuid-a": [{ id: 2 }, { id: 4294967294 }],
    "uuid-b": [{ id: 4294967295 }]
  });
  await opGetCurrent(t);
  await opSaveSatelliteIds(t, { u: [{ id: 5 }] });
  await opGetCurrent(t);
  await opSaveSatelliteIds(t, {});
  await opGetCurrent(t);
  for (const bad of [null, 5, "x", { u: 5 }, { u: "x" }, { u: [{ id: "s" }] }, { u: [null] }]) {
    await opSaveSatelliteIds(t, bad);
  }
  await opGetCurrent(t);
  for (const id of WINDOW_IDS) await opGetRendererState(t, id);

  // Tick/write behaviour: no hubs, writes, dedupe, ordering.
  caseIndex++;
  stepIndex = 0;
  await opSetDisplays([{ x: 0, y: 0, width: 1920, height: 1080 }]);
  const t2 = trackers.length;
  await opNewTracker();
  await opSetWindow(1, { x: 10, y: 10, width: 100, height: 100 });
  await opSaveRendererState(t2, 1, { v: 1 });
  await opTick(0); // the app's own tracker also ticks: it has no hub state of its own
  await opTick(t2);
  await opTick(t2);
  await opSaveRendererState(t2, 1, { v: 2 });
  await opTick(t2);
  await opSaveSatelliteIds(t2, { sat: [{ id: 1 }] });
  await opTick(t2); // 0 hubs: nothing written even though the state changed
  await opSetWindow(1, { x: 20, y: 20, width: 100, height: 100 });
  await opTick(t2);
  await opSaveRendererState(t2, 7, "seven");
  await opSetWindow(7, { x: 5, y: 5, width: 50, height: 50 });
  await opTick(t2);

  // Randomized churn across trackers sharing the file.
  for (let i = 0; i < count; i++) {
    caseIndex++;
    stepIndex = 0;
    await opSetDisplays(pick(DISPLAYS_SETS));
    const file = pick(STATE_FILES);
    if (file !== null) await opPutFile({ name: STATE_NAME, data: file });
    const id = trackers.length;
    await opNewTracker();
    const ops = 1 + below(8);
    for (let s = 0; s < ops; s++) {
      switch (below(9)) {
        case 0:
          await opGetSaved(id, below(2) === 0);
          break;
        case 1:
          await opSetWindow(pick(WINDOW_IDS), pick(BOUNDS_POOL));
          break;
        case 2:
          await opSetWindow(pick(WINDOW_IDS), null);
          break;
        case 3:
          await opSaveRendererState(id, pick(WINDOW_IDS), pick(STATE_VALUES));
          break;
        case 4:
          await opSaveSatelliteIds(id, {
            [pick(["u1", "u2", "satellite-x"])]: [{ id: pick(WINDOW_IDS) }]
          });
          break;
        case 5:
          await opTick(id);
          break;
        case 6:
          await opGetCurrent(id);
          break;
        case 7:
          await opGetRendererState(id, pick(WINDOW_IDS));
          break;
        default:
          await opPutFile({ name: STATE_NAME, data: pick(STATE_FILES) ?? j("{}") });
          break;
      }
    }
  }
}

// ---------- flows mode ----------

const TYPE_MEMORY_DATA: unknown[] = [
  { m1: { "/a": { type: "number" } } },
  { m1: { "/b": { type: "string" }, "/a": { type: "number" } } },
  { m1: { "/a": { changed: true } }, m2: { "/x": 1 } },
  { hasOwnProperty: { "/a": 1 }, toString: { "/b": 2 }, constructor: { "/c": 3 }, valueOf: { "/d": 4 } },
  { __proto__: { custom: { t: 1 } } },
  { custom: { x: 2 } },
  { m1: 5 },
  { m1: null },
  { m1: "str" },
  { m1: [1, 2] },
  { m1: { deep: { nested: { value: [1, 2] } } } },
  {},
  null,
  5,
  "ab",
  ["/a", "/b"],
  { "0": { z: 1 }, "1": { z: 2 }, "10": { z: 3 }, "2": { z: 4 } }
];

const TYPE_MEMORY_FILES: (Uint8Array | null)[] = [
  null,
  j("{}"),
  j('{"m1":{"/a":{"type":"number"}}}'),
  j('{"m1":{"/a":{"type":"number"}},"unknown":{"keep":1}}'),
  j('{"m1":5,"m2":{"a":1}}'),
  j("[]"),
  j("[1,2]"),
  j("null"),
  j("5"),
  j('"s"'),
  j("{"),
  j('{"__proto__":{"p":1},"m1":{"/a":2}}'),
  j('{"a":1,"m1":{"/a":9},"z":2}')
];

const RECENTS_FILES: (Uint8Array | null)[] = [
  null,
  j("[]"),
  j('[{"type":"length","from":"in","to":"ft","factor":0.0833}]'),
  j('[{"type":"a","factor":2},{"type":"b","factor":3},{"type":"c","factor":4},{"type":"d","factor":5},{"type":"e","factor":6}]'),
  j("{}"),
  j("5"),
  j("null"),
  j('"x"'),
  j('[1,"x",{},null]'),
  j("{")
];

const UNIT_PRESETS: unknown[] = [
  { type: "length", from: "inches", to: "feet", factor: 0.08333333333333333 },
  { type: "length", from: "in", to: "ft", factor: 0.0833 },
  { type: null, factor: 1 },
  { factor: 1, type: null },
  { type: "angle", factor: 1e-7 },
  { type: "a", factor: 2 },
  { type: "b", factor: 3 },
  { type: "c", factor: 4 },
  { type: "d", factor: 5 },
  { type: "e", factor: 6 },
  { type: "f", factor: 7 },
  { type: "x", factor: -0 },
  { type: null, from: undefined, to: undefined, factor: 1 },
  5,
  "x",
  null,
  { type: "nested", factor: 1.5, extra: { deep: [1] } }
];

async function askEditAxis(windowId: number) {
  await opEmit(windowId, "ask-edit-axis", {
    legend: "left",
    lockedRange: null,
    autoUnitGroup: "none",
    autoUnitSelected: null,
    autoUnitDefault: null,
    unitConversion: { autoTarget: null, preset: null },
    filter: 0,
    showRobotMode: false,
    config: {},
    rect: { x: 0, y: 0, width: 100, height: 30 }
  });
}

let currentWindowId = 0;

function flowsCase() {
  caseIndex++;
  stepIndex = 0;
  electronFake.popped.splice(0, electronFake.popped.length);
}

// The first entry in the window map is the oldest live window, which is always a hub:
// hubs are created before satellites at startup and during layout import.
function firstWindowId(): number {
  return electronFake.windowList()[0]?.id ?? 0;
}

// Clicks item `index` of the "Recent Presets" submenu inside the last popped axis menu.
// Emits a record only when the item exists upstream.
async function clickRecentPreset(index: number) {
  const popped = electronFake.popped[electronFake.popped.length - 1];
  if (popped === undefined) return;
  const manual = popped.menu.items.findIndex(
    (item: any) => (item instanceof electronFake.MenuItem ? item.label : item?.label) === "Manual Units"
  );
  if (manual < 0) return;
  const manualItem = popped.menu.items[manual];
  const manualSub = manualItem instanceof electronFake.MenuItem ? manualItem.submenu : manualItem?.submenu;
  const recentsItem = (manualSub as any[]).find(
    (item: any) => (item instanceof electronFake.MenuItem ? item.label : item?.label) === "Recent Presets"
  );
  const recentsSub = recentsItem instanceof electronFake.MenuItem ? recentsItem.submenu : recentsItem?.submenu;
  const item = (recentsSub as any[])?.[index];
  if (item === undefined) return;
  await record("clickRecent", { index }, async () => {
    (item instanceof electronFake.MenuItem ? item.options?.click : item?.click)?.call(item, item, undefined);
    return null;
  });
}

// Clicks "Edit Conversion..." in the last popped axis menu, dom-readies the new conversion
// window and answers it with `preset` on its port.
async function clickEditConversion(preset: unknown) {
  const popped = electronFake.popped[electronFake.popped.length - 1];
  if (popped === undefined) return;
  const manualIdx = popped.menu.items.findIndex(
    (item: any) => (item instanceof electronFake.MenuItem ? item.label : item?.label) === "Manual Units"
  );
  if (manualIdx < 0) return;
  const manualItem = popped.menu.items[manualIdx];
  const manualSub = (
    manualItem instanceof electronFake.MenuItem ? manualItem.submenu : manualItem?.submenu
  ) as any[];
  const editItem = manualSub.find(
    (item: any) => (item instanceof electronFake.MenuItem ? item.label : item?.label) === "Edit Conversion..."
  );
  if (editItem === undefined) return;
  await record("clickRecent", { preset }, async () => {
    (editItem instanceof electronFake.MenuItem ? editItem.options?.click : editItem?.click)?.call(
      editItem,
      editItem,
      popped.window
    );
    const conversionId = electronFake.created[electronFake.created.length - 1]?.id;
    if (conversionId === undefined) return "no-window";
    electronFake.FakeBrowserWindow.fromId(conversionId)?.webContents.emit("dom-ready");
    const channel = electronFake.channels[electronFake.channels.length - 1];
    channel?.port2.emit("message", { data: preset });
    return null;
  });
}

async function flowsMode(count: number, startup: Uint8Array | null) {
  // One app lifetime: upstream's `whenReady` handler runs once, so the session starts
  // here and every case below replays against the same live window and tracker.
  flowsCase();
  await opSetDisplays([
    { x: 0, y: 0, width: 1920, height: 1080 },
    { x: 1920, y: 0, width: 1280, height: 720 }
  ]);
  if (startup !== null) await opPutFile({ name: STATE_NAME, data: startup });
  await opReady();
  currentWindowId = firstWindowId();
  await opDomReady(currentWindowId);
  await opTick(0);

  const setFile = async (name: string, file: Uint8Array | null) => {
    if (file === null) await opRemoveFile({ name });
    else await opPutFile({ name, data: file });
  };

  // Type memory: save through the real port plumbing, then read on dom-ready.
  for (const file of TYPE_MEMORY_FILES) {
    for (const data of TYPE_MEMORY_DATA) {
      flowsCase();
      await setFile(TYPE_MEMORY_NAME, file);
      await opEmit(currentWindowId, "save-state", { tabs: { selected: 0 } });
      await opEmit(currentWindowId, "save-type-memory", data);
      await opTick(0);
    }
  }
  for (const file of TYPE_MEMORY_FILES) {
    flowsCase();
    await setFile(TYPE_MEMORY_NAME, file);
    await opDomReady(currentWindowId);
  }

  // Recent units through the real ask-edit-axis menu, both entry paths.
  for (const file of RECENTS_FILES) {
    for (const preset of UNIT_PRESETS) {
      flowsCase();
      await setFile(RECENT_UNITS_NAME, file);
      await askEditAxis(currentWindowId);
      await clickEditConversion(preset);
    }
    for (const index of [0, 1, 2]) {
      flowsCase();
      await setFile(RECENT_UNITS_NAME, file);
      await askEditAxis(currentWindowId);
      await clickRecentPreset(index);
    }
  }

  // Layout export and import through the real menu handlers.
  const layoutFiles: (Uint8Array | null)[] = [
    null,
    j('{"version":"dev","hubs":[],"satellites":[]}'),
    j('{"hubs":[],"satellites":[],"version":"dev"}'),
    j('{"version":"dev","hubs":[{"x":1,"y":2,"width":3,"height":4,"state":{"k":1}}],"satellites":[{"x":5,"y":6,"width":7,"height":8,"state":null,"uuid":"q"}]}'),
    j('{"version":"other","hubs":[],"satellites":[]}'),
    j('{"version":5,"hubs":[],"satellites":[]}'),
    j('{"hubs":[],"satellites":[]}'),
    j('{"version":"dev","hubs":{},"satellites":[]}'),
    j('{"version":"dev","hubs":"ab","satellites":[]}'),
    j('{"version":"dev"}'),
    j('{"layout":[1,2,3]}'),
    j('{"layout":5}'),
    j("[]"),
    j("null"),
    j("5"),
    j("{")
  ];
  const exportPath = "/fake/temp/AdvantageScope layout.json";
  for (const file of layoutFiles) {
    flowsCase();
    await setFile("/fake/temp/a.json", file);
    await opPutFile({
      name: "/fake/temp/b.json",
      data: j('{"version":"dev","hubs":[{"x":9,"y":9,"width":9,"height":9,"state":2}],"satellites":[]}')
    });
    await opPutFile({
      name: "/fake/temp/c.json",
      data: j('{"version":"other","hubs":[{"x":8,"y":8,"width":8,"height":8,"state":3}],"satellites":[]}')
    });
    await opEmit(currentWindowId, "save-state", { origin: "window" });
    const exportIdx = fileMenuPath("Export Layout...");
    if (exportIdx !== null) {
      await opDialog("save", { canceled: false, filePath: exportPath });
      await opMenuClick(exportIdx, { op: "exportLayout", args: { path: exportPath } });
    }
    const importIdx = fileMenuPath("Import Layout...");
    for (const paths of [
      ["/fake/temp/a.json"],
      ["/fake/temp/a.json", "/fake/temp/b.json", "/fake/temp/c.json"],
      ["/fake/temp/b.json", "/fake/temp/c.json"],
      []
    ]) {
      if (importIdx === null) break;
      await opDialog("open", { canceled: false, filePaths: paths });
      await opDialog("boxSync", 0);
      const r = await opMenuClick(importIdx, { op: "importLayout", args: { paths, answer: 0 } });
      if (r.created.length > 0) currentWindowId = r.created[0];
      else if (electronFake.FakeBrowserWindow.fromId(currentWindowId) === null) currentWindowId = firstWindowId();
    }
    const dsIdx = fileMenuPath("Reset to DS Layout");
    if (dsIdx !== null) {
      await opDialog("boxSync", 0);
      await opMenuClick(dsIdx, { op: "dsLayout", args: { window: currentWindowId } });
    }
  }

  // Randomized churn across the same session.
  for (let i = 0; i < count; i++) {
    flowsCase();
    const ops = 1 + below(6);
    for (let s = 0; s < ops; s++) {
      switch (below(7)) {
        case 0:
          await opEmit(currentWindowId, "save-state", pick(STATE_VALUES));
          break;
        case 1:
          await opEmit(currentWindowId, "save-type-memory", pick(TYPE_MEMORY_DATA));
          break;
        case 2:
          await opDomReady(currentWindowId);
          break;
        case 3:
          await opTick(0);
          break;
        case 4:
          await setFile(RECENT_UNITS_NAME, pick(RECENTS_FILES));
          await askEditAxis(currentWindowId);
          await clickRecentPreset(below(3));
          break;
        case 5:
          await setFile(TYPE_MEMORY_NAME, pick(TYPE_MEMORY_FILES));
          break;
        default:
          await opEmit(currentWindowId, "save-state", null);
          await opTick(0);
          break;
      }
    }
    currentWindowId = firstWindowId();
  }
}

// ---------- beta mode ----------

const BETA_FILES: (Uint8Array | null)[] = [
  null,
  j("{}"),
  j('{"welcomeComplete":true,"surveyStatus":false}'),
  j('{"welcomeComplete":false,"surveyStatus":true}'),
  j('{"welcomeComplete":false,"surveyStatus":1700000000000}'),
  j('{"welcomeComplete":false,"surveyStatus":1799999999999}'),
  j('{"welcomeComplete":false,"surveyStatus":"later"}'),
  j('{"welcomeComplete":false,"surveyStatus":{}}'),
  j('{"welcomeComplete":false,"surveyStatus":[]}'),
  j('{"surveyStatus":1700000000000,"extra":7}'),
  j('{"welcomeComplete":"yes","surveyStatus":1}'),
  j("null"),
  j("5"),
  j('"ab"'),
  j("[1,2]"),
  j("{")
];

async function betaOp(name: string, args: unknown, run: () => unknown) {
  await record(name, args, run);
}

async function betaMode(count: number) {
  const epoch0 = Date.UTC(2026, 10, 21);
  await opSetNow(epoch0);
  for (const file of BETA_FILES) {
    caseIndex++;
    stepIndex = 0;
    if (file !== null) await opPutFile({ name: BETA_NAME, data: file });
    for (const offset of [0, 1000, 30 * 24 * 3600 * 1000, 45 * 24 * 3600 * 1000, 400 * 24 * 3600 * 1000]) {
      await opSetNow(epoch0 + offset);
      await betaOp("isBetaWelcomeComplete", undefined, () => betaUtil.isBetaWelcomeComplete());
      await betaOp("isBetaExpired", undefined, () => betaUtil.isBetaExpired());
      await betaOp("shouldPromptBetaSurvey", undefined, () => betaUtil.shouldPromptBetaSurvey());
      await betaOp("delayBetaSurvey", undefined, () => {
        betaUtil.delayBetaSurvey();
        return null;
      });
      await betaOp("openBetaSurvey", undefined, () => {
        betaUtil.openBetaSurvey();
        return null;
      });
      await betaOp("isBeta", undefined, () => betaUtil.isBeta());
      await betaOp("isAlpha", undefined, () => betaUtil.isAlpha());
      await betaOp("saveBetaWelcomeComplete", undefined, () => {
        betaUtil.saveBetaWelcomeComplete();
        return null;
      });
    }
  }
  await betaOp("exportName", { ms: epoch0 }, () => "AdvantageScope " + formatDate() + ".json");
  await betaOp("defaultDsLayout", undefined, () => DEFAULT_DS_LAYOUT);
}

const count = Number(countArg) || 0;
if (mode === "state") await stateMode(count);
else if (mode === "flows") {
  const startupIndex = Number(process.argv[6]);
  const startup = Number.isNaN(startupIndex) ? null : STATE_FILES[startupIndex] ?? null;
  await flowsMode(count, startup);
} else if (mode === "beta") await betaMode(count);
else throw new Error(`genFiles: unknown mode ${mode}`);
