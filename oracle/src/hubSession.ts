import { EXITED, OracleWorker, UPSTREAM_DIR, resolveWorkerScript, setWorkerRouter, terminatedWorkersExited } from "./shims";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { realClearTimeout, realSetImmediate, realSetTimeout } from "./historical/timers";

// hubSession [<spec or fixture file>]
//   Loads upstream's www/hub.html in jsdom and evaluates the real hub (src/hub/hub.ts, with
//   src/dom/env.ts installed first) inside it, as `dom` does, and adds what the hub lifecycle
//   needs: decode and export workers on real worker threads, and main's side of
//   `historical-start`. See oracle/README.md, "Hub sessions".

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
  if (text.startsWith("Failed to load: ")) write({ console: "warn", text });
});
virtualConsole.on("jsdomError", (error: Error & { type?: string; cause?: unknown }) => {
  log(`jsdom: ${error.message}`);
  if (error.type === "not-implemented") return;
  write({ exception: "event" });
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
    if (slot === undefined) return;
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
      write({ exception: "worker" });
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
page.__oracleFixture = page.JSON.parse(JSON.stringify(fixture));
page.__oracleYield = settle;
page.__oracleWrite = (line: string) => lines.push(line);
page.__oracleLog = log;
page.__oracleOuter = { TextEncoder, TextDecoder };
page.__oracleHub = {
  /** `new Worker(script)` on a real thread; `deliver` gets each message in page order. */
  spawn(script: string, deliver: (data: unknown) => void): number {
    const index = slots.length;
    const worker = new OracleWorker(pinnedLoader(script));
    worker.onmessage = (event: { data: unknown }) => deliver(event.data);
    return index;
  },
  post(index: number, data: unknown): void {
    slots[index].worker.postMessage(data);
  },
  terminate(index: number): void {
    slots[index].worker.terminate();
  },
  /** Main's reply to `historical-start` (src/main/electron/main.ts), or null for another name. */
  historicalStart(uuid: string, logPath: string): unknown {
    if (CONVERTED.some((ext) => logPath.endsWith(ext))) throw new Error(`${logPath}: the oracle cannot convert this format`);
    if (logPath.endsWith(".dslog")) {
      const events = logPath.slice(0, logPath.length - 5) + "dsevents";
      return { uuid, files: [readLog(logPath), readLog(events)], error: null };
    }
    return { uuid, files: [readLog(logPath)], error: null };
  }
};
process.on("unhandledRejection", (reason) => {
  const error = reason as Error;
  log(`unhandled rejection: ${error?.stack ?? String(reason)}`);
  write({ exception: "unhandledRejection" });
});

const here = path.dirname(fileURLToPath(import.meta.url));
page.eval(fs.readFileSync(path.join(here, "hubPage.js"), "utf8"));
await page.__oracleRun();
for (const slot of slots) OracleWorker.prototype.terminate.call(slot.worker);
setWorkerRouter(null);
await terminatedWorkersExited();
process.stdout.write(
  lines.map((line) => line + "\n").join(""),
  () => process.exit(0)
);
