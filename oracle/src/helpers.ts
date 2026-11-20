import StructDecoder from "@upstream/shared/log/StructDecoder";
import { flushAnimationFrames } from "./shims";

const indirectEval = eval;

export const HELPERS: { [name: string]: (...args: any[]) => unknown } = {
  identity: (value: unknown) => value,

  // Drives a fresh upstream StructDecoder through an op list:
  //   ["addSchema", name, bytes]              -> toSerialized() snapshot
  //   ["decode", name, bytes]                 -> {data, schemaTypes}
  //   ["decodeArray", name, bytes, length?]   -> {data, schemaTypes}
  //   ["toSerialized"]                        -> serialized state
  //   ["fromSerialized", serialized]          -> serialized state after restore
  // Each element comes back as {ok: ...} or {throw: ...}.
  struct: (ops: unknown[][]) => {
    let decoder = new StructDecoder();
    return ops.map((item) => {
      try {
        const [kind, ...args] = item as [string, ...unknown[]];
        switch (kind) {
          case "addSchema":
            decoder.addSchema(args[0] as string, args[1] as Uint8Array);
            return { ok: decoder.toSerialized() };
          case "decode":
            return {
              ok: decoder.decode(args[0] as string, args[1] as Uint8Array, args[2] as number | undefined)
            };
          case "decodeArray":
            return { ok: decoder.decodeArray(args[0] as string, args[1] as Uint8Array, args[2] as number | undefined) };
          case "toSerialized":
            return { ok: decoder.toSerialized() };
          case "fromSerialized":
            decoder = StructDecoder.fromSerialized(args[0]);
            return { ok: decoder.toSerialized() };
          default:
            throw new Error(`oracle:struct: unknown op ${String(kind)}`);
        }
      } catch (e) {
        return { throw: e };
      }
    });
  },

  typeof: (value: unknown) => typeof value,

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

  flushAnimationFrames: (time: number) => flushAnimationFrames(time),

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
