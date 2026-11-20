import Log from "@upstream/shared/log/Log";
import { getMergeOffset } from "@upstream/shared/log/MergeCalculator";

// Rebuilds two logs from their toSerialized() dumps and returns upstream's
// getMergeOffset(target, source). args[0] is a case label the run ignores.
export function run(_name: string, targetSerialized: any, sourceSerialized: any): number {
  return getMergeOffset(Log.fromSerialized(targetSerialized), Log.fromSerialized(sourceSerialized));
}
