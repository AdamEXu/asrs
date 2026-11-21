import "./shims";
import { jsonCopy } from "@upstream/shared/util";
import { toTree } from "./cve1";

// Emits JSON.parse / JSON.stringify ground truth for the asrs_js::json vectors.
//   genJson docs <count> <seed>
//     {"doc":"<text>","ok":<CVE-1>,"s":"<compact>","p":"<indent 2>"[,"c":<CVE-1>]}
//     "c" is jsonCopy(parsed), present only when it encodes differently from "ok".
//   genJson invalid <count> <seed>
//     {"doc":"<text>","throw":<CVE-1 error>} for mutated documents (or "ok" if a mutation
//     happens to stay valid).
//   genJson values <count> <seed>
//     fn records for JSON.stringify(value, null, space) and shared/util:jsonCopy(value) over
//     values JSON.parse cannot produce: undefined, NaN, infinities, -0, dates, Uint8Arrays,
//     BigInts, holes.
//   genJson deep <count> <seed>
//     {"deep":"parse"|"stringify"|"jsonCopy","shape":"array"|"object","depth":<count>,
//      "ok":true} or {"throw":<CVE-1 error>}; "gap":"  " marks the bounded stringify.

const [mode, countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
let seed = Number(seedArg) >>> 0 || 0x6a09e667;

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function next(): number {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return seed >>> 0;
}
const below = (n: number) => next() % n;
const chance = (p: number) => next() / 2 ** 32 < p;
const pick = <T>(items: readonly T[]): T => items[below(items.length)];

const bits = new DataView(new ArrayBuffer(8));
function randomDouble(): number {
  bits.setUint32(0, next());
  bits.setUint32(4, next());
  return bits.getFloat64(0);
}

const HEX = "0123456789abcdefABCDEF";
const hex4 = (u: number) => u.toString(16).padStart(4, "0");
const mixCase = (s: string) => [...s].map((c) => (chance(0.3) ? c.toUpperCase() : c)).join("");

function ws(): string {
  if (chance(0.7)) return "";
  let s = "";
  const n = 1 + below(3);
  for (let i = 0; i < n; i++) s += pick([" ", " ", "\t", "\n", "\r", "\r\n", "  "]);
  return s;
}

const EXTREME_NUMBERS = [
  "0", "-0", "-0.0", "0.0", "0e0", "-0e-0", "0E+5", "1e400", "-1e400", "1e-400", "-1e-400",
  "5e-324", "-5e-324", "4.9406564584124654e-324", "2.4703282292062327e-324",
  "2.4703282292062328e-324", "2.2250738585072011e-308", "2.2250738585072014e-308",
  "1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308",
  "-1.7976931348623157e+308", "9007199254740991", "9007199254740992", "9007199254740993",
  "-9007199254740993", "18446744073709551615", "18446744073709551616", "-9223372036854775808",
  "123456789012345678901234567890", "1e21", "1e-7", "1e-6", "0.1", "0.2", "0.30000000000000004",
  "999999999", "1000000000", "-999999999", "-1000000000", "2147483647", "2147483648",
  "4294967295", "4294967296", "1E2", "1e+2", "1.5e-10", "100e-2", "0.000001", "1.0",
  "123.456e78", "7.0e-0", "1e308", "1e309", "1e-323", "1e-324",
  "0." + "0".repeat(330) + "1", "1" + "0".repeat(400), "3." + "1415926535".repeat(90)
];

function randomNumberText(): string {
  switch (below(8)) {
    case 0:
    case 1:
      return pick(EXTREME_NUMBERS);
    case 2: {
      const x = randomDouble();
      return Number.isFinite(x) ? String(x) : "1";
    }
    case 3:
      return String(below(2000) - 1000);
    case 4: {
      const x = (next() / 2 ** 32) * 10 ** (below(40) - 20);
      return chance(0.5) ? String(x) : x.toExponential(below(20));
    }
    case 5: {
      let s = chance(0.3) ? "-" : "";
      s += String(1 + below(9));
      const digits = below(25);
      for (let i = 0; i < digits; i++) s += String(below(10));
      if (chance(0.5)) {
        s += ".";
        const frac = 1 + below(20);
        for (let i = 0; i < frac; i++) s += String(below(10));
      }
      if (chance(0.5)) s += pick(["e", "E"]) + pick(["", "+", "-"]) + String(below(330));
      return s;
    }
    case 6:
      return (chance(0.5) ? "-" : "") + "0." + String(below(1000000));
    default:
      return String(next());
  }
}

function randomUnitEscape(): string {
  const u = pick([
    below(0x20),
    0x22,
    0x5c,
    0x7f,
    0x2028,
    0xfeff,
    0xd800 + below(0x400),
    0xdc00 + below(0x400),
    below(0x10000)
  ]);
  return "\\u" + mixCase(hex4(u));
}

function randomStringBody(): string {
  let s = "";
  const n = chance(0.1) ? below(60) : below(10);
  for (let i = 0; i < n; i++) {
    switch (below(14)) {
      case 0:
        s += "\\" + pick(['"', "\\", "/", "b", "f", "n", "r", "t"]);
        break;
      case 1:
        s += randomUnitEscape();
        break;
      case 2:
        s += "\\u" + mixCase(hex4(0xd800 + below(0x400))) + "\\u" + mixCase(hex4(0xdc00 + below(0x400)));
        break;
      case 3:
        s += String.fromCharCode(0xd800 + below(0x800));
        break;
      case 4:
        s += String.fromCodePoint(0x10000 + below(0x100000));
        break;
      case 5:
        s += String.fromCharCode(0x80 + below(0xff80 - 0x80));
        break;
      case 6:
        s += pick(["\u00e9", "\u2028", "\u2029", "\u00a0", "\u0085", "\ufeff", "\u007f", "'"]);
        break;
      default:
        s += String.fromCharCode(0x20 + below(0x5f)).replace(/["\\]/, "a");
    }
  }
  return s;
}

const KEY_POOL = [
  "0", "1", "2", "9", "10", "11", "99", "100", "007", "01", "-1", "-0", "1.5", "1e3", " 1", "1 ",
  "4294967294", "4294967295", "4294967296", "9007199254740993", "18446744073709551616",
  "__proto__", "constructor", "toString", "valueOf", "hasOwnProperty", "length", "", "a", "b",
  "key", "NaN", "Infinity", "undefined", "null", "\\u0030", "\\u0031\\u0030", "\\ud800", "x y"
];

function randomKey(previous: string[]): string {
  if (previous.length > 0 && chance(0.12)) return pick(previous);
  if (chance(0.25)) return String(below(chance(0.5) ? 20 : 5000));
  if (chance(0.4)) return pick(KEY_POOL);
  return randomStringBody();
}

function randomDoc(depth: number): string {
  const kind = depth <= 0 ? below(5) : below(9);
  switch (kind) {
    case 0:
      return randomNumberText();
    case 1:
      return '"' + randomStringBody() + '"';
    case 2:
      return pick(["true", "false", "null"]);
    case 3:
      return randomNumberText();
    case 4:
      return '"' + randomStringBody() + '"';
    case 5:
    case 6: {
      const n = chance(0.15) ? 0 : below(7);
      const parts: string[] = [];
      for (let i = 0; i < n; i++) parts.push(ws() + randomDoc(depth - 1 - below(2)) + ws());
      return "[" + (n === 0 ? ws() : parts.join(",")) + "]";
    }
    default: {
      const n = chance(0.15) ? 0 : below(8);
      const keys: string[] = [];
      const parts: string[] = [];
      for (let i = 0; i < n; i++) {
        const key = randomKey(keys);
        keys.push(key);
        parts.push(ws() + '"' + key + '"' + ws() + ":" + ws() + randomDoc(depth - 1 - below(2)) + ws());
      }
      return "{" + (n === 0 ? ws() : parts.join(",")) + "}";
    }
  }
}

function deepDoc(): string {
  const depth = 20 + below(200);
  let open = "";
  let close = "";
  for (let i = 0; i < depth; i++) {
    if (chance(0.5)) {
      open += "[";
      close = "]" + close;
    } else {
      const key = randomKey([]);
      open += '{"' + key + '":';
      close = "}" + close;
    }
  }
  return open + randomDoc(1) + close;
}

function generateDoc(): string {
  if (chance(0.01)) return deepDoc();
  return ws() + randomDoc(below(6)) + ws();
}

function attempt<T>(f: () => T): { ok: T } | { throw: unknown } {
  try {
    return { ok: f() };
  } catch (e) {
    return { throw: e };
  }
}

const tree = (value: unknown) => toTree(value);
const result = (r: { ok: unknown } | { throw: unknown }) =>
  "ok" in r ? { ok: tree(r.ok) } : { throw: tree(r.throw) };

function docRecord(doc: string): object {
  const parsed = attempt(() => JSON.parse(doc));
  if (!("ok" in parsed)) return { doc, throw: tree(parsed.throw) };
  const value = parsed.ok;
  const record: { [key: string]: unknown } = {
    doc,
    ok: tree(value),
    s: JSON.stringify(value),
    p: JSON.stringify(value, null, 2)
  };
  const copied = tree(jsonCopy(value));
  if (JSON.stringify(copied) !== JSON.stringify(record.ok)) record.c = copied;
  return record;
}

const INVALID_INSERTS = [
  "", "]", "}", "[", "{", ",", ":", '"', "\\", "-", "+", ".", "e", "0", "1", "t", "f", "n", "x",
  " ", "\n", "\r\n", "\u0000", "\u001f", "\u00a0", "\u00e9", "\u20ac", "\ud83d", "\ude00",
  "\ud83d\ude00", "\\u", "\\u12", "\\x", "/", "'", "NaN", "undefined", "Infinity"
];

function mutate(doc: string): string {
  if (doc.length === 0) return pick(INVALID_INSERTS);
  const at = below(doc.length + 1);
  switch (below(6)) {
    case 0:
      return doc.slice(0, at);
    case 1:
      return doc.slice(0, at) + pick(INVALID_INSERTS) + doc.slice(at);
    case 2:
      return doc.slice(0, at) + doc.slice(at + 1 + below(3));
    case 3:
      return doc.slice(0, at) + pick(INVALID_INSERTS) + doc.slice(at + 1);
    case 4:
      return doc + pick(INVALID_INSERTS.filter((s) => s.trim() !== "")) + (chance(0.5) ? generateDoc() : "");
    default:
      return mutate(mutate(doc));
  }
}

const SPECIAL_INVALID = [
  "", " ", "undefined", "NaN", "Infinity", "-Infinity", "[object Object]", " undefined", "undefined ",
  "'a'", "{'a':1}", "[1,]", "{\"a\":1,}", "01", "-", "--1", "+1", ".5", "5.", "1e", "1e+", "0x10",
  "tru", "nul", "falsey", "\"\\u00\"", "\"\t\"", "[" + "1,".repeat(30) + "]", "\u2028", "\ufeff1"
];

function genDocs() {
  for (let i = 0; i < count; i++) out(docRecord(generateDoc()));
}

function genInvalid() {
  for (const doc of SPECIAL_INVALID) out(docRecord(doc));
  for (let i = SPECIAL_INVALID.length; i < count; i++) out(docRecord(mutate(generateDoc())));
}

function randomValue(depth: number): unknown {
  switch (below(depth <= 0 ? 9 : 12)) {
    case 0:
      return pick([undefined, null, true, false]);
    case 1:
      return pick([NaN, Infinity, -Infinity, -0, 0, 5e-324, Number.MAX_VALUE, 1e21, 1e-7, 123.456]);
    case 2:
      return randomDouble();
    case 3:
      return JSON.parse('"' + randomStringBody() + '"');
    case 4: {
      const t = pick([
        0,
        -1,
        1e12 + below(1e9),
        -62198755200000,
        -62198755200001,
        253402300799999,
        253402300800000,
        8.64e15,
        -8.64e15,
        NaN,
        (next() / 2 ** 32 - 0.5) * 1.7e16
      ]);
      return new Date(t);
    }
    case 5: {
      const bytes = new Uint8Array(below(6));
      for (let i = 0; i < bytes.length; i++) bytes[i] = below(256);
      return bytes;
    }
    case 6:
      return chance(0.2) ? BigInt(next()) * BigInt(next()) : below(1000);
    case 7:
      return JSON.parse(randomNumberText());
    case 8:
      return pick(["", "a", "\ud800", "\udc00\ud800", '"\\\b\f\n\r\t\u0001\u001f\u007f']);
    case 9:
    case 10: {
      const n = below(6);
      const items: unknown[] = new Array(n);
      for (let i = 0; i < n; i++) if (!chance(0.1)) items[i] = randomValue(depth - 1);
      return items;
    }
    default: {
      const object: { [key: string]: unknown } = {};
      const n = below(7);
      const keys: string[] = [];
      for (let i = 0; i < n; i++) {
        const raw = randomKey(keys);
        keys.push(raw);
        const key = JSON.parse('"' + raw + '"');
        if (chance(0.1)) delete object[key];
        else
          Object.defineProperty(object, key, {
            value: randomValue(depth - 1),
            enumerable: true,
            writable: true,
            configurable: true
          });
      }
      return object;
    }
  }
}

const SPACES: unknown[] = [undefined, null, 2, 2, 2, 4, 0, -3, 1, 10, 11, 3.7, Infinity, NaN, "\t", "--", "", "0123456789abc"];

function genDeep() {
  // V8's parser and its two-undefined-argument stringify are iterative (any depth works);
  // its general stringify path recurses and throws RangeError. toTree cannot encode a deep
  // value, so only the outcome kind is recorded and the port rebuilds the input itself.
  const depth = count || 20000;
  const cases = [
    ["array", "[".repeat(depth) + "]".repeat(depth)],
    ["object", '{"a":'.repeat(depth) + "1" + "}".repeat(depth)]
  ] as const;
  const outcome = (r: { ok: unknown } | { throw: unknown }) =>
    "ok" in r ? { ok: true } : { throw: tree(r.throw) };
  for (const [shape, doc] of cases) {
    out({ deep: "parse", shape, depth, ...outcome(attempt(() => JSON.parse(doc))) });
  }
  for (const [shape, doc] of cases) {
    const value = JSON.parse(doc);
    out({ deep: "stringify", shape, depth, ...outcome(attempt(() => JSON.stringify(value))) });
    out({
      deep: "stringify",
      shape,
      depth,
      gap: "  ",
      ...outcome(attempt(() => JSON.stringify(value, null, 2)))
    });
    out({ deep: "jsonCopy", shape, depth, ...outcome(attempt(() => jsonCopy(value))) });
  }
}

function genValues() {
  for (let i = 0; i < count; i++) {
    const value = randomValue(below(5));
    const space = pick(SPACES);
    const args = space === undefined ? [value] : [value, null, space];
    out({ fn: "JSON.stringify", args: args.map(tree), ...result(attempt(() => JSON.stringify(value, null, space as any))) });
    if (chance(0.5)) out({ fn: "shared/util:jsonCopy", args: [tree(value)], ...result(attempt(() => jsonCopy(value))) });
  }
}

switch (mode) {
  case "docs":
    genDocs();
    break;
  case "invalid":
    genInvalid();
    break;
  case "values":
    genValues();
    break;
  case "deep":
    genDeep();
    break;
  default:
    throw new Error("usage: genJson docs|invalid|values|deep <count> <seed>");
}
