import "./environment";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import https from "node:https";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import zlib from "node:zlib";

// Ground truth for asrs-services' asset downloader: upstream's real `assetDownloader.ts` and
// `github.ts`, with the real `download` package (got 8, archive-type, decompress), built at run
// time so that only `electron` (userData points at a per-case temp dir) and `check-disk-space`
// (scripted free space) are replaced. Everything writes to the real filesystem under that temp
// dir, reported as /fake/userData. Scripted inputs:
//   fetch: a queue of outcomes for the GitHub API call ({status, body} | {network} | {hang});
//     an empty queue is a network error.
//   transport: got's http/https requests are rerouted to a local server that answers each URL
//     from its own queue ({status, headers, body} | {reset} | {hang}; an empty queue is a bare
//     404); every request got makes (redirects and retries included) is recorded.
//   clock: Date, setTimeout and AbortSignal.timeout are fake. A hung fetch rejects when its
//     abort timer fires. Math.random returns the case's constant (got's retry noise).
// Records: {"case","step","op","args"?,"status","timers","fetches","requests","disk",
// "callbacks","files","dirs"}. `timers` is every pending fake timer as {due, delay} sorted by
// due then delay; `fetches`, `requests` and `disk` are the calls made during the op (requests
// sorted by URL, arrival order within a URL); `callbacks` counts updateCallback calls during
// the op; `files`/`dirs` is the whole tree under /fake/userData after the op. Every op waits
// until nothing but hung requests and pending timers is left running.
//
//   genAssetDownload <randomCount> <seed>

declare const __UPSTREAM_DIR__: string;
declare const __HARNESS_DIR__: string;

const USER_DATA = "/fake/userData";
const AUTO = `${USER_DATA}/autoAssets`;
const INLINE_LIMIT = 4096;
const API_URL = "https://api.github.com/repos/Mechanical-Advantage/AdvantageScopeAssets/releases";
const TAG_DEFAULT = "default-assets-v2";
const TAG_FRC6328 = "frc-6328-assets-v2";
const DOWNLOAD_BASE = "https://github.com/Mechanical-Advantage/AdvantageScopeAssets/releases/download";
const OBJECT_BASE = "https://objects.githubusercontent.com/github-production-release-asset-2e65be";

type Distribution = "FRC6328" | "WPILib";
type FetchOutcome = { status: number; body: string | Uint8Array | null } | { network: true } | { hang: true };
type ServeOutcome = { status: number; headers?: Record<string, string>; body?: Uint8Array } | { reset: true } | { hang: true };

const realSetTimeout = globalThis.setTimeout;
const RealDate = Date;
const realHttpRequest = http.request;

// Fake clock and timers.
let nowMs = 0;
let timerSeq = 0;
type Timer = { id: number; due: number; delay: number; callback: (...args: any[]) => void; args: any[]; retry: boolean };
let timers: Timer[] = [];
let randomValue = 0;

const g = globalThis as any;
g.setTimeout = (callback: (...args: any[]) => void, delay: number, ...args: any[]): number => {
  const id = ++timerSeq;
  const ms = Number(delay);
  timers.push({ id, due: nowMs + ms, delay: ms, callback, args, retry: /^get\d*$/.test(callback?.name ?? "") });
  return id;
};
g.clearTimeout = (id: number) => {
  timers = timers.filter((t) => t.id !== id);
};
g.Date = class extends RealDate {
  constructor(...args: any[]) {
    // @ts-expect-error spread construct
    super(...(args.length === 0 ? [nowMs] : args));
  }
  static now(): number {
    return nowMs;
  }
  static parse = RealDate.parse;
  static UTC = RealDate.UTC;
};
Math.random = () => randomValue;
process.on("unhandledRejection", () => {});

const signalTimeouts = new WeakMap<AbortSignal, { ms: number; controller: AbortController }>();
(AbortSignal as any).timeout = (ms: number) => {
  const controller = new AbortController();
  signalTimeouts.set(controller.signal, { ms, controller });
  return controller.signal;
};

// Per-case scripted state.
let fetchQueue: FetchOutcome[] = [];
let serveQueues = new Map<string, ServeOutcome[]>();
let diskFree: number | undefined | "throw" = 1e12;
let callbackFails = false;
let fetchLog: object[] = [];
let requestLog: { url: string; seq: number; method: string; headers: Record<string, string> }[] = [];
let diskLog: string[] = [];
let callbackCount = 0;
let requestSeq = 0;
let work = { downloads: 0, hung: 0, inflight: 0 };
let pendingReaddirs = 0;
let generatorError: string | null = null;

let realRoot = "";
let caseRoot = "";
let caseIndex = -1;
let stepIndex = 0;
let mod: any = null;
const modules: Record<Distribution, string> = { FRC6328: "", WPILib: "" };
let distribution: Distribution = "FRC6328";

const real = (fake: string) => (fake.startsWith(USER_DATA) ? caseRoot + fake.slice(USER_DATA.length) : fake);
const fakePath = (text: string) => (caseRoot === "" ? text : text.split(caseRoot).join(USER_DATA));

g.__asrsAssetDownload = {
  app: {
    isPackaged: false,
    getVersion: () => "0.0.0",
    getPath: (name: string) => (name === "userData" ? caseRoot : path.join(realRoot, "temp"))
  },
  checkDiskSpace: async (directory: string) => {
    diskLog.push(fakePath(String(directory)));
    if (diskFree === "throw") throw new Error("df failed");
    return { diskPath: "/", free: diskFree, size: 2e12 };
  },
  download: (realDownload: (...args: any[]) => any, ...args: any[]) => {
    const stream = realDownload(...args);
    const counters = work;
    counters.downloads++;
    stream.then(
      () => counters.downloads--,
      () => counters.downloads--
    );
    return stream;
  }
};

const realReaddir = fs.promises.readdir;
(fs.promises as any).readdir = async (...args: any[]) => {
  pendingReaddirs++;
  try {
    return await (realReaddir as any)(...args);
  } finally {
    pendingReaddirs--;
  }
};

g.fetch = async (input: unknown, init: any = {}) => {
  const timeout = init.signal ? signalTimeouts.get(init.signal) : undefined;
  fetchLog.push({
    url: String(input),
    method: init.method ?? null,
    headers: init.headers ?? null,
    timeoutMs: timeout?.ms ?? null
  });
  const outcome = fetchQueue.shift();
  if (outcome === undefined || "network" in outcome) throw new TypeError("fetch failed");
  if ("hang" in outcome) {
    if (timeout === undefined) {
      generatorError ??= `case ${caseIndex}: hung fetch without a timeout`;
      return new Promise(() => {});
    }
    return new Promise((_, reject) => {
      g.setTimeout(() => {
        timeout.controller.abort(new DOMException("The operation was aborted due to timeout", "TimeoutError"));
        reject(timeout.controller.signal.reason);
      }, timeout.ms);
    });
  }
  const body = outcome.body === null ? null : typeof outcome.body === "string" ? outcome.body : Buffer.from(outcome.body);
  return new Response(body, { status: outcome.status });
};

// got resolves `https`/`http` at request time; requests go to the local server instead, carrying
// the URL got asked for.
let serverPort = 0;
const localAgent = new http.Agent({ keepAlive: false });
function reroute(options: any, callback?: (res: http.IncomingMessage) => void) {
  const original: string = options.href ?? `${options.protocol}//${options.host ?? options.hostname}${options.path ?? "/"}`;
  return realHttpRequest(
    {
      method: options.method,
      host: "127.0.0.1",
      port: serverPort,
      path: "/",
      headers: { ...(options.headers ?? {}), "x-asrs-url": original },
      agent: localAgent
    },
    callback
  );
}
(https as any).request = reroute;
(http as any).request = reroute;

const IGNORED_HEADERS = new Set(["host", "connection", "x-asrs-url"]);
const server = http.createServer((req, res) => {
  const url = String(req.headers["x-asrs-url"]);
  const headers: Record<string, string> = {};
  for (const name of Object.keys(req.headers).sort()) {
    if (!IGNORED_HEADERS.has(name)) headers[name] = String(req.headers[name]);
  }
  requestLog.push({ url, seq: requestSeq++, method: req.method ?? "", headers });
  const outcome: ServeOutcome = serveQueues.get(url)?.shift() ?? { status: 404 };
  if ("hang" in outcome) {
    work.hung++;
    return;
  }
  if ("reset" in outcome) {
    req.socket.destroy();
    return;
  }
  const counters = work;
  counters.inflight++;
  res.on("close", () => counters.inflight--);
  res.writeHead(outcome.status, outcome.headers ?? {});
  res.end(outcome.body === undefined ? undefined : Buffer.from(outcome.body));
});

const realSleep = (ms: number) => new Promise((resolve) => realSetTimeout(resolve, ms));
const immediate = () => new Promise((resolve) => setImmediate(resolve));

function fingerprint(): string {
  return JSON.stringify([mod?.getAssetDownloadStatus(), timers.length, callbackCount, requestLog.length, fetchLog.length, work.downloads]);
}

async function quiesce() {
  let stable = 0;
  let last = "";
  for (let round = 0; round < 20000; round++) {
    await realSleep(1);
    for (let i = 0; i < 4; i++) await immediate();
    const retries = timers.filter((t) => t.retry).length;
    const active = work.downloads - retries - work.hung;
    const print = fingerprint();
    if (active <= 0 && pendingReaddirs === 0 && work.inflight === 0 && print === last) stable++;
    else stable = 0;
    last = print;
    if (stable >= 4) return;
  }
  throw new Error(`case ${caseIndex}: did not settle`);
}

function walk(): { files: object[]; dirs: string[] } {
  const files: object[] = [];
  const dirs: string[] = [];
  const visit = (dir: string) => {
    dirs.push(fakePath(dir));
    for (const name of fs.readdirSync(dir).sort()) {
      const full = path.join(dir, name);
      const stat = fs.lstatSync(full);
      if (stat.isDirectory()) visit(full);
      else if (stat.isFile()) files.push({ name: fakePath(full), ...fileValue(fs.readFileSync(full)) });
      else throw new Error(`case ${caseIndex}: unexpected entry ${full}`);
    }
  };
  if (fs.existsSync(caseRoot)) visit(caseRoot);
  return { files, dirs: dirs.sort() };
}

const fileValue = (bytes: Uint8Array) =>
  bytes.length <= INLINE_LIMIT
    ? { hex: Buffer.from(bytes).toString("hex") }
    : { len: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };

const out = (value: object) => process.stdout.write(fakePath(JSON.stringify(value)) + "\n");

async function record(op: string, args: unknown, run: () => unknown | Promise<unknown>) {
  const r: any = { case: caseIndex, step: stepIndex++, op };
  if (args !== undefined) r.args = args;
  await run();
  await quiesce();
  if (generatorError !== null) throw new Error(generatorError);
  r.status = mod.getAssetDownloadStatus();
  r.timers = [...timers].sort((a, b) => a.due - b.due || a.delay - b.delay).map((t) => ({ due: t.due, delay: t.delay }));
  r.fetches = fetchLog.splice(0);
  r.requests = requestLog
    .splice(0)
    .sort((a, b) => (a.url < b.url ? -1 : a.url > b.url ? 1 : a.seq - b.seq))
    .map(({ url, method, headers }) => ({ url, method, headers }));
  r.disk = diskLog.splice(0);
  r.callbacks = callbackCount;
  callbackCount = 0;
  Object.assign(r, walk());
  out(r);
}

async function buildModules() {
  const esbuild = createRequire(path.join(__HARNESS_DIR__, "package.json"))("esbuild");
  const upstreamSrc = path.join(__UPSTREAM_DIR__, "src");
  const realDownload = createRequire(path.join(__UPSTREAM_DIR__, "package.json")).resolve("download");
  for (const dist of ["FRC6328", "WPILib"] as Distribution[]) {
    const fakes: Record<string, string> = {
      electron: "export const app = globalThis.__asrsAssetDownload.app;",
      "check-disk-space": "export default (directory) => globalThis.__asrsAssetDownload.checkDiskSpace(directory);",
      download: `import realDownload from ${JSON.stringify(realDownload)}; export default (...args) => globalThis.__asrsAssetDownload.download(realDownload, ...args);`
    };
    const result = await esbuild.build({
      entryPoints: [path.join(upstreamSrc, "main/electron/assetDownloader.ts")],
      bundle: true,
      write: false,
      format: "esm",
      platform: "node",
      target: "esnext",
      logLevel: "warning",
      nodePaths: [path.join(__UPSTREAM_DIR__, "node_modules")],
      banner: {
        js: 'import { createRequire as __oracleCreateRequire } from "node:module"; import { fileURLToPath as __oracleFileURLToPath } from "node:url"; import { dirname as __oracleDirname } from "node:path"; const require = __oracleCreateRequire(import.meta.url); const __filename = __oracleFileURLToPath(import.meta.url); const __dirname = __oracleDirname(__filename);'
      },
      plugins: [
        {
          name: "asset-download-fakes",
          setup(build: any) {
            build.onResolve({ filter: /^(electron|check-disk-space|download)$/ }, (resolveArgs: any) =>
              /[\\/]src[\\/]main[\\/]electron[\\/]/.test(resolveArgs.importer) ? { path: resolveArgs.path, namespace: "asrs-fake" } : undefined
            );
            build.onLoad({ filter: /.*/, namespace: "asrs-fake" }, (loadArgs: any) => ({
              contents: fakes[loadArgs.path],
              loader: "js",
              resolveDir: __UPSTREAM_DIR__
            }));
            build.onLoad({ filter: /buildConstants\.ts$/ }, (loadArgs: any) => ({
              contents: fs.readFileSync(loadArgs.path, "utf8").replace("Distribution.__distribution__", `Distribution.${dist}`),
              loader: "ts"
            }));
          }
        }
      ]
    });
    const file = path.join(realRoot, `assetDownloader-${dist}.mjs`);
    fs.writeFileSync(file, result.outputFiles[0].contents);
    modules[dist] = file;
  }
}

// Scenario operations.

async function reset(dist: Distribution, now: number, random = 0.5) {
  caseIndex++;
  stepIndex = 0;
  distribution = dist;
  caseRoot = path.join(realRoot, `case${caseIndex}`, "userData");
  fs.mkdirSync(caseRoot, { recursive: true });
  nowMs = now;
  timers = [];
  work = { downloads: 0, hung: 0, inflight: 0 };
  randomValue = random;
  fetchQueue = [];
  serveQueues = new Map();
  diskFree = 1e12;
  callbackFails = false;
  fetchLog = [];
  requestLog = [];
  diskLog = [];
  callbackCount = 0;
  mod = await import(pathToFileURL(modules[dist]).href + `?case=${caseIndex}`);
  await record("reset", { distribution: dist, now, random }, () => {});
}

const putDir = (name: string) => record("putDir", { name }, () => fs.mkdirSync(real(name), { recursive: true }));
const putFile = (name: string, data: Uint8Array) =>
  record("putFile", { name, data: fileValue(data) }, () => {
    fs.mkdirSync(path.dirname(real(name)), { recursive: true });
    fs.writeFileSync(real(name), data);
  });
const putText = (name: string, text: string) => putFile(name, utf8(text));

function fetchArgs(outcome: FetchOutcome): object {
  if ("status" in outcome) {
    const body = outcome.body === null ? null : fileValue(typeof outcome.body === "string" ? utf8(outcome.body) : outcome.body);
    return { status: outcome.status, body };
  }
  return outcome;
}
const queueFetch = (outcome: FetchOutcome) => record("queueFetch", fetchArgs(outcome), () => fetchQueue.push(outcome));

function serveArgs(url: string, outcome: ServeOutcome): object {
  if ("status" in outcome) {
    return { url, status: outcome.status, headers: outcome.headers ?? {}, body: fileValue(outcome.body ?? new Uint8Array()) };
  }
  return { url, ...outcome };
}
const serve = (url: string, outcome: ServeOutcome) =>
  record("serve", serveArgs(url, outcome), () => {
    if (!serveQueues.has(url)) serveQueues.set(url, []);
    serveQueues.get(url)!.push(outcome);
  });

const setDisk = (free: number | null | "throw") =>
  record("setDisk", { free }, () => {
    diskFree = free === null ? undefined : free;
  });
const setCallback = (fails: boolean) =>
  record("setCallback", { fails }, () => {
    callbackFails = fails;
  });

const updateCallback = async () => {
  callbackCount++;
  if (callbackFails) throw new Error("reload failed");
};

const start = () =>
  record("start", undefined, () => {
    void mod.startAssetDownloadLoop(updateCallback);
  });
const status = () => record("status", undefined, () => {});

async function advance(to: number) {
  await record("advance", { to }, async () => {
    for (;;) {
      const due = timers.filter((t) => t.due <= to).sort((a, b) => a.due - b.due || a.id - b.id)[0];
      if (due === undefined) break;
      timers = timers.filter((t) => t !== due);
      nowMs = due.due;
      due.callback(...due.args);
      await quiesce();
    }
    nowMs = to;
  });
}
const advanceBy = (ms: number) => advance(nowMs + ms);

// Fixtures.

const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));

type ZipEntry = { name: string | Uint8Array; data?: Uint8Array; dir?: boolean; method?: 0 | 8; madeBy?: number; attrs?: number; utf8Flag?: boolean };

function zip(entries: ZipEntry[], options: { truncate?: number } = {}): Uint8Array {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = typeof entry.name === "string" ? Buffer.from(entry.name, "utf8") : Buffer.from(entry.name);
    const data = Buffer.from(entry.data ?? new Uint8Array());
    const method = entry.method ?? (data.length > 0 ? 8 : 0);
    const stored = method === 8 ? zlib.deflateRawSync(data) : data;
    const crc = zlib.crc32(data);
    const flags = entry.utf8Flag ?? typeof entry.name === "string" ? 0x800 : 0;
    const madeBy = entry.madeBy ?? 3;
    const attrs = entry.attrs ?? (entry.dir ? ((0o040755 << 16) | 0x10) >>> 0 : (0o100644 << 16) >>> 0);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(flags, 6);
    local.writeUInt16LE(method, 8);
    local.writeUInt16LE(0x6000, 10);
    local.writeUInt16LE(0x5a21, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(stored.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(0, 28);
    locals.push(local, name, stored);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE((madeBy << 8) | 20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(flags, 8);
    central.writeUInt16LE(method, 10);
    central.writeUInt16LE(0x6000, 12);
    central.writeUInt16LE(0x5a21, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(stored.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE(attrs, 38);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, name);
    offset += 30 + name.length + stored.length;
  }
  const cd = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cd.length, 12);
  end.writeUInt32LE(offset, 16);
  const all = Buffer.concat([...locals, cd, end]);
  return new Uint8Array(options.truncate === undefined ? all : all.subarray(0, options.truncate));
}

const file = (name: string, text = name) => ({ name, data: utf8(text) });
const dirEntry = (name: string) => ({ name, dir: true });
const fieldZip = (prefix: string) =>
  zip([file("config.json", JSON.stringify({ name: prefix, isFTC: false })), file("model.glb", `glb ${prefix}`.repeat(40)), file("model_0.glb", `piece ${prefix}`)]);

const assetUrl = (tag: string, name: string) => `${DOWNLOAD_BASE}/${tag}/${name}`;
const objectUrl = (id: number) => `${OBJECT_BASE}/${id}?response-content-disposition=attachment`;

type Asset = { name?: unknown; url?: unknown; extra?: object };
const release = (tag: unknown, assets: Asset[] | unknown, extra: object = {}) => ({
  tag_name: tag,
  ...(Array.isArray(assets)
    ? { assets: assets.map((a: Asset | null) => a === null ? null : ({ ...(a.name === undefined ? {} : { name: a.name }), ...(a.url === undefined ? {} : { browser_download_url: a.url }), ...(a.extra ?? {}) })) }
    : assets === undefined
      ? {}
      : { assets }),
  ...extra
});
const asset = (tag: string, name: string): Asset => ({ name, url: assetUrl(tag, name) });
const apiOk = (releases: unknown) => queueFetch({ status: 200, body: JSON.stringify(releases) });

// Serves `url` through the GitHub redirect to an object URL with `body`.
async function serveRedirected(url: string, id: number, body: Uint8Array, headers: Record<string, string> = {}) {
  await serve(url, { status: 302, headers: { location: objectUrl(id) } });
  await serve(objectUrl(id), { status: 200, headers: { "content-type": "application/octet-stream", ...headers }, body });
}
const serveZip = (url: string, body: Uint8Array) => serve(url, { status: 200, headers: { "content-type": "application/zip" }, body });

// Wall clocks that exercise toLocaleTimeString: midnight and noon edges, DST transitions in
// Los Angeles and Auckland, the Kolkata half hour, sub-second and pre-1970 instants.
const CLOCKS = [
  Date.UTC(2026, 10, 22, 0, 0, 0),
  Date.UTC(2026, 10, 22, 12, 0, 0, 999),
  Date.UTC(2026, 10, 22, 23, 59, 59, 500),
  Date.UTC(2026, 2, 8, 9, 59, 59),
  Date.UTC(2026, 2, 8, 10, 0, 0),
  Date.UTC(2026, 3, 4, 13, 59, 59),
  Date.UTC(2026, 3, 4, 14, 0, 0),
  Date.UTC(2026, 8, 26, 14, 0, 0),
  Date.UTC(2026, 0, 1, 6, 30, 0),
  Date.UTC(2025, 5, 15, 18, 29, 59, 1),
  -1,
  -86_400_000 * 365.25 * 10 + 1234.5,
  Date.UTC(2038, 0, 19, 3, 14, 8),
  1e12 + 0.25
];

const DEFAULT_ASSETS = ["Field2d_2026FRC.zip", "Field3d_2026FRC.zip", "Robot_KitBot.zip"];
const FRC6328_ASSETS = ["Robot_6328Sim.zip", "Field3d_6328Practice.zip"];

async function successfulSetup(dist: Distribution, now: number) {
  await reset(dist, now);
  await putDir(AUTO);
  await apiOk([
    release("unrelated-v1", [asset("unrelated-v1", "Field2d_Old.zip")]),
    release(TAG_DEFAULT, DEFAULT_ASSETS.map((n) => asset(TAG_DEFAULT, n))),
    release(TAG_FRC6328, FRC6328_ASSETS.map((n) => asset(TAG_FRC6328, n)))
  ]);
}

async function handWritten() {
  // Status before and after a clean first run, then the 6 hour recheck that finds nothing new.
  for (const dist of ["FRC6328", "WPILib"] as Distribution[]) {
    await successfulSetup(dist, CLOCKS[1]);
    await status();
    let id = 1;
    for (const name of DEFAULT_ASSETS) await serveRedirected(assetUrl(TAG_DEFAULT, name), id++, fieldZip(name));
    if (dist === "FRC6328") for (const name of FRC6328_ASSETS.slice(1)) await serveRedirected(assetUrl(TAG_FRC6328, name), id++, fieldZip(name));
    await putDir(`${AUTO}/Robot_Stale`);
    await putText(`${AUTO}/Robot_Stale/config.json`, "{}");
    await putText(`${AUTO}/stray.txt`, "stray");
    await putDir(`${AUTO}/Robot_6328Sim`);
    await start();
    await advanceBy(21_600_000 - 1);
    await apiOk([release(TAG_DEFAULT, DEFAULT_ASSETS.map((n) => asset(TAG_DEFAULT, n)))]);
    await advanceBy(1);
    await status();
  }

  // Every clock through "Download started at", each zone's text, held there by a hung download.
  for (const clock of CLOCKS) {
    await reset("FRC6328", clock);
    await putDir(AUTO);
    await apiOk([release(TAG_DEFAULT, [asset(TAG_DEFAULT, "Field2d_Clock.zip")])]);
    await serve(assetUrl(TAG_DEFAULT, "Field2d_Clock.zip"), { hang: true });
    await start();
    await advanceBy(3_600_000);
  }

  // Disk space: under 8 GB, exactly 8 GB, unknown (NaN passes), a failing check, recovery.
  await reset("FRC6328", CLOCKS[0]);
  await putDir(AUTO);
  await setDisk(7_999_999_999);
  await start();
  await advanceBy(29_999);
  await setDisk("throw");
  await advanceBy(1);
  await setDisk(8e9);
  await apiOk([]);
  await advanceBy(30_000);
  await reset("FRC6328", CLOCKS[2]);
  await putDir(AUTO);
  await setDisk(null);
  await apiOk([]);
  await start();
  await reset("FRC6328", CLOCKS[2]);
  await putDir(AUTO);
  await setDisk(0);
  await start();

  // API failures: HTTP errors (rate limit, server error, redirect status), network error, timeout
  // and recovery, malformed bodies.
  const apiFailures: FetchOutcome[] = [
    { status: 403, body: JSON.stringify({ message: "API rate limit exceeded for 0.0.0.0.", documentation_url: "https://docs.github.com/rest/overview/resources-in-the-rest-api#rate-limiting" }) },
    { status: 429, body: "" },
    { status: 500, body: "oops" },
    { status: 304, body: null },
    { status: 204, body: null },
    { network: true },
    { status: 200, body: "not json" },
    { status: 200, body: "" },
    { status: 200, body: "null" },
    { status: 200, body: "{}" },
    { status: 200, body: '"text"' },
    { status: 200, body: "5" },
    { status: 200, body: JSON.stringify([null]) },
    { status: 200, body: JSON.stringify([release(TAG_DEFAULT, undefined)]) },
    { status: 200, body: JSON.stringify([release(TAG_DEFAULT, "nope")]) },
    { status: 200, body: JSON.stringify([release(TAG_DEFAULT, [{ url: assetUrl(TAG_DEFAULT, "x.zip") }])]) },
    { status: 200, body: JSON.stringify([release(TAG_DEFAULT, [{ name: 7, url: "x" }])]) },
    { status: 200, body: JSON.stringify([release(TAG_DEFAULT, [null])]) }
  ];
  for (const failure of apiFailures) {
    await reset("FRC6328", CLOCKS[3]);
    await putDir(AUTO);
    await putDir(`${AUTO}/Field2d_Keep`);
    await queueFetch(failure);
    await start();
    await apiOk([release(TAG_DEFAULT, [])]);
    await advanceBy(30_000);
  }
  await reset("FRC6328", CLOCKS[4]);
  await putDir(AUTO);
  await queueFetch({ hang: true });
  await start();
  await advanceBy(4_999);
  await queueFetch({ hang: true });
  await advanceBy(1);
  await advanceBy(30_000);
  await apiOk([]);
  await advanceBy(5_000);
  await advanceBy(30_000);

  // Response bodies: a UTF-8 BOM, ignored and repeated tags, a WPILib-optional tag only, other
  // release fields, tag names that only look alike.
  await reset("WPILib", CLOCKS[5]);
  await putDir(AUTO);
  await putDir(`${AUTO}/Robot_6328Sim`);
  await putDir(`${AUTO}/Field3d_Gone`);
  await queueFetch({
    status: 200,
    body: "﻿" + JSON.stringify([
      release("Default-Assets-v2", [asset("x", "Field2d_Case.zip")]),
      release("default-assets-v1", [asset("x", "Field2d_V1.zip")]),
      release(TAG_FRC6328, [asset(TAG_FRC6328, "Robot_6328Sim.zip"), asset(TAG_FRC6328, "Field3d_Optional.zip")], { draft: false, prerelease: true }),
      release(TAG_DEFAULT, [], { name: "Default assets" })
    ])
  });
  await start();

  // Asset names: no .zip suffix, shorter than 4 code units, exactly ".zip", nested and parent
  // paths, unicode, spaces and percent signs, duplicates within a tag.
  await reset("FRC6328", CLOCKS[6]);
  await putDir(AUTO);
  const odd = ["Robot_NoSuffix", "abc", "ab", ".zip", "nested/Field2d_Inner.zip", "../Escaped.zip", "Robot_Ünïcode ☃.zip", "Field2d_100% Fun.zip"];
  await apiOk([release(TAG_DEFAULT, odd.map((n) => ({ name: n, url: `${DOWNLOAD_BASE}/${TAG_DEFAULT}/${encodeURIComponent(n)}` })))]);
  for (const name of odd) {
    if (name === "ab" || name === ".zip") continue;
    await serveZip(`${DOWNLOAD_BASE}/${TAG_DEFAULT}/${encodeURIComponent(name)}`, zip([file("config.json", name)]));
  }
  await start();
  await apiOk([release(TAG_DEFAULT, [asset(TAG_DEFAULT, "nested/Field2d_Inner.zip")])]);
  await serveZip(assetUrl(TAG_DEFAULT, "nested/Field2d_Inner.zip"), zip([file("config.json", "again")]));
  await advanceBy(21_600_000);

  // Downloads: one asset fails while the others land in the background; HTTP errors are not
  // retried; resets retry twice with backoff and then fail; a stalled download blocks the loop.
  await reset("FRC6328", CLOCKS[7], 0.25);
  await putDir(AUTO);
  await apiOk([release(TAG_DEFAULT, ["Robot_A.zip", "Robot_B.zip", "Robot_C.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serveRedirected(assetUrl(TAG_DEFAULT, "Robot_A.zip"), 1, fieldZip("A"));
  await serve(assetUrl(TAG_DEFAULT, "Robot_B.zip"), { status: 404, body: utf8("Not Found") });
  await serveZip(assetUrl(TAG_DEFAULT, "Robot_C.zip"), fieldZip("C"));
  await start();
  await apiOk([release(TAG_DEFAULT, ["Robot_A.zip", "Robot_B.zip", "Robot_C.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serve(assetUrl(TAG_DEFAULT, "Robot_B.zip"), { status: 302, headers: { location: objectUrl(9) } });
  await serve(objectUrl(9), { status: 500 });
  await advanceBy(30_000);
  await apiOk([release(TAG_DEFAULT, ["Robot_A.zip", "Robot_B.zip", "Robot_C.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serveZip(assetUrl(TAG_DEFAULT, "Robot_B.zip"), fieldZip("B"));
  await advanceBy(30_000);

  for (const random of [0, 0.999, 0.123456789]) {
    await reset("FRC6328", CLOCKS[8], random);
    await putDir(AUTO);
    await apiOk([release(TAG_DEFAULT, ["Robot_R.zip", "Robot_S.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
    await serve(assetUrl(TAG_DEFAULT, "Robot_R.zip"), { reset: true });
    await serve(assetUrl(TAG_DEFAULT, "Robot_R.zip"), { status: 302, headers: { location: objectUrl(3) } });
    await serve(objectUrl(3), { reset: true });
    await serveZip(objectUrl(3), fieldZip("R"));
    await serve(assetUrl(TAG_DEFAULT, "Robot_S.zip"), { reset: true });
    await serve(assetUrl(TAG_DEFAULT, "Robot_S.zip"), { reset: true });
    await serve(assetUrl(TAG_DEFAULT, "Robot_S.zip"), { reset: true });
    await start();
    await advanceBy(1_999);
    await advanceBy(200);
    await advanceBy(4_200);
    await apiOk([release(TAG_DEFAULT, ["Robot_R.zip", "Robot_S.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
    await serveZip(assetUrl(TAG_DEFAULT, "Robot_S.zip"), fieldZip("S"));
    await advanceBy(30_000);
  }

  await reset("FRC6328", CLOCKS[9]);
  await putDir(AUTO);
  await putDir(`${AUTO}/Robot_Old`);
  await apiOk([release(TAG_DEFAULT, ["Robot_Hang.zip", "Robot_Ok.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serve(assetUrl(TAG_DEFAULT, "Robot_Hang.zip"), { hang: true });
  await serveZip(assetUrl(TAG_DEFAULT, "Robot_Ok.zip"), fieldZip("Ok"));
  await start();
  await advanceBy(21_600_000);
  await reset("FRC6328", CLOCKS[9]);
  await putDir(AUTO);
  await apiOk([release(TAG_DEFAULT, ["Robot_Hang.zip", "Robot_Bad.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serve(assetUrl(TAG_DEFAULT, "Robot_Hang.zip"), { hang: true });
  await serve(assetUrl(TAG_DEFAULT, "Robot_Bad.zip"), { status: 503 });
  await start();
  await apiOk([release(TAG_DEFAULT, ["Robot_Hang.zip", "Robot_Bad.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serve(assetUrl(TAG_DEFAULT, "Robot_Hang.zip"), { status: 404 });
  await serve(assetUrl(TAG_DEFAULT, "Robot_Bad.zip"), { reset: true });
  await serveZip(assetUrl(TAG_DEFAULT, "Robot_Bad.zip"), fieldZip("Bad"));
  await advanceBy(30_000);
  await advanceBy(2_500);

  // Redirects: relative and absolute locations, every redirect status, a 304 body, 3xx without
  // a location, more than 10 redirects, a retry after a redirect starts from the redirected URL.
  await reset("FRC6328", CLOCKS[10]);
  await putDir(AUTO);
  const redirects: [string, ServeOutcome[]][] = [
    ["Robot_301.zip", [{ status: 301, headers: { location: "/moved/Robot_301.zip" } }]],
    ["Robot_303.zip", [{ status: 303, headers: { location: "../other/Robot_303.zip" } }]],
    ["Robot_307.zip", [{ status: 307, headers: { location: "https://mirror.example.com/Robot_307.zip" } }]],
    ["Robot_308.zip", [{ status: 308, headers: { location: "http://plain.example.com/Robot_308.zip?x=1" } }]],
    ["Robot_300.zip", [{ status: 300, headers: { location: "Robot_300b.zip" } }]],
    ["Robot_305.zip", [{ status: 305, headers: { location: "//cdn.example.com/Robot_305.zip" } }]]
  ];
  const targets: Record<string, string> = {
    "Robot_301.zip": "https://github.com/moved/Robot_301.zip",
    "Robot_303.zip": `${DOWNLOAD_BASE}/other/Robot_303.zip`,
    "Robot_307.zip": "https://mirror.example.com/Robot_307.zip",
    "Robot_308.zip": "http://plain.example.com/Robot_308.zip?x=1",
    "Robot_300.zip": `${DOWNLOAD_BASE}/${TAG_DEFAULT}/Robot_300b.zip`,
    "Robot_305.zip": "https://cdn.example.com/Robot_305.zip"
  };
  await apiOk([release(TAG_DEFAULT, redirects.map(([n]) => asset(TAG_DEFAULT, n)))]);
  for (const [name, outcomes] of redirects) {
    for (const outcome of outcomes) await serve(assetUrl(TAG_DEFAULT, name), outcome);
    await serveZip(targets[name], fieldZip(name));
  }
  await start();

  await reset("FRC6328", CLOCKS[11]);
  await putDir(AUTO);
  await apiOk([release(TAG_DEFAULT, ["Robot_304.zip", "Robot_302Bare.zip", "Robot_Loop.zip", "Robot_RetryRedirect.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serve(assetUrl(TAG_DEFAULT, "Robot_304.zip"), { status: 304 });
  await serve(assetUrl(TAG_DEFAULT, "Robot_302Bare.zip"), { status: 302 });
  for (let i = 0; i < 11; i++) await serve(i === 0 ? assetUrl(TAG_DEFAULT, "Robot_Loop.zip") : objectUrl(100 + i), { status: 302, headers: { location: objectUrl(101 + i) } });
  await serve(assetUrl(TAG_DEFAULT, "Robot_RetryRedirect.zip"), { status: 302, headers: { location: objectUrl(50) } });
  await serve(objectUrl(50), { reset: true });
  await serveZip(objectUrl(50), fieldZip("RetryRedirect"));
  await start();
  await advanceBy(3_000);

  // Bodies that are not archives are written whole under the download folder, named from
  // Content-Disposition or the requested URL's last path segment.
  await reset("FRC6328", CLOCKS[12]);
  await putDir(AUTO);
  await apiOk([
    release(TAG_DEFAULT, [
      asset(TAG_DEFAULT, "Robot_Html.zip"),
      asset(TAG_DEFAULT, "Robot_Disposition.zip"),
      asset(TAG_DEFAULT, "Robot_Quoted.zip"),
      asset(TAG_DEFAULT, "Robot_Reserved.zip"),
      { name: "Robot_NoExt.zip", url: `${DOWNLOAD_BASE}/${TAG_DEFAULT}/noext` },
      { name: "Robot_Encoded.zip", url: `${DOWNLOAD_BASE}/${TAG_DEFAULT}/a%20b.bin?q=1` },
      asset(TAG_DEFAULT, "Robot_Empty.zip")
    ])
  ]);
  await serveRedirected(assetUrl(TAG_DEFAULT, "Robot_Html.zip"), 20, utf8("<html>maintenance</html>"), { "content-type": "text/html" });
  await serve(assetUrl(TAG_DEFAULT, "Robot_Disposition.zip"), { status: 200, headers: { "content-disposition": "attachment; filename=payload.txt" }, body: utf8("plain") });
  await serve(assetUrl(TAG_DEFAULT, "Robot_Quoted.zip"), { status: 200, headers: { "content-disposition": 'attachment; filename="my file.dat"' }, body: utf8("quoted") });
  await serve(assetUrl(TAG_DEFAULT, "Robot_Reserved.zip"), { status: 200, headers: { "content-disposition": 'attachment; filename="a:b?c.txt"' }, body: utf8("reserved") });
  await serve(`${DOWNLOAD_BASE}/${TAG_DEFAULT}/noext`, { status: 200, body: utf8("no extension") });
  await serve(`${DOWNLOAD_BASE}/${TAG_DEFAULT}/a%20b.bin?q=1`, { status: 200, body: utf8("encoded") });
  await serve(assetUrl(TAG_DEFAULT, "Robot_Empty.zip"), { status: 200 });
  await start();

  // Archive layouts: nested folders with and without directory entries, stored and deflated
  // entries, a DOS-made directory, a directory entry without attributes, backslash separators,
  // CP437 and UTF-8 names, an empty archive, an empty directory, a truncated archive, an entry
  // escaping the output folder, an absolute entry.
  const layouts: [string, Uint8Array][] = [
    ["Field3d_Nested", zip([dirEntry("assets/"), file("assets/config.json", "{}"), file("assets/deep/model.glb", "glb".repeat(2000)), file("top.txt")])],
    ["Field3d_NoDirEntries", zip([file("a/b/c.txt", "c"), file("a/d.txt", "d")])],
    ["Field3d_Stored", zip([{ ...file("stored.bin", "stored"), method: 0 }, { ...file("deflated.bin", "deflated".repeat(10)), method: 8 }])],
    ["Field3d_Dos", zip([{ name: "dosdir/", madeBy: 0, attrs: 16 }, { ...file("dosdir/f.txt", "f"), madeBy: 0, attrs: 32 }])],
    ["Field3d_BareDir", zip([{ name: "bare/", attrs: 0 }])],
    ["Field3d_Backslash", zip([file("win\\path\\f.txt", "w")])],
    ["Field3d_Cp437", zip([{ name: new Uint8Array([0x81, 0x62, 0x65, 0x72, 0x2e, 0x74, 0x78, 0x74]), data: utf8("cp437"), utf8Flag: false }, { name: "ünï.txt", data: utf8("utf8") }])],
    ["Field3d_Empty", zip([])],
    ["Field3d_EmptyDir", zip([dirEntry("only/")])],
    ["Field3d_Truncated", zip([file("a.txt", "aaaa".repeat(50))], { truncate: 60 })],
    ["Field3d_Slip", zip([file("ok.txt", "ok"), file("../slip.txt", "slip")])],
    ["Field3d_Absolute", zip([file("/abs.txt", "abs")])]
  ];
  for (const [name, body] of layouts) {
    await reset("FRC6328", CLOCKS[13]);
    await putDir(AUTO);
    await apiOk([release(TAG_DEFAULT, [asset(TAG_DEFAULT, name + ".zip")])]);
    await serveZip(assetUrl(TAG_DEFAULT, name + ".zip"), body);
    await start();
  }

  // Leftovers: a stale _download folder is merged into, an existing target is never
  // redownloaded even when it is a file, the update callback failing, a missing autoAssets
  // folder with and without downloads, URLs got rejects before any request.
  await reset("FRC6328", CLOCKS[0]);
  await putDir(AUTO);
  await putText(`${AUTO}/Robot_Partial_download/leftover.txt`, "left");
  await putText(`${AUTO}/Robot_File`, "a file");
  await apiOk([release(TAG_DEFAULT, ["Robot_Partial.zip", "Robot_File.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await serveZip(assetUrl(TAG_DEFAULT, "Robot_Partial.zip"), fieldZip("Partial"));
  await setCallback(true);
  await start();
  await apiOk([release(TAG_DEFAULT, ["Robot_Partial.zip", "Robot_File.zip"].map((n) => asset(TAG_DEFAULT, n)))]);
  await setCallback(false);
  await advanceBy(30_000);

  await reset("FRC6328", CLOCKS[1]);
  await apiOk([release(TAG_DEFAULT, [])]);
  await start();
  await apiOk([release(TAG_DEFAULT, [asset(TAG_DEFAULT, "Robot_First.zip")])]);
  await serveZip(assetUrl(TAG_DEFAULT, "Robot_First.zip"), fieldZip("First"));
  await advanceBy(30_000);

  await reset("FRC6328", CLOCKS[2]);
  await putDir(AUTO);
  await apiOk([
    release(TAG_DEFAULT, [
      asset(TAG_DEFAULT, "Robot_Before.zip"),
      { name: "Robot_BadUrl.zip", url: `${DOWNLOAD_BASE}/%E0%A4%A.zip` },
      asset(TAG_DEFAULT, "Robot_After.zip")
    ])
  ]);
  await serveZip(assetUrl(TAG_DEFAULT, "Robot_Before.zip"), fieldZip("Before"));
  await start();
  for (const url of [undefined, "ftp://example.com/Robot.zip", "https://user:pass@example.com/Robot.zip", "github.com/no-scheme/Robot.zip"]) {
    await reset("FRC6328", CLOCKS[3]);
    await putDir(AUTO);
    await apiOk([release(TAG_DEFAULT, [{ name: "Robot_Url.zip", url }])]);
    if (url === "github.com/no-scheme/Robot.zip") await serveZip("https://github.com/no-scheme/Robot.zip", fieldZip("NoScheme"));
    await start();
  }
}

let rng = 1;
function nextU32(): number {
  rng ^= rng << 13;
  rng ^= rng >>> 17;
  rng ^= rng << 5;
  return rng >>> 0;
}
const below = (n: number) => nextU32() % n;
const chance = (p: number) => nextU32() / 2 ** 32 < p;
const pick = <T>(items: readonly T[]): T => items[below(items.length)];

const RANDOM_NAMES = ["Field2d_A", "Field3d_B", "Robot_C", "Joystick_D", "Robot_E", "Field3d_F"];

async function randomCase() {
  const dist: Distribution = chance(0.3) ? "WPILib" : "FRC6328";
  await reset(dist, pick(CLOCKS), Math.floor(nextU32() / 2 ** 16) / 2 ** 16);
  if (chance(0.9)) await putDir(AUTO);
  for (const name of RANDOM_NAMES) {
    if (chance(0.2)) await putText(`${AUTO}/${name}/config.json`, "{}");
  }
  if (chance(0.3)) await putText(`${AUTO}/${pick(RANDOM_NAMES)}_download/part`, "part");
  const rounds = 1 + below(3);
  let objectId = 1000;
  for (let round = 0; round < rounds; round++) {
    const roll = below(10);
    if (roll === 0) await queueFetch({ network: true });
    else if (roll === 1) await queueFetch({ status: pick([403, 404, 502]), body: "{}" });
    else if (roll === 2) await queueFetch({ hang: true });
    else {
      const releases = [TAG_DEFAULT, TAG_FRC6328].map((tag) =>
        release(
          tag,
          RANDOM_NAMES.filter(() => chance(0.4)).map((n) => asset(tag, `${n}${tag === TAG_FRC6328 ? "6328" : ""}.zip`))
        )
      );
      await apiOk(chance(0.2) ? releases.reverse() : releases);
      for (const rel of releases) {
        for (const a of rel.assets as { name: string; browser_download_url: string }[]) {
          const kind = below(8);
          const url = a.browser_download_url;
          if (kind === 0) await serve(url, { status: pick([404, 500, 403]) });
          else if (kind === 1) {
            await serve(url, { reset: true });
            await serveZip(url, fieldZip(a.name));
          }
          else if (kind === 2 && round === rounds - 1) await serve(url, { hang: true });
          else if (kind === 3) await serveRedirected(url, objectId++, fieldZip(a.name));
          else await serveZip(url, zip([file("config.json", a.name), ...(chance(0.5) ? [file("sub/model.glb", "m".repeat(below(5000)))] : [])]));
        }
      }
    }
    if (chance(0.2)) await setDisk(chance(0.5) ? 1e9 : 9e9);
    if (chance(0.2)) await setCallback(chance(0.5));
    if (round === 0) await start();
    else await advanceBy(pick([30_000, 21_600_000, 25_000]));
    if (chance(0.5)) await advanceBy(pick([2_100, 6_200, 5_000]));
  }
}

const [countArg, seedArg] = process.argv.slice(2);
realRoot = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "asrs-asset-download-")));
try {
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  serverPort = (server.address() as any).port;
  await buildModules();
  rng = Number(seedArg) >>> 0 || 1;
  await handWritten();
  const count = Number(countArg) || 0;
  for (let i = 0; i < count; i++) await randomCase();
} finally {
  server.closeAllConnections();
  server.close();
  fs.rmSync(realRoot, { recursive: true, force: true });
}
process.exit(0);
