// Deep sweep counterpart of crates/asrs-js/tests/collation_sweep.rs. Runs under the oracle runtime
// (ELECTRON_RUN_AS_NODE=1 electron collation.mjs), reads one JSON array [a, b] per line on stdin
// and writes two characters per pair, a.localeCompare(b) then
// a.localeCompare(b, undefined, { numeric: true }), each as "<", "=" or ">".

import readline from "node:readline";

const symbol = (x) => (x < 0 ? "<" : x > 0 ? ">" : "=");
const out = [];
const flush = () => {
  if (out.length) process.stdout.write(out.join(""));
  out.length = 0;
};

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", (line) => {
  const [a, b] = JSON.parse(line);
  out.push(symbol(a.localeCompare(b)) + symbol(a.localeCompare(b, undefined, { numeric: true })));
  if (out.length >= 4096) flush();
});
lines.on("close", flush);
