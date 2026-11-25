import Log from "@upstream/shared/log/Log";
import { applyOp } from "../logRun";

// oracle:logRealmRun: a logRun script that may write to what every object inherits. An op that
// throws pushes {threw: error}, the ops go on, and the realm's prototypes are restored after the
// last one. One op is added:
//   ["type", key]   getType(key)

export function run(enableTimestampSetCache: boolean, ops: any[][]): unknown[] {
  const inherited = Object.getOwnPropertyNames(Object.prototype)
    .filter((name) => name !== "__proto__")
    .map((name) => (Object.prototype as any)[name]);
  const before = [Object.prototype, ...inherited].map((target) => [target, Object.getOwnPropertyNames(target)]);
  const state = { log: new Log(enableTimestampSetCache) };
  const results: unknown[] = [];
  try {
    for (const op of ops) {
      try {
        if (op[0] === "type") results.push(state.log.getType(op[1]));
        else applyOp(state, op, results);
      } catch (e) {
        results.push({ threw: e });
      }
    }
  } finally {
    before.forEach(([target, names]) => {
      Object.getOwnPropertyNames(target)
        .filter((name) => !names.includes(name))
        .forEach((name) => delete target[name]);
    });
  }
  return results;
}
