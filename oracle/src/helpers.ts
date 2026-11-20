import StructDecoder from "@upstream/shared/log/StructDecoder";
import PhotonStructDecoder from "@upstream/shared/log/PhotonStructDecoder";
import ConsoleRenderer from "@upstream/shared/renderers/ConsoleRenderer";
import { AnsiUp } from "ansi_up";
import { flushAnimationFrames } from "./shims";

const indirectEval = eval;

// A `__proto__` field assigns the decoded object's prototype (to an array, for an
// array field). Log.putUnknownStruct only reads own properties, so the records
// keep just those.
export const ownView = (value: unknown): unknown => {
  if (Array.isArray(value)) return value.map(ownView);
  if (typeof value !== "object" || value === null) return value;
  const out: Record<string, unknown> = {};
  for (const [key, item] of Object.entries(value)) out[key] = ownView(item);
  return out;
};

export const HELPERS: { [name: string]: (...args: any[]) => unknown } = {
  identity: (value: unknown) => value,

  // Drives a fresh upstream StructDecoder through an op list:
  //   ["addSchema", name, bytes]              -> toSerialized() snapshot
  //   ["decode", name, bytes]                 -> {data, schemaTypes}
  //   ["decodeArray", name, bytes, length?]   -> {data, schemaTypes}
  //   ["toSerialized"]                        -> serialized state
  //   ["fromSerialized", serialized]          -> serialized state after restore
  // Each element comes back as {ok: ...} or {throw: ...}, serialized states
  // structured-cloned when taken, as Log's postMessage hand-offs do.
  struct: (ops: unknown[][]) => {
    let decoder = new StructDecoder();
    return ops.map((item) => {
      try {
        const [kind, ...args] = item as [string, ...unknown[]];
        switch (kind) {
          case "addSchema":
            decoder.addSchema(args[0] as string, args[1] as Uint8Array);
            return { ok: structuredClone(decoder.toSerialized()) };
          case "decode":
            return { ok: ownView(decoder.decode(args[0] as string, args[1] as Uint8Array)) };
          case "decodeArray":
            return {
              ok: ownView(
                decoder.decodeArray(args[0] as string, args[1] as Uint8Array, args[2] as number | undefined)
              )
            };
          case "toSerialized":
            return { ok: structuredClone(decoder.toSerialized()) };
          case "fromSerialized":
            decoder = StructDecoder.fromSerialized(args[0]);
            return { ok: structuredClone(decoder.toSerialized()) };
          default:
            throw new Error(`oracle:struct: unknown op ${String(kind)}`);
        }
      } catch (e) {
        return { throw: e };
      }
    });
  },

  typeof: (value: unknown) => typeof value,

  // ConsoleRenderer.formatValue is private upstream; the prototype method is the real
  // implementation, it just does not read `this`.
  consoleFormatValue: (value: string, filter: string) =>
    (ConsoleRenderer.prototype as { formatValue: (v: string, f: string) => string }).formatValue(value, filter),

  // Console export: this.lines.map((l) => l.value).join("\n").
  consoleExport: (lines: string[]) => lines.join("\n"),

  ansiUp: (text: string) => new AnsiUp().ansi_to_html(text),

  // A persistent AnsiUp across calls, so Incomplete packets and rendition state carry
  // over exactly as upstream's streaming API leaves them.
  ansiUpAppend: (chunks: string[]) => {
    const ansiUp = new AnsiUp();
    return chunks.map((chunk) => ansiUp.ansi_to_html(chunk));
  },

  call: (source: string, ...args: unknown[]) => {
    const fn = indirectEval(`(${source})`);
    if (typeof fn !== "function") throw new TypeError("oracle:call source must evaluate to a function");
    return fn(...args);
  },

  numberFormat: (locales: string | string[] | undefined, options: Intl.NumberFormatOptions | undefined, value: number) =>
    new Intl.NumberFormat(locales, options).format(value),

  textDecode: (bytes: Uint8Array, label = "utf-8", options: TextDecoderOptions = {}) =>
    new TextDecoder(label, options).decode(bytes),

  textEncode: (text: string) => new TextEncoder().encode(text),

  sort: (array: unknown[]) => array.sort(),

  localeSort: (array: string[], numeric = false) =>
    array.sort((a, b) => (numeric ? a.localeCompare(b, undefined, { numeric: true }) : a.localeCompare(b))),

  withRandom: async (values: unknown, name: string, args: unknown[] = []) => {
    const seq = Array.isArray(values) ? values : [];
    let index = 0;
    const real = Math.random;
    Math.random = () => seq[index++];
    try {
      const { invoke } = await import("./registry");
      return await invoke(name, args);
    } finally {
      Math.random = real;
    }
  },

  logFieldRun: async (type: number, ops: any[][]) => (await import("./logFieldRun")).run(type, ops),

  flushAnimationFrames: (time: number) => flushAnimationFrames(time),

  // A PhotonStructDecoder session on a fresh decoder; each op result is {ok|throw} so a
  // failed decode can't abort the session.
  photon: (ops: [string, string, Uint8Array][]) => {
    let decoder = new PhotonStructDecoder();
    return ops.map((op) => {
      try {
        switch (op[0]) {
          case "reload":
            decoder = PhotonStructDecoder.fromSerialized(structuredClone(decoder.toSerialized()));
            return { ok: structuredClone(decoder.toSerialized()) };
          case "addSchema":
            decoder.addSchema(op[1], op[2]);
            return { ok: Object.keys(decoder.getSchemas()) };
          case "decode":
            return { ok: decoder.decode(op[1], op[2]) };
          case "decodeImpl":
            return { ok: decoder.decodeImpl(op[1], op[2]) };
          default:
            throw new Error(`oracle:photon: bad op ${op[0]}`);
        }
      } catch (e) {
        return { throw: e };
      }
    });
  },

  runtime: () => ({
    electron: process.versions.electron,
    node: process.versions.node,
    v8: process.versions.v8,
    icu: process.versions.icu,
    unicode: process.versions.unicode,
    locale: Intl.DateTimeFormat().resolvedOptions().locale,
    runAsNode: process.env.ELECTRON_RUN_AS_NODE
  })
};
