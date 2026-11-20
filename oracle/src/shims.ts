import "./environment";
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
const dispatch = (data) => {
  const event = { data, target: globalThis, currentTarget: globalThis };
  if (typeof globalThis.onmessage === "function") globalThis.onmessage(event);
  for (const fn of listeners.slice()) fn(event);
};
parentPort.on("message", (data) => (ready ? dispatch(data) : queued.push(data)));
import(workerData.url).then(
  () => { ready = true; queued.splice(0).forEach(dispatch); },
  (error) => { parentPort.postMessage({ __oracleWorkerError: String(error && error.stack || error) }); }
);
`;

type Listener = (event: any) => void;

export class OracleWorker {
  onmessage: Listener | null = null;
  onerror: Listener | null = null;
  private listeners: { [type: string]: Listener[] } = {};
  private thread: ThreadWorker;

  constructor(script: string | URL, _options?: { type?: string; name?: string }) {
    this.thread = new ThreadWorker(WORKER_BOOTSTRAP, {
      eval: true,
      workerData: { url: resolveWorkerScript(script) }
    });
    this.thread.on("message", (data: any) => {
      if (data !== null && typeof data === "object" && "__oracleWorkerError" in data) {
        this.emit("error", { message: data.__oracleWorkerError, error: data.__oracleWorkerError });
        return;
      }
      this.emit("message", { data, target: this, currentTarget: this });
    });
    this.thread.on("error", (error) => this.emit("error", { message: String(error), error }));
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
    void this.thread.terminate();
  }
}

g.Worker = OracleWorker;
