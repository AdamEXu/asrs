import * as fakeFs from "../fakes/mainFs";
import * as electronFake from "../fakes/mainElectron";
import { toTree } from "../cve1";

// `genMenus context <light|dark>`: every context menu main.ts pops up for a hub or satellite
// message, built by main.ts's own handlers. For each case the generator sends the message on the
// window's port, records the menu main.ts handed to `popup` (its items' constructor options and
// popup options), then sends the message again before each click and records what the click did:
// messages posted to each port, clipboard writes, windows opened (with the prompt window's reply
// for windows that take one) and the recent-units file.

type Out = (value: object) => void;
type Flush = (times?: number) => Promise<void>;

interface Case {
  window: "hub" | "satellite";
  name: string;
  data: any;
  recentUnits?: any;
  clicks?: boolean;
  responses?: Record<string, any[]>;
}

const PROMPT_FILES = ["editRange.html", "unitConversion.html", "renameTab.html", "editFov.html"];

const popups: { menu: electronFake.Menu; options: any }[] = [];
const clipboard: any[] = [];
const loads = new Map<number, string>();

function install(): void {
  electronFake.Menu.prototype.popup = function (this: electronFake.Menu, options?: any) {
    popups.push({ menu: this, options });
  };
  (electronFake.nativeImage as any).createFromBitmap = (buffer: Buffer, options: any) => {
    let uniform = true;
    for (let i = 4; i < buffer.length; i++) if (buffer[i] !== buffer[i % 4]) uniform = false;
    return {
      bitmap: { width: options.width, height: options.height, length: buffer.length, uniform, bgra: [...buffer.subarray(0, 4)] }
    };
  };
  electronFake.clipboard.writeText = (text: any) => {
    clipboard.push(text);
  };
  electronFake.FakeBrowserWindow.prototype.loadFile = function (this: electronFake.FakeBrowserWindow, file: string) {
    loads.set(this.id, file.split("/").pop()!);
  };
}

function windowPort(window: electronFake.FakeBrowserWindow): electronFake.FakePort | undefined {
  for (const posted of window.webContents?.posted ?? []) {
    if (posted.channel !== "port") continue;
    const channel = electronFake.channels.find((c) => c.port1 === posted.ports?.[0]);
    if (channel !== undefined) return channel.port2;
  }
  return undefined;
}

function serialize(item: any): any {
  const options = item instanceof electronFake.MenuItem ? item.options : item;
  const out: any = {};
  for (const key of Object.keys(options)) {
    const value = options[key];
    if (key === "submenu" && Array.isArray(value)) out[key] = value.map(serialize);
    else out[key] = value;
  }
  return out;
}

function itemAt(items: any[], path: number[]): any {
  let item = items[path[0]];
  for (const index of path.slice(1)) {
    const options = item instanceof electronFake.MenuItem ? item.options : item;
    item = options.submenu[index];
  }
  return item instanceof electronFake.MenuItem ? item.options : item;
}

function clickPaths(items: any[], prefix: number[] = []): number[][] {
  const paths: number[][] = [];
  items.forEach((item, index) => {
    const options = item instanceof electronFake.MenuItem ? item.options : item;
    if (typeof options.click === "function") paths.push([...prefix, index]);
    if (Array.isArray(options.submenu)) paths.push(...clickPaths(options.submenu, [...prefix, index]));
  });
  return paths;
}

export async function runContext(theme: string, out: Out, flush: Flush, hubPort: () => electronFake.FakePort) {
  install();
  electronFake.nativeTheme.shouldUseDarkColors = theme === "dark";
  const Constants = await import("@upstream/main/electron/ElectronConstants");
  const { Units } = await import("@upstream/shared/units");
  const configs: [string, any][] = [
    ["lineGraphNumeric", (await import("@upstream/hub/controllers/LineGraphController_Config")).LineGraphController_NumericConfig],
    ["lineGraphDiscrete", (await import("@upstream/hub/controllers/LineGraphController_Config")).LineGraphController_DiscreteConfig],
    ["field2d", (await import("@upstream/hub/controllers/Field2dController_Config")).default],
    ["field3d", (await import("@upstream/hub/controllers/Field3dController_Config")).default],
    ["statistics", (await import("@upstream/hub/controllers/StatisticsController_Config")).default],
    ["swerve", (await import("@upstream/hub/controllers/SwerveController_Config")).default],
    ["mechanism", (await import("@upstream/hub/controllers/MechanismController_Config")).default],
    ["points", (await import("@upstream/hub/controllers/PointsController_Config")).default]
  ];

  const hub = hubPort();
  hub.emit("message", { data: { name: "create-satellite", data: { uuid: "satellite-uuid", type: 4 } } });
  await flush();
  const satelliteWindow = electronFake.windowList().find((w) => loads.get(w.id) === "satellite.html");
  if (satelliteWindow === undefined) throw new Error("create-satellite opened no satellite window");
  satelliteWindow.webContents.emit("dom-ready");
  await flush();
  const satellite = windowPort(satelliteWindow);
  if (satellite === undefined) throw new Error("satellite window has no port");
  electronFake.takeCreated();

  const labels = new Map<electronFake.FakePort, string>([
    [hub, "hub"],
    [satellite, "satellite"]
  ]);
  const portLabel = (port: electronFake.FakePort): string => {
    const label = labels.get(port);
    if (label === undefined) throw new Error("a message went to a port no window owns");
    return label;
  };

  const cases: Case[] = [];
  const add = (c: Case) => cases.push(c);
  const rect = (x: number, y: number, width: number, height: number) => ({ x, y, width, height });
  const full = theme === "light";

  if (full) {
    add({ window: "hub", name: "ask-open-sidebar-context-menu", data: { title: "Velocity", fullTitle: "/Drive/Velocity", position: [120.5, 44.49] } });
    add({ window: "hub", name: "ask-open-sidebar-context-menu", data: { title: 'Say "hi" é', fullTitle: "NT:/a/b c", position: [-2.5, 3.5] } });
    add({ window: "hub", name: "ask-open-tuning-context-menu", data: { isTuningMode: true, values: "kP = 1.5\nkD = 0", position: [10, 20] } });
    add({ window: "hub", name: "ask-open-tuning-context-menu", data: { isTuningMode: false, values: "", position: [0.5, 1.4999] } });
    for (const [speed, looping] of [[1, false], [0.25, true], [8, false], [3, true]] as [number, boolean][]) {
      add({ window: "hub", name: "ask-playback-options", data: { speed, looping, rect: rect(100.25, 600.5, 24, 24.75) } });
    }
    add({ window: "hub", name: "ask-new-tab", data: rect(30.4, 4, 28.2, 30) });
    add({ window: "hub", name: "ask-new-tab", data: rect(1000.5, 0.5, 0, 0) });
    add({ window: "hub", name: "source-list-clear-prompt", data: { uuid: "list-uuid-1", rect: rect(200, 300.5, 18, 18) } });
    add({ window: "hub", name: "ask-rename-tab", data: { index: 3, name: "Odometry", rect: rect(80, 0, 90, 30) }, responses: { "renameTab.html": ["Renamed \u{1F680}"] } });
    add({ window: "hub", name: "ask-rename-tab", data: { index: 1, name: "", rect: rect(0, 0, 10, 10) }, responses: { "renameTab.html": [""] } });
    for (const window of ["hub", "satellite"] as const) {
      for (const selectedIndex of [0, 1, 2, 5]) {
        add({ window, name: "ask-2d-camera", data: { position: [300.5, 200.4], selectedIndex } });
      }
    }
    const cameras: [string[], number, boolean][] = [
      [[], -1, false],
      [[], -2, true],
      [["Front Camera", "Back Camera"], -3, false],
      [["Arm"], 0, true],
      [["A", "B", "C"], 2, false],
      [[], -6, true],
      [[], -9, false]
    ];
    for (const window of ["hub", "satellite"] as const) {
      for (const [options, selectedIndex, isFTC] of cameras) {
        add({
          window,
          name: "ask-3d-camera",
          data: { position: [640.5, 360.5], options, selectedIndex, fov: 60, isFTC },
          responses: { "editFov.html": [75.5] }
        });
      }
    }

    const discrete = configs.find(([name]) => name === "lineGraphDiscrete")![1];
    const numeric = configs.find(([name]) => name === "lineGraphNumeric")![1];
    for (const showRobotMode of [true, false]) {
      add({ window: "hub", name: "ask-edit-axis", data: { rect: rect(10, 400, 20, 20), legend: "discrete", showRobotMode, config: discrete } });
    }
    const recents = [
      { type: "length", factor: 1, from: "meters", to: "feet" },
      { type: null, factor: 2.5 },
      { type: "angular velocity", factor: 0.5, from: "radians/second", to: "rotations/minute" },
      { type: "length", factor: 1e-7, from: "light years", to: "nautical miles" },
      { type: "time", factor: 1, from: "seconds", to: "milliseconds" }
    ];
    const prompts = {
      "editRange.html": [[0, 5.5]],
      "unitConversion.html": [
        null,
        { type: null, factor: 1 },
        { type: "length", factor: 1, from: "meters", to: "feet" },
        { type: "velocity", factor: 3, from: "feet/second", to: "meters/second" }
      ]
    };
    const groups = ["none", "inconsistent", ...Object.keys(Units.UNIT_GROUPS)];
    groups.forEach((group, i) => {
      const units = Object.keys((Units.UNIT_GROUPS as any)[group] ?? {});
      const legend = i % 2 === 0 ? "left" : "right";
      const preset = [null, { type: null, factor: 1 }, { type: "length", factor: 3, from: "inches", to: "meters" }][i % 3];
      const data: any = { rect: rect(5.5, 100, 20, 20.5), legend };
      data.lockedRange = i % 4 === 1 ? null : i % 4 === 3 ? [-1.5, 10] : i % 4 === 0 ? null : [0, 0];
      data.autoUnitGroup = group;
      data.autoUnitSelected = units.length > 0 && preset === null ? units[i % units.length] : null;
      data.autoUnitDefault = units.length > 1 ? units[(i + 1) % units.length] : null;
      data.unitConversion = { autoTarget: data.autoUnitSelected, preset };
      data.filter = i % 3;
      data.config = numeric;
      add({
        window: "hub",
        name: "ask-edit-axis",
        data,
        recentUnits: i % 3 === 0 ? undefined : i % 3 === 1 ? recents.slice(0, (i % 5) + 1) : [],
        responses: prompts
      });
    });
  }

  const models = [
    { key: "Model A", display: "Model A" },
    { key: "Model B", display: "Model B" }
  ];
  for (const [configName, upstreamConfig] of configs) {
    const config = structuredClone(upstreamConfig);
    for (const type of config.types) {
      for (const option of type.options) if (option.values.length === 0) option.values = models;
    }
    for (const type of config.types) {
      const logTypes: string[] = type.sourceTypes.length > 1 ? [type.sourceTypes[0], type.sourceTypes[type.sourceTypes.length - 1]] : type.sourceTypes;
      logTypes.forEach((logType, variant) => {
        const options: any = {};
        type.options.forEach((option: any, j: number) => {
          options[option.key] = variant === 0 ? option.values[0].key : j === 0 ? "bogus" : option.values[option.values.length - 1].key;
        });
        add({
          window: "hub",
          name: "source-list-type-prompt",
          data: {
            uuid: configName + "-" + type.key,
            config,
            state: { type: type.key, logKey: "/Log/" + type.key, logType, visible: variant === 0, options },
            rect: rect(150.5, 88.25, 16, 16)
          },
          clicks: full
        });
      });
    }
  }

  const hubWindow = electronFake.windowList().find((w) => windowPort(w) === hub)!;
  out({ op: "config", platform: process.platform, theme, hubBounds: toTree(hubWindow.getBounds()), satelliteBounds: toTree(satelliteWindow.getBounds()) });
  const send = (c: Case) => {
    const port = c.window === "hub" ? hub : satellite;
    port.emit("message", { data: { name: c.name, data: structuredClone(c.data) } });
  };
  const setRecents = (value: any) => {
    if (value === undefined) fakeFs.removeFile(Constants.RECENT_UNITS_FILENAME);
    else fakeFs.putFile(Constants.RECENT_UNITS_FILENAME, new Uint8Array(Buffer.from(JSON.stringify(value), "utf8")));
  };
  const askerWindow = (c: Case) => (c.window === "hub" ? hubWindow : satelliteWindow);

  const open = async (c: Case) => {
    setRecents(c.recentUnits);
    popups.splice(0, popups.length);
    const logStart = electronFake.sendLog.length;
    send(c);
    await flush();
    if (electronFake.sendLog.length !== logStart) throw new Error(`${c.name}: the ask itself sent messages`);
    if (popups.length !== 1) throw new Error(`${c.name}: ${popups.length} popups`);
    return popups[0];
  };

  const click = async (c: Case, path: number[], response: any): Promise<string | null> => {
    const { menu } = await open(c);
    const start = electronFake.sendLog.length;
    const clipStart = clipboard.length;
    electronFake.takeCreated();
    fakeFs.takeWrites();
    itemAt(menu.items, path).click();
    await flush();
    const opened: any[] = [];
    let prompted: string | null = null;
    for (const { id } of electronFake.takeCreated()) {
      const window = electronFake.BrowserWindow.fromId(id)!;
      const file = loads.get(id) ?? null;
      const record: any = { file, bounds: window.getBounds() };
      window.webContents.emit("dom-ready");
      const port = windowPort(window);
      if (port === undefined) throw new Error(`${file}: no port after dom-ready`);
      labels.set(port, "window:" + file);
      await flush();
      if (file !== null && PROMPT_FILES.includes(file)) {
        prompted = file;
        if (response !== undefined) {
          port.emit("message", { data: structuredClone(response) });
          await flush();
          record.response = response;
        }
      }
      record.destroyed = window.isDestroyed();
      opened.push(record);
      if (!window.isDestroyed()) window.destroy();
    }
    const recents = fakeFs.dumpFiles()[Constants.RECENT_UNITS_FILENAME];
    out({
      op: "click",
      path,
      sent: toTree(electronFake.sendLog.slice(start).map((e) => ({ to: portLabel(e.port), message: e.message }))),
      clipboard: toTree(clipboard.slice(clipStart)),
      windows: toTree(opened),
      writes: toTree(fakeFs.takeWrites()),
      recentUnits: recents === undefined ? null : Buffer.from(recents).toString("utf8")
    });
    return prompted;
  };

  for (const c of cases) {
    const { menu, options } = await open(c);
    const popupOptions: any = {};
    for (const key of Object.keys(options ?? {})) if (key !== "window") popupOptions[key] = options[key];
    out({ op: "case", window: c.window, name: c.name, data: toTree(c.data), recentUnits: toTree(c.recentUnits) });
    out({ op: "popup", ownWindow: options?.window === askerWindow(c), options: toTree(popupOptions), items: toTree(menu.items.map(serialize)) });
    if (c.clicks === false) continue;
    for (const path of clickPaths(menu.items)) {
      const file = await click(c, path, undefined);
      if (file === null) continue;
      for (const response of c.responses?.[file] ?? []) await click(c, path, response);
    }
  }
}
