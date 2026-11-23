import fakeFsApi, * as fakeFs from "../fakes/mainFs";
import * as electronFake from "../fakes/mainElectron";
import { setNow, tickInterval } from "../fakes/globals";
import { toTree } from "../cve1";

// `genFiles routing 0 0 dev <scenario>`: main.ts's isDsLog, openFilesSmart and DS windows, reached
// through the hub's `open-files-request`, the Open Log(s) menu item, the `open-file` event, the
// AdvantageKit path poll and startup; and its export flow: `prompt-export`, the export window's
// port, `write-export`, `set-exporting` and the hub's `close` listener.
//
// Scenarios: `running` (everything while running), `startup-plain`, `startup-argv` and
// `startup-ds` (a file to open at launch, from the `open-file` event, or the arguments), and
// `startup-ds-saved` (the same, with a saved DS window).
//
// Every observed record has what main did:
//   dialogs    dialog calls: kind, parent window id, options (`icon` as "WINDOW_ICON") and reply
//   sent       messages posted to each hub's port: open-files, restore-state,
//              restore-type-memory, cancel-export, prepare-export, finish-export and, for an
//              export window (`page`), everything posted to it
//   created    windows opened: {id, kind, parent}; kind is the file they load
//   focused, destroyed, closed   window ids in order; external  shell.openExternal URLs
//   files      the files written, with their bytes; null for a file that is gone
//   throw      the name of an error the event's own handler threw; uncaught and rejections, those
//              of exceptions and promise rejections that followed
// Other records set the world up: "start", "openFileEarly", "putFile", "putDir", "setFailure"
// and "saveState".

const T0 = Date.UTC(2026, 10, 21, 12, 34, 56);
const BETA_FILE = "/fake/userData/beta-2027.json";
const AKIT_INPUT = "/fake/temp/ascope-log-path.txt";
const SENT = ["open-files", "restore-state", "restore-type-memory", "cancel-export", "prepare-export", "finish-export"];

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const flush = async (times = 20) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};
const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));

type Window = electronFake.FakeBrowserWindow;
type Reply = { kind: "open" | "save" | "box" | "boxSync"; value: any; before?: () => void; ifAsked?: boolean };

let windowIcon: unknown;
const files = new Map<number, string>();
const replies: Reply[] = [];
const dialogs: any[] = [];
const focusedIds: number[] = [];
const destroyedIds: number[] = [];
const uncaught: string[] = [];
const rejections: string[] = [];
const ports = new Map<electronFake.FakePort, number>();

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
  reply.before?.();
  return reply.value;
}

const later =
  (kind: Reply["kind"]) =>
  async (...args: any[]) => {
    const value = showDialog(kind, args);
    await flush(2);
    return value;
  };

function install(): void {
  const fds = new Map<number, string>();
  let nextFd = 1000;
  const api = fakeFsApi as any;
  // node's own errors are not observable: isDsLog catches everything it opens and reads.
  api.openSync = (name: string) => {
    const opened = fakeFs.existsSync(name) && !failing.has(name);
    if (!opened) throw new Error(`cannot open ${name}`);
    fds.set(nextFd, name);
    return nextFd++;
  };
  api.readSync = (fd: number, buffer: Buffer, offset: number, length: number, position: number) => {
    const name = fds.get(fd);
    if (name === undefined) throw new Error("bad fd");
    if (unreadable.has(name) || dirs.has(name)) throw new Error("EISDIR");
    return fakeFs.readFileSync(name).copy(buffer, offset, position, position + length);
  };
  api.closeSync = (fd: number) => {
    if (!fds.delete(fd)) throw new Error("bad fd");
  };
  // node calls back after the caller returned, so a throw in the callback is uncaught.
  const writeFile = api.writeFile;
  api.writeFile = (name: unknown, data: unknown, ...rest: any[]) => {
    const callback = rest[rest.length - 1];
    writeFile(name, data, (error: unknown) => setImmediate(() => callback(error)));
  };

  const readFile = api.readFile;
  api.readFile = (name: unknown, ...rest: any[]) => {
    const callback = rest[rest.length - 1];
    readFile(name, (error: unknown, data?: Buffer) =>
      setImmediate(() => callback(error, rest[0] === "utf8" ? data?.toString("utf8") : data))
    );
  };

  const proto = electronFake.FakeBrowserWindow.prototype as any;
  proto.loadFile = function (this: Window, file: string) {
    files.set(this.id, file.split("/").pop()!);
  };
  proto.focus = function (this: Window) {
    focusedIds.push(this.id);
    this.emit("focus");
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
  electronFake.dialog.showOpenDialog = later("open");
  electronFake.dialog.showSaveDialog = later("save");
  electronFake.dialog.showMessageBox = later("box");
  electronFake.dialog.showMessageBoxSync = (...args: any[]) => showDialog("boxSync", args);
  process.on("unhandledRejection", (error: any) => {
    rejections.push(error?.name ?? String(error));
  });
  process.on("uncaughtException", (error: any) => {
    uncaught.push(error?.name ?? String(error));
  });
}

const failing = new Set<string>();
const unreadable = new Set<string>();
const dirs = new Set<string>();

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

const byFile = (file: string): Window[] => electronFake.windowList().filter((w) => files.get(w.id) === file);
const windowOf = (id: number): Window => {
  const window = electronFake.FakeBrowserWindow.fromId(id);
  if (window === null) throw new Error(`window ${id} is gone`);
  return window;
};

function drain(): void {
  for (const list of [dialogs, focusedIds, destroyedIds, uncaught, rejections, electronFake.externalUrls]) list.splice(0, list.length);
  electronFake.takeClosed();
  electronFake.takeCreated();
  electronFake.sendLog.splice(0, electronFake.sendLog.length);
  fakeFs.takeWrites();
}

async function observe(record: any, script: Reply[], run: () => unknown): Promise<void> {
  labelPorts();
  drain();
  replies.splice(0, replies.length, ...script);
  const r: any = record;
  r.throw = null;
  try {
    run();
  } catch (error) {
    r.throw = (error as Error).name;
  }
  await flush();
  if (replies.some((reply) => !reply.ifAsked)) throw new Error(`${JSON.stringify(r)}: ${replies.length} scripted replies were not asked for`);
  labelPorts();
  r.dialogs = [...dialogs];
  r.sent = electronFake.sendLog
    .map(({ port, message }) => {
      const to = ports.get(port);
      if (to === undefined) throw new Error(`${JSON.stringify(r)}: a message went to a port no window owns`);
      return { to, message };
    })
    .filter(({ to, message }) => files.get(to) === "export.html" || SENT.includes(message.name))
    .map((sent) => toTree(sent));
  r.created = electronFake.takeCreated().map(({ id }) => ({
    id,
    kind: files.get(id),
    parent: electronFake.FakeBrowserWindow.fromId(id)?.options.parent?.id ?? null
  }));
  r.focused = [...focusedIds];
  r.destroyed = [...destroyedIds];
  r.closed = electronFake.takeClosed();
  r.external = [...electronFake.externalUrls];
  const dump = fakeFs.dumpFiles();
  r.files = [...new Set(fakeFs.takeWrites())].map((name) => toTree({ name, bytes: dump[name] ?? null }));
  r.uncaught = [...uncaught];
  r.rejections = [...rejections];
  out(r);
  drain();
}

function putFile(name: string, bytes: Uint8Array): void {
  fakeFs.putFile(name, bytes);
  out({ op: "putFile", name: toTree(name), bytes: toTree(bytes) });
}

function putDir(name: string): void {
  fakeFs.putDir(name);
  dirs.add(name);
  out({ op: "putDir", name: toTree(name) });
}

function setFailure(name: string, kind: "read" | "readOpened" | "write"): void {
  fakeFs.setFailure(name, kind);
  if (kind === "read") failing.add(name);
  if (kind === "readOpened") unreadable.add(name);
  out({ op: "setFailure", name: toTree(name), kind });
}

function emit(id: number, name: string, data: any): void {
  labelPorts();
  const port = windowPort(windowOf(id));
  if (port === undefined) throw new Error(`window ${id} has no port`);
  port.emit("message", { data: { name, data } });
}

const message = (id: number, name: string, data: any, script: Reply[] = []) =>
  observe({ op: "message", window: id, name, data: toTree(data) }, script, () => emit(id, name, data));
const openRequest = (id: number, files: string[], script: Reply[] = []) => message(id, "open-files-request", { files }, script);
const openFile = (path: string, script: Reply[] = []) =>
  observe({ op: "openFile", path: toTree(path) }, script, () => electronFake.fireAppEvent("open-file", {}, path));
const domReady = (id: number, script: Reply[] = []) =>
  observe({ op: "domReady", window: id }, script, () => windowOf(id).webContents.emit("dom-ready"));
const close = (id: number, script: Reply[] = []) => observe({ op: "close", window: id }, script, () => windowOf(id).close());
const focus = (id: number) => observe({ op: "focus", window: id }, [], () => windowOf(id).emit("focus"));
const akit = (index: number, script: Reply[] = []) => observe({ op: "akit" }, script, () => tickInterval(index));
const page = (id: number, data: any, script: Reply[] = []) =>
  observe({ op: "page", window: id, data: toTree(data) }, script, () => {
    const port = windowPort(windowOf(id));
    if (port === undefined) throw new Error(`window ${id} has no port`);
    port.emit("message", { data });
  });
function openLogs(id: number, paths: string[], before: (() => void) | undefined, script: Reply[] = []) {
  const item = electronFake.menuTemplate()![1].submenu[0];
  const reply: Reply = { kind: "open", value: { canceled: false, filePaths: paths }, before };
  const gone = before !== undefined;
  return observe({ op: "openLogs", window: id, paths: toTree(paths), gone }, [reply, ...script], () => item.click(item, windowOf(id)));
}

async function saveState(id: number, state: any): Promise<void> {
  emit(id, "save-state", state);
  await flush();
  drain();
  out({ op: "saveState", window: id, state: toTree(state) });
}

const boxSync = (value: number): Reply => ({ kind: "boxSync", value });
const box = (value: number): Reply => ({ kind: "box", value: { response: value, checkboxChecked: false } });
const save = (filePath: string | null): Reply => ({
  kind: "save",
  value: filePath === null ? { canceled: true, filePath: "" } : { canceled: false, filePath }
});

const DS_HEADER = (() => {
  const extra = Buffer.from("DS Log File", "utf8");
  const buffer = Buffer.alloc(12);
  buffer.write("WPILOG", 0, "ascii");
  buffer[7] = 1;
  buffer.writeUInt32LE(extra.length, 8);
  return Buffer.concat([buffer, extra]);
})();

function variant(edit: (bytes: Buffer) => void): Uint8Array {
  const bytes = Buffer.from(DS_HEADER);
  edit(bytes);
  return new Uint8Array(bytes);
}

const DS = "/logs/ds.wpilog";
const PLAIN = "/logs/plain.wpilog";

function headerFiles(): [string, Uint8Array][] {
  const list: [string, Uint8Array][] = [
    [DS, new Uint8Array(DS_HEADER)],
    [PLAIN, utf8("not a log")],
    ["/logs/empty.wpilog", new Uint8Array(0)],
    ["/logs/ds-records.wpilog", new Uint8Array(Buffer.concat([DS_HEADER, Buffer.from([0x30, 1, 2, 3, 4])]))],
    ["/logs/ds.txt", new Uint8Array(DS_HEADER)],
    ["/logs/high-bits.wpilog", variant((b) => [0, 1, 2, 3, 4, 5].forEach((i) => (b[i] |= 0x80)))],
    ["/logs/one-high-bit.wpilog", variant((b) => (b[3] |= 0x80))],
    ["/logs/extra-high-bit.wpilog", variant((b) => (b[14] |= 0x80))],
    ["/logs/minor.wpilog", variant((b) => (b[6] = 0xff))],
    ["/logs/major-0.wpilog", variant((b) => (b[7] = 0))],
    ["/logs/major-2.wpilog", variant((b) => (b[7] = 2))],
    ["/logs/major-129.wpilog", variant((b) => (b[7] = 129))],
    ["/logs/length-10.wpilog", variant((b) => b.writeUInt32LE(10, 8))],
    ["/logs/length-12.wpilog", variant((b) => b.writeUInt32LE(12, 8))],
    ["/logs/length-high.wpilog", variant((b) => b.writeUInt32LE(11 + 0x01000000, 8))],
    ["/logs/length-big-endian.wpilog", variant((b) => b.writeUInt32BE(11, 8))],
    ["/logs/lowercase.wpilog", variant((b) => b.write("wpilog", 0, "ascii"))],
    ["/logs/é🚀.wpilog", new Uint8Array(DS_HEADER)]
  ];
  for (let i = 0; i < DS_HEADER.length; i++) {
    if (i === 6) continue;
    list.push([`/logs/byte-${i}.wpilog`, variant((b) => (b[i] ^= 1))]);
    list.push([`/logs/short-${i}.wpilog`, new Uint8Array(DS_HEADER.subarray(0, i))]);
  }
  return list;
}

async function start(scenario: string, before: () => void): Promise<void> {
  install();
  const Constants = await import("@upstream/main/electron/ElectronConstants");
  windowIcon = Constants.WINDOW_ICON;
  setNow(T0);
  fakeFs.putFile(BETA_FILE, utf8('{"welcomeComplete":true,"surveyStatus":true}'));
  process.argv.splice(2, process.argv.length);
  before();
  electronFake.fireReady();
  await flush(60);
  const dump = fakeFs.dumpFiles();
  out({
    op: "start",
    scenario,
    files: Object.keys(dump).map((name) => toTree({ name, bytes: dump[name] })),
    argv: toTree(process.argv.slice(2)),
    created: electronFake.takeCreated().map(({ id }) => id)
  });
  drain();
}

async function running(dsLayout: any): Promise<void> {
  await start("running", () => {});
  let [a] = byFile("hub.html").map((w) => w.id);
  await domReady(a);
  for (const [name, bytes] of headerFiles()) putFile(name, bytes);
  putDir("/logs/dir.wpilog");
  putFile("/logs/locked.wpilog", new Uint8Array(DS_HEADER));
  setFailure("/logs/locked.wpilog", "read");
  putFile("/logs/unreadable.wpilog", new Uint8Array(DS_HEADER));
  setFailure("/logs/unreadable.wpilog", "readOpened");

  // isDsLog: a DS log asks, anything else opens.
  const thisWindow: Reply = { ...boxSync(1), ifAsked: true };
  for (const [name] of headerFiles()) await openRequest(a, [name], [thisWindow]);
  for (const name of ["/logs/dir.wpilog", "/logs/locked.wpilog", "/logs/unreadable.wpilog", "/logs/missing.wpilog", ""]) await openRequest(a, [name], [thisWindow]);
  for (const list of [[], [DS, DS], [DS, PLAIN], [PLAIN, DS]]) await openRequest(a, list);

  // Every reply, without and with a DS window.
  const createdHub = () => byFile("hub.html").pop()!.id;
  for (const reply of [1, 2, 0]) await openRequest(a, [DS], [boxSync(reply)]);
  const ds = createdHub();
  await domReady(ds);
  await openRequest(a, [DS], [boxSync(2)]);
  await saveState(ds, dsLayout);
  await openRequest(ds, [DS]);
  await openRequest(ds, [PLAIN]);
  await focus(a);
  for (const reply of [0, 1, 2]) await openRequest(a, [DS], [boxSync(reply)]);

  // What counts as a DS window.
  const tabs = dsLayout.tabs.tabs;
  const states: any[] = [
    { ...dsLayout, extra: 1 },
    { tabs: { tabs } },
    { tabs: { selected: 0, tabs: tabs.slice(1) } },
    { tabs: { tabs: [...tabs, tabs[0]] } },
    { tabs: { tabs: [...tabs].reverse() } },
    { tabs: { tabs: tabs.map((tab: any) => ({ type: tab.type, title: tab.title })) } },
    { tabs: { tabs: tabs.map((tab: any, i: number) => (i === 3 ? { ...tab, title: "Power 2" } : tab)) } },
    { tabs: { tabs: tabs.map((tab: any, i: number) => (i === 1 ? { ...tab, type: "3" } : tab)) } },
    { tabs: { tabs: tabs.map((tab: any, i: number) => (i === 1 ? { ...tab, title: undefined } : tab)) } },
    { tabs: { tabs: tabs.map((tab: any, i: number) => (i === 1 ? { ...tab, type: -0 } : tab)) } },
    { tabs: { tabs: [] } },
    { tabs: { tabs: [5, "x", true] } },
    { tabs: {} },
    { tabs: null },
    { tabs: "text" },
    {},
    null,
    undefined,
    "state",
    { tabs: { tabs: [null] } },
    { tabs: { tabs: [undefined] } },
    { tabs: { tabs: null } },
    { tabs: { tabs: "text" } },
    { tabs: { tabs: 7 } },
    { tabs: { tabs: { length: 0 } } }
  ];
  for (const state of states) {
    await saveState(a, state);
    await openRequest(a, [DS], [thisWindow]);
  }
  // A malformed state of the window before the DS window in the list.
  await saveState(a, { tabs: { tabs: 7 } });
  await saveState(ds, dsLayout);
  await focus(ds);
  await openRequest(ds, [DS]);
  await saveState(a, { tabs: { tabs: [] } });

  // open-file while running: the most recent hub is the source of a DS log only.
  await focus(a);
  await openFile(PLAIN);
  await domReady(createdHub());
  for (const reply of [0, 1, 2]) {
    await focus(a);
    await openFile(DS, [boxSync(reply)]);
  }
  await focus(ds);
  await openFile(DS);
  await openFile("/logs/missing.wpilog");
  await domReady(createdHub());

  // The Open Log(s) dialog, with the window gone by the time it answers.
  await openLogs(a, [PLAIN], undefined);
  await openLogs(a, [DS], undefined, [boxSync(1)]);
  const victim = async (paths: string[], script: Reply[]) => {
    const id = createdHub();
    if (id === a || id === ds) throw new Error("no spare hub");
    await openLogs(id, paths, () => windowOf(id).destroy(), script);
  };
  await openFile(PLAIN);
  await victim([PLAIN], []);
  await domReady(createdHub());
  await victim([DS], [boxSync(0)]);
  await openFile(PLAIN);
  await victim([DS], [boxSync(1)]);
  await saveState(ds, { tabs: { tabs: [] } });
  await openFile(PLAIN);
  await victim([DS], [boxSync(1)]);
  await openFile(PLAIN);
  await victim([DS], [boxSync(0)]);
  await domReady(createdHub());

  // The AdvantageKit poll.
  // Registered after StateTracker's and main.ts's RLOG interval.
  const AKIT = 2;
  await akit(AKIT);
  await focus(a);
  putFile(AKIT_INPUT, utf8(` ${PLAIN}\r\n`));
  await akit(AKIT);
  await akit(AKIT);
  putFile(AKIT_INPUT, utf8(DS));
  await akit(AKIT, [boxSync(0)]);
  await domReady(createdHub());
  putFile(AKIT_INPUT, new Uint8Array([0xef, 0xbb, 0xbf, 0x2f, 0x61, 0xff]));
  await akit(AKIT);

  // No hubs left.
  for (const window of byFile("hub.html")) await close(window.id);
  putFile(AKIT_INPUT, utf8(PLAIN));
  await akit(AKIT);
  await openFile(DS);
  await domReady(createdHub());
  for (const window of byFile("hub.html")) await close(window.id);
  await openFile(PLAIN);
  a = createdHub();
  await domReady(a);
  await exportFlow(a);
}

async function exportFlow(hub: number): Promise<void> {
  const paths: any[] = [
    null,
    "/logs/match.wpilog",
    "/logs/match.2026.rlog",
    "/logs.d/noextension",
    "noextension",
    ".hidden",
    "",
    "/logs/é🚀.csv",
    "/logs/trailing."
  ];
  const options = (format: string) => ({ format, samplingMode: "changes", samplingPeriod: 20, prefixes: "", includeGenerated: true });
  const formats = ["csv-table", "csv-list", "wpilog", "mcap"];
  const prompt = async (data: any, script: Reply[] = []): Promise<number | null> => {
    await message(hub, "prompt-export", data, script);
    const opened = byFile("export.html").pop();
    if (opened === undefined) return null;
    await domReady(opened.id);
    return opened.id;
  };
  const request = (path: any, incompleteWarning = false, supportsAkit = false) => ({ path, incompleteWarning, supportsAkit });

  await prompt(request("/logs/a.wpilog", true), [box(1)]);
  let id = (await prompt(request("/logs/a.wpilog", true, true), [box(0)]))!;
  await page(id, "https://docs.advantagescope.org/export");
  await page(id, null);
  for (const path of paths) {
    for (const format of formats) {
      id = (await prompt(request(path, false, format === "mcap")))!;
      await page(id, options(format), [save(null)]);
      await page(id, options(format), [save(`/out/${format}.bin`)]);
    }
  }
  id = (await prompt(request("/logs/a.wpilog")))!;
  await page(id, { ...options("csv-table"), format: null });
  await page(id, options("wpilog"), [save(null)]);
  await close(id);

  putFile("/out/existing.csv", utf8("old contents, longer than the new ones"));
  putFile("/out/locked.csv", utf8("old"));
  setFailure("/out/locked.csv", "write");
  const contents: [string, any][] = [
    ["/out/table.csv", "Timestamp,/Value\n0,é🚀"],
    ["/out/lone.csv", "a\ud800b\udc00c"],
    ["/out/empty.csv", ""],
    ["/out/existing.csv", "new"],
    ["/out/log.wpilog", new Uint8Array([0x57, 0, 255, 128])],
    ["/out/none.mcap", new Uint8Array(0)],
    ["/out/locked.csv", "lost"]
  ];
  for (const [path, content] of contents) await message(hub, "write-export", { path, content });

  await message(hub, "set-exporting", true);
  await close(hub, [boxSync(0)]);
  await message(hub, "set-exporting", false);
  await message(hub, "set-exporting", true);
  await close(hub, [boxSync(0)]);
  await close(hub, [boxSync(1)]);
  await openFile(PLAIN);
  const next = byFile("hub.html").pop()!.id;
  await domReady(next);
  await message(next, "set-exporting", true);
  await message(next, "set-exporting", false);
  await close(next);
}

async function startup(scenario: string, dsLayout: any): Promise<void> {
  const state = {
    hubs: [dsLayout, { sidebar: { width: 100, expanded: [] }, tabs: { selected: 0, tabs: [] } }].map((s, i) => ({
      x: 10 * i,
      y: 20,
      width: 900,
      height: 500,
      state: s
    })),
    satellites: []
  };
  await start(scenario, () => {
    for (const [name, bytes] of headerFiles().slice(0, 2)) fakeFs.putFile(name, bytes);
    if (scenario === "startup-ds-saved") fakeFs.putFile("/fake/userData/state-dev.json", utf8(JSON.stringify(state)));
    if (scenario === "startup-argv") process.argv.push("--flag", "/logs/x.txt", DS, PLAIN);
    if (scenario === "startup-plain") process.argv.push(PLAIN);
    const early = scenario === "startup-argv" ? [PLAIN] : scenario === "startup-plain" ? [] : [PLAIN, DS];
    for (const path of early) {
      out({ op: "openFileEarly", path: toTree(path) });
      electronFake.fireAppEvent("open-file", {}, path);
    }
  });
  const hubs = byFile("hub.html").map((w) => w.id);
  const asks = scenario !== "startup-plain";
  const reply = scenario === "startup-ds" ? 0 : scenario === "startup-argv" ? 1 : 2;
  await domReady(hubs[0], asks ? [boxSync(reply)] : []);
  for (const window of byFile("hub.html")) if (window.id !== hubs[0]) await domReady(window.id);
}

export async function run(scenario: string): Promise<void> {
  const dsLayout = (await import("@upstream/main/lite/dsLayout")).default;
  // The generator's own errors must not reach the listeners that record main.ts's.
  try {
    if (scenario === "running") await running(dsLayout);
    else if (["startup-plain", "startup-argv", "startup-ds", "startup-ds-saved"].includes(scenario)) await startup(scenario, dsLayout);
    else throw new Error(`routing: unknown scenario ${scenario}`);
  } catch (error) {
    process.stderr.write(`routing: ${(error as Error).stack}\n`);
    process.exit(1);
  }
  process.stdout.write("", () => process.exit(0));
}
