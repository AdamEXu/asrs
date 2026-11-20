import Log from "@upstream/shared/log/Log";
import LogField from "@upstream/shared/log/LogField";
import LogFieldTree from "@upstream/shared/log/LogFieldTree";
import { findKey } from "@upstream/shared/log/LogKeyUtils";
import { LogFieldIterator, getLogValueText, getOrDefault, getURCLKeys } from "@upstream/shared/log/LogUtil";

// Replays an operation script against a real upstream Log. Writes:
//   ["putRaw" | "putBoolean" | "putNumber" | "putString" | "putBooleanArray" | "putNumberArray"
//     | "putStringArray" | "putJSON", key, timestamp, value]
//   ["putUnknownStruct", key, timestamp, value, allowRootWrite]
//   ["putPose", key, timestamp, [x, y], rotation]
//   ["putTranslationArray", key, timestamp, [[x, y], ...]]
//   ["createBlankField", key, type]
//   ["setField", key, serializedField]   log.setField(key, LogField.fromSerialized(serializedField))
//   ["deleteField", key]
//   ["clearBeforeTime", timestamp, updateZero]
//   ["updateRangeWithTimestamp", timestamp]
//   ["setStructuredType", key, type | null], ["setWpilibType", key, type],
//   ["setMetadataString", key, metadata], ["setGeneratedParent", key]
//   ["putStruct", key, timestamp, bytes, schemaType, isArray]
//   ["putPhotonStruct" | "putProto", key, timestamp, bytes, schemaType]
//   ["putMsgpack", key, timestamp, bytes]
//   ["reload"]   log = Log.fromSerialized(log.toSerialized()); a new Log if that throws
// A write or reload that throws pushes {threw: error}.
// Queries, whose results are returned in order:
//   ["get", method, key, start, end, uuid, startOffset]
//   ["getTimestamps", keys, uuid | null]
//   ["getChangedFields"]
//   ["query", key]   the per-key getters and the field's toSerialized()
//   ["keys"]   getFieldKeys() and getFieldCount()
//   ["fieldTree", includeGenerated, prefix]
//   ["range"]   getTimestampRange() and getLastTimestamp()
//   ["displayOffset", "start-zero" | "original" | null]
//   ["findKey", search]   LogKeyUtils.findKey(log, search)
//   ["getOrDefault", key, type, timestamp, default, uuid]   LogUtil.getOrDefault
//   ["urclKeys"]   LogUtil.getURCLKeys(log)
//   ["iterator", key, times]   LogFieldIterator over getRange(-Infinity, Infinity),
//     getAtTime per element of times
//   ["valueText", key]   getLogValueText on the field's last sample, or null
//   ["dump"]   toSerialized() with the field order spelled out
//   ["dumpAll"]   dump plus struct schemas, proto descriptors and the retry queues
//   ["getUnit", key]
//   ["units"]   getUnit of every key in getFieldKeys() order

const PUTS = new Set([
  "putRaw",
  "putBoolean",
  "putNumber",
  "putString",
  "putBooleanArray",
  "putNumberArray",
  "putStringArray",
  "putJSON",
  "putStruct",
  "putPhotonStruct",
  "putProto",
  "putMsgpack"
]);

function fieldState(field: LogField): unknown {
  const s = field.toSerialized();
  return { ...s, timestamps: [...s.timestamps], values: [...s.values] };
}

function tree(children: { [id: string]: LogFieldTree }): unknown {
  const out: { [id: string]: unknown } = {};
  for (const [name, node] of Object.entries(children)) {
    out[name] = { fullKey: node.fullKey, children: tree(node.children) };
  }
  return out;
}

export function dump(log: Log): unknown {
  const s = log.toSerialized();
  const keys = Object.keys(s.fields);
  return {
    keys,
    fields: keys.map((key) => fieldState(log.getField(key)!)),
    generatedParents: [...s.generatedParents],
    timestampRange: s.timestampRange === null ? null : [...s.timestampRange],
    timestampZero: s.timestampZero
  };
}

export function dumpAll(log: Log): unknown {
  const s = log.toSerialized();
  return {
    ...(dump(log) as object),
    structSchemaStrings: s.structDecoder.schemaStrings,
    structSchemas: s.structDecoder.schemas,
    protoDescriptors: s.protoDecoder,
    queuedStructs: s.queuedStructs,
    queuedStructArrays: s.queuedStructArrays,
    queuedProtos: s.queuedProtos
  };
}

export function run(enableTimestampSetCache: boolean, ops: any[][]): unknown[] {
  return runWithLog(enableTimestampSetCache, ops).results;
}

export function runWithLog(enableTimestampSetCache: boolean, ops: any[][]): { log: Log; results: unknown[] } {
  const state = { log: new Log(enableTimestampSetCache) };
  const results: unknown[] = [];
  for (const op of ops) applyOp(state, op, results);
  return { log: state.log, results };
}

function attempt(results: unknown[], f: () => void) {
  try {
    f();
  } catch (e) {
    results.push({ threw: e });
  }
}

export function applyOp(state: { log: Log }, op: any[], results: unknown[]): void {
  const log = state.log;
  {
    const [name, ...args] = op;
    if (PUTS.has(name)) {
      attempt(results, () => (log as any)[name](...args));
      return;
    }
    switch (name) {
      case "putUnknownStruct":
        attempt(results, () => log.putUnknownStruct(args[0], args[1], args[2], args[3]));
        break;
      case "putPose":
        log.putPose(args[0], args[1], { translation: args[2], rotation: args[3] });
        break;
      case "putTranslationArray":
        log.putTranslationArray(args[0], args[1], args[2]);
        break;
      case "createBlankField":
        log.createBlankField(args[0], args[1]);
        break;
      case "setField":
        log.setField(args[0], LogField.fromSerialized(args[1]));
        break;
      case "deleteField":
        log.deleteField(args[0]);
        break;
      case "clearBeforeTime":
        log.clearBeforeTime(args[0], args[1]);
        break;
      case "updateRangeWithTimestamp":
        log.updateRangeWithTimestamp(args[0]);
        break;
      case "setStructuredType":
        log.setStructuredType(args[0], args[1]);
        break;
      case "setWpilibType":
        log.setWpilibType(args[0], args[1]);
        break;
      case "setMetadataString":
        log.setMetadataString(args[0], args[1]);
        break;
      case "setGeneratedParent":
        log.setGeneratedParent(args[0]);
        break;
      case "reload":
        try {
          state.log = Log.fromSerialized(log.toSerialized());
        } catch (e) {
          results.push({ threw: e });
          state.log = new Log();
        }
        break;
      case "getUnit":
        results.push(log.getUnit(args[0]));
        break;
      case "units":
        results.push(log.getFieldKeys().map((key) => log.getUnit(key)));
        break;
      case "dumpAll":
        results.push(dumpAll(log));
        break;
      case "get":
        results.push((log as any)[args[0]](args[1], args[2], args[3], args[4], args[5]));
        break;
      case "getTimestamps":
        // A cache miss returns the cache's own array, which later writes splice into.
        results.push([...log.getTimestamps(args[0], args[1])]);
        break;
      case "getChangedFields":
        results.push([...log.getChangedFields()]);
        break;
      case "query": {
        const key = args[0];
        const field = log.getField(key);
        results.push({
          type: log.getType(key),
          structuredType: log.getStructuredType(key),
          wpilibType: log.getWpilibType(key),
          metadataString: log.getMetadataString(key),
          typeWarning: log.getTypeWarning(key),
          stripingReference: log.getStripingReference(key),
          isGenerated: log.isGenerated(key),
          generatedParent: log.getGeneratedParent(key),
          isGeneratedParent: log.isGeneratedParent(key),
          field: field === null ? null : fieldState(field)
        });
        break;
      }
      case "keys":
        results.push({ keys: log.getFieldKeys(), count: log.getFieldCount() });
        break;
      case "fieldTree":
        results.push(tree(log.getFieldTree(args[0], args[1])));
        break;
      case "range":
        results.push({ range: log.getTimestampRange(), last: log.getLastTimestamp() });
        break;
      case "displayOffset": {
        const g = globalThis as any;
        const saved = g.preferences;
        g.preferences = args[0] === null ? undefined : { timestamps: args[0] };
        try {
          results.push(log.getTimestampDisplayOffset());
        } finally {
          g.preferences = saved;
        }
        break;
      }
      case "findKey":
        results.push(findKey(log, args[0]));
        break;
      case "getOrDefault":
        results.push(getOrDefault(log, args[0], args[1], args[2], args[3], args[4]));
        break;
      case "urclKeys":
        results.push(getURCLKeys(log));
        break;
      case "iterator": {
        const data = log.getRange(args[0], -Infinity, Infinity);
        const iter = new LogFieldIterator<any>(data);
        results.push((args[1] as number[]).map((t) => iter.getAtTime(t)));
        break;
      }
      case "valueText": {
        const data = log.getRange(args[0], -Infinity, Infinity);
        results.push(
          data === undefined || data.values.length === 0
            ? null
            : getLogValueText(data.values[data.values.length - 1], log.getType(args[0])!)
        );
        break;
      }
      case "dump":
        results.push(dump(log));
        break;
      default:
        throw new Error(`logRun: unknown op ${JSON.stringify(name)}`);
    }
  }
}
