import { MessageChannel, Worker, isMainThread, receiveMessageOnPort, workerData } from "node:worker_threads";
import type { MessagePort } from "node:worker_threads";
import Log from "@upstream/shared/log/Log";
import StructDecoder from "@upstream/shared/log/StructDecoder";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";
import { applyOp } from "./logRun";

// Runs `oracle:logScript` op lists (see crates/asrs-cli/src/log_run.rs) across real JS realms.
// Each realm is its own worker thread, so it has its own @msgpack/msgpack key cache, protobufjs
// counters and Math.random, as the hub renderer and a decode worker do. A transfer posts
// `toSerialized()` between threads, so the receiving realm sees a structured clone, as
// HistoricalDataSource does. Math.random in a realm is a seeded stream whose consumed values
// become the vector's per-realm random lists.
//
// A struct array decode that would run past 2^20 items makes upstream allocate until the realm
// dies. The realm flags it and the generator drops the script.

const NEVER_ITEMS = 2 ** 20;

type Request =
  | { cmd: "new"; id: string; cache: boolean }
  | { cmd: "serialize"; id: string }
  | { cmd: "deserialize"; id: string; data: unknown }
  | { cmd: "merge"; target: string; source: string; prefix: string }
  | { cmd: "op"; id: string; op: unknown[] }
  | { cmd: "finish" };

function guardNeverCompletes(flag: { never: boolean }) {
  const proto = StructDecoder.prototype as any;
  const decodeArray = proto.decodeArray;
  proto.decodeArray = function (this: any, name: string, value: Uint8Array, arrayLength?: number) {
    if (name in this.schemas) {
      const length = arrayLength ?? value.length / (this.schemas[name].length / 8);
      if (length > NEVER_ITEMS) {
        flag.never = true;
        throw new Error("oracle: decodeArray never completes");
      }
    }
    return decodeArray.call(this, name, value, arrayLength);
  };
}

/** Starts the realm server when this bundle runs as a realm thread; returns whether it did. */
export function serveRealm(): boolean {
  if (isMainThread || workerData?.logRealm !== true) return false;
  const port = workerData.port as MessagePort;
  const signal = workerData.signal as Int32Array;
  const rng = new Rng(workerData.seed, `realm/${workerData.name}`);
  const consumed: number[] = [];
  Math.random = () => {
    const x = rng.unit();
    consumed.push(x);
    return x;
  };
  const flag = { never: false };
  guardNeverCompletes(flag);
  const logs = new Map<string, { log: Log }>();

  const handle = (msg: Request): unknown => {
    const results: unknown[] = [];
    switch (msg.cmd) {
      case "new":
        logs.set(msg.id, { log: new Log(msg.cache) });
        break;
      case "serialize":
        return logs.get(msg.id)!.log.toSerialized();
      case "deserialize":
        try {
          logs.set(msg.id, { log: Log.fromSerialized(msg.data) });
        } catch (e) {
          results.push({ threw: e });
          logs.set(msg.id, { log: new Log() });
        }
        break;
      case "merge": {
        const source = logs.get(msg.source)!.log;
        logs.delete(msg.source);
        try {
          results.push(logs.get(msg.target)!.log.mergeWith(source, msg.prefix));
        } catch (e) {
          results.push({ threw: e });
        }
        break;
      }
      case "op":
        applyOp(logs.get(msg.id)!, msg.op as any[], results);
        break;
      case "finish":
        return { random: consumed, never: flag.never };
    }
    return toTree(results);
  };

  port.on("message", (msg: Request) => {
    let reply: unknown;
    try {
      reply = { ok: handle(msg) };
    } catch (e) {
      reply = { error: String((e as Error)?.stack ?? e) };
    }
    port.postMessage(reply);
    Atomics.store(signal, 0, 1);
    Atomics.notify(signal, 0);
  });
  return true;
}

class RealmClient {
  private worker: Worker;
  private port: MessagePort;
  private signal = new Int32Array(new SharedArrayBuffer(4));

  constructor(name: string, seed: number) {
    const { port1, port2 } = new MessageChannel();
    this.port = port1;
    this.worker = new Worker(new URL(import.meta.url), {
      workerData: { logRealm: true, port: port2, signal: this.signal, name, seed },
      transferList: [port2]
    });
  }

  call(msg: Request): any {
    Atomics.store(this.signal, 0, 0);
    this.port.postMessage(msg);
    for (;;) {
      const waited = Atomics.wait(this.signal, 0, 0, 120000);
      const reply = receiveMessageOnPort(this.port);
      if (reply !== undefined) {
        if ("error" in reply.message) throw new Error(`realm: ${reply.message.error}`);
        return reply.message.ok;
      }
      if (waited === "timed-out") throw new Error("realm: no reply");
    }
  }

  async close(): Promise<void> {
    this.port.close();
    await this.worker.terminate();
  }
}

export interface ScriptRun {
  realms: [string, number[]][];
  results: unknown[];
  never: boolean;
}

/** Runs a script; `results` are CVE-1 trees. */
export async function runScript(realmNames: string[], ops: any[][], seed: number): Promise<ScriptRun> {
  const realms = new Map(realmNames.map((name) => [name, new RealmClient(name, seed)]));
  const owner = new Map<string, string>();
  const results: unknown[] = [];
  const realmOf = (id: string) => realms.get(owner.get(id)!)!;
  try {
    for (const op of ops) {
      switch (op[0]) {
        case "new":
          owner.set(op[1], op[2]);
          realms.get(op[2])!.call({ cmd: "new", id: op[1], cache: op[3] });
          break;
        case "transfer": {
          const data = realmOf(op[1]).call({ cmd: "serialize", id: op[1] });
          owner.set(op[2], op[3]);
          results.push(...realms.get(op[3])!.call({ cmd: "deserialize", id: op[2], data }));
          break;
        }
        case "merge":
          results.push(...realmOf(op[1]).call({ cmd: "merge", target: op[1], source: op[2], prefix: op[3] }));
          owner.delete(op[2]);
          break;
        case "op":
          results.push(...realmOf(op[1]).call({ cmd: "op", id: op[1], op: op.slice(2) }));
          break;
        default:
          throw new Error(`logScript: unknown op ${JSON.stringify(op[0])}`);
      }
    }
    let never = false;
    const random: [string, number[]][] = [];
    for (const [name, realm] of realms) {
      const finished = realm.call({ cmd: "finish" });
      never ||= finished.never;
      random.push([name, finished.random]);
    }
    return { realms: random, results, never };
  } finally {
    for (const realm of realms.values()) await realm.close();
  }
}
