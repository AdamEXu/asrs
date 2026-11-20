import Log from "@upstream/shared/log/Log";
import * as geometry from "@upstream/shared/geometry";
import { runWithLog } from "./logRun";

// Replays logRun write ops against a real upstream Log, then runs a list of
// geometry calls and reports each result as {ok: ...} or {throw: ...}.
//   ["grabPose2d", key, timestamp, uuid]
//   ["grabPosesAuto", key, logType, timestamp, uuid, numberRotationUnits]
//   ["grabNumberRotation", key, timestamp, unit, uuid]
//   ["grabModuleVelocities", key, timestamp, arrangement, uuid]
//   ["grabHeatmapData", key, logType, timeRange, timelineRange | null, uuid]
//     timelineRange stubs window.selection.getTimelineRange(); null leaves
//     window.selection undefined, which is the TypeError a real session hits
//     with no selection.
// Every other exported grab* name dispatches as fn(log, ...args).

function grabHeatmap(log: Log, args: any[]): unknown {
  const [key, logType, timeRange, timelineRange, uuid] = args;
  const g = globalThis as any;
  if (g.window === undefined) g.window = {};
  const saved = g.window.selection;
  g.window.selection =
    timelineRange === null || timelineRange === undefined
      ? undefined
      : { getTimelineRange: () => timelineRange };
  try {
    return geometry.grabHeatmapData(log, key, logType, timeRange, uuid ?? undefined);
  } finally {
    g.window.selection = saved;
  }
}

export function run(enableTimestampSetCache: boolean, ops: any[][], calls: any[][]): unknown[] {
  const { log } = runWithLog(enableTimestampSetCache, ops);
  return calls.map((call) => {
    try {
      const [name, ...args] = call;
      if (name === "grabHeatmapData") return { ok: grabHeatmap(log, args) };
      const fn = (geometry as any)[name];
      if (typeof fn !== "function") throw new Error(`geometryRun: unknown call ${JSON.stringify(name)}`);
      return { ok: fn(log, ...args) };
    } catch (e) {
      return { throw: e };
    }
  });
}
