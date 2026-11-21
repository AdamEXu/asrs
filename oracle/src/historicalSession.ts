import "./shims";
import * as fs from "node:fs";
import { HistoricalDataSource, HistoricalDataSourceStatus } from "@upstream/hub/dataSources/HistoricalDataSource";
import Log from "@upstream/shared/log/Log";
import { encode } from "./cve1";
import { dumpLog } from "./historical/dump";
import { MainProcess } from "./historical/mainProcess";
import { FakeTimers, realClearTimeout, realSetImmediate, realSetTimeout } from "./historical/timers";
import { EXITED, OracleWorker, WorkerRouter, resolveWorkerScript, setWorkerRouter, terminatedWorkersExited } from "./shims";

const USAGE = `usage: historicalSession <sessions.jsonl> [--stall-ms N]

Runs scripted hub sessions against the real HistoricalDataSource and worker bundles and prints a
CVE-1 trace per session (see oracle/README.md):
  {"session": name}, then one line per step, callback, main message, worker creation, worker
  request and termination, page throw and dump, then {"end": name}.
Each input line is {"name", "steps": [step, ...]}; a step is one of
  {"op": "open", "path", "prefix"}       new source on the session's Log, as startHistorical does
  {"op": "settle"}                       run page work until nothing is queued
  {"op": "tick"}                         fire every 50 ms interval once
  {"op": "frame", "ms"}                  advance the page clock and run animation frames
  {"op": "pick", "indices", "parents"}   active tab fields: keys at those getFieldKeys() indices
                                         (negative from the end, modulo the count) and the parent
                                         paths of the keys at "parents"
  {"op": "loadAll"}                      loadAllFields() on every open source
  {"op": "loadEverything"}               tick and loadAll until a round posts nothing
  {"op": "stop", "source"?}              stop one source, or stop all and drop them from the
                                         hub's list (no more main replies or loadAll)
  {"op": "dump", "phase"}                canonical Log dump
Every op but dump settles afterwards unless it has "settle": false. A worker that sends nothing
for --stall-ms (default 30000) while busy counts as hung: it is killed and never heard from again,
as a worker that loops forever or dies.`;

const g = globalThis as any;
const PAGE_EPOCH = 1_700_000_000_000;
const WORKER_EPOCH = 1_000_000_000_000;

function write(lines: string[]): void {
  const bytes = Buffer.from(lines.map((l) => l + "\n").join(""), "utf8");
  let offset = 0;
  while (offset < bytes.length) {
    try {
      offset += fs.writeSync(1, bytes, offset);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EAGAIN") throw error;
    }
  }
}

function trace(value: object): void {
  write([encode(value)]);
}

function turn(): Promise<void> {
  return new Promise((resolve) => realSetImmediate(resolve));
}

/** The worker realm's `Date` is pinned as in decodeWorker.ts; asrs-cli pins the same value. */
function pinnedLoader(script: string): string {
  return (
    "data:text/javascript," +
    encodeURIComponent(`
const RealDate = globalThis.Date;
const FROZEN = ${WORKER_EPOCH};
globalThis.Date = new Proxy(RealDate, {
  construct: (target, args) => (args.length === 0 ? new RealDate(FROZEN) : Reflect.construct(target, args)),
  get: (target, property, receiver) => (property === "now" ? () => FROZEN : Reflect.get(target, property, receiver))
});
await import(${JSON.stringify(resolveWorkerScript(script))});
`)
  );
}

const killWorker = (worker: OracleWorker) => OracleWorker.prototype.terminate.call(worker);

interface Slot {
  worker: OracleWorker;
  inbox: unknown[];
  wake: (() => void) | null;
  hung: boolean;
}

type Step = { op: string; [key: string]: any };

class Session {
  private log = new Log();
  private timers = new FakeTimers();
  private slots: Slot[] = [];
  private roots: HistoricalDataSource[] = [];
  private statuses: HistoricalDataSourceStatus[] = [];
  private open: number[] = [];
  private active: string[] = [];
  private frames = new Map<number, (time: number) => void>();
  private nextFrame = 1;
  private pageNow = PAGE_EPOCH;
  private saved: { [name: string]: unknown } = {};

  constructor(private stallMs: number) {}

  install(): void {
    for (const name of ["Worker", "Date", "requestAnimationFrame", "sendMainMessage", "log", "tabs", "sidebar"]) {
      this.saved[name] = g[name];
    }
    this.timers.install();
    setWorkerRouter(this.router());
    const session = this;
    g.Worker = class extends OracleWorker {
      private index: number;
      constructor(script: string, options?: { type?: string }) {
        super(pinnedLoader(script), options);
        this.index = session.slots.length - 1;
        trace({ event: "worker", worker: this.index, script: script.split("/").pop() });
      }
      postMessage(data: any, transfer?: any): void {
        if (data.type === "start") {
          const files = data.data.map((file: Uint8Array | null) => (file === null ? null : file.length));
          trace({ event: "post", worker: this.index, type: "start", files });
        } else {
          trace({ event: "post", worker: this.index, type: data.type, key: data.key });
        }
        super.postMessage(data, transfer);
      }
      terminate(): void {
        trace({ event: "terminate", worker: this.index });
        super.terminate();
      }
    };
    const RealDate = this.saved.Date as DateConstructor;
    g.Date = new Proxy(RealDate, {
      construct: (target, args) => (args.length === 0 ? new RealDate(this.pageNow) : Reflect.construct(target, args)),
      get: (target, property, receiver) => (property === "now" ? () => this.pageNow : Reflect.get(target, property, receiver))
    });
    g.requestAnimationFrame = (callback: (time: number) => void) => {
      const id = this.nextFrame++;
      this.frames.set(id, callback);
      return id;
    };
    const main = new MainProcess((message) => {
      for (const i of this.open.slice()) this.roots[i].handleMainMessage(message);
    });
    g.sendMainMessage = (name: string, data: any) => {
      if (name === "historical-start") {
        trace({ event: "main", name, path: data.path });
      } else if (name === "historical-start-raw") {
        trace({ event: "main", name, extension: data.extension, bytes: data.data.length });
      } else {
        trace({ event: "main", name });
      }
      main.handle(name, data);
    };
    g.log = this.log;
    g.tabs = { getActiveFields: () => new Set(this.active) };
    g.sidebar = { getActiveFields: () => new Set<string>() };
  }

  async uninstall(): Promise<void> {
    for (const slot of this.slots) killWorker(slot.worker);
    setWorkerRouter(null);
    this.timers.uninstall();
    Object.assign(g, this.saved);
    await terminatedWorkersExited();
  }

  private router(): WorkerRouter {
    return {
      created: (worker) => this.slots.push({ worker, inbox: [], wake: null, hung: false }),
      received: (worker, raw) => {
        const slot = this.slots.find((s) => s.worker === worker);
        if (slot === undefined) return;
        if (raw !== EXITED) slot.inbox.push(raw);
        const wake = slot.wake;
        slot.wake = null;
        wake?.();
      }
    };
  }

  /** A browser reports a throw out of a task and goes on with the next one. */
  private task(run: () => void): void {
    try {
      run();
    } catch {
      trace({ event: "threw" });
    }
  }

  private activity(): string {
    return `${this.slots.length}:${this.slots.reduce((sum, s) => sum + s.worker.posted, 0)}`;
  }

  async settle(): Promise<void> {
    for (;;) {
      await turn();
      await terminatedWorkersExited();
      let ran = false;
      this.task(() => (ran = this.timers.runNextTimeout()));
      if (ran) continue;
      const slot = this.slots.find((s) => !s.hung && (s.inbox.length > 0 || s.worker.busy));
      if (slot === undefined) return;
      if (slot.inbox.length === 0) {
        if (!(await this.waitFor(slot))) {
          slot.hung = true;
          killWorker(slot.worker);
        }
        continue;
      }
      const raw = slot.inbox.shift();
      this.task(() => slot.worker.deliver(raw));
    }
  }

  private waitFor(slot: Slot): Promise<boolean> {
    return new Promise((resolve) => {
      const timer = realSetTimeout(() => {
        slot.wake = null;
        resolve(false);
      }, this.stallMs);
      slot.wake = () => {
        realClearTimeout(timer);
        resolve(true);
      };
    });
  }

  private tickIntervals(): void {
    const intervals: { callback: () => void }[] = (this.timers as any).intervals;
    for (const interval of intervals.slice()) {
      if (intervals.includes(interval)) this.task(() => interval.callback());
    }
  }

  private loadAll(): void {
    for (const i of this.open.slice()) {
      let promise: Promise<void> | undefined;
      this.task(() => (promise = this.roots[i].loadAllFields()));
      promise?.then(() => trace({ event: "loaded", source: i }));
    }
  }

  private pick(indices: number[], parents: number[]): string[] {
    const keys = this.log.getFieldKeys();
    if (keys.length === 0) return [];
    const at = (i: number) => keys[((i % keys.length) + keys.length) % keys.length];
    const picked = indices.map(at);
    for (const i of parents) {
      const key = at(i);
      const slash = key.lastIndexOf("/");
      if (slash > 0) picked.push(key.slice(0, slash));
    }
    return picked;
  }

  async step(index: number, step: Step): Promise<void> {
    await terminatedWorkersExited();
    trace({ step: index, op: step.op });
    switch (step.op) {
      case "open": {
        const i = this.roots.length;
        const source = new HistoricalDataSource();
        this.roots.push(source);
        this.statuses.push(HistoricalDataSourceStatus.Waiting);
        this.open.push(i);
        this.task(() =>
          source.openFile(
            this.log,
            step.path,
            step.prefix,
            (status) => {
              this.statuses[i] = status;
              const name = HistoricalDataSourceStatus[status];
              if (status === HistoricalDataSourceStatus.Error) {
                trace({ event: "status", source: i, status: name, customError: source.getCustomError() });
              } else {
                trace({ event: "status", source: i, status: name });
              }
            },
            (progress) => trace({ event: "progress", source: i, value: progress }),
            (hasNewFields) => trace({ event: "refresh", source: i, hasNewFields })
          )
        );
        break;
      }
      case "settle":
        break;
      case "tick":
        this.tickIntervals();
        break;
      case "frame": {
        this.pageNow += step.ms;
        const callbacks = Array.from(this.frames.values());
        this.frames.clear();
        for (const callback of callbacks) this.task(() => callback(this.pageNow));
        break;
      }
      case "pick":
        this.active = this.pick(step.indices, step.parents);
        trace({ event: "active", fields: this.active });
        break;
      case "loadAll":
        this.loadAll();
        break;
      case "loadEverything":
        for (;;) {
          const before = this.activity();
          for (;;) {
            const tickStart = this.activity();
            this.tickIntervals();
            await this.settle();
            if (this.activity() === tickStart) break;
          }
          this.loadAll();
          await this.settle();
          if (this.activity() === before) break;
        }
        break;
      case "stop": {
        const stopped = step.source === undefined ? this.open.slice() : [step.source];
        if (step.source === undefined) this.open = [];
        for (const i of stopped) this.task(() => this.roots[i].stop());
        break;
      }
      case "dump": {
        const loading = new Set<string>();
        for (const i of this.open) this.roots[i].getLoadingFields().forEach((field) => loading.add(field));
        write(
          dumpLog(this.log.toSerialized(), {
            phase: step.phase,
            status: this.statuses.map((s) => HistoricalDataSourceStatus[s]).join(","),
            loadingFields: Array.from(loading)
          })
        );
        return;
      }
      default:
        throw new Error(`unknown op ${JSON.stringify(step.op)}`);
    }
    if (step.settle !== false) await this.settle();
  }
}

const file = process.argv[2];
if (file === undefined || file.startsWith("-")) {
  console.error(USAGE);
  process.exit(2);
}
const stallIndex = process.argv.indexOf("--stall-ms");
const stallMs = stallIndex > 0 ? Number(process.argv[stallIndex + 1]) : 30_000;

for (const line of fs.readFileSync(file, "utf8").split("\n")) {
  if (line.trim().length === 0) continue;
  const spec: { name: string; steps: Step[] } = JSON.parse(line);
  trace({ session: spec.name });
  const session = new Session(stallMs);
  try {
    session.install();
    for (let i = 0; i < spec.steps.length; i++) await session.step(i, spec.steps[i]);
  } catch (error) {
    trace({ error: String(error) });
  } finally {
    await session.uninstall();
  }
  trace({ end: spec.name });
}
