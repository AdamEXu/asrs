import "./shims";
import { decode, encode } from "./cve1";

class Point {
  x = 1;
  y = -0;
}

const nanWithPayload = new DataView(new ArrayBuffer(8));
nanWithPayload.setUint32(0, 0x7ff00000);
nanWithPayload.setUint32(4, 0x00000001);

const sparse: unknown[] = [1];
sparse[2] = 3;

const protoKey = JSON.parse('{"__proto__":1,"b":2}');

const fixed: [unknown, string][] = [
  [undefined, '{"u":null}'],
  [null, "null"],
  [true, "true"],
  [0, '{"n":"0000000000000000"}'],
  [-0, '{"n":"8000000000000000"}'],
  [1, '{"n":"3ff0000000000000"}'],
  [0.1, '{"n":"3fb999999999999a"}'],
  [NaN, '{"n":"7ff8000000000000"}'],
  [-Infinity, '{"n":"fff0000000000000"}'],
  [Number.MIN_VALUE, '{"n":"0000000000000001"}'],
  [nanWithPayload.getFloat64(0), '{"n":"7ff0000000000001"}'],
  [10n ** 30n, '{"bi":"1000000000000000000000000000000"}'],
  ["", '""'],
  ['a"\\\n\u0001\u007f ', '"a\\"\\\\\\n\\u0001\u007f "'],
  ["\ud800x\udc00", '"\\ud800x\\udc00"'],
  ["😀", '"😀"'],
  [[1, "a"], '[{"n":"3ff0000000000000"},"a"]'],
  [sparse, '[{"n":"3ff0000000000000"},{"hole":null},{"n":"4008000000000000"}]'],
  [{ b: 1, 2: true, a: undefined, 1: null }, '{"o":[["1",null],["2",true],["b",{"n":"3ff0000000000000"}],["a",{"u":null}]]}'],
  [protoKey, '{"o":[["__proto__",{"n":"3ff0000000000000"}],["b",{"n":"4000000000000000"}]]}'],
  [new Point(), '{"c":["Point",[["x",{"n":"3ff0000000000000"}],["y",{"n":"8000000000000000"}]]]}'],
  [new Uint8Array([0, 15, 255]), '{"b":"000fff"}'],
  [new Float32Array([0.5]), '{"ta":["Float32Array",[{"n":"3fe0000000000000"}]]}'],
  [new BigInt64Array([-1n]), '{"ta":["BigInt64Array",[{"bi":"-1"}]]}'],
  [new Uint8Array([1, 2]).buffer, '{"ab":"0102"}'],
  [new Date(0), '{"d":"0000000000000000"}'],
  [new Map<unknown, unknown>([[1, "x"]]), '{"map":[[{"n":"3ff0000000000000"},"x"]]}'],
  [new Set(["a"]), '{"set":["a"]}'],
  [new RangeError("bad"), '{"err":["RangeError","bad"]}'],
  [Object("s"), '{"box":"s"}'],
  [Symbol("tag"), '{"sym":"tag"}']
];

let failures = 0;
const fail = (message: string) => {
  failures++;
  console.error("FAIL " + message);
};

for (const [value, expected] of fixed) {
  const actual = encode(value);
  if (actual !== expected) {
    fail(`encode: expected ${expected} got ${actual}`);
    continue;
  }
  if (typeof value === "symbol" || value instanceof Point) continue;
  const again = encode(decode(actual));
  if (again !== actual) fail(`round trip: ${actual} became ${again}`);
}

const roundTripOnly = ['{"n":"7ff8000000000001"}', '{"n":"fff8000000000000"}', '{"n":"7fefffffffffffff"}'];
for (const line of roundTripOnly) {
  const again = encode(decode(line));
  if (again !== line) fail(`round trip: ${line} became ${again}`);
}

const decodeErrors = ['{"n":"3FF0000000000000"}', '{"a":1,"b":2}', '{"nope":1}', '{"c":["X",[]]}', '{"hole":null}'];
for (const line of decodeErrors) {
  let threw = false;
  try {
    decode(line);
  } catch {
    threw = true;
  }
  if (!threw) fail(`decode should reject ${line}`);
}

const cyclic: any = {};
cyclic.self = cyclic;
try {
  encode(cyclic);
  fail("encode should reject a cyclic value");
} catch {}

const total = fixed.length + roundTripOnly.length + decodeErrors.length + 1;
console.log(`cve1 selftest: ${total - failures}/${total} passed`);
process.exitCode = failures === 0 ? 0 : 1;
