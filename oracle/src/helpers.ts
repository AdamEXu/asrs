import { flushAnimationFrames } from "./shims";

const indirectEval = eval;

export const HELPERS: { [name: string]: (...args: any[]) => unknown } = {
  identity: (value: unknown) => value,

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
