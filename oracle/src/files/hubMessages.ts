import * as fakeFs from "../fakes/mainFs";
import * as electronFake from "../fakes/mainElectron";
import { setNow } from "../fakes/globals";
import { toTree } from "../cve1";

// `genFiles hub-messages`: main.ts's `alert`, `error`, `frc-coordinate-system-warning`,
// `ftc-coordinate-system-warning` and `open-link`, and its `set-focused` and `set-battery` sends
// to hub windows and satellites.
//
// Every record has `op`, its arguments, and what main did:
//   dialogs    message boxes: parent window id, options (`icon` as "WINDOW_ICON") and reply;
//              `during` is a message main got while the box was open
//   sent       set-focused, set-battery and set-preferences messages: {to, message}
//   external   shell.openExternal arguments
//   prefs      the bytes of prefs.json if it was written, else null
//   throw      the name of an error the handler threw; rejections, those of promises
// A window opens with "open" ({kind, window}), whose dom-ready sends are observed.

const T0 = Date.UTC(2026, 10, 21, 12, 34, 56);
const PREFS = "/fake/userData/prefs.json";
const BETA_FILE = "/fake/userData/beta-2027.json";
const SENT = ["set-focused", "set-battery", "set-preferences"];

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const flush = async (times = 20) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};

type Window = electronFake.FakeBrowserWindow;
type Reply = { checkboxChecked: boolean; before?: { window: number; name: string; data: unknown } };

let windowIcon: unknown;
let onBattery = false;
const powerListeners = new Map<string, (() => void)[]>();
const replies: Reply[] = [];
const dialogs: any[] = [];
const external: any[] = [];
const rejections: string[] = [];
const ports = new Map<electronFake.FakePort, number>();

function install(): void {
  const proto = electronFake.FakeBrowserWindow.prototype as any;
  const destroy = proto.destroy;
  proto.destroy = function (this: Window) {
    destroy.call(this);
    this.emit("closed");
  };
  (electronFake.powerMonitor as any).isOnBatteryPower = () => onBattery;
  (electronFake.powerMonitor as any).on = (name: string, listener: () => void) => {
    powerListeners.set(name, [...(powerListeners.get(name) ?? []), listener]);
  };
  (electronFake.shell as any).openExternal = async (url: unknown) => {
    external.push(toTree(url));
  };
  electronFake.dialog.showMessageBox = async (...args: any[]) => {
    const [parent, options] = args.length >= 2 ? [args[0], args[1]] : [null, args[0]];
    const reply = replies.shift();
    if (reply === undefined) throw new Error(`unscripted dialog: ${JSON.stringify(options)}`);
    const described: any = {};
    for (const key of Object.keys(options)) {
      described[key] = key === "icon" && options.icon === windowIcon ? "WINDOW_ICON" : options[key];
    }
    const { checkboxChecked } = reply;
    dialogs.push(toTree({ window: parent === null ? null : parent.id, options: described, checkboxChecked }));
    await flush(2);
    if (reply.before !== undefined) emit(reply.before.window, reply.before.name, reply.before.data);
    return { response: 0, checkboxChecked };
  };
  process.on("unhandledRejection", (error: any) => {
    rejections.push(error?.name ?? String(error));
  });
}

function windowPort(window: Window): electronFake.FakePort | undefined {
  const posted = window.webContents.posted.filter((p) => p.channel === "port");
  const port1 = posted[posted.length - 1]?.ports?.[0];
  return electronFake.channels.find((c) => c.port1 === port1)?.port2;
}

function labelPorts(): void {
  for (const window of electronFake.windowList()) {
    const port = windowPort(window);
    if (port !== undefined) ports.set(port, window.id);
  }
}

const windowOf = (id: number): Window => {
  const window = electronFake.FakeBrowserWindow.fromId(id);
  if (window === null) throw new Error(`window ${id} is gone`);
  return window;
};

function drain(): void {
  for (const list of [dialogs, external, rejections]) list.splice(0, list.length);
  electronFake.sendLog.splice(0, electronFake.sendLog.length);
  fakeFs.takeWrites();
}

async function observe(record: any, script: Reply[], run: () => unknown): Promise<void> {
  labelPorts();
  drain();
  replies.splice(0, replies.length, ...script);
  const r: any = { ...record, throw: null };
  try {
    run();
  } catch (error) {
    r.throw = (error as Error).name;
  }
  await flush();
  if (replies.length > 0) throw new Error(`${JSON.stringify(r)}: ${replies.length} scripted replies were not asked for`);
  labelPorts();
  r.dialogs = [...dialogs];
  r.sent = electronFake.sendLog
    .filter(({ message }) => SENT.includes(message?.name))
    .map(({ port, message }) => {
      const to = ports.get(port);
      if (to === undefined) throw new Error(`${JSON.stringify(r)}: a message went to a port no window owns`);
      return toTree({ to, message });
    });
  r.external = [...external];
  r.prefs = fakeFs.takeWrites().includes(PREFS) ? toTree(fakeFs.dumpFiles()[PREFS]) : null;
  r.rejections = [...rejections];
  out(r);
  drain();
}

function emit(id: number, name: string, data?: unknown): void {
  const port = windowPort(windowOf(id));
  if (port === undefined) throw new Error(`window ${id} has no port`);
  port.emit("message", { data: { name, data } });
}

const message = (id: number, name: string, data?: unknown, script: Reply[] = []) =>
  observe({ op: "message", window: id, name, data: toTree(data), during: toTree(script[0]?.before ?? null) }, script, () =>
    emit(id, name, data)
  );
// Focus moves to `to` from `from`, which gets its blur first.
const focus = (to: number, from: number | null) =>
  observe({ op: "focus", window: to, from }, [], () => {
    if (from !== null) windowOf(from).emit("blur");
    windowOf(to).emit("focus");
  });
const power = (name: "on-battery" | "on-ac") =>
  observe({ op: "power", name }, [], () => {
    onBattery = name === "on-battery";
    for (const listener of powerListeners.get(name) ?? []) listener();
  });
const destroy = (id: number) => observe({ op: "destroy", window: id }, [], () => windowOf(id).destroy());
const open = (kind: string, id: number) =>
  observe({ op: "open", kind, window: id }, [], () => windowOf(id).webContents.emit("dom-ready"));
function setPrefs(text: string): void {
  fakeFs.putFile(PREFS, new Uint8Array(Buffer.from(text, "utf8")));
  out({ op: "setPrefs", text });
}

export async function run(): Promise<void> {
  install();
  const Constants = await import("@upstream/main/electron/ElectronConstants");
  windowIcon = Constants.WINDOW_ICON;
  setNow(T0);
  fakeFs.putFile(BETA_FILE, new Uint8Array(Buffer.from('{"welcomeComplete":true,"surveyStatus":true}')));
  process.argv.splice(2, process.argv.length);
  electronFake.fireReady();
  await flush(60);
  const first = electronFake.takeCreated()[0].id;
  out({ op: "start", prefs: toTree(fakeFs.dumpFiles()[PREFS]) });
  drain();

  await open("hub", first);
  const newHub = async () => {
    electronFake.fireAppEvent("open-file", {}, "/logs/plain.wpilog");
    await flush();
    const id = electronFake.takeCreated().pop()!.id;
    await open("hub", id);
    return id;
  };
  const newSatellite = async (parent: number, uuid: string) => {
    emit(parent, "create-satellite", { uuid, type: 2 });
    await flush();
    const id = electronFake.takeCreated().pop()!.id;
    await open("satellite", id);
    return id;
  };

  await message(first, "alert", { title: "Conflicting types", content: "Values with conflicting types have been written to this field. Only values compatible with the initial type can be viewed and exported." }, [{ checkboxChecked: false }]);
  await message(first, "error", { title: "Failed to load assets", content: "Some asset files failed to load. Check the developer console for details." }, [{ checkboxChecked: false }]);
  await message(first, "open-link", "https://example.com/field?a=b#c");

  for (const name of ["frc-coordinate-system-warning", "ftc-coordinate-system-warning"]) {
    await message(first, name, undefined, [{ checkboxChecked: false }]);
    await message(first, name, undefined, [{ checkboxChecked: false }]);
    // What else writes the preferences while the box is open is lost.
    const before = { window: first, name: "update-preferences", data: { liveDiscard: 900 } };
    await message(first, name, undefined, [{ checkboxChecked: true, before }]);
    await message(first, name);
  }
  for (const value of ["false", "0", '""', "1", '"no"', "null"]) {
    setPrefs(`{"theme":"system","skipFRCCoordinateSystemWarning":${value}}\n`);
    const skipped = Boolean(JSON.parse(value));
    await message(first, "frc-coordinate-system-warning", undefined, skipped ? [] : [{ checkboxChecked: true }]);
  }

  await focus(first, null);
  const second = await newHub();
  await focus(second, first);
  await focus(first, second);
  await focus(second, first);

  await power("on-battery");
  const satellite = await newSatellite(first, "satellite-uuid");
  const third = await newHub();
  await focus(satellite, second);
  await focus(third, satellite);
  await power("on-ac");
  await power("on-battery");
  await destroy(satellite);
  await destroy(third);
  await power("on-ac");
}
