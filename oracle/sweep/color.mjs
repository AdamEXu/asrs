// Deep sweep counterpart of crates/asrs-js/tests/color_sweep.rs. Runs under the oracle
// runtime (ELECTRON_RUN_AS_NODE=1 electron color.mjs), reads 3-byte RGB records on stdin
// and writes a 30-byte result record per input as it goes:
// [h:f64le, s:f64le, l:f64le, rt:6 ASCII]. hsl is convert.hex.hsl(hex6), the wrapRounded
// export, and rt is convert.hsl.hex(hsl) applied to that rounded triple, so each record
// covers both directions.

import { createRequire } from "module";
import { once } from "events";

const require = createRequire(import.meta.url);
const convert = require("../../target/oracle/upstream/node_modules/color-convert/index.js");

let leftover = Buffer.alloc(0);
for await (const chunk of process.stdin) {
  const input = leftover.length ? Buffer.concat([leftover, chunk]) : chunk;
  const n = Math.floor(input.length / 3);
  const out = Buffer.allocUnsafe(n * 30);
  for (let i = 0; i < n; i++) {
    const hex = input.toString("hex", i * 3, i * 3 + 3);
    const hsl = convert.hex.hsl(hex);
    out.writeDoubleLE(hsl[0], i * 30);
    out.writeDoubleLE(hsl[1], i * 30 + 8);
    out.writeDoubleLE(hsl[2], i * 30 + 16);
    out.write(convert.hsl.hex(hsl), i * 30 + 24, 6, "ascii");
  }
  leftover = Buffer.from(input.subarray(n * 3));
  if (!process.stdout.write(out)) {
    await once(process.stdout, "drain");
  }
}
