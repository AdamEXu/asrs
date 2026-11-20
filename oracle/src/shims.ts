import { pathToFileURL } from "node:url";
import * as path from "node:path";
import { Worker as ThreadWorker } from "node:worker_threads";

declare const __UPSTREAM_DIR__: string;

export const UPSTREAM_DIR: string = process.env.ORACLE_UPSTREAM_DIR ?? __UPSTREAM_DIR__;

const g = globalThis as any;

g.window ??= globalThis;
g.self ??= globalThis;

type FrameCallback = (time: number) => void;
let nextFrameHandle = 1;
const pendingFrames = new Map<number, FrameCallback>();

g.requestAnimationFrame = (callback: FrameCallback): number => {
  const handle = nextFrameHandle++;
  pendingFrames.set(handle, callback);
  return handle;
};

g.cancelAnimationFrame = (handle: number): void => {
  pendingFrames.delete(handle);
};

export function flushAnimationFrames(time: number): number {
  const callbacks = Array.from(pendingFrames.values());
  pendingFrames.clear();
  callbacks.forEach((callback) => callback(time));
  return callbacks.length;
}

export const postedMessages: unknown[] = [];
g.postMessage ??= (message: unknown) => {
  postedMessages.push(message);
};

let workerBase = path.join(UPSTREAM_DIR, "www");

export function setWorkerBase(dir: string): void {
  workerBase = dir;
}

export function resolveWorkerScript(script: string | URL): string {
  if (script instanceof URL) return script.href;
  if (/^[a-z]+:/i.test(script)) return script;
  return pathToFileURL(path.resolve(workerBase, script)).href;
}

const WORKER_BOOTSTRAP = `
const { parentPort, workerData } = require("node:worker_threads");
const emitWarning = process.emitWarning;
process.emitWarning = function (warning, ...rest) {
  const code = typeof rest[0] === "object" && rest[0] !== null ? rest[0].code : rest[1];
  if (code === "MODULE_TYPELESS_PACKAGE_JSON") return;
  return emitWarning.call(this, warning, ...rest);
};
const listeners = [];
const queued = [];
let ready = false;
globalThis.self = globalThis;
globalThis.window = undefined;
globalThis.onmessage = null;
globalThis.postMessage = (data, transfer) => parentPort.postMessage(data, transfer);
globalThis.addEventListener = (type, fn) => { if (type === "message") listeners.push(fn); };
globalThis.removeEventListener = (type, fn) => {
  const i = listeners.indexOf(fn);
  if (type === "message" && i >= 0) listeners.splice(i, 1);
};
globalThis.close = () => process.exit(0);
const reportError = (error) => parentPort.postMessage({ __oracleWorkerError: String(error && error.stack || error) });
const dispatch = (data) => {
  const event = { data, target: globalThis, currentTarget: globalThis };
  const handlers = typeof globalThis.onmessage === "function" ? [globalThis.onmessage] : [];
  handlers.push(...listeners);
  const settled = handlers.map((fn) => {
    try {
      return Promise.resolve(fn(event)).catch(reportError);
    } catch (error) {
      reportError(error);
      return undefined;
    }
  });
  Promise.all(settled).then(() => parentPort.postMessage({ __oracleDone: true }));
};
parentPort.on("message", (data) => (ready ? dispatch(data) : queued.push(data)));
import(workerData.url).then(
  () => { ready = true; queued.splice(0).forEach(dispatch); },
  (error) => { parentPort.postMessage({ __oracleWorkerError: String(error && error.stack || error) }); }
);
`;

type Listener = (event: any) => void;

/**
 * Takes over delivery of worker messages, so a harness can decide the order in which messages
 * from several workers reach the main thread. Without a router each message is delivered when
 * it arrives.
 */
export interface WorkerRouter {
  created(worker: OracleWorker): void;
  received(worker: OracleWorker, raw: unknown): void;
}

let workerRouter: WorkerRouter | null = null;

export function setWorkerRouter(router: WorkerRouter | null): void {
  workerRouter = router;
}

export class OracleWorker {
  onmessage: Listener | null = null;
  onerror: Listener | null = null;
  private listeners: { [type: string]: Listener[] } = {};
  private thread: ThreadWorker;
  private pending = 0;
  private sent = 0;
  private terminated = false;
  private exited = false;
  readonly script: string;

  constructor(script: string | URL, _options?: { type?: string; name?: string }) {
    this.script = resolveWorkerScript(script);
    this.thread = new ThreadWorker(WORKER_BOOTSTRAP, {
      eval: true,
      workerData: { url: this.script }
    });
    const router = workerRouter;
    this.thread.on("message", (raw: unknown) => (router ? router.received(this, raw) : this.deliver(raw)));
    this.thread.on("error", (error) => this.emit("error", { message: String(error), error }));
    this.thread.on("exit", () => {
      this.exited = true;
      if (stoppingThreads.delete(this) && stoppingThreads.size === 0) {
        exitWaiters.splice(0).forEach((resolve) => resolve());
      }
      if (router) router.received(this, EXITED);
    });
    router?.created(this);
  }

  /** True while a posted message is still being handled inside the worker. */
  get busy(): boolean {
    return this.pending > 0 && !this.terminated && !this.exited;
  }

  /** How many messages the page has posted to this worker. */
  get posted(): number {
    return this.sent;
  }

  /** Hands one raw thread message to the page side, as the browser would. */
  deliver(raw: unknown): void {
    if (raw === EXITED) return;
    if (raw !== null && typeof raw === "object" && "__oracleDone" in raw) {
      this.pending--;
      return;
    }
    if (this.terminated) return;
    if (raw !== null && typeof raw === "object" && "__oracleWorkerError" in raw) {
      const message = (raw as any).__oracleWorkerError;
      this.emit("error", { message, error: message });
      return;
    }
    this.emit("message", { data: raw, target: this, currentTarget: this });
  }

  private emit(type: string, event: any) {
    const handler = type === "message" ? this.onmessage : type === "error" ? this.onerror : null;
    if (handler) handler.call(this, event);
    for (const listener of (this.listeners[type] ?? []).slice()) listener.call(this, event);
    if (type === "error" && !handler && !(this.listeners[type]?.length ?? 0)) {
      console.error(`oracle worker error: ${event.message}`);
    }
  }

  postMessage(data: unknown, transfer?: any): void {
    if (this.terminated) return;
    this.pending++;
    this.sent++;
    this.thread.postMessage(data, Array.isArray(transfer) ? transfer : transfer?.transfer);
  }

  addEventListener(type: string, listener: Listener): void {
    (this.listeners[type] ??= []).push(listener);
  }

  removeEventListener(type: string, listener: Listener): void {
    const list = this.listeners[type];
    if (list) this.listeners[type] = list.filter((l) => l !== listener);
  }

  terminate(): void {
    if (this.terminated) return;
    this.terminated = true;
    if (!this.exited) stoppingThreads.add(this);
    void this.thread.terminate();
  }
}

const stoppingThreads = new Set<OracleWorker>();
const exitWaiters: (() => void)[] = [];

/**
 * Resolves once every terminated worker thread has exited. Starting a worker while a terminated
 * one is still being torn down can abort the process (Node asserts `!flush_tasks_` in
 * `PerIsolatePlatformData`), so harnesses that create many workers wait here in between.
 */
export function terminatedWorkersExited(): Promise<void> {
  if (stoppingThreads.size === 0) return Promise.resolve();
  return new Promise((resolve) => exitWaiters.push(resolve));
}

/** Marker a router receives when a worker thread has exited. */
export const EXITED = Symbol("oracle worker exited");

g.Worker = OracleWorker;
