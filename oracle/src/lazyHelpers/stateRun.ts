import { decodePovBitfield, getJoystickState } from "@upstream/shared/log/LogJoystickState";
import { getMechanismState, mergeMechanismStates } from "@upstream/shared/log/LogMechanismState";
import { runWithLog } from "../logRun";

// Replays logRun write ops against a real upstream Log, then runs a list of
// joystick and mechanism calls, reporting each result as {ok: ...} or
// {throw: ...}.
//   ["getJoystickState", joystickId, time]
//   ["getMechanismState", key, time]
//   ["mergeMechanismStates", [key, ...], time]   merges the non-null
//     getMechanismState results as MechanismController does, or null if none
//   ["decodePovBitfield", value]

export function run(enableTimestampSetCache: boolean, ops: any[][], calls: any[][]): unknown[] {
  const { log } = runWithLog(enableTimestampSetCache, ops);
  return calls.map((call) => {
    try {
      const [name, ...args] = call;
      switch (name) {
        case "getJoystickState":
          return { ok: getJoystickState(log, args[0], args[1]) };
        case "getMechanismState":
          return { ok: getMechanismState(log, args[0], args[1]) };
        case "mergeMechanismStates": {
          const states = (args[0] as string[])
            .map((key) => getMechanismState(log, key, args[1]))
            .filter((state) => state !== null);
          return { ok: states.length === 0 ? null : mergeMechanismStates(states) };
        }
        case "decodePovBitfield":
          return { ok: decodePovBitfield(args[0]) };
        default:
          throw new Error(`stateRun: unknown call ${JSON.stringify(name)}`);
      }
    } catch (e) {
      return { throw: e };
    }
  });
}
