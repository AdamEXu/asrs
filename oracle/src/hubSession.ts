import { EXITED, OracleWorker, UPSTREAM_DIR, resolveWorkerScript, setWorkerRouter, terminatedWorkersExited } from "./shims";
import { fromTree, toTree } from "./cve1";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { realClearTimeout, realSetImmediate, realSetTimeout } from "./historical/timers";
import { Net, Trace } from "./live/net";
import { NT4Server } from "./live/nt4Server";
import { PageLoop } from "./live/pageLoop";
import { RLOGMain } from "./live/rlogMain";
import { MockServer, Outbox, ServerContext, ServerHost } from "./live/server";
import { FTCServer, PhoenixServer, RLOGServer } from "./live/simpleServers";

// hubSession [<spec or fixture file>]
//   Runs a `dom` fixture on the same page bundle (domPage.js: src/dom/env.ts, then the real
//   src/hub/hub.ts) and adds what the hub lifecycle needs: decode and export workers on real
//   worker threads, and main's side of `historical-start`. See oracle/README.md, "Hub
//   sessions".

declare const __HARNESS_DIR__: string;

const WORKER_EPOCH = 1_000_000_000_000;
const STALL_MS = 120_000;
const CONVERTED = [".hoot", ".revlog", ".wpilogxz"];

const file = process.argv[2] ?? process.env.ASRS_ORACLE_SPEC;
if (file === undefined) {
  process.stderr.write("usage: hubSession <spec or fixture file>\n");
  process.exit(2);
}
const parsed = JSON.parse(fs.readFileSync(file, "utf8"));
const isSpec = "fixture" in parsed && "entry" in parsed;
const fixture = isSpec ? parsed.fixture : parsed;
const baseDir = path.dirname(path.resolve(file));
const declared: string[] = isSpec ? (parsed.inputs ?? []).map((p: string) => path.resolve(baseDir, p)) : [];

/** Log paths in `open-files` are relative to the fixture; under a spec each must be an input. */
function readLog(logPath: string): Uint8Array | null {
  const target = path.resolve(baseDir, logPath);
  if (isSpec && fs.existsSync(target) && !declared.includes(target)) {
    process.stderr.write(`hubSession: ${file}: ${logPath} is not listed in "inputs"\n`);
    process.exit(2);
  }
  try {
    return new Uint8Array(fs.readFileSync(target));
  } catch {
    return null;
  }
}

const { JSDOM, VirtualConsole } = createRequire(path.join(__HARNESS_DIR__, "package.json"))("jsdom");
const log = (text: string) => process.stderr.write(text + "\n");
const lines: string[] = [];
const write = (record: object) => lines.push(JSON.stringify(record));
const virtualConsole = new VirtualConsole();
for (const level of ["log", "info", "debug", "error"]) {
  virtualConsole.on(level, (...args: unknown[]) => log(`page ${level}: ${args.map(String).join(" ")}`));
}
virtualConsole.on("warn", (...args: unknown[]) => {
  const text = args.map(String).join(" ");
  log(`page warn: ${text}`);
  if (text.startsWith("Failed to load: ")) write({ console: toTree("warn"), text: toTree(text) });
});
virtualConsole.on("jsdomError", (error: Error & { type?: string; cause?: unknown }) => {
  log(`jsdom: ${error.message}`);
  if (error.type === "not-implemented") return;
  const cause = (error.cause ?? error) as Error;
  write({ exception: "event", error: toTree(`${cause?.name}: ${cause?.message}`) });
});

/** The worker realm's `Date` is pinned as in historicalSession.ts; asrs tests pin the same value. */
function pinnedLoader(script: string): string {
  return (
    "data:text/javascript," +
    encodeURIComponent(`
process.on("unhandledRejection", () => {});
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

interface Slot {
  worker: OracleWorker;
  inbox: unknown[];
  wake: (() => void) | null;
  hung: boolean;
}
const slots: Slot[] = [];
setWorkerRouter({
  created: (worker) => slots.push({ worker, inbox: [], wake: null, hung: false }),
  received: (worker, raw) => {
    const slot = slots.find((s) => s.worker === worker);
    if (slot === undefined) return;
    if (raw !== EXITED) slot.inbox.push(raw);
    const wake = slot.wake;
    slot.wake = null;
    wake?.();
  }
});

const turn = () => new Promise<void>((resolve) => realSetImmediate(resolve));

function waitFor(slot: Slot): Promise<boolean> {
  return new Promise((resolve) => {
    const timer = realSetTimeout(() => {
      slot.wake = null;
      resolve(false);
    }, STALL_MS);
    slot.wake = () => {
      realClearTimeout(timer);
      resolve(true);
    };
  });
}

/**
 * Page work between tasks: let microtasks finish, then hand worker messages to the page one at
 * a time, always from the oldest worker that has one or is still busy, until no worker is busy.
 * Worker work takes no page time.
 */
async function settle(): Promise<void> {
  for (;;) {
    await turn();
    await terminatedWorkersExited();
    const slot = slots.find((s) => !s.hung && (s.inbox.length > 0 || s.worker.busy));
    if (slot === undefined) {
      live?.taskEnded();
      return;
    }
    if (slot.inbox.length === 0) {
      if (!(await waitFor(slot))) {
        slot.hung = true;
        OracleWorker.prototype.terminate.call(slot.worker);
      }
      continue;
    }
    const raw = slot.inbox.shift();
    try {
      slot.worker.deliver(raw);
    } catch (error) {
      log(`worker message: ${(error as Error)?.stack ?? String(error)}`);
      const e = error as Error;
      write({ exception: "worker", error: toTree(`${e?.name}: ${e?.message}`) });
    }
  }
}

const hubHtml = path.join(UPSTREAM_DIR, "www", "hub.html");
const dom = new JSDOM(fs.readFileSync(hubHtml, "utf8"), {
  url: pathToFileURL(hubHtml).href,
  runScripts: "outside-only",
  virtualConsole
});
const page = dom.window;

/** A copy built from the page realm's constructors, as a structured clone into the page gives. */
const adopt: (value: unknown) => unknown = page.eval(`(function adopt(value) {
  if (value === null || typeof value !== "object") return value;
  const tag = Object.prototype.toString.call(value);
  if (ArrayBuffer.isView(value)) return new globalThis[value.constructor.name](value);
  if (tag === "[object ArrayBuffer]") return new Uint8Array(new Uint8Array(value)).buffer;
  if (Array.isArray(value)) return Array.from(value, adopt);
  if (tag === "[object Map]") return new Map(Array.from(value, ([k, v]) => [adopt(k), adopt(v)]));
  if (tag === "[object Set]") return new Set(Array.from(value, adopt));
  if (tag === "[object Date]") return new Date(value.getTime());
  const out = {};
  for (const key of Object.keys(value)) out[key] = adopt(value[key]);
  return out;
})`);

/** The page's `Worker`: a real thread, reporting what the page does with it. */
class PageWorker {
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  private index: number;
  private worker: OracleWorker;

  constructor(script: string) {
    this.index = slots.length;
    this.worker = new OracleWorker(pinnedLoader(script));
    this.worker.onmessage = (event: { data: unknown }) => this.onmessage?.({ data: adopt(event.data) });
    write({ worker: toTree(this.index), script: toTree(script.split("/").pop()) });
  }

  postMessage(data: any) {
    if (data?.type === "start") {
      const files = data.data.map((f: Uint8Array | null) => (f === null ? null : f.length));
      write({ post: toTree(this.index), type: toTree("start"), files: toTree(files) });
    } else if (data?.type === "parseField") {
      write({ post: toTree(this.index), type: toTree("parseField"), key: toTree(data.key) });
    } else {
      write({ post: toTree(this.index), type: toTree("request") });
    }
    this.worker.postMessage(data);
  }

  terminate() {
    write({ terminate: toTree(this.index) });
    this.worker.terminate();
  }
}

/** Main's reply to `historical-start` (src/main/electron/main.ts). */
function historicalStart(uuid: string, logPath: string): unknown {
  if (CONVERTED.some((ext) => logPath.endsWith(ext))) throw new Error(`${logPath}: the oracle cannot convert this format`);
  if (logPath.endsWith(".dslog")) {
    const events = logPath.slice(0, logPath.length - 5) + "dsevents";
    return { uuid, files: [readLog(logPath), readLog(events)], error: null };
  }
  return { uuid, files: [readLog(logPath)], error: null };
}

let mainTimeout: (run: () => void, ms: number) => void;

/** Main answers the page's requests as later tasks; the page's port records them first. */
function pageWrite(line: string) {
  lines.push(line);
  const record = JSON.parse(line);
  if (record.main === "historical-start") {
    const data = fromTree(record.data) as { uuid: string; path: string };
    const reply = adopt(historicalStart(data.uuid, data.path));
    mainTimeout(() => page.messagePort.onmessage({ data: { name: "historical-data", data: reply } }), 0);
  } else if (record.main === "historical-start-raw") {
    throw new Error("historical-start-raw is not modelled");
  } else if (record.main === "live-rlog-start" || record.main === "live-rlog-stop") {
    if (live === null || rlogMain === null) throw new Error("an RLOG source needs the fixture's \"live\"");
    const data = record.data === undefined ? undefined : fromTree(record.data);
    const main = rlogMain;
    live.deliver(0, () => main.handle(record.main, data));
  }
}

const SERVERS: { [protocol: string]: new (out: Outbox, ctx: ServerContext, config: any) => MockServer } = {
  nt4: NT4Server,
  rlog: RLOGServer,
  phoenix: PhoenixServer,
  ftc: FTCServer
};
let live: PageLoop | null = null;
let rlogMain: RLOGMain | null = null;

/**
 * The fixture's `live` is a live session (`liveSession.ts`) with no client steps: its server
 * answers the page's WebSockets and requests and main's RLOG socket, on the page's clock.
 */
function installLive(session: any): void {
  const loop = new PageLoop(page, (error) => log(`live task: ${(error as Error)?.stack ?? String(error)}`));
  const outbox: { current: Outbox | null } = { current: null };
  const relay = new Proxy({} as Outbox, { get: (_t, name) => (outbox.current as any)[name].bind(outbox.current) });
  const server = new SERVERS[session.protocol](relay, { epochMs: loop.epochMs, now: () => loop.now }, session.server ?? {});
  const steps = (session.steps ?? []).map((step: any) => ({ t: step.t, op: step.server }));
  const latencyMs: number = session.latencyMs ?? 1;
  const host = new ServerHost(server, steps, latencyMs, () => loop.now);
  const trace = new Trace(loop, lines);
  const net = new Net(loop, host, trace, latencyMs, adopt as <T>(value: T) => T);
  outbox.current = net;
  loop.server = host;
  loop.install();
  net.install(page);
  rlogMain = new RLOGMain({ createConnection: (options) => net.createConnection(options) }, (_window, name, data) => {
    loop.deliver(0, () => page.messagePort.onmessage({ data: { name, data: adopt(data) } }));
    return true;
  });
  live = loop;
}

page.Worker = PageWorker;
page.__oracleFixture = page.JSON.parse(JSON.stringify(fixture));
page.__oracleYield = settle;
page.__oracleWrite = pageWrite;
page.__oracleLog = log;
page.__oracleOuter = { TextEncoder, TextDecoder };
process.on("unhandledRejection", (reason) => {
  const error = reason as Error;
  log(`unhandled rejection: ${error?.stack ?? String(reason)}`);
  if (live === null) write({ exception: "unhandledRejection", error: toTree(`${error?.name}: ${error?.message}`) });
});

const here = path.dirname(fileURLToPath(import.meta.url));
page.eval(fs.readFileSync(path.join(here, "domPage.js"), "utf8"));
mainTimeout = page.setTimeout.bind(page);
if (fixture.live !== undefined) installLive(fixture.live);
await page.__oracleRun();
for (const slot of slots) OracleWorker.prototype.terminate.call(slot.worker);
setWorkerRouter(null);
await terminatedWorkersExited();
process.stdout.write(
  lines.map((line) => line + "\n").join(""),
  () => process.exit(0)
);
