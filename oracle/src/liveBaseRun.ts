import { LiveDataSource, LiveDataSourceStatus } from "@upstream/hub/dataSources/LiveDataSource";
import Log from "@upstream/shared/log/Log";
import { ScriptedClock } from "./live/clock";
import { dump } from "./logRun";

// Drives upstream's LiveDataSource through a subclass that only exposes its protected members,
// under a ScriptedClock. Script ops:
//   ["connect", address, enableClearData | undefined]   undefined calls connect with 3 args
//   ["setStatus", status]
//   ["newLog"], ["dropLog"]   this.log = new Log() | null
//   ["put", key, timestamp, value]   this.log.putNumber, skipped while this.log is null
//   ["output", supplier]   this.outputCallback(this.log, supplier) if it is set, where supplier
//                          is ["wall", zeroSecs] (Date.now() / 1000 - zeroSecs) or ["const", v]
//   ["prefs", 0 | 1 | 2, value]   window.preferences = undefined | {} | {liveDiscard: value}
//   ["advance", ms]   ScriptedClock.advance
//   ["stop"]
// Records, in the order they happen:
//   ["uuid", UUID]   first, from Math.random returning the script's values
//   ["status", status, Date.now()]   the status callback
//   ["output", Date.now(), log === null, timeSupplier()]   the output callback
//   ["state", status, address], ["range", getTimestampRange() | null], ["dump", dump | null]
//   for the query ops "state", "range" and "dump"

class ScriptedSource extends LiveDataSource {
  get uuid(): string {
    return this.UUID;
  }

  get currentStatus(): LiveDataSourceStatus {
    return this.status;
  }

  get currentAddress(): string | null {
    return this.address;
  }

  get currentLog(): Log | null {
    return this.log;
  }

  set currentLog(log: Log | null) {
    this.log = log;
  }

  changeStatus(status: LiveDataSourceStatus): void {
    this.setStatus(status);
  }

  emit(timeSupplier: () => number): void {
    if (this.outputCallback !== null) this.outputCallback(this.log as Log, timeSupplier);
  }
}

function supplier(spec: [string, number]): () => number {
  const [kind, value] = spec;
  if (kind === "wall") return () => Date.now() / 1000 - value;
  if (kind === "const") return () => value;
  throw new Error(`liveBaseRun: unknown supplier ${JSON.stringify(kind)}`);
}

export function run(random: number[], startMs: number, ops: any[][]): unknown[] {
  const g = globalThis as any;
  const hadPreferences = "preferences" in g;
  const savedPreferences = g.preferences;
  const realRandom = Math.random;
  const clock = new ScriptedClock(startMs);
  const results: unknown[] = [];
  delete g.preferences;
  clock.install();
  try {
    let next = 0;
    Math.random = () => random[next++];
    const source = new ScriptedSource();
    Math.random = realRandom;
    results.push(["uuid", source.uuid]);

    for (const op of ops) {
      const [name, ...args] = op;
      switch (name) {
        case "connect": {
          const statusCallback = (status: LiveDataSourceStatus) => results.push(["status", status, Date.now()]);
          const outputCallback = (log: Log, timeSupplier: () => number) =>
            results.push(["output", Date.now(), log === null, timeSupplier()]);
          if (args[1] === undefined) source.connect(args[0], statusCallback, outputCallback);
          else source.connect(args[0], statusCallback, outputCallback, args[1]);
          break;
        }
        case "setStatus":
          source.changeStatus(args[0]);
          break;
        case "newLog":
          source.currentLog = new Log();
          break;
        case "dropLog":
          source.currentLog = null;
          break;
        case "put":
          source.currentLog?.putNumber(args[0], args[1], args[2]);
          break;
        case "output":
          source.emit(supplier(args[0]));
          break;
        case "prefs":
          if (args[0] === 0) delete g.preferences;
          else g.preferences = args[0] === 1 ? {} : { liveDiscard: args[1] };
          break;
        case "advance":
          clock.advance(args[0]);
          break;
        case "stop":
          source.stop();
          break;
        case "state":
          results.push(["state", source.currentStatus, source.currentAddress]);
          break;
        case "range": {
          const log = source.currentLog;
          results.push(["range", log === null ? null : log.getTimestampRange()]);
          break;
        }
        case "dump": {
          const log = source.currentLog;
          results.push(["dump", log === null ? null : dump(log)]);
          break;
        }
        default:
          throw new Error(`liveBaseRun: unknown op ${JSON.stringify(name)}`);
      }
    }
  } finally {
    Math.random = realRandom;
    clock.uninstall();
    if (hadPreferences) g.preferences = savedPreferences;
    else delete g.preferences;
  }
  return results;
}
