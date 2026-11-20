// Deep sweep counterpart of crates/asrs-js/tests/math_sweep.rs. Runs under the oracle runtime
// (ELECTRON_RUN_AS_NODE=1 electron math.mjs <op>), reads records of little-endian f64 operands
// on stdin and writes one 8-byte result per record on stdout. Each op gets its own loop
// function so V8 optimizes it the way it would optimize a hot loop in the app.

const SENTINEL = 0x7ff8deadbeef0000n;

const toBigInt = (x) => {
  try {
    return BigInt(x);
  } catch {
    return undefined;
  }
};

const number = {
  abs: [1, "Math.abs(a)"],
  acos: [1, "Math.acos(a)"],
  acosh: [1, "Math.acosh(a)"],
  asin: [1, "Math.asin(a)"],
  asinh: [1, "Math.asinh(a)"],
  atan: [1, "Math.atan(a)"],
  atanh: [1, "Math.atanh(a)"],
  cbrt: [1, "Math.cbrt(a)"],
  ceil: [1, "Math.ceil(a)"],
  clz32: [1, "Math.clz32(a)"],
  cos: [1, "Math.cos(a)"],
  cosh: [1, "Math.cosh(a)"],
  exp: [1, "Math.exp(a)"],
  expm1: [1, "Math.expm1(a)"],
  floor: [1, "Math.floor(a)"],
  fround: [1, "Math.fround(a)"],
  log: [1, "Math.log(a)"],
  log1p: [1, "Math.log1p(a)"],
  log10: [1, "Math.log10(a)"],
  log2: [1, "Math.log2(a)"],
  round: [1, "Math.round(a)"],
  sign: [1, "Math.sign(a)"],
  sin: [1, "Math.sin(a)"],
  sinh: [1, "Math.sinh(a)"],
  sqrt: [1, "Math.sqrt(a)"],
  tan: [1, "Math.tan(a)"],
  tanh: [1, "Math.tanh(a)"],
  trunc: [1, "Math.trunc(a)"],
  hypot1: [1, "Math.hypot(a)"],
  to_int32: [1, "a | 0"],
  to_uint32: [1, "a >>> 0"],
  to_int8: [1, "(i8[0] = a, i8[0])"],
  to_uint8: [1, "(u8[0] = a, u8[0])"],
  to_uint8_clamp: [1, "(u8c[0] = a, u8c[0])"],
  to_int16: [1, "(i16[0] = a, i16[0])"],
  to_uint16: [1, "(u16[0] = a, u16[0])"],
  atan2: [2, "Math.atan2(a, b)"],
  pow: [2, "Math.pow(a, b)"],
  exponent_op: [2, "a ** b"],
  rem: [2, "a % b"],
  max2: [2, "Math.max(a, b)"],
  min2: [2, "Math.min(a, b)"],
  hypot2: [2, "Math.hypot(a, b)"],
  imul: [2, "Math.imul(a, b)"],
  shift_left: [2, "a << b"],
  shift_right: [2, "a >> b"],
  shift_right_unsigned: [2, "a >>> b"],
  max3: [3, "Math.max(a, b, c)"],
  min3: [3, "Math.min(a, b, c)"],
  hypot3: [3, "Math.hypot(a, b, c)"],
  hypot4: [4, "Math.hypot(a, b, c, d)"],
  hypot5: [5, "Math.hypot(a, b, c, d, e)"],
};

const bigint = {
  bigint_from_number: [1, "f", "n", "((r) => r === undefined ? S : BigInt.asIntN(64, r))(toBigInt(a))"],
  bigint_to_number_i64: [1, "i", "f", "Number(a)"],
  bigint_to_number_u64: [1, "u", "f", "Number(a)"],
  bigint_arith: [
    1,
    "f",
    "f",
    "((r) => r === undefined ? NaN : Number((r << 75n) + (r >> 3n) - (r << 20n) + 5n))(toBigInt(a))",
  ],
  bigint_bitwise: [
    1,
    "f",
    "f",
    "((r) => r === undefined ? NaN : Number(((r & 0xfffff0000fffffn) | (r >> 40n)) ^ (r << 2n)))(toBigInt(a))",
  ],
};

const op = process.argv[2];
let arity, inKind, outKind, expr;
if (number[op]) {
  [arity, expr] = number[op];
  inKind = "f";
  outKind = "f";
} else if (bigint[op]) {
  [arity, inKind, outKind, expr] = bigint[op];
} else {
  console.error(`unknown op ${op}; ops: ${[...Object.keys(number), ...Object.keys(bigint)].join(" ")}`);
  process.exit(2);
}

const names = ["a", "b", "c", "d", "e"].slice(0, arity);
const view = { f: "Float64Array", i: "BigInt64Array", u: "BigUint64Array", n: "BigInt64Array" };
const loop = new Function(
  "toBigInt",
  "S",
  "i8",
  "u8",
  "u8c",
  "i16",
  "u16",
  `return function (input, output, count) {
    for (let k = 0; k < count; k++) {
      ${names.map((n, j) => `const ${n} = input[k * ${arity} + ${j}];`).join("\n      ")}
      output[k] = ${expr};
    }
  };`,
)(toBigInt, SENTINEL, new Int8Array(1), new Uint8Array(1), new Uint8ClampedArray(1), new Int16Array(1), new Uint16Array(1));

const recordBytes = 8 * arity;
let pending = Buffer.alloc(0);

process.stdin.on("data", (data) => {
  pending = pending.length ? Buffer.concat([pending, data]) : data;
  const count = Math.floor(pending.length / recordBytes);
  if (count === 0) return;
  const used = count * recordBytes;
  const inBuf = new ArrayBuffer(used);
  new Uint8Array(inBuf).set(pending.subarray(0, used));
  pending = pending.subarray(used);
  const outBuf = new ArrayBuffer(count * 8);
  const input = new globalThis[view[inKind]](inBuf);
  const output = new globalThis[view[outKind]](outBuf);
  loop(input, output, count);
  if (!process.stdout.write(new Uint8Array(outBuf))) {
    process.stdin.pause();
    process.stdout.once("drain", () => process.stdin.resume());
  }
});
process.stdin.on("end", () => {
  if (pending.length) {
    console.error(`trailing ${pending.length} bytes`);
    process.exit(1);
  }
});
