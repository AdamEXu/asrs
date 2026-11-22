import path from "node:path";

// Ground truth for asrs-services' `path` module: node's own `path.posix.join` and
// `path.win32.join` (the runtime's `lib/path.js`) over hand-picked and random argument lists.
// Each record is {"args":[...],"posix":<string>,"win32":<string>}.
//   genPath <count> <seed>

const [countArg, seedArg] = process.argv.slice(2);

const FIXED: string[][] = [
  [],
  [""],
  ["", ""],
  ["/"],
  ["//"],
  ["///"],
  ["."],
  [".."],
  ["./"],
  ["a"],
  ["a", "b"],
  ["/a", "b", "c.json"],
  ["/a/", "/b/"],
  ["/a//b", "c/"],
  ["/a/../b"],
  ["/a/../../b"],
  ["../a", "../b"],
  ["a/..", ".."],
  ["a/..", ""],
  ["/..", "x"],
  ["./a/./b/."],
  ["..."],
  ["a/...", "b"],
  ["C:", "x"],
  ["C:\\", "x"],
  ["C:\\a\\..\\b"],
  ["c:/a", "b"],
  ["C:"],
  ["C:."],
  ["C:.."],
  ["\\\\server\\share", "x"],
  ["\\\\server\\share"],
  ["\\\\server\\share\\"],
  ["\\\\server"],
  ["\\\\server\\"],
  ["//server/share/x"],
  ["///server/share"],
  ["\\", "a"],
  ["\\\\", "a"],
  ["\\\\\\a"],
  ["a:b"],
  ["a:b", "c"],
  ["x/a:", "b"],
  ["%41:", "Field2d_X", "image.png"],
  ["/fake/userData", "userAssets", "README.txt"],
  ["/fake/userData/", "autoAssets"],
  ["/fake/./custom/../custom", "Field2d_A", "config.json"],
  ["rel/dir", "Robot_A", "model.glb"],
  ["/sp ace/#%?", "Field2d_Spaced Name #1%", "image.png"],
  ["/u\u00dc", "\u2603"],
  ["a\\b/c", "..\\d"],
  ["C:/Users/x/AppData", "AdvantageScope", "userAssets"],
  ["\\\\?\\C:\\a", "b"],
  ["\\\\.\\pipe", "x"],
  ["CON", "x"],
  ["/a", "CON"],
  ["a.", ".b"],
  [".a", "..b"]
];

const ATOMS = ["", ".", "..", "/", "//", "\\", "\\\\", "a", "b.c", "C:", "c:\\", "x:", ":", "...", "a/", "/a", "a\\", "\\a", "Ü", "%", "a b", "..\\", "../", "server"];

let state = Number(seedArg) >>> 0 || 1;
function next(): number {
  state ^= state << 13;
  state ^= state >>> 17;
  state ^= state << 5;
  return state >>> 0;
}
const below = (n: number) => next() % n;

const out = (args: string[]) =>
  process.stdout.write(JSON.stringify({ args, posix: path.posix.join(...args), win32: path.win32.join(...args) }) + "\n");

FIXED.forEach(out);
const count = Number(countArg) || 0;
for (let i = 0; i < count; i++) {
  const args: string[] = [];
  const parts = 1 + below(4);
  for (let p = 0; p < parts; p++) {
    let arg = "";
    const pieces = below(4);
    for (let k = 0; k < pieces; k++) arg += ATOMS[below(ATOMS.length)];
    args.push(arg);
  }
  out(args);
}
