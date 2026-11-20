// Deep sweep counterpart of crates/asrs-js/tests/number_sweep.rs. Runs under the oracle
// runtime (ELECTRON_RUN_AS_NODE=1 electron number.mjs <op>), reads operand records on
// stdin and writes result records on stdout.
//
// Formatting ops take 16-byte records [a:f64, b:f64] and write u16-length-prefixed UTF-8
// strings; a length of 0xffff prefixes an error record "Name: message". The b operand
// carries the method argument; the bit pattern SENTINEL means the argument was omitted.
// Parsing ops take u32-length-prefixed u16 strings (plus an f64 radix for parseint) and
// write one f64 result per record.

const SENTINEL = 0x7ff8deadbeef0000n;

const intl = new Intl.NumberFormat("en-US", { maximumFractionDigits: 6, useGrouping: false });
const intlDefault = new Intl.NumberFormat("en-US");

const fmtOps = {
  tostring: (a, b) => (b === undefined ? a.toString() : a.toString(b)),
  tofixed: (a, b) => (b === undefined ? a.toFixed() : a.toFixed(b)),
  toprecision: (a, b) => (b === undefined ? a.toPrecision() : a.toPrecision(b)),
  toexponential: (a, b) => (b === undefined ? a.toExponential() : a.toExponential(b)),
  string: (a) => String(a),
  intl: (a) => intl.format(a),
  intl_default: (a) => intlDefault.format(a),
};

const parseOps = {
  parsefloat: { args: 0, call: (s) => parseFloat(s) },
  numberstr: { args: 0, call: (s) => Number(s) },
  parseint: { args: 1, call: (s, r) => parseInt(s, r) },
};

const op = process.argv[2];
const fmt = fmtOps[op];
const parse = parseOps[op];
if (!fmt && !parse) {
  console.error(`unknown op ${op}; ops: ${[...Object.keys(fmtOps), ...Object.keys(parseOps)].join(" ")}`);
  process.exit(2);
}

let pending = Buffer.alloc(0);

if (fmt) {
  const arity = op === "string" || op === "intl" || op === "intl_default" ? 1 : 2;
  const recordBytes = 8 * arity;
  process.stdin.on("data", (data) => {
    pending = pending.length ? Buffer.concat([pending, data]) : data;
    const count = Math.floor(pending.length / recordBytes);
    if (count === 0) return;
    const used = count * recordBytes;
    const chunk = pending.subarray(0, used);
    pending = pending.subarray(used);
    const out = [];
    let outLen = 0;
    for (let k = 0; k < count; k++) {
      const a = chunk.readDoubleLE(k * recordBytes);
      const bBits = arity === 2 ? chunk.readBigUInt64LE(k * recordBytes + 8) : SENTINEL;
      const b = bBits === SENTINEL ? undefined : chunk.readDoubleLE(k * recordBytes + 8);
      let bytes;
      let failed = false;
      try {
        bytes = Buffer.from(fmt(a, b), "utf8");
      } catch (e) {
        failed = true;
        bytes = Buffer.from(`${e.name}: ${e.message}`, "utf8");
      }
      out.push([failed, bytes]);
      outLen += bytes.length + (failed ? 4 : 2);
    }
    const buf = Buffer.alloc(outLen);
    let pos = 0;
    for (const [failed, bytes] of out) {
      pos = buf.writeUInt16LE(failed ? 0xffff : bytes.length, pos);
      if (failed) pos = buf.writeUInt16LE(bytes.length, pos);
      pos += bytes.copy(buf, pos);
    }
    if (!process.stdout.write(buf)) {
      process.stdin.pause();
      process.stdout.once("drain", () => process.stdin.resume());
    }
  });
} else {
  const tailBytes = 8 * parse.args;
  process.stdin.on("data", (data) => {
    pending = pending.length ? Buffer.concat([pending, data]) : data;
    const results = [];
    while (pending.length >= 4) {
      const units = pending.readUInt32LE(0);
      const recordBytes = 4 + units * 2 + tailBytes;
      if (pending.length < recordBytes) break;
      const u16 = [];
      for (let i = 0; i < units; i++) u16.push(pending.readUInt16LE(4 + i * 2));
      const s = String.fromCharCode(...u16);
      const r = tailBytes ? pending.readDoubleLE(4 + units * 2) : undefined;
      results.push(parse.call(s, r));
      pending = pending.subarray(recordBytes);
    }
    if (results.length === 0) return;
    const buf = new ArrayBuffer(results.length * 8);
    const f64 = new Float64Array(buf);
    for (let k = 0; k < results.length; k++) f64[k] = results[k];
    if (!process.stdout.write(new Uint8Array(buf))) {
      process.stdin.pause();
      process.stdout.once("drain", () => process.stdin.resume());
    }
  });
}

process.stdin.on("end", () => {
  if (pending.length) {
    console.error(`trailing ${pending.length} bytes`);
    process.exit(1);
  }
});
