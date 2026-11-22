import { EventEmitter } from "node:events";
import nodeCrypto from "node:crypto";
import nodePath from "node:path";
import { toTree } from "../cve1";

// One scripted environment for upstream's VideoProcessor: fake fs, child_process, electron,
// download, crypto, fetch, timers and Math.random, installed on `globalThis.__asrsVideo` where
// the runtime bundle (bundle.ts) picks them up. Every call upstream makes into the fakes is an
// effect, in call order. Anything asynchronous (a process's output, a dialog's answer, a download,
// a fetch, a menu, a timer, a timer-text read) waits in its own channel until the driver delivers
// it; `fs.promises` calls settle at once.

export type Platform = "darwin" | "linux" | "win32";

export type ProcScript =
  | { kind: "run"; events: (["stdout" | "stderr", string] | ["stdoutHex" | "stderrHex", string])[]; exit: number }
  | { kind: "enoent" }
  | { kind: "hang"; events?: (["stdout" | "stderr", string])[] };

export type DownloadScript = { ok: true; data: Buffer } | { ok: false };
export type FetchScript = { network: true } | { status: number; body: string | Buffer };
export type MenuScript = ({ click: number } | { close: true })[];
export type FsOp = "rm" | "mkdir" | "readFile" | "readFileSync" | "rename" | "unlink" | "chmod";

export interface Script {
  platform: Platform;
  arch: string;
  seed: number;
  files?: { [path: string]: string | Buffer };
  dirs?: string[];
  fail?: { op: FsOp; path: string; code: string }[];
  clipboard?: string;
  destroyed?: boolean;
  random?: number[];
  process?: (cmd: string, args: string[], index: number) => ProcScript;
  messageBox?: (options: any, index: number) => number;
  openDialog?: (options: any, index: number) => { canceled: boolean; filePaths: string[] };
  download?: (url: string, index: number) => DownloadScript;
  fetch?: (url: string, index: number) => FetchScript;
  menu?: (labels: string[], index: number) => MenuScript;
  timerText?: (frame: number, path: string) => string;
}

export const PLATFORM_PATHS: { [p in Platform]: { userData: string; temp: string; dirname: string } } = {
  darwin: {
    userData: "/Users/frc/Library/Application Support/AdvantageScope",
    temp: "/var/folders/xy/4k2qz8_n1x3c0000gn/T",
    dirname: "/Applications/AdvantageScope.app/Contents/Resources/app.asar/bundles"
  },
  linux: {
    userData: "/home/frc/.config/AdvantageScope",
    temp: "/tmp",
    dirname: "/opt/AdvantageScope/resources/app.asar/bundles"
  },
  win32: {
    userData: "C:\\Users\\frc\\AppData\\Roaming\\AdvantageScope",
    temp: "C:\\Users\\frc\\AppData\\Local\\Temp",
    dirname: "C:\\Program Files\\AdvantageScope\\resources\\app.asar\\bundles"
  }
};

export const pathFor = (platform: Platform) => (platform === "win32" ? nodePath.win32 : nodePath.posix);

export const GENUINE_PREFIX = "GENUINE-FFMPEG:";

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

type Delivery = { record: any; run: () => void };

function errorTree(error: any) {
  return { name: String(error?.name), code: error?.code === undefined ? null : String(error.code) };
}

function fsError(code: string, syscall: string, p: string) {
  const messages: { [code: string]: string } = {
    ENOENT: "no such file or directory",
    EACCES: "permission denied",
    EBUSY: "resource busy or locked",
    ENOSPC: "no space left on device",
    EPERM: "operation not permitted"
  };
  return Object.assign(new Error(`${code}: ${messages[code] ?? "error"}, ${syscall} '${p}'`), {
    errno: -1,
    code,
    syscall,
    path: p
  });
}

export class World {
  readonly script: Script;
  readonly paths: { userData: string; temp: string; dirname: string };
  readonly path: typeof nodePath.posix;
  effects: any[] = [];
  private nextId = 0;
  private channels = new Map<number, Delivery[]>();
  private order = 0;
  private channelOrder = new Map<number, number>();
  private rng: () => number;
  private randomRng: () => number;
  private randomIndex = 0;
  private files = new Map<string, Buffer>();
  private dirs = new Set<string>();
  private counts: { [kind: string]: number } = {};
  readonly window: any;
  readonly glue: any;

  constructor(script: Script) {
    this.script = script;
    this.paths = PLATFORM_PATHS[script.platform];
    this.path = pathFor(script.platform);
    this.rng = mulberry32(script.seed);
    this.randomRng = mulberry32(script.seed ^ 0x5bd1e995);
    for (const [p, data] of Object.entries(script.files ?? {})) {
      this.files.set(p, Buffer.isBuffer(data) ? Buffer.from(data) : Buffer.from(data, "utf8"));
      this.addParents(p);
    }
    for (const d of script.dirs ?? []) this.addDir(d);
    const world = this;
    this.window = {
      isDestroyed() {
        const r = script.destroyed === true;
        world.effect({ k: "isDestroyed", r });
        return r;
      }
    };
    this.glue = {
      electron: this.electron(),
      fs: this.fs(),
      childProcess: { spawn: (cmd: string, args: unknown[], opts?: any) => this.spawn(cmd, args, opts) },
      download: (url: string, folder: string, opts: any) => this.download(url, folder, opts),
      tesseract: {
        createScheduler: () => ({ addWorker() {}, addJob: () => new Promise(() => {}) }),
        createWorker: () => new Promise(() => {})
      },
      crypto: { createHash: (alg: string) => this.createHash(alg) },
      fetch: (url: string, init: any) => this.fetch(url, init),
      abortTimeout: (ms: number) => ({ asrsTimeoutMs: ms }),
      setTimeout: (fn: () => void, ms: number) => this.setTimeout(fn, ms),
      random: () => this.random(),
      console: {
        log: (...args: unknown[]) => this.log("log", args),
        warn: (...args: unknown[]) => this.log("warn", args),
        error: (...args: unknown[]) => this.log("error", args)
      }
    };
  }

  effect(e: any) {
    this.effects.push(e);
  }

  takeEffects() {
    const out = this.effects;
    this.effects = [];
    return out;
  }

  private count(kind: string) {
    const n = this.counts[kind] ?? 0;
    this.counts[kind] = n + 1;
    return n;
  }

  private open(): number {
    const id = this.nextId++;
    this.channels.set(id, []);
    this.channelOrder.set(id, this.order++);
    return id;
  }

  private queue(id: number, delivery: Delivery) {
    this.channels.get(id)!.push(delivery);
  }

  pending(): number {
    let n = 0;
    for (const list of this.channels.values()) if (list.length > 0) n++;
    return n;
  }

  /** Delivers the head of one channel chosen at random among the non-empty ones. */
  deliverOne(): any | null {
    const ready = [...this.channels.entries()]
      .filter(([, list]) => list.length > 0)
      .sort((a, b) => this.channelOrder.get(a[0])! - this.channelOrder.get(b[0])!);
    if (ready.length === 0) return null;
    const [, list] = ready[Math.floor(this.rng() * ready.length)];
    const delivery = list.shift()!;
    delivery.run();
    return delivery.record;
  }

  private normal(p: string) {
    const sep = this.path.sep;
    let out = p;
    while (out.length > 1 && out.endsWith(sep) && !/^[A-Za-z]:\\$/.test(out)) out = out.slice(0, -1);
    return out;
  }

  private addDir(p: string) {
    let current = this.normal(p);
    while (true) {
      this.dirs.add(current);
      const parent = this.path.dirname(current);
      if (parent === current) break;
      current = parent;
    }
  }

  private addParents(p: string) {
    this.addDir(this.path.dirname(p));
  }

  private failure(op: FsOp, p: string) {
    const hit = this.script.fail?.find((f) => f.op === op && this.normal(f.path) === this.normal(p));
    return hit?.code ?? null;
  }

  private fs() {
    const world = this;
    const exists = (p: string) => world.files.has(world.normal(p)) || world.dirs.has(world.normal(p));
    const removeTree = (p: string) => {
      const root = world.normal(p);
      const prefix = root + world.path.sep;
      for (const key of [...world.files.keys()]) if (key === root || key.startsWith(prefix)) world.files.delete(key);
      for (const key of [...world.dirs]) if (key === root || key.startsWith(prefix)) world.dirs.delete(key);
    };
    return {
      existsSync(p: string) {
        const r = exists(p);
        world.effect({ k: "exists", path: p, r });
        return r;
      },
      rmSync(p: string, opts: any) {
        const e: any = { k: "rmSync", path: p, options: toTree(opts) };
        world.effect(e);
        const code = world.failure("rm", p) ?? (exists(p) ? null : "ENOENT");
        if (code !== null) {
          const error = fsError(code, code === "ENOENT" ? "lstat" : "rm", p);
          e.throw = errorTree(error);
          throw error;
        }
        removeTree(p);
      },
      mkdirSync(p: string, opts: any) {
        const e: any = { k: "mkdirSync", path: p, options: toTree(opts) };
        world.effect(e);
        const code = world.failure("mkdir", p);
        if (code !== null) {
          const error = fsError(code, "mkdir", p);
          e.throw = errorTree(error);
          throw error;
        }
        world.addDir(p);
      },
      readFileSync(p: string, opts: any) {
        const e: any = { k: "readFileSync", path: p, options: toTree(opts) };
        world.effect(e);
        const code = world.failure("readFileSync", p) ?? (world.files.has(world.normal(p)) ? null : "ENOENT");
        if (code !== null) {
          const error = fsError(code, "open", p);
          e.throw = errorTree(error);
          throw error;
        }
        const data = Buffer.from(world.files.get(world.normal(p))!);
        e.r = data.toString("hex");
        return data;
      },
      promises: {
        readFile(p: string) {
          const e: any = { k: "readFile", path: p };
          world.effect(e);
          const code = world.failure("readFile", p) ?? (world.files.has(world.normal(p)) ? null : "ENOENT");
          if (code !== null) {
            const error = fsError(code, "open", p);
            e.throw = errorTree(error);
            return Promise.reject(error);
          }
          const data = Buffer.from(world.files.get(world.normal(p))!);
          e.r = data.toString("hex");
          return Promise.resolve(data);
        },
        unlink(p: string) {
          const e: any = { k: "unlink", path: p };
          world.effect(e);
          const code = world.failure("unlink", p) ?? (world.files.has(world.normal(p)) ? null : "ENOENT");
          if (code !== null) {
            const error = fsError(code, "unlink", p);
            e.throw = errorTree(error);
            return Promise.reject(error);
          }
          world.files.delete(world.normal(p));
          return Promise.resolve();
        },
        rename(from: string, to: string) {
          const e: any = { k: "rename", from, to };
          world.effect(e);
          const code = world.failure("rename", from) ?? (world.files.has(world.normal(from)) ? null : "ENOENT");
          if (code !== null) {
            const error = fsError(code, "rename", from);
            e.throw = errorTree(error);
            return Promise.reject(error);
          }
          const data = world.files.get(world.normal(from))!;
          world.files.delete(world.normal(from));
          world.files.set(world.normal(to), data);
          return Promise.resolve();
        },
        chmod(p: string, mode: number) {
          const e: any = { k: "chmod", path: p, mode };
          world.effect(e);
          const code = world.failure("chmod", p) ?? (exists(p) ? null : "ENOENT");
          if (code !== null) {
            const error = fsError(code, "chmod", p);
            e.throw = errorTree(error);
            return Promise.reject(error);
          }
          return Promise.resolve();
        }
      }
    };
  }

  private electron() {
    const world = this;
    const script = this.script;
    class MenuItem {
      label: string;
      click: () => void;
      constructor(options: any) {
        this.label = options.label;
        this.click = options.click;
      }
    }
    class Menu {
      items: MenuItem[] = [];
      listeners: { [event: string]: (() => void)[] } = {};
      append(item: MenuItem) {
        this.items.push(item);
      }
      addListener(event: string, fn: () => void) {
        (this.listeners[event] ??= []).push(fn);
        return this;
      }
      popup(options: any) {
        const id = world.open();
        const labels = this.items.map((item) => item.label);
        world.effect({ k: "menu", id, labels: toTree(labels), x: toTree(options.x), y: toTree(options.y) });
        const plan = script.menu?.(labels, world.count("menu")) ?? [{ close: true }];
        for (const step of plan) {
          if ("click" in step) {
            world.queue(id, {
              record: { e: "menuClick", id, index: step.click },
              run: () => this.items[step.click].click()
            });
          } else {
            world.queue(id, {
              record: { e: "menuClose", id },
              run: () => (this.listeners["menu-will-close"] ?? []).forEach((fn) => fn())
            });
          }
        }
      }
    }
    return {
      app: {
        isPackaged: true,
        getPath(name: string) {
          if (name === "userData") return world.paths.userData;
          if (name === "temp") return world.paths.temp;
          throw new Error("unexpected getPath " + name);
        },
        getVersion: () => "27.0.0-alpha-6"
      },
      clipboard: {
        readText() {
          const r = script.clipboard ?? "";
          world.effect({ k: "clipboard", r });
          return r;
        }
      },
      dialog: {
        showMessageBox(_window: unknown, options: any) {
          const id = world.open();
          world.effect({ k: "messageBox", id, options: toTree(options) });
          const response = script.messageBox?.(options, world.count("messageBox")) ?? 0;
          return new Promise((resolve) => {
            world.queue(id, {
              record: { e: "messageBox", id, response },
              run: () => resolve({ response, checkboxChecked: false })
            });
          });
        },
        showOpenDialog(_window: unknown, options: any) {
          const id = world.open();
          world.effect({ k: "openDialog", id, options: toTree(options) });
          const result = script.openDialog?.(options, world.count("openDialog")) ?? { canceled: true, filePaths: [] };
          return new Promise((resolve) => {
            world.queue(id, {
              record: { e: "openDialog", id, canceled: result.canceled, filePaths: result.filePaths },
              run: () => resolve({ canceled: result.canceled, filePaths: result.filePaths })
            });
          });
        }
      },
      Menu,
      MenuItem,
      BrowserWindow: class {}
    };
  }

  private spawn(cmd: string, rawArgs: unknown[], opts?: any) {
    const args = rawArgs.map((a) => String(a));
    const cwd = opts?.cwd === undefined ? null : String(opts.cwd);
    const bad = rawArgs.findIndex((a) => typeof a === "string" && a.includes("\u0000"));
    if (bad >= 0) {
      const error = Object.assign(
        new TypeError(`The argument 'args[${bad}]' must be a string without null bytes. Received ${JSON.stringify(rawArgs[bad])}`),
        { code: "ERR_INVALID_ARG_VALUE" }
      );
      this.effect({ k: "spawn", cmd, args, cwd, throw: errorTree(error) });
      throw error;
    }
    const id = this.open();
    this.effect({ k: "spawn", id, cmd, args, cwd });
    const plan = this.script.process?.(cmd, args, this.count("spawn")) ?? { kind: "hang" };
    const world = this;
    class FakeChildProcess extends EventEmitter {
      stdout = new EventEmitter();
      stderr = new EventEmitter();
      exitCode: number | null = null;
      signalCode: string | null = null;
      killed = false;
      spawnfile = cmd;
      spawnargs = [cmd, ...args];
      exited = false;
      kill() {
        const r = !this.exited;
        world.effect({ k: "kill", id, r });
        if (!r) return false;
        this.killed = true;
        this.exited = true;
        world.channels.set(id, []);
        world.queue(id, {
          record: { e: "exit", id, code: null, signal: "SIGTERM" },
          run: () => {
            this.signalCode = "SIGTERM";
            this.emit("exit", null, "SIGTERM");
          }
        });
        world.queue(id, { record: { e: "close", id, code: null, signal: "SIGTERM" }, run: () => this.emit("close", null, "SIGTERM") });
        return true;
      }
      ref() {}
      unref() {}
    }
    const child = new FakeChildProcess();
    const data = (stream: "stdout" | "stderr", bytes: Buffer) =>
      this.queue(id, {
        record: { e: stream, id, data: bytes.toString("hex") },
        run: () => child[stream].emit("data", Buffer.from(bytes))
      });
    if (plan.kind === "enoent") {
      child.exited = true;
      this.queue(id, {
        record: { e: "error", id, code: "ENOENT", errno: -2, syscall: "spawn " + cmd, path: cmd },
        run: () => {
          child.exitCode = -2;
          child.emit(
            "error",
            Object.assign(new Error(`spawn ${cmd} ENOENT`), { errno: -2, code: "ENOENT", syscall: "spawn " + cmd, path: cmd, spawnargs: args })
          );
        }
      });
      this.queue(id, { record: { e: "close", id, code: -2, signal: null }, run: () => child.emit("close", -2, null) });
    } else {
      for (const [kind, text] of plan.events ?? []) {
        if (kind === "stdoutHex" || kind === "stderrHex") data(kind === "stdoutHex" ? "stdout" : "stderr", Buffer.from(text, "hex"));
        else data(kind, Buffer.from(text, "latin1"));
      }
      if (plan.kind === "run") {
        const code = plan.exit;
        this.queue(id, {
          record: { e: "exit", id, code, signal: null },
          run: () => {
            child.exited = true;
            child.exitCode = code;
            child.emit("exit", code, null);
          }
        });
        this.queue(id, { record: { e: "close", id, code, signal: null }, run: () => child.emit("close", code, null) });
      }
    }
    return child;
  }

  private download(url: string, folder: string, opts: any) {
    const id = this.open();
    this.effect({ k: "download", id, url, folder, options: toTree(opts) });
    const plan = this.script.download?.(url, this.count("download")) ?? { ok: false };
    return new Promise((resolve, reject) => {
      if (plan.ok) {
        const data = plan.data;
        this.queue(id, {
          record: { e: "download", id, ok: true },
          run: () => {
            const target = this.path.join(folder, opts.filename);
            this.files.set(this.normal(target), Buffer.from(data));
            this.addParents(target);
            resolve(Buffer.from(data));
          }
        });
      } else {
        this.queue(id, {
          record: { e: "download", id, ok: false },
          run: () => reject(Object.assign(new Error("Response code 404 (Not Found)"), { name: "HTTPError" }))
        });
      }
    });
  }

  private createHash(alg: string) {
    const world = this;
    const chunks: Buffer[] = [];
    return {
      update(data: Buffer) {
        chunks.push(Buffer.from(data));
        return this;
      },
      digest(encoding: string) {
        const data = Buffer.concat(chunks);
        const text = data.toString("latin1");
        const r = text.startsWith(GENUINE_PREFIX)
          ? text.slice(GENUINE_PREFIX.length)
          : nodeCrypto.createHash(alg).update(data).digest(encoding as "hex");
        world.effect({ k: "hash", alg, encoding, data: data.toString("hex"), r });
        return r;
      }
    };
  }

  private fetch(url: string, init: any) {
    const id = this.open();
    this.effect({
      k: "fetch",
      id,
      url,
      method: toTree(init?.method),
      headers: toTree(init?.headers),
      timeout: toTree(init?.signal?.asrsTimeoutMs)
    });
    const plan = this.script.fetch?.(url, this.count("fetch")) ?? { network: true };
    return new Promise((resolve, reject) => {
      if ("network" in plan) {
        this.queue(id, {
          record: { e: "fetch", id, ok: false },
          run: () => reject(new TypeError("fetch failed"))
        });
      } else {
        const body = Buffer.isBuffer(plan.body) ? plan.body : Buffer.from(plan.body, "utf8");
        this.queue(id, {
          record: { e: "fetch", id, ok: true, status: plan.status, body: body.toString("hex") },
          run: () => resolve(new Response(new Uint8Array(body), { status: plan.status }))
        });
      }
    });
  }

  private setTimeout(fn: () => void, ms: number) {
    const id = this.open();
    this.effect({ k: "setTimeout", id, ms: toTree(ms) });
    this.queue(id, { record: { e: "timer", id }, run: () => fn() });
    return id;
  }

  private random() {
    const scripted = this.script.random;
    const r = scripted ? scripted[this.randomIndex++ % scripted.length] : this.randomRng();
    this.effect({ k: "random", r: toTree(r) });
    return r;
  }

  private log(level: string, args: unknown[]) {
    this.effect({
      k: "log",
      level,
      args: args.map((a) => {
        if (typeof a === "string") return { s: a };
        if (a instanceof Error) return { error: a.name };
        throw new Error("unexpected console argument " + String(a));
      })
    });
  }

  readTimerText(imagePath: string, width: number, height: number): Promise<string> {
    const id = this.open();
    this.effect({ k: "timerText", id, path: imagePath, width: toTree(width), height: toTree(height) });
    const frame = Number(imagePath.slice(-12, -4));
    const text = this.script.timerText?.(frame, imagePath) ?? "";
    return new Promise((resolve) => {
      this.queue(id, { record: { e: "timerText", id, text }, run: () => resolve(text) });
    });
  }
}
