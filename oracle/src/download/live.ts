import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { buildMain, upstreamRequire } from "./bundle";
import { World, electronFake, fakeFs } from "./world";

// main.ts's download section with the real ssh2 package against real servers on 127.0.0.1, one
// process per scenario of the spec's scenario file. crates/asrs-services/tests/download_live.rs
// runs the same scenarios against the port; both sides build the same fixture:
//   sshd    an unprivileged sshd on an ephemeral port with throwaway keys. A real sshd only logs
//           in the user it runs as, so the scenario names the robot account ("systemcore" or
//           "lvuser") that exists: connecting as it logs in with the fixture's key, every other
//           username goes through upstream's own password authentication and fails.
//   proxy   every sshd scenario connects through a byte-counting proxy that can drop the
//           connection after `bytes` from the server, or on a `drop` step
//   raw     a TCP server that sends `send` (latin1), then holds, closes or resets the connection, or
//           lingers: holds it and never closes its own side
//           (`when`: at once, after the client's ident, or after its KEXINIT)
//   closed  a port nothing listens on
//   blackhole  no server: `host` is a TEST-NET address that never answers
// A scenario with `level: "client"` runs one ssh2 Client with no main.ts: its effects are the
// events error (with the message), ready and close, and `onError` says whether the error listener
// calls Client.end(), as main.ts's downloadStart does.
// Timers main.ts sets run ten times faster; `readyTimeout` is the scenario's `timeoutMs`.
// Each scenario is one record {scenario, effects}; after a last `await` step nothing later than
// the effect it waited for is kept. `attempt` is a Client.connect call, `uncaught` an uncaught exception (repeats of the
// same one in a row are one effect), `saved` the local folder at the end
// (names only where the scenario is `partial`). set-list is sorted by
// name (the server's order is its filesystem's) and repeats in a row are one effect; a run of
// set-progress objects is one {total, complete}. The fixture's folder and port read <root>, <port>.

declare const __filename: string;

const SETTLE_LIMIT_MS = 30000;
const TIME_SCALE = 10;

type TreeEntry = { path: string; size?: number; dir?: boolean; link?: string; mode?: number };
type Step = { op: string; [key: string]: any };
type Scenario = {
  name: string;
  server: any;
  host?: string;
  path?: string;
  timeoutMs?: number;
  partial?: boolean;
  level?: "client";
  onError?: "end" | "ignore";
  steps: Step[];
};
type Spec = { tree: TreeEntry[]; scenarios: Scenario[] };

const out = (value: object) => new Promise<void>((resolve) => process.stdout.write(JSON.stringify(value) + "\n", () => resolve()));
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export function fileBytes(size: number): Buffer {
  const data = Buffer.alloc(size);
  for (let i = 0; i < size; i++) data[i] = (i * 31 + (i >>> 8) * 7 + (i >>> 16)) & 0xff;
  return data;
}

function readSpec(): { spec: Spec; file: string } {
  const entry = process.env.ASRS_ORACLE_SPEC;
  if (entry === undefined) throw new Error("genDownload live runs through `cargo xtask oracle gen`");
  const file = path.join(path.dirname(entry), "download-live.scenarios.json");
  return { spec: JSON.parse(fs.readFileSync(file, "utf8")), file };
}

/** Runs every scenario of the spec, each in its own process. */
export async function runLive(only: string[]): Promise<void> {
  const { spec } = readSpec();
  const scenarios = spec.scenarios.filter((s) => only.length === 0 || only.includes(s.name));
  const results = new Array<string>(scenarios.length);
  const failed: string[] = [];
  let next = 0;
  const worker = async () => {
    for (let i = next++; i < scenarios.length; i = next++) {
      const child = spawn(process.execPath, [process.argv[1], "live-one", scenarios[i].name], {
        env: process.env,
        stdio: ["ignore", "pipe", "inherit"]
      });
      let text = "";
      child.stdout.on("data", (chunk) => (text += chunk));
      const code = await new Promise((resolve) => child.on("close", resolve));
      if (code !== 0) failed.push(scenarios[i].name);
      results[i] = text;
    }
  };
  await Promise.all([worker(), worker(), worker(), worker()]);
  if (failed.length > 0) throw new Error(`failed: ${failed.join(", ")}`);
  process.stdout.write(results.join(""));
}

class Fixture {
  root: string;
  port = 0;
  key = "";
  /** The proxy forwards nothing more from the server. */
  stalled = false;
  private sshd: ChildProcess | null = null;
  private servers: net.Server[] = [];
  private links: { client: net.Socket; server: net.Socket; dropped: boolean }[] = [];

  constructor(private spec: Spec, private scenario: Scenario) {
    this.root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "asrs-sshd-")));
    fs.mkdirSync(path.join(this.root, "local"));
  }

  async start(): Promise<void> {
    for (const entry of this.spec.tree) this.put(entry);
    const { server } = this.scenario;
    switch (server.kind) {
      case "closed": {
        const listener = net.createServer();
        this.port = await listen(listener);
        await new Promise((resolve) => listener.close(resolve));
        break;
      }
      case "raw": {
        const listener = net.createServer((socket) => {
          socket.on("error", () => undefined);
          const act = () => {
            if (server.then === "fin") socket.end();
            else if (server.then === "rst") socket.resetAndDestroy();
          };
          let received = Buffer.alloc(0);
          if (server.then === "linger") socket.allowHalfOpen = true;
          if (server.send !== "") socket.write(Buffer.from(server.send, "latin1"), () => server.when === "accept" && act());
          else if (server.when === "accept") act();
          socket.on("data", (data) => {
            received = Buffer.concat([received, data]);
            const ident = received.indexOf("\r\n") + 2;
            if (server.when === "data" && ident > 1) act();
            else if (server.when === "kexinit" && ident > 1 && received.length >= ident + 4 + (received.length >= ident + 4 ? received.readUInt32BE(ident) : 0)) act();
          });
        });
        this.servers.push(listener);
        this.port = await listen(listener);
        break;
      }
      case "blackhole":
        this.port = 22;
        break;
      case "sshd":
        await this.startSshd(server);
        break;
      default:
        throw new Error(`unknown server ${server.kind}`);
    }
  }

  put(entry: TreeEntry): void {
    const full = path.join(this.root, "remote", entry.path);
    fs.mkdirSync(path.dirname(full), { recursive: true });
    if (entry.dir) fs.mkdirSync(full, { recursive: true });
    else if (entry.link !== undefined) fs.symlinkSync(entry.link, full);
    else fs.writeFileSync(full, fileBytes(entry.size!));
    if (entry.mode !== undefined) fs.chmodSync(full, entry.mode);
  }

  private async startSshd(server: any): Promise<void> {
    const file = (name: string) => path.join(this.root, name);
    const keygen = (type: string, name: string) => {
      const args = ["-q", "-t", type, "-N", "", "-C", "asrs-fixture", "-f", file(name)];
      if (spawnSync("ssh-keygen", args).status !== 0) throw new Error(`ssh-keygen ${type}`);
    };
    keygen("ed25519", "user");
    keygen(server.hostKey ?? "ed25519", "host");
    this.key = fs.readFileSync(file("user"), "utf8");
    fs.copyFileSync(file("user.pub"), file("authorized"));
    const reserve = net.createServer();
    const sshdPort = await listen(reserve);
    await new Promise((resolve) => reserve.close(resolve));
    const subsystem = server.subsystem === undefined ? sftpServer() : server.subsystem;
    const base = [
      `Port ${sshdPort}`,
      "ListenAddress 127.0.0.1",
      `HostKey ${file("host")}`,
      "PidFile none",
      `AuthorizedKeysFile ${file("authorized")}`,
      "StrictModes no",
      "UsePAM no",
      "PasswordAuthentication yes",
      "KbdInteractiveAuthentication no",
      ...(subsystem === null ? [] : [`Subsystem sftp ${String(subsystem).replace("SFTP_SERVER", sftpServer())}`]),
      ...(server.options ?? [])
    ];
    const config = file("sshd_config");
    fs.writeFileSync(config, base.concat("PerSourcePenalties no").join("\n") + "\n");
    if (spawnSync(sshdPath(), ["-t", "-f", config]).status !== 0) fs.writeFileSync(config, base.join("\n") + "\n");
    const check = spawnSync(sshdPath(), ["-t", "-f", config]);
    if (check.status !== 0) throw new Error(`sshd -t: ${check.stderr}`);
    this.sshd = spawn(sshdPath(), ["-D", "-e", "-f", config], { stdio: ["ignore", "ignore", fs.openSync(file("sshd.log"), "w")] });
    for (let i = 0; !(await reachable(sshdPort)); i++) {
      if (i > 200) throw new Error("sshd did not start: " + fs.readFileSync(file("sshd.log"), "utf8"));
      await sleep(25);
    }
    const proxy = net.createServer({ allowHalfOpen: true }, (client) => {
      if (this.stalled) return;
      const link = { client, server: net.connect({ port: sshdPort, host: "127.0.0.1", allowHalfOpen: true }), dropped: false };
      this.links.push(link);
      client.on("error", () => link.server.destroy());
      link.server.on("error", () => client.destroy());
      client.on("data", (data) => link.dropped || link.server.write(data));
      client.on("end", () => link.server.end());
      let sent = 0;
      const limit: number = server.drop?.bytes ?? Infinity;
      link.server.on("data", (data) => {
        if (link.dropped || this.stalled) return;
        const left = limit - sent;
        sent += data.length;
        client.write(data.subarray(0, Math.max(0, Math.min(left, data.length))));
        if (sent >= limit) this.drop(link, server.drop.how);
      });
      link.server.on("end", () => link.dropped || client.end());
    });
    this.servers.push(proxy);
    this.port = await listen(proxy);
  }

  private drop(link: { client: net.Socket; server: net.Socket; dropped: boolean }, how: string): void {
    if (link.dropped) return;
    link.dropped = true;
    link.server.destroy();
    if (how === "rst") link.client.resetAndDestroy();
    else if (how === "fin") link.client.end();
    else throw new Error(`unknown drop ${how}`);
  }

  dropAll(how: string): void {
    for (const link of this.links) this.drop(link, how);
  }

  saved(): object {
    const files: { [name: string]: object } = {};
    const visit = (dir: string, prefix: string) => {
      for (const name of fs.readdirSync(dir).sort()) {
        const full = path.join(dir, name);
        if (fs.statSync(full).isDirectory()) {
          files[prefix + name + "/"] = {};
          visit(full, prefix + name + "/");
        } else {
          const data = fs.readFileSync(full);
          files[prefix + name] = this.scenario.partial ? {} : { size: data.length, sha256: createHash("sha256").update(data).digest("hex").slice(0, 16) };
        }
      }
    };
    visit(path.join(this.root, "local"), "");
    return files;
  }

  async stop(): Promise<void> {
    this.servers.forEach((server) => server.close());
    this.links.forEach((link) => (link.client.destroy(), link.server.destroy()));
    if (this.sshd !== null) {
      const closed = new Promise((resolve) => this.sshd!.on("close", resolve));
      this.sshd.kill("SIGTERM");
      await closed;
    }
    const restore = (dir: string) => {
      fs.chmodSync(dir, 0o700);
      for (const entry of fs.readdirSync(dir, { withFileTypes: true })) if (entry.isDirectory()) restore(path.join(dir, entry.name));
    };
    restore(this.root);
    fs.rmSync(this.root, { recursive: true, force: true });
  }
}

const listen = (server: net.Server): Promise<number> =>
  new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve((server.address() as net.AddressInfo).port)));
const reachable = (port: number): Promise<boolean> =>
  new Promise((resolve) => {
    const socket = net.connect(port, "127.0.0.1");
    socket.on("data", () => (socket.destroy(), resolve(true))).on("error", () => resolve(false));
  });
const firstExisting = (candidates: string[]): string => {
  const found = candidates.find((candidate) => fs.existsSync(candidate));
  if (found === undefined) throw new Error(`none of ${candidates.join(", ")} exists`);
  return found;
};
const sshdPath = () => firstExisting(["/usr/sbin/sshd", "/usr/local/sbin/sshd", "/opt/homebrew/sbin/sshd"]);
const sftpServer = () => firstExisting(["/usr/libexec/sftp-server", "/usr/lib/openssh/sftp-server", "/usr/libexec/openssh/sftp-server", "/usr/lib/ssh/sftp-server"]);

export async function runLiveOne(name: string): Promise<void> {
  const { spec } = readSpec();
  const scenario = spec.scenarios.find((s) => s.name === name)!;
  const fixture = new Fixture(spec, scenario);
  let failure: unknown = null;
  const effects: any[] = [];
  let world: World;

  const normalize = (value: any): any => {
    if (typeof value === "string") return value.split(fixture.root).join("<root>").split(String(fixture.port)).join("<port>");
    if (Array.isArray(value)) return value.map(normalize);
    if (value !== null && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, normalize(v)]));
    return value;
  };
  const push = (effect: any) => {
    effect = normalize(effect);
    const previous = effects[effects.length - 1];
    if (effect.k === "send" && effect.name === "set-list") {
      effect.data.sort((a: any, b: any) => (a.name < b.name ? -1 : 1));
    } else if (effect.k === "send" && effect.name === "set-progress" && typeof effect.data === "object") {
      effect = { k: "progress", total: effect.data.total, complete: effect.data.current === effect.data.total };
      if (scenario.partial && effects.some((e) => e.k === "progress")) return;
      if (previous?.k === "progress") effects.pop();
      effects.push(effect);
      return;
    }
    const repeats = effect.k === "uncaught" || effect.name === "set-list";
    if (repeats && JSON.stringify(previous) === JSON.stringify(effect)) return;
    effects.push(effect);
  };
  process.on("uncaughtException", (error) => push({ k: "uncaught", message: error.message }));

  try {
    await fixture.start();
    Object.defineProperty(process, "platform", { value: "darwin" });
    const ssh2 = upstreamRequire()("ssh2");
    if (scenario.level === "client") {
      await runClient(scenario, fixture, ssh2, push, effects);
      await out({ scenario: name, effects });
      await fixture.stop();
      process.exit(0);
    }
    class Client extends ssh2.Client {
      connect(config: any): any {
        push({ k: "attempt", host: config.host, username: config.username, password: config.password });
        const { account } = scenario.server;
        const login = account !== undefined && config.username === account;
        return super.connect({
          ...config,
          port: fixture.port,
          readyTimeout: scenario.timeoutMs ?? 15000,
          ...(login ? { username: os.userInfo().username, password: undefined, privateKey: fixture.key } : {})
        });
      }
    }
    const scale = (ms: number) => (ms === 1000 || ms === 5000 ? ms / TIME_SCALE : ms);
    const realFs = (call: string) => (name: any, ...args: any[]) =>
      (String(name).startsWith(fixture.root) ? (fs as any) : (fakeFs as any))[call](name, ...args);
    const localFs = { existsSync: realFs("existsSync"), statSync: realFs("statSync"), mkdirSync: realFs("mkdirSync") };
    (globalThis as any).__asrsDownload = {
      electron: electronFake,
      fs: { ...fakeFs, default: { ...(fakeFs as any).default, ...localFs }, ...localFs },
      ssh2: { ...ssh2, Client },
      setTimeout: (callback: () => void, ms: number) => setTimeout(callback, scale(ms)),
      clearTimeout: (id: any) => clearTimeout(id),
      setInterval: (callback: () => void, ms: number) => setInterval(callback, scale(ms)),
      clearInterval: (id: any) => clearInterval(id)
    };
    world = new World(await buildMain("posix"));
    await world.start();
    await world.openDownload();
    world.take();
    const dialogs: number[] = [];
    world.effects = new Proxy([] as any[], {
      get(target, prop) {
        if (prop !== "push") return (target as any)[prop];
        return (effect: any) => {
          const plain = { ...effect };
          if (typeof effect.token === "number") {
            dialogs.push(effect.token);
            delete plain.token;
            delete plain.options;
          }
          if ("data" in plain) plain.data = untree(plain.data);
          push(plain);
          return 0;
        };
      }
    });
    const matches = (step: Step) => (e: any) => (e.k === "send" ? e.name === step.name : e.k === step.name);
    const matching = (step: Step) => effects.filter(matches(step)).length;
    for (const step of scenario.steps) {
      switch (step.op) {
        case "start":
          world.fromWindow("start", { address: scenario.host ?? "127.0.0.1", path: path.join(fixture.root, "remote", scenario.path ?? "logs") });
          break;
        case "await": {
          const deadline = Date.now() + SETTLE_LIMIT_MS;
          while (matching(step) < (step.count ?? 1)) {
            if (Date.now() > deadline) throw new Error(`${name}: no ${step.name}: ${JSON.stringify(effects)}`);
            await sleep(1);
          }
          break;
        }
        case "settle":
        case "wait":
          await sleep(step.ms);
          break;
        case "save":
          world.fromWindow("save", step.files);
          break;
        case "answer": {
          const token = dialogs.shift()!;
          if (step.response !== undefined) world.answer(token, { response: step.response });
          else {
            const target = path.join(fixture.root, "local", step.path);
            world.answer(token, { canceled: false, filePath: target, filePaths: [target] });
          }
          break;
        }
        case "drop":
          fixture.dropAll(step.how);
          break;
        case "remote":
          fixture.put(step as any);
          break;
        case "removeRemote":
          fs.rmSync(path.join(fixture.root, "remote", step.path), { recursive: true });
          break;
        case "local":
          fs.writeFileSync(path.join(fixture.root, "local", step.path), fileBytes(step.size));
          break;
        case "close":
          world.fromWindow("close");
          break;
        default:
          throw new Error(`unknown step ${step.op}`);
      }
    }
    const last = scenario.steps[scenario.steps.length - 1];
    if (last.op === "await") {
      let count = last.count ?? 1;
      effects.splice(effects.findIndex((e) => matches(last)(e) && --count === 0) + 1);
    }
    push({ k: "saved", files: fixture.saved() });
    await out({ scenario: name, effects });
  } catch (error) {
    failure = error;
  }
  await fixture.stop();
  if (failure !== null) {
    console.error(failure);
    process.exit(1);
  }
  process.exit(0);
}

async function runClient(scenario: Scenario, fixture: Fixture, ssh2: any, push: (effect: any) => void, effects: any[]): Promise<void> {
  const client = new ssh2.Client();
  client
    .on("error", (error: Error) => {
      push({ k: "error", message: error.message });
      if (scenario.onError === "end") client.end();
    })
    .on("ready", () => push({ k: "ready" }))
    .on("close", () => push({ k: "close" }));
  for (const step of scenario.steps) {
    switch (step.op) {
      case "start": {
        const login = scenario.server.account !== undefined;
        client.connect({
          host: scenario.host ?? "127.0.0.1",
          port: fixture.port,
          forceIPv4: true,
          readyTimeout: scenario.timeoutMs ?? 15000,
          username: login ? os.userInfo().username : "systemcore",
          ...(login ? { privateKey: fixture.key } : { password: "systemcore" })
        });
        break;
      }
      case "await": {
        const deadline = Date.now() + SETTLE_LIMIT_MS;
        while (!effects.some((e) => e.k === step.name)) {
          if (Date.now() > deadline) throw new Error(`${scenario.name}: no ${step.name}: ${JSON.stringify(effects)}`);
          await sleep(1);
        }
        break;
      }
      case "settle":
      case "wait":
        await sleep(step.ms);
        break;
      case "end":
        client.end();
        break;
      case "drop":
        fixture.dropAll(step.how);
        break;
      case "stall":
        fixture.stalled = true;
        break;
      case "sftp":
        try {
          client.sftp((error: Error | undefined) => push({ k: "sftp", error: error ? error.message : null }));
        } catch (error) {
          push({ k: "sftp", threw: (error as Error).message });
        }
        break;
      default:
        throw new Error(`unknown step ${step.op}`);
    }
  }
}

function untree(tree: any): any {
  if (tree === null || typeof tree !== "object") return tree;
  if (Array.isArray(tree)) return tree.map(untree);
  if ("n" in tree) return Buffer.from(tree.n, "hex").readDoubleBE(0);
  if ("o" in tree) return Object.fromEntries(tree.o.map(([k, v]: any) => [k, untree(v)]));
  throw new Error(`unexpected value ${JSON.stringify(tree)}`);
}
