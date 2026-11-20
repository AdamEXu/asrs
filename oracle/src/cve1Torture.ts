import "./shims";
import { decode, encode } from "./cve1";

type Kind = "roundtrip" | "normalize" | "reject" | "rustOnly";
type Case = { name: string; kind: Kind; input: string; rust?: string };

const cases: Case[] = [];

const value = (name: string, v: unknown, kind: Kind = "roundtrip") => cases.push({ name, kind, input: encode(v) });
const text = (name: string, input: string, kind: Kind = "normalize", rust?: string) =>
  cases.push({ name, kind, input, rust });

let seed = 0x2545f491;
function next(): number {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return seed >>> 0;
}
const below = (n: number) => next() % n;
const range = (lo: number, hi: number) => lo + below(hi - lo + 1);
const pick = <T>(items: T[]): T => items[below(items.length)];

const bits = new DataView(new ArrayBuffer(8));
function fromBits(hex: string): number {
  bits.setUint32(0, parseInt(hex.slice(0, 8), 16));
  bits.setUint32(4, parseInt(hex.slice(8), 16));
  return bits.getFloat64(0);
}
function randomNumber(): number {
  switch (below(4)) {
    case 0:
      return pick([0, -0, 1, -1, 0.5, 0.1, Number.MIN_VALUE, Number.MAX_VALUE, Infinity, -Infinity, NaN, 2 ** 53 + 2, 1e21]);
    case 1: {
      bits.setUint32(0, next());
      bits.setUint32(4, next());
      const hi = bits.getUint32(0);
      if ((hi & 0x7ff00000) === 0x7ff00000) bits.setUint32(0, hi | 0x00080000);
      return bits.getFloat64(0);
    }
    case 2:
      return range(-1000, 1000);
    default:
      return (next() / 2 ** 32) * 2e6 - 1e6;
  }
}

function randomString(): string {
  const units: number[] = [];
  const n = below(12);
  for (let i = 0; i < n; i++) {
    switch (below(9)) {
      case 0:
      case 1:
      case 2:
        units.push(range(0x20, 0x7e));
        break;
      case 3:
        units.push(range(0, 0x1f));
        break;
      case 4:
        units.push(pick([0x22, 0x5c, 0x7f, 0x2028, 0x2029, 0xfeff, 0xfffd]));
        break;
      case 5:
        units.push(range(0x80, 0xd7ff));
        break;
      case 6:
        units.push(range(0xd800, 0xdfff));
        break;
      case 7: {
        const cp = range(0x10000, 0x10ffff) - 0x10000;
        units.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        break;
      }
      default:
        units.push(range(0xe000, 0xffff));
    }
  }
  return String.fromCharCode(...units);
}

const ERRORS = [Error, TypeError, RangeError, SyntaxError, ReferenceError, EvalError, URIError];
const TYPED = [Int8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array, Float32Array, Float64Array];

function randomLeaf(): unknown {
  switch (below(14)) {
    case 0:
      return undefined;
    case 1:
      return null;
    case 2:
      return below(2) === 0;
    case 3:
    case 4:
      return randomNumber();
    case 5:
    case 6:
      return randomString();
    case 7:
      return (below(2) ? -1n : 1n) * BigInt(next()) * BigInt(next()) * BigInt(next());
    case 8: {
      const bytes = new Uint8Array(below(16)).map(() => below(256));
      return pick([bytes, bytes.buffer, new DataView(bytes.buffer)]);
    }
    case 9:
      return new Date(below(8) === 0 ? NaN : Math.trunc((next() / 2 ** 32) * 1.728e16 - 8.64e15));
    case 10: {
      const error = new (pick(ERRORS))(randomString());
      if (below(3) === 0) error.name = String.fromCharCode(...Array.from({ length: 1 + below(8) }, () => range(97, 122)));
      return error;
    }
    case 11:
      return below(3) === 0 ? Symbol() : Symbol(randomString());
    case 12:
      return Object(pick([randomNumber(), randomString(), below(2) === 0]));
    default: {
      if (below(5) === 0) return pick([BigInt64Array, BigUint64Array]).from(Array.from({ length: below(6) }, () => BigInt(next()) << 32n));
      const ctor = pick(TYPED);
      return ctor.from(Array.from({ length: below(6) }, () => randomNumber() * 300));
    }
  }
}

function randomValue(depth: number): unknown {
  if (depth === 0 || below(18) < 14) return randomLeaf();
  switch (below(4)) {
    case 0: {
      const out: unknown[] = new Array(below(6));
      for (let i = 0; i < out.length; i++) if (below(6) !== 0) out[i] = randomValue(depth - 1);
      return out;
    }
    case 1: {
      const out: { [key: string]: unknown } = {};
      const n = below(6);
      for (let i = 0; i < n; i++) {
        const key = pick([String(below(20)), pick(["4294967294", "4294967295", "01", "-0", "1.5", "", "length"]), randomString()]);
        out[key] = randomValue(depth - 1);
      }
      return out;
    }
    case 2:
      return new Map(Array.from({ length: below(5) }, () => [randomValue(depth - 1), randomValue(depth - 1)]));
    default:
      return new Set(Array.from({ length: below(5) }, () => randomValue(depth - 1)));
  }
}

for (const n of [
  0, -0, 1, -1, 0.1, 0.5, 1 / 3, Math.PI, Number.MIN_VALUE, -Number.MIN_VALUE, Number.MAX_VALUE, -Number.MAX_VALUE,
  Number.MAX_SAFE_INTEGER, 2 ** 53 + 2, Number.EPSILON, 2.2250738585072014e-308, 2.225073858507201e-308, 1e21, 1e-7,
  123456789012345680000, Infinity, -Infinity, NaN
]) {
  value(`number ${Object.is(n, -0) ? "-0" : String(n)}`, n);
}
for (const hex of ["7ff8000000000001", "fff8000000000000", "7fffffffffffffff", "fffc000000000123"]) {
  value(`quiet NaN ${hex}`, fromBits(hex));
}

const allUnits = String.fromCharCode(...Array.from({ length: 0x10000 }, (_, i) => i));
for (const [name, s] of [
  ["empty string", ""],
  ["printable ASCII", String.fromCharCode(...Array.from({ length: 95 }, (_, i) => i + 0x20))],
  ["C0 controls and DEL", String.fromCharCode(...Array.from({ length: 32 }, (_, i) => i), 0x7f)],
  ["C1 controls", String.fromCharCode(...Array.from({ length: 32 }, (_, i) => i + 0x80))],
  ["line and paragraph separators", "a b c"],
  ["BOM and replacement", "﻿x�"],
  ["lone high surrogate", "\ud800"],
  ["lone low surrogate", "\udfff"],
  ["reversed pair", "\udc00\ud800"],
  ["high surrogate at end", "ab\udbff"],
  ["astral", "\u{1f600}\u{10ffff}\u{10000}"],
  ["slash and quotes", "/'\"\\`"],
  ["every code unit", allUnits]
] as [string, string][]) {
  value(`string ${name}`, s);
}

for (const b of [0n, -1n, 2n ** 64n, -(2n ** 200n), 12345678901234567890123456789n]) value(`bigint ${b}`, b);

const sparse: unknown[] = [1];
sparse[3] = 2;
const trailingHoles: unknown[] = new Array(5);
trailingHoles[0] = "x";
const withExtras: any = [1, 2];
withExtras.extra = 3;
let deep: unknown = "bottom";
for (let i = 0; i < 500; i++) deep = [deep];
value("array empty", []);
value("array nested", [[], [[]], [1, "a", null, undefined, true]]);
value("array sparse", sparse);
value("array all holes", new Array(4));
value("array trailing holes", trailingHoles);
value("array extra properties dropped", withExtras);
value("array 500 deep", deep);
value("array 5000 numbers", Array.from({ length: 5000 }, randomNumber));

const nullProto = Object.create(null);
nullProto.z = 1;
nullProto["0"] = 2;
const protoKey = JSON.parse('{"__proto__":{"a":1},"b":2}');
const mixedKeys: { [key: string]: unknown } = {};
for (const key of ["b", "10", "a", "2", "4294967295", "4294967294", "01", "-1", "1.0", "", "\ud800", "0"]) mixedKeys[key] = key;
const hidden: any = { visible: 1 };
Object.defineProperty(hidden, "hidden", { value: 2, enumerable: false });
hidden[Symbol("s")] = 3;
const getter = { get computed() { return 42; } };
const wide: { [key: string]: unknown } = {};
for (let i = 0; i < 1000; i++) wide[below(2) ? String(below(2000)) : "k" + below(2000)] = i;
value("object empty", {});
value("object null prototype", nullProto);
value("object __proto__ own key", protoKey);
value("object integer-like key order", mixedKeys);
value("object non-enumerable and symbol keys dropped", hidden);
value("object getter", getter);
value("object undefined values", { a: undefined, b: [undefined] });
value("object 1000 keys", wide);

class Point {
  x = 1;
  y = -0;
}
value("instance of a class", new Point(), "rustOnly");
value("instance of an anonymous class", new (class {})(), "rustOnly");
value("object whose prototype has no constructor", Object.create(Object.create(null)), "rustOnly");
value("boxed bigint", Object(1n), "rustOnly");
value("boxed symbol", Object(Symbol("s")), "rustOnly");
value("named function", function named() {}, "rustOnly");
value("arrow function", () => 0, "rustOnly");
value("bound function", function f() {}.bind(null), "rustOnly");
value("class constructor", Point, "rustOnly");
value("instance nested in an array", [1, { o: new Point() }], "rustOnly");

const bytes = Uint8Array.from({ length: 256 }, (_, i) => i);
value("Uint8Array empty", new Uint8Array(0));
value("Uint8Array every byte", bytes);
value("Uint8Array subarray", bytes.subarray(250, 253));
value("Buffer", Buffer.from("buf"));
value("Int8Array", Int8Array.from([-128, 127, 0, -1]));
value("Uint8ClampedArray", Uint8ClampedArray.from([0, 255, 128]));
value("Int16Array", Int16Array.from([-32768, 32767]));
value("Uint16Array", Uint16Array.from([0, 65535]));
value("Int32Array", Int32Array.from([-(2 ** 31), 2 ** 31 - 1]));
value("Uint32Array", Uint32Array.from([0, 2 ** 32 - 1]));
value("Float32Array", Float32Array.from([-0, 3.4028234663852886e38, 1.401298464324817e-45, NaN, Infinity, 0.1]));
value("Float64Array", Float64Array.from([-0, Number.MIN_VALUE, NaN, -Infinity, 0.1]));
value("BigInt64Array", BigInt64Array.from([-(2n ** 63n), 2n ** 63n - 1n, 0n]));
value("BigUint64Array", BigUint64Array.from([0n, 2n ** 64n - 1n]));
value("ArrayBuffer empty", new ArrayBuffer(0));
value("ArrayBuffer", bytes.slice(0, 8).buffer);
value("SharedArrayBuffer", new SharedArrayBuffer(3));
value("DataView with offset", new DataView(bytes.buffer, 4, 3));

for (const t of [0, -1, 1.7e12, 8.64e15, -8.64e15, NaN]) value(`Date ${t}`, new Date(t));

value("Map empty", new Map());
value(
  "Map mixed keys",
  new Map<unknown, unknown>([
    [1, "one"],
    ["1", "string one"],
    [NaN, "nan"],
    [null, undefined],
    [undefined, null],
    [{}, "object"],
    [{}, "another object"],
    [[1], new Map([[2, 3]])],
    [1n, "bigint"],
    [Symbol("k"), "symbol"]
  ])
);
value("Set mixed", new Set<unknown>([1, "1", NaN, 0, null, undefined, {}, {}, new Set([1])]));

class MyError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MyError";
  }
}
const extraProps: any = new TypeError("with props");
extraProps.code = 7;
const numericName: any = new Error("m");
numericName.name = 5;
for (const ctor of ERRORS) value(`${ctor.name}`, new ctor(`${ctor.name} message`));
value("Error subclass", new MyError("custom"));
value("Error with extra properties", extraProps);
value("Error with numeric name", numericName);
value("Error with lone surrogate message", new RangeError("\udc00 bad"));
value("Error empty message", new Error());
value("AggregateError", new AggregateError([], "x"), "normalize");

value("boxed number", Object(1));
value("boxed negative zero", Object(-0));
value("boxed string", Object("str"));
value("boxed boolean", Object(false));
value("symbol without description", Symbol());
value("symbol with description", Symbol("desc"));
value("well-known symbol", Symbol.iterator);
value("undefined", undefined);
value("null", null);
value("true", true);

for (let i = 0; i < 300; i++) value(`random ${i}`, randomValue(4));

text("whitespace between tokens", ' { "n" : "3ff0000000000000" } ');
text("bare numbers", "[1,-0,0.1,1e400,-1e400,5e-324,2.4703282292062328e-324,123456789012345678901234567890]");
text("string escapes", '"\\u0041\\u00e9\\ud83d\\ude00\\ud800\\/\\b"');
text("object key order and duplicates", '{"o":[["b",1],["1",2],["b",3],["4294967295",4],["4294967294",5],["01",6]]}');
text("Map negative zero and duplicate keys", '{"map":[[{"n":"8000000000000000"},"a"],[{"n":"0000000000000000"},"b"],["k",1],["k",2]]}');
text("Set duplicates and NaN payloads", '{"set":[{"n":"8000000000000000"},{"n":"7ff8000000000000"},{"n":"7ff8000000000001"},"a","a",{"bi":"1"},{"bi":"01"}]}');
text("bigint leading zeros", '[{"bi":"-000"},{"bi":"0012"},{"bi":"-007"}]');
text("Date time clip", '[{"d":"3ff8000000000000"},{"d":"bff8000000000000"},{"d":"8000000000000000"},{"d":"7ff8000000000001"},{"d":"4340000000000000"},{"d":"fff0000000000000"}]');
text("Int8Array conversion", '{"ta":["Int8Array",[128,-129,1.9,-1.9,{"n":"8000000000000000"},1e400,{"n":"7ff8000000000000"}]]}');
text("Uint8ClampedArray conversion", '{"ta":["Uint8ClampedArray",[2.5,3.5,-1,300,254.5,255.5,{"n":"7ff8000000000000"},0.5,1.5]]}');
text("Uint32Array conversion", '{"ta":["Uint32Array",[-1,4294967296.5,1e20]]}');
text("Int32Array conversion", '{"ta":["Int32Array",[2147483648,-2147483649,1e300]]}');
text("Float32Array conversion", '{"ta":["Float32Array",[0.1,1e40,1e-46,1.401298464324817e-45,{"n":"7ffc000000000123"}]]}');
text("BigInt64Array wrap", '{"ta":["BigInt64Array",[{"bi":"9223372036854775808"},{"bi":"-9223372036854775809"},{"bi":"18446744073709551616"}]]}');
text("BigUint64Array wrap", '{"ta":["BigUint64Array",[{"bi":"-1"},{"bi":"340282366920938463463374607431768211457"}]]}');
text("box of bigint", '{"box":{"bi":"1"}}');
text("box of symbol", '{"box":{"sym":"q"}}');
text("box of null and undefined", '[{"box":null},{"box":{"u":null}}]');
text("box of an object", '[{"box":[1]},{"box":{"box":1}},{"box":{"o":[["a",1]]}}]');
text("AggregateError and SuppressedError lose the message", '[{"err":["AggregateError","x"]},{"err":["SuppressedError","y"]}]');
text("undefined and hole bodies are ignored", '[{"u":5},{"hole":7},{"hole":null,"x":1}]');
text("duplicate tag key", '{"n":"3ff0000000000000","n":"4000000000000000"}');
text("__proto__ pair", '{"o":[["__proto__",{"o":[]}]]}');

for (const [name, input] of [
  ["uppercase number bits", '{"n":"3FF0000000000000"}'],
  ["short number bits", '{"n":"3ff"}'],
  ["number bits not a string", '{"n":1}'],
  ["two tag keys", '{"a":1,"b":2}'],
  ["unknown tag", '{"nope":1}'],
  ["empty object", "{}"],
  ["hole outside an array", '{"hole":null}'],
  ["odd hex", '{"b":"0"}'],
  ["bad hex digit", '{"b":"0g"}'],
  ["uppercase hex", '{"ab":"0A"}'],
  ["unknown typed array", '{"ta":["Float16Array",[]]}'],
  ["number in BigInt64Array", '{"ta":["BigInt64Array",[1]]}'],
  ["bigint in Int8Array", '{"ta":["Int8Array",[{"bi":"1"}]]}'],
  ["DOMException", '{"err":["DOMException","x"]}'],
  ["fractional bigint", '{"bi":"1.5"}'],
  ["object key not a string", '{"o":[[1,2]]}'],
  ["trailing comma", "[1,]"],
  ["leading zero", "01"],
  ["bad escape", '"\\x"'],
  ["unterminated string", '"abc'],
  ["raw control character", '"a\u0001"'],
  ["bad literal", "nul"],
  ["trailing data", "null x"],
  ["empty input", ""],
  ["bad date bits", '{"d":"x"}']
]) {
  text(`reject ${name}`, input, "reject");
}

text("class instance tag", '{"c":["X",[["b",1],["1",2]]]}', "rustOnly", '{"c":["X",[["1",{"n":"4000000000000000"}],["b",{"n":"3ff0000000000000"}]]]}');
text("function tag", '{"fn":"f"}', "rustOnly", '{"fn":"f"}');

const names = new Set<string>();
let failed = false;
for (const c of cases) {
  if (names.has(c.name)) throw new Error(`duplicate case name ${c.name}`);
  names.add(c.name);
  let output: string | null;
  try {
    output = encode(decode(c.input));
  } catch {
    output = null;
  }
  const ok =
    (c.kind === "roundtrip" && output === c.input) ||
    (c.kind === "normalize" && output !== null) ||
    ((c.kind === "reject" || c.kind === "rustOnly") && output === null);
  if (!ok) {
    failed = true;
    console.error(`case ${JSON.stringify(c.name)} (${c.kind}): input ${c.input.slice(0, 200)} gave ${output?.slice(0, 200)}`);
  }
  const record: { [key: string]: unknown } = { case: c.name, kind: c.kind, input: c.input, output };
  if (c.kind === "rustOnly") record.rust = c.rust ?? c.input;
  process.stdout.write(JSON.stringify(record) + "\n");
}
if (failed) process.exitCode = 1;
