import LogField from "@upstream/shared/log/LogField";

// Replays an operation script against a real upstream LogField. Ops:
//   ["put", <LoggableType of the put method>, timestamp, value]
//   ["get", <LoggableType of the typed getter, or -1 for getRange>, start, end, uuid, startOffset]
//   ["clear", timestamp]
//   ["meta", wpilibType, structuredType, metadataString]
//   ["reload"]  field = LogField.fromSerialized(field.toSerialized())
//   ["state"]   a copy of toSerialized()
// Returns the results of the get and state ops, in order.

const PUT = [
  "putRaw",
  "putBoolean",
  "putNumber",
  "putString",
  "putBooleanArray",
  "putNumberArray",
  "putStringArray"
] as const;

const GET = [
  "getRaw",
  "getBoolean",
  "getNumber",
  "getString",
  "getBooleanArray",
  "getNumberArray",
  "getStringArray"
] as const;

export function snapshot(field: LogField): unknown {
  const s = field.toSerialized();
  return { ...s, timestamps: [...s.timestamps], values: [...s.values] };
}

export function run(type: number, ops: any[][]): unknown[] {
  let field = new LogField(type);
  const results: unknown[] = [];
  for (const op of ops) {
    switch (op[0]) {
      case "put":
        (field as any)[PUT[op[1]]](op[2], op[3]);
        break;
      case "get": {
        const [, kind, start, end, uuid, offset] = op;
        results.push(
          kind === -1 ? field.getRange(start, end, uuid, offset) : (field as any)[GET[kind]](start, end, uuid, offset)
        );
        break;
      }
      case "clear":
        field.clearBeforeTime(op[1]);
        break;
      case "meta":
        field.wpilibType = op[1];
        field.structuredType = op[2];
        field.metadataString = op[3];
        break;
      case "reload":
        field = LogField.fromSerialized(field.toSerialized());
        break;
      case "state":
        results.push(snapshot(field));
        break;
      default:
        throw new Error(`logFieldRun: unknown op ${JSON.stringify(op[0])}`);
    }
  }
  return results;
}
