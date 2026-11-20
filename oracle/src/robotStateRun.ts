import Log from "@upstream/shared/log/Log";
import * as robotState from "@upstream/shared/log/RobotState";
import { runWithLog } from "./logRun";

// Replays logRun write ops against a real upstream Log, then runs a list of
// RobotState calls and reports each result as {ok: ...} or {throw: ...}.
// Every exported function dispatches as fn(log, ...args), except:
//   ["getMatchInfo", nowMs]   runs with `new Date()` reporting nowMs, so the
//     "current year" fallback is deterministic; `new Date(ms)` is unchanged.

const RealDate = Date;

function withNow(nowMs: number, body: () => unknown): unknown {
  class ScriptedDate extends RealDate {
    constructor(...args: any[]) {
      if (args.length === 0) {
        super(nowMs);
      } else {
        // @ts-expect-error Date's overloads do not take a rest tuple
        super(...args);
      }
    }
    static now(): number {
      return nowMs;
    }
  }
  (globalThis as any).Date = ScriptedDate;
  try {
    return body();
  } finally {
    (globalThis as any).Date = RealDate;
  }
}

export function run(enableTimestampSetCache: boolean, ops: any[][], calls: any[][]): unknown[] {
  const { log } = runWithLog(enableTimestampSetCache, ops);
  return calls.map((call) => {
    try {
      const [name, ...args] = call;
      if (name === "getMatchInfo") return { ok: withNow(args[0] as number, () => robotState.getMatchInfo(log)) };
      const fn = (robotState as any)[name];
      if (typeof fn !== "function") throw new Error(`robotStateRun: unknown call ${JSON.stringify(name)}`);
      return { ok: fn(log, ...args) };
    } catch (e) {
      return { throw: e };
    }
  });
}
