import path from "node:path";
import { toTree } from "../cve1";
import { buildMain } from "./bundle";
import { World, electronFake, fakeFs } from "./world";

// main.ts's download section, driven against a scripted ssh2 `Client`, fake timers and a scripted
// local folder. One long session: main.ts keeps its download state in module variables.
// Further effects:
//   {k: "connect", client, config}, {k: "end", client}
//   {k: "sftp", client, token | threw}          Client.sftp; `threw` when not connected
//   {k: "readdir", sftp, path, token | threw}
//   {k: "fastGet", sftp, remote, local, token | threw}
//   {k: "timeout" | "interval", id, ms}, {k: "clearTimeout" | "clearInterval", id}
//   {k: "mkdir", path, recursive}
// Every op is recorded as {op, args?, effects, threw?}; `threw` is the message of what main.ts let
// escape from the callback.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

type Entry = { filename: string; mode: number | null; size: number | null };
type Timer = { id: number; ms: number; callback: () => void; repeat: boolean };
const DIR = 0o040755;
const FILE = 0o100644;
const LINK = 0o120777;

export async function runScript(platform: "darwin" | "win32", count: number, seed: number): Promise<void> {
  const bundle = await buildMain(platform === "win32" ? "win32" : "posix");
  Object.defineProperty(process, "platform", { value: platform });
  const style = platform === "win32" ? path.win32 : path.posix;
  const save = platform === "win32" ? "C:\\save" : "/save";

  let world: World;
  const clients: FakeClient[] = [];
  const sftps: FakeSftp[] = [];
  const timers = new Map<number, Timer>();
  let nextTimer = 1;
  let nextToken = 0;
  const pending = new Map<number, { kind: "sftp" | "readdir" | "fastGet"; callback: (...args: any[]) => void; step?: (...args: any[]) => void }>();
  let recordTimers = false;

  const disk = new Map<string, number | "dir" | "statThrows">();
  let mkdirThrows = false;

  class FakeClient {
    id: number;
    connected = true;
    readyHandlers: (() => void)[] = [];
    errorHandlers: ((error: Error) => void)[] = [];
    constructor() {
      this.id = clients.push(this) - 1;
    }
    once(name: string, callback: () => void): this {
      if (name !== "ready") throw new Error(`Client.once(${name})`);
      this.readyHandlers.push(callback);
      return this;
    }
    on(name: string, callback: (error: Error) => void): this {
      if (name !== "error") throw new Error(`Client.on(${name})`);
      this.errorHandlers.push(callback);
      return this;
    }
    connect(config: object): this {
      world.effects.push({ k: "connect", client: this.id, config: toTree(config) });
      return this;
    }
    end(): this {
      world.effects.push({ k: "end", client: this.id });
      return this;
    }
    sftp(callback: (error: Error | undefined, sftp?: FakeSftp) => void): this {
      if (!this.connected) {
        world.effects.push({ k: "sftp", client: this.id, threw: "Not connected" });
        throw new Error("Not connected");
      }
      const token = nextToken++;
      pending.set(token, { kind: "sftp", callback });
      world.effects.push({ k: "sftp", client: this.id, token });
      return this;
    }
  }

  class FakeSftp {
    id: number;
    dead = false;
    constructor() {
      this.id = sftps.push(this) - 1;
    }
    private request(kind: "readdir" | "fastGet", effect: any, callback: (...args: any[]) => void, step?: (...args: any[]) => void) {
      if (this.dead) {
        world.effects.push({ k: kind, sftp: this.id, ...effect, threw: "Instance unusable after fatal error" });
        throw new Error("Instance unusable after fatal error");
      }
      const token = nextToken++;
      pending.set(token, { kind, callback, step });
      world.effects.push({ k: kind, sftp: this.id, ...effect, token });
    }
    readdir(where: string, callback: (...args: any[]) => void) {
      this.request("readdir", { path: where }, callback);
    }
    fastGet(remote: string, local: string, options: any, callback: (...args: any[]) => void) {
      if (Object.keys(options).join() !== "step") throw new Error(`fastGet options ${Object.keys(options)}`);
      this.request("fastGet", { remote, local }, callback, options.step);
    }
  }

  const addTimer = (repeat: boolean) => (callback: () => void, ms: number) => {
    const id = nextTimer++;
    timers.set(id, { id, ms, callback, repeat });
    if (recordTimers) world.effects.push({ k: repeat ? "interval" : "timeout", id, ms });
    return id;
  };
  const clearTimer = (k: string) => (id: number) => {
    timers.delete(id);
    if (recordTimers) world.effects.push({ k, id });
  };

  const inSave = (name: unknown) => String(name).startsWith(save);
  const localFs = {
    existsSync: (name: unknown) => (inSave(name) ? disk.has(String(name)) : fakeFs.existsSync(name)),
    statSync: (name: unknown) => {
      const entry = disk.get(String(name));
      if (entry === undefined || entry === "statThrows") throw new Error(`stat ${String(name)}`);
      return { size: entry === "dir" ? 64 : entry };
    },
    mkdirSync: (name: unknown, options: any) => {
      if (!inSave(name)) return (fakeFs as any).mkdirSync(name, options);
      world.effects.push({ k: "mkdir", path: String(name), recursive: options?.recursive === true });
      if (mkdirThrows) throw new Error("EACCES");
      disk.set(String(name), "dir");
      return undefined;
    }
  };
  (globalThis as any).__asrsDownload = {
    electron: electronFake,
    fs: { ...fakeFs, default: { ...(fakeFs as any).default, ...localFs }, ...localFs },
    ssh2: { Client: FakeClient },
    setTimeout: addTimer(false),
    clearTimeout: clearTimer("clearTimeout"),
    setInterval: addTimer(true),
    clearInterval: clearTimer("clearInterval")
  };
  console.error = () => undefined;

  world = new World(bundle);
  await world.start();
  timers.clear();
  nextTimer = 1;
  recordTimers = true;
  world.take();
  out({ platform, save });

  async function record(op: string, args: any, run: () => unknown): Promise<void> {
    const r: any = { op };
    if (args !== undefined) r.args = args;
    try {
      await run();
    } catch (error) {
      r.threw = (error as Error).message;
    }
    await world.settle();
    r.effects = world.take();
    out(r);
  }

  const attrs = (entry: Entry) => {
    const mode = entry.mode ?? undefined;
    return { size: entry.size ?? undefined, isDirectory: () => ((mode as number) & 0o170000) === 0o040000 };
  };
  const open = () => record("open", undefined, () => world.openDownload());
  const userClose = () => record("userClose", undefined, () => world.closeDownload());
  const licenses = () => record("licenses", undefined, () => world.licenses());
  const start = (address: string, remote: string) =>
    record("start", { address, path: remote }, () => world.fromWindow("start", { address, path: remote }));
  const closeMessage = () => record("close", undefined, () => world.fromWindow("close"));
  const saveMessage = (files: string[]) => record("save", { files }, () => world.fromWindow("save", files));
  const ready = (client: number) =>
    record("ready", { client }, () => {
      clients[client].readyHandlers.splice(0).forEach((callback) => callback());
    });
  const fail = (client: number, message: string) =>
    record("error", { client, message }, () => clients[client].errorHandlers.forEach((callback) => callback(new Error(message))));
  const setConnected = (client: number, connected: boolean) =>
    record("setConnected", { client, connected }, () => {
      clients[client].connected = connected;
    });
  const setDead = (sftp: number, dead: boolean) =>
    record("setDead", { sftp, dead }, () => {
      sftps[sftp].dead = dead;
    });
  const finish = (token: number, kind: string) => {
    const request = pending.get(token);
    if (request?.kind !== kind) throw new Error(`token ${token} is not a pending ${kind}`);
    pending.delete(token);
    return request.callback;
  };
  const sftpResult = (token: number, error: string | null) =>
    record("sftpResult", { token, error }, () => {
      const callback = finish(token, "sftp");
      if (error === null) callback(undefined, new FakeSftp());
      else callback(new Error(error));
    });
  const readdirResult = (token: number, result: string | Entry[]) =>
    record("readdirResult", typeof result === "string" ? { token, error: result } : { token, list: result }, () => {
      const callback = finish(token, "readdir");
      if (typeof result === "string") callback(new Error(result));
      else callback(undefined, result.map((entry) => ({ filename: entry.filename, longname: "", attrs: attrs(entry) })));
    });
  const progress = (token: number, transferred: number, chunk: number, total: number) =>
    record("progress", { token, transferred, chunk, total }, () => pending.get(token)!.step!(transferred, chunk, total));
  const getResult = (token: number, error: string | null) =>
    record("getResult", { token, error }, () => {
      const callback = finish(token, "fastGet");
      if (error === null) callback();
      else callback(new Error(error));
    });
  const fire = (id: number) =>
    record("timer", { id }, () => {
      const timer = timers.get(id);
      if (timer === undefined) throw new Error(`timer ${id} is not set`);
      if (!timer.repeat) timers.delete(id);
      timer.callback();
    });
  const answer = (token: number, value: any) => record("answer", { token, value }, () => world.answer(token, value));
  const local = (name: string, entry: number | "dir" | "statThrows" | null) =>
    record("local", { path: name, entry }, () => {
      if (entry === null) disk.delete(name);
      else disk.set(name, entry);
    });
  const setMkdir = (throws: boolean) =>
    record("setMkdirThrows", { throws }, () => {
      mkdirThrows = throws;
    });

  const tokens = (kind: string) => [...pending].filter(([, request]) => request.kind === kind).map(([token]) => token);
  const last = (kind: string) => {
    const list = tokens(kind);
    return list[list.length - 1];
  };
  const lastClient = () => clients.length - 1;
  const lastSftp = () => sftps.length - 1;
  const timerIds = (ms: number) => [...timers.values()].filter((t) => t.ms === ms).map((t) => t.id);
  const fireAll = async (ms: number) => {
    for (const id of timerIds(ms)) if (timers.has(id)) await fire(id);
  };
  const connect = async (list: Entry[] = LIST) => {
    await ready(lastClient());
    await sftpResult(last("sftp"), null);
    await readdirResult(last("readdir"), list);
  };
  const drain = async () => {
    for (const token of tokens("readdir")) if (pending.has(token)) await readdirResult(token, "drained");
    for (const token of tokens("sftp")) await sftpResult(token, "drained");
    for (const token of tokens("fastGet")) if (pending.has(token)) await getResult(token, "drained");
  };
  const LIST: Entry[] = [
    { filename: "b.wpilog", mode: FILE, size: 2500000 },
    { filename: "a.rlog", mode: FILE, size: 12 },
    { filename: "c.wpilogxz", mode: FILE, size: 0 },
    { filename: "d.revlog", mode: FILE, size: 99999 },
    { filename: "e.hoot", mode: FILE, size: 100000 },
    { filename: "notes.txt", mode: FILE, size: 5 },
    { filename: ".hidden.wpilog", mode: FILE, size: 5 },
    { filename: "UPPER.WPILOG", mode: FILE, size: 5 },
    { filename: "link", mode: LINK, size: 5 },
    { filename: "link.wpilog", mode: LINK, size: 7 },
    { filename: "nomode.wpilog", mode: null, size: 8 },
    { filename: "nosize.wpilog", mode: FILE, size: null },
    { filename: "big.wpilog", mode: FILE, size: 6442450944 }
  ];

  await open();
  await start("10.63.28.2", "/U/logs");
  await connect();
  await fire(timerIds(5000)[0]);
  await readdirResult(last("readdir"), []);
  await fire(timerIds(5000)[0]);
  await readdirResult(last("readdir"), "No such file");
  await fire(timerIds(1000)[0]);
  await fail(lastClient(), "All configured authentication methods failed");
  await fail(lastClient(), "All configured authentication methods failed");
  await fire(timerIds(1000)[0]);
  await fail(lastClient(), "All configured authentication methods failed");
  await ready(lastClient());
  await sftpResult(last("sftp"), "Unable to start subsystem: sftp");
  await fire(timerIds(1000)[0]);
  await setConnected(lastClient(), false);
  await ready(lastClient());
  await fail(lastClient(), "Not connected");
  await setConnected(lastClient(), true);
  await ready(lastClient());
  await sftpResult(last("sftp"), null);
  const folders: Entry[] = [
    { filename: "match1", mode: DIR, size: 4096 },
    { filename: "x.wpilog", mode: FILE, size: 10 },
    { filename: ".git", mode: DIR, size: 4096 },
    { filename: "dotted.dir", mode: DIR, size: 4096 },
    { filename: "failing", mode: DIR, size: 4096 },
    { filename: "empty", mode: DIR, size: 4096 }
  ];
  await readdirResult(last("readdir"), folders);
  await readdirResult(last("readdir"), [
    { filename: "inner.wpilog", mode: FILE, size: 100 },
    { filename: "inner.txt", mode: FILE, size: 11 },
    { filename: ".dot", mode: FILE, size: 1000 },
    { filename: "deep", mode: DIR, size: 4096 }
  ]);
  await readdirResult(last("readdir"), [{ filename: "unsized", mode: FILE, size: null }]);
  await readdirResult(last("readdir"), "Permission denied");
  await readdirResult(last("readdir"), []);

  for (const file of ["x.wpilog", "x.wpilogxz", "x.rlog", "x.hoot", "x.revlog", "x.other", "x.", ".wpilog", "a.b.rlog", "ünïcode.wpilog"]) {
    await saveMessage([file]);
    if (world.pendingDialogs().length > 0) await answer(world.pendingDialogs()[0], { canceled: true, filePath: "", filePaths: [] });
  }
  await saveMessage(["x.wpilog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePath: "" });
  await saveMessage(["x.wpilog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePath: style.join(save, "renamed.wpilog") });
  await sftpResult(last("sftp"), null);
  await progress(last("fastGet"), 4, 4, 10);
  await progress(last("fastGet"), 10, 6, 10);
  await getResult(last("fastGet"), null);
  await answer(world.pendingDialogs()[0], { response: 1 });
  await saveMessage(["x.wpilog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePath: style.join(save, "x.wpilog") });
  await sftpResult(last("sftp"), "(SSH) Channel open failure: open failed");
  await fire(timerIds(1000)[0]);
  await connect();
  await saveMessage(["x.wpilog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePath: style.join(save, "x.wpilog") });
  await sftpResult(last("sftp"), null);
  await getResult(last("fastGet"), "ENOENT: no such file or directory, open '/save/x.wpilog'");
  await fire(timerIds(1000)[0]);
  await connect();
  await setDead(lastSftp(), true);
  await fireAll(5000);
  await setDead(lastSftp(), false);

  // Several files and folders.
  await saveMessage(["b.wpilog", "match1", "dotted.dir", "unknown.rlog", "empty", "failing"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await sftpResult(last("sftp"), null);
  await readdirResult(last("readdir"), [
    { filename: "inner.wpilog", mode: FILE, size: 100 },
    { filename: ".dot", mode: FILE, size: 1 },
    { filename: "deep", mode: DIR, size: 4096 },
    { filename: "inner.txt", mode: FILE, size: 50 }
  ]);
  await readdirResult(last("readdir"), []);
  await readdirResult(last("readdir"), "Failure");
  const batch = tokens("fastGet");
  await progress(batch[0], 32768, 32768, 2500000);
  await progress(batch[1], 100, 100, 100);
  await progress(batch[0], 65536, 32768, 2500000);
  for (const token of batch) await getResult(token, null);

  // Everything already saved, then one file short.
  await local(style.join(save, "b.wpilog"), 2500000);
  await local(style.join(save, "a.rlog"), 11);
  await saveMessage(["b.wpilog", "a.rlog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await sftpResult(last("sftp"), null);
  await getResult(last("fastGet"), null);
  await local(style.join(save, "a.rlog"), 12);
  await saveMessage(["b.wpilog", "a.rlog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await sftpResult(last("sftp"), null);
  await local(style.join(save, "nosize.wpilog"), 1);
  await local(style.join(save, "c.wpilogxz"), "statThrows");
  await saveMessage(["nosize.wpilog", "c.wpilogxz", "b.wpilog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await sftpResult(last("sftp"), null);
  const partial = tokens("fastGet");
  await getResult(partial[0], "Permission denied");
  await getResult(partial[1], "No response from server");
  await fireAll(1000);
  await connect();
  await setMkdir(true);
  await saveMessage(["folder"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await sftpResult(last("sftp"), null);
  await readdirResult(last("readdir"), [{ filename: "one.wpilog", mode: FILE, size: 1 }]);
  await setMkdir(false);
  await saveMessage(["folder"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await sftpResult(last("sftp"), null);
  await readdirResult(last("readdir"), [{ filename: "one.wpilog", mode: FILE, size: 1 }]);
  await getResult(last("fastGet"), null);
  await saveMessage(["folder", "a.rlog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await setConnected(lastClient(), false);
  await saveMessage(["a.rlog", "b.wpilog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePaths: [save] });
  await setConnected(lastClient(), true);
  await drain();

  // Open the saved log, which closes the window.
  await fireAll(1000);
  await connect();
  await saveMessage(["b.wpilog"]);
  await answer(world.pendingDialogs()[0], { canceled: false, filePath: style.join(save, "out", "b.wpilog") });
  await sftpResult(last("sftp"), null);
  await getResult(last("fastGet"), null);
  await answer(world.pendingDialogs()[0], { response: 0 });
  await open();
  await start("10.63.28.2", "/home/lvuser/logs/");
  await userClose();
  await fail(lastClient(), "connect ECONNREFUSED 10.63.28.2:22");
  await fail(lastClient(), "connect ECONNREFUSED 10.63.28.2:22");
  await fireAll(1000);
  await open();
  await start("roborio-6328-frc.local", "");
  await licenses();
  await closeMessage();
  await closeMessage();

  // Random walks over the same operations.
  let state = seed >>> 0 || 1;
  const next = () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return state >>> 0;
  };
  const below = (n: number) => next() % n;
  const pick = <T>(list: readonly T[]): T => list[below(list.length)];
  const NAMES = ["a.rlog", "b.wpilog", "c.wpilogxz", "d.revlog", "e.hoot", "match1", "dotted.dir", "notes.txt", ".hidden", "Log_1.wpilog", "x"];
  const MESSAGES = ["No such file", "Failure", "Not connected", "Timed out while waiting for handshake", "All configured authentication methods failed", "read ECONNRESET"];
  const MODES = [FILE, FILE, FILE, DIR, DIR, LINK, null];
  const entries = (): Entry[] => {
    const list: Entry[] = [];
    for (let i = below(6); i > 0; i--) list.push({ filename: pick(NAMES), mode: pick(MODES), size: below(8) === 0 ? null : below(1 << 22) });
    return list;
  };
  const alive = () => world.downloadWindow !== null && !world.downloadWindow.isDestroyed();
  const recent = (list: number[]) => (below(4) === 0 ? pick(list) : list[list.length - 1]);
  for (let i = 0; i < count; i++) {
    const roll = below(100);
    if (!alive()) {
      if (roll < 40) {
        await open();
        continue;
      }
    } else if (roll < 8) {
      await start(pick(["10.0.0.2", "172.22.11.2", "robot.local"]), pick(["/U/logs", "/home/lvuser/logs/", "", "/"]));
      continue;
    } else if (roll < 11) {
      await pick([userClose, closeMessage, licenses])();
      continue;
    } else if (roll < 22 && world.pendingDialogs().length < 3) {
      const files: string[] = [];
      for (let k = 1 + below(4); k > 0; k--) files.push(pick(NAMES));
      await saveMessage([...new Set(files)]);
      continue;
    } else if (roll < 36 && world.pendingDialogs().length > 0) {
      const name = pick(NAMES);
      await answer(
        pick(world.pendingDialogs()),
        pick([
          { canceled: below(4) === 0, filePath: style.join(save, name), filePaths: [save], response: below(2) }
        ])
      );
      continue;
    }
    if (roll < 44 && clients.length > 0) await ready(recent(clients.map((c) => c.id)));
    else if (roll < 48 && clients.length > 0) await fail(recent(clients.map((c) => c.id)), pick(MESSAGES));
    else if (roll < 50 && clients.length > 0) await setConnected(lastClient(), below(3) !== 0);
    else if (roll < 52 && sftps.length > 0) await setDead(lastSftp(), below(3) === 0);
    else if (roll < 62 && tokens("sftp").length > 0) await sftpResult(recent(tokens("sftp")), below(5) === 0 ? pick(MESSAGES) : null);
    else if (roll < 76 && tokens("readdir").length > 0) await readdirResult(recent(tokens("readdir")), below(6) === 0 ? pick(MESSAGES) : entries());
    else if (roll < 82 && tokens("fastGet").length > 0) await progress(pick(tokens("fastGet")), below(1 << 22), 32768, 1 << 22);
    else if (roll < 90 && tokens("fastGet").length > 0 && alive()) await getResult(pick(tokens("fastGet")), below(5) === 0 ? pick(MESSAGES) : null);
    else if (roll < 94) await local(style.join(save, pick(NAMES)), pick([null, 0, 1000, 1 << 23, "dir", "statThrows"] as const));
    else if (roll < 95) await setMkdir(below(3) === 0);
    else if (timers.size > 0) await fire(pick([...timers.keys()]));
  }
}
