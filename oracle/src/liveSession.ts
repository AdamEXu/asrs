import "./shims";
import * as fs from "node:fs";
import { DEFAULT_PREFS } from "@upstream/shared/Preferences";
import Log from "@upstream/shared/log/Log";
import FTCDashboardSource from "@upstream/hub/dataSources/ftcdashboard/FTCDashboardSource";
import { LiveDataSource } from "@upstream/hub/dataSources/LiveDataSource";
import { NT4Publisher } from "@upstream/hub/dataSources/nt4/NT4Publisher";
import NT4Source, { NT4Mode } from "@upstream/hub/dataSources/nt4/NT4Source";
import PhoenixDiagnosticsSource from "@upstream/hub/dataSources/PhoenixDiagnosticsSource";
import RLOGServerSource from "@upstream/hub/dataSources/rlog/RLOGServerSource";
import { fromTree } from "./cve1";
import { dump } from "./logRun";
import { ClientStep, LiveLoop } from "./live/loop";
import { Net, Trace } from "./live/net";
import { NT4Server } from "./live/nt4Server";
import { V8MathRandom } from "./live/random";
import { RLOGMain } from "./live/rlogMain";
import { MockServer, Outbox, ServerContext, ServerHost } from "./live/server";
import { FTCServer, PhoenixServer, RLOGServer } from "./live/simpleServers";

// Runs one live session (parity/specs/live/<protocol>/<name>.session.json, format in
// oracle/README.md): upstream's real source against the session's mock server on a fake clock,
// printing the trace, one JSON line per event.
//   liveSession <session file>

const NT4_MODES: { [mode: string]: NT4Mode } = {
  default: NT4Mode.Default,
  akit: NT4Mode.AdvantageKit,
  systemcore: NT4Mode.Systemcore,
  ds: NT4Mode.DriverStation
};

const SERVERS: { [protocol: string]: new (out: Outbox, ctx: ServerContext, config: any) => MockServer } = {
  nt4: NT4Server,
  rlog: RLOGServer,
  phoenix: PhoenixServer,
  ftc: FTCServer
};

const SOURCES: { [source: string]: string } = {
  nt4: "nt4",
  "nt4-publisher": "nt4",
  rlog: "rlog",
  phoenix: "phoenix",
  ftc: "ftc"
};

function fail(message: string): never {
  throw new Error(`liveSession: ${message}`);
}

function integer(value: unknown, what: string, min: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < min) fail(`${what} must be an integer >= ${min}`);
  return value as number;
}

const g = globalThis as any;

async function main() {
  const file = process.argv[2];
  if (file === undefined) fail("usage: liveSession <session file>");
  const session = JSON.parse(fs.readFileSync(file, "utf8"));

  const protocol: string = session.protocol;
  if (!(protocol in SERVERS)) fail(`unknown protocol ${JSON.stringify(protocol)}`);
  const seed = BigInt(integer(session.seed, "seed", 1));
  const epochMs = integer(session.epochMs ?? 1700000000000, "epochMs", 0);
  const latencyMs = integer(session.latencyMs ?? 1, "latencyMs", 1);
  const endMs = integer(session.endMs, "endMs", 0);
  const client = session.client ?? fail("needs \"client\"");
  if (SOURCES[client.source] !== protocol) fail(`source ${JSON.stringify(client.source)} does not speak ${protocol}`);
  const address: string = client.address ?? "127.0.0.1";
  if (client.source === "nt4" && !(client.mode in NT4_MODES)) fail(`unknown nt4 mode ${JSON.stringify(client.mode)}`);
  if (client.source === "nt4-publisher" && client.mode !== "sim" && client.mode !== "robot") {
    fail(`publisher mode must be "sim" or "robot"`);
  }

  const loop = new LiveLoop(epochMs, new V8MathRandom(seed));
  const trace = new Trace(loop);
  const outbox: { current: Outbox | null } = { current: null };
  const relay = new Proxy({} as Outbox, { get: (_t, name) => (outbox.current as any)[name].bind(outbox.current) });
  const server = new SERVERS[protocol](relay, { epochMs, now: () => loop.now }, session.server ?? {});

  let lastT = 0;
  const serverSteps: { t: number; op: any[] }[] = [];
  const clientOps: { t: number; op: any[] }[] = [];
  const allSteps = session.steps ?? [];
  if (!Array.isArray(allSteps)) fail("steps must be a list");
  for (const [i, step] of allSteps.entries()) {
    const t = integer(step.t, `steps[${i}].t`, 0);
    if (t < lastT || t > endMs) fail(`steps[${i}]: times must be non-decreasing and at most endMs`);
    lastT = t;
    if (Array.isArray(step.server) === Array.isArray(step.client)) fail(`steps[${i}] needs exactly one of "server" and "client"`);
    (step.server ? serverSteps : clientOps).push({ t, op: step.server ?? step.client });
  }

  const host = new ServerHost(server, serverSteps, latencyMs, () => loop.now);
  const net = new Net(loop, host, trace, latencyMs);
  outbox.current = net;
  loop.server = host;

  const logIds = new Map<Log, number>();
  const logId = (log: Log | null) => {
    if (log === null || log === undefined) return null;
    if (!logIds.has(log)) logIds.set(log, logIds.size);
    return logIds.get(log)!;
  };

  let tabs: string[] = [...(client.tabs ?? [])];
  let sidebar: string[] = [...(client.sidebar ?? [])];
  let selected: number | null = null;
  let hovered: number | null = null;
  let liveSource: LiveDataSource | null = null;
  let publisher: NT4Publisher | null = null;

  const saved = {
    preferences: g.preferences,
    log: g.log,
    tabs: g.tabs,
    sidebar: g.sidebar,
    selection: g.selection,
    platform: g.platform,
    sendMainMessage: g.sendMainMessage
  };
  const savedConsole = { log: console.log, info: console.info, warn: console.warn, error: console.error };
  const ignoreRejection = () => {};

  const restore: (() => void)[] = [];
  try {
    loop.install();
    restore.push(() => loop.uninstall());
    restore.push(net.install());
    process.on("unhandledRejection", ignoreRejection);
    restore.push(() => process.off("unhandledRejection", ignoreRejection));
    console.log = console.info = console.warn = console.error = () => {};
    restore.push(() => Object.assign(console, savedConsole));
    restore.push(() => Object.assign(g, saved));

    g.preferences = client.prefs === null ? null : Object.assign({}, DEFAULT_PREFS, client.prefs ?? {});
    g.log = new Log();
    logId(g.log);
    g.platform = client.platform ?? "darwin";
    g.tabs = { getActiveFields: () => [...tabs] };
    g.sidebar = { getActiveFields: () => [...sidebar] };
    g.selection = { getSelectedTime: () => selected, getHoveredTime: () => hovered };

    const rlogMain = new RLOGMain({ createConnection: (options) => net.createConnection(options) }, (_window, name, data) => {
      loop.deliver(0, () => {
        if (name === "live-data" && liveSource !== null) liveSource.handleMainMessage(data);
      });
      return true;
    });
    g.sendMainMessage = (name: string, data?: unknown) => {
      if (name === "live-rlog-start" || name === "live-rlog-stop") loop.deliver(0, () => rlogMain.handle(name, data));
      else trace.add("main", undefined, [name, data ?? null]);
    };

    const guarded = (run: () => void) => {
      try {
        run();
      } catch (error) {
        trace.add("throw", undefined, String(error));
      }
    };

    const connect = () => {
      if (client.source === "nt4-publisher") {
        publisher?.stop();
        publisher = new NT4Publisher(client.mode === "sim", (status) => trace.add("status", undefined, status));
        return;
      }
      liveSource?.stop();
      publisher?.stop();
      if (!g.preferences) return;
      switch (client.source) {
        case "nt4":
          liveSource = new NT4Source(NT4_MODES[client.mode]);
          break;
        case "rlog":
          liveSource = new RLOGServerSource();
          break;
        case "phoenix":
          liveSource = new PhoenixDiagnosticsSource();
          break;
        case "ftc":
          liveSource = new FTCDashboardSource();
          break;
      }
      liveSource!.connect(
        address,
        (status) => trace.add("status", undefined, status),
        (log, timeSupplier) => {
          g.log = log;
          trace.add("output", undefined, [logId(log), timeSupplier()]);
        }
      );
    };

    const tuner = () => {
      const t = liveSource?.getTuner() ?? null;
      if (t === null) throw "no tuner";
      return t;
    };

    const runOp = (op: any[]) => {
      switch (op[0]) {
        case "connect":
          connect();
          break;
        case "stop":
          liveSource?.stop();
          publisher?.stop();
          break;
        case "prefs":
          g.preferences = op[1] === null ? null : Object.assign(g.preferences ?? { ...DEFAULT_PREFS }, op[1]);
          break;
        case "fields":
          tabs = [...op[1]];
          sidebar = [...op[2]];
          break;
        case "log":
          (g.log as any)[op[1]](...op.slice(2).map(fromTree));
          break;
        case "select":
          selected = op[1];
          hovered = op[2];
          break;
        case "tune":
          tuner().publish(op[1], op[2]);
          break;
        case "untune":
          tuner().unpublish(op[1]);
          break;
        case "dump":
          trace.add("dump", undefined, [logId(g.log), dump(g.log)]);
          break;
        default:
          fail(`unknown client op ${JSON.stringify(op[0])}`);
      }
    };

    for (const { op } of clientOps) {
      if (!["connect", "stop", "prefs", "fields", "log", "select", "tune", "untune", "dump"].includes(op[0])) {
        fail(`unknown client op ${JSON.stringify(op[0])}`);
      }
      if (op[0] === "log" && typeof (Log.prototype as any)[op[1]] !== "function") fail(`Log has no method ${op[1]}`);
    }
    const steps: ClientStep[] = clientOps.map(({ t, op }) => ({ t, run: () => guarded(() => runOp(op)) }));
    await loop.run(endMs, steps);
    trace.add("dump", undefined, [logId(g.log), dump(g.log)]);
  } finally {
    for (const undo of restore.reverse()) undo();
  }
  process.stdout.write(trace.lines.map((line) => line + "\n").join(""));
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exit(1);
});
