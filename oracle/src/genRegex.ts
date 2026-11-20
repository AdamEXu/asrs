import "./shims";
import { toTree } from "./cve1";
import { invoke } from "./registry";

// Emits fn records for asrs_js::regex.
//   genRegex sites <count per site> <seed>
//     every upstream regex use (upstream function or the exact literal through oracle:call)
//     over fixed edge cases plus token-built random strings.
//   genRegex engine <count> <seed>
//     random patterns in the engine's subset under exec loops, split with limits, replace with
//     templates and callbacks, and match; also the upstream patterns with random inputs.

const [mode, countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
let seed = Number(seedArg) >>> 0 || 0x2545f491;

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

async function record(fn: string, args: unknown[]) {
  let result: object;
  try {
    result = { ok: toTree(await invoke(fn, args)) };
  } catch (e) {
    result = { throw: toTree(e) };
  }
  out({ fn, args: args.map((a) => toTree(a)), ...result });
}

const GENERIC = [
  "", "a", "Z", "0", "7", "_", " ", "\t", "\n", "\r", "\v", "\f", " ", " ", " ", " ", "​",
  " ", " ", " ", " ", "　", "﻿", "\ud800", "\udfff", "\ud83d", "\ude00", "😀",
  "\u{1d7ce}", "é", "ß", "ǅ", "\u0000", "\\", "/", ":", "-", ".", "x", "<", ">", "&"
];

const loneSurrogate = () => String.fromCharCode(0xd800 + below(0x800));

function build(tokens: readonly string[], maxTokens: number): string {
  let s = "";
  const n = below(maxTokens + 1);
  for (let i = 0; i < n; i++) {
    const r = below(20);
    if (r === 0) s += loneSurrogate();
    else if (r < 4) s += pick(GENERIC);
    else s += pick(tokens);
  }
  return s;
}

type Site = { fn: string; source?: string; tokens: string[]; edges: string[]; max?: number };

const KEY_TOKENS = [
  "/", "/", ":", "_", "NT", "NT:", "DS:", "AdvantageKit", "Phoenix6", "/Phoenix6", "Log", "/Log", "Log0", "Log12",
  "Log1/", ".schema", ".schema/", "/.schema/", "schema/", "struct:", "Pose2d", "\n", "\r", "Drive", "a_b", "0", "123"
];

const HEX_TOKENS = ["0", "a", "f", "9", "g", "A", "F", "-", "--", "...", "-...", "0a", "ff", "0a-", "ff-", "1b-2c", "-.."];

const SITES: Site[] = [
  { fn: "shared/log/LogKeyUtils:isSchema", tokens: KEY_TOKENS, edges: [".schema/", "/.schema/", ".schema/x\ny", "a.schema/b", "a/.schema/ b", "/.schema", ".schema/" + "\ud800"] },
  { fn: "shared/log/LogKeyUtils:getSchemaType", tokens: KEY_TOKENS, edges: [".schema/", "/.schema/", ".schema/x\ny", "a.schema/b", "x/.schema/a/.schema/b", "/.schema/\r", ".schema/.schema/q", "\n/.schema/t u"] },
  { fn: "shared/log/LogKeyUtils:isMergePrefix", tokens: KEY_TOKENS, edges: ["Log", "/Log", "Log1", "/Log1", "//Log1", "Log１", "xLog1", "/Log12/a", "Log01"] },
  { fn: "shared/log/LogKeyUtils:extractMergePrefix", tokens: KEY_TOKENS, edges: ["Log", "/Log", "Log1", "/Log1", "//Log1", "Log１", "xLog1", "/Log12/a", "Log0123x"] },
  { fn: "shared/util:htmlEncode", tokens: ["<", ">", "&", "\u009f", " ", "香", "馚", "퟿", "", "a", "\""], edges: [" 香馚", "<&>", "𐀀"] },
  { fn: "oracle:call", source: String.raw`(s) => s.replace(/^0+/, "")`, tokens: ["0", "00", "1", "10", ".", "a", "٠", "０"], edges: ["", "0", "000", "0001", "100", "0.0", " 0"] },
  { fn: "oracle:call", source: String.raw`(s) => s.match(/.{1,2}/g)`, tokens: ["#", "ff", "0", "a", "\n", "\r", " ", " ", "\u0085", "abc"], edges: ["", "\n", "\n\n", "abc", "a\nb", "ffeedd", "😀x"] },
  {
    fn: "oracle:call",
    source: String.raw`(s) => s.replace(/\\(?:033|x1b|x1B|e)(?=\[[0-9;:]*[a-zA-Z])/g, "\x1b")`,
    tokens: ["\\", "\\033", "\\x1b", "\\x1B", "\\X1b", "\\e", "\\E", "033", "[", "[0", "[0;31", "[1:2", "m", "K", ";", ":", "0", "9", "١", "\\\\", "e[", "x1b["],
    edges: ["\\033[0m", "\\x1b[31;1m", "\\e[K", "\\e[", "\\e[0", "\\e[;:m", "\\\\033[0m", "\\033\\033[m"],
    max: 10
  },
  { fn: "oracle:call", source: String.raw`(s) => s.replace(/\\r\\n|\\n/g, "\n")`, tokens: ["\\", "\\r", "\\n", "\\r\\n", "\\\\n", "r", "n", "\r\n", "\\r\\r\\n"], edges: ["\\r\\n", "\\r\\\\n", "\\\\n", "\\r"], max: 10 },
  { fn: "oracle:call", source: String.raw`(s) => s.split(/(<[^>]*>)/g)`, tokens: ["<", ">", "<b>", "</span>", "<span style=\"color:red\">", "<>", "a", "<<", ">>", "\n", "text"], edges: ["", "<", ">", "<>", "<<>>", "a<b", "<a>b<c", "<a\n>"], max: 10 },
  { fn: "oracle:call", source: String.raw`(s) => s.split(/\/|:/)`, tokens: KEY_TOKENS, edges: ["", "/", ":", "//", "/a/b", "NT:/x", "a_b"] },
  { fn: "oracle:call", source: String.raw`(s) => s.split(/\/|:|_/)`, tokens: KEY_TOKENS, edges: ["", "_", "Phoenix6/Talon_1/Pos", "__", "a:_/b"] },
  { fn: "oracle:call", source: String.raw`(s) => /^([0123456789abcdef]{2}-)+[0123456789abcdef]{2}(-\.\.\.)?$/.test(s)`, tokens: HEX_TOKENS, edges: ["0a-0b", "0a", "0a-", "0a-0b-...", "0a-...", "0A-0b", "0a-0b-..", "0a-0b\n", "00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-..."], max: 12 },
  { fn: "oracle:call", source: String.raw`(s) => /^[0123456789abcdef]{2}$/.test(s)`, tokens: HEX_TOKENS, edges: ["0a", "ff", "FF", "f", "fff", "0a\n", "\n0a"], max: 3 },
  {
    fn: "oracle:call",
    source: String.raw`(s) => { const m = s.match(/(?:youtube\.com\/(?:[^\/]+\/.+\/|(?:v|e(?:mbed)?)\/|.*[?&]v=)|youtu\.be\/)([^"&?\/\s]{11})/); return m ? m[1] : null; }`,
    tokens: ["https://", "www.", "youtube.com/", "youtu.be/", "watch", "?v=", "&v=", "v=", "/v/", "v/", "e/", "embed/", "emb/", "user/", "a/", "/", "?", "&", "\"", " ", "\n", "dQw4w9WgXcQ", "abcdefghij", "0123456789A", "x", "-_", "&feature=share", "#t=1"],
    edges: [
      "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
      "https://youtu.be/dQw4w9WgXcQ",
      "https://www.youtube.com/embed/dQw4w9WgXcQ",
      "https://www.youtube.com/v/dQw4w9WgXcQ?version=3",
      "https://www.youtube.com/user/x/a/dQw4w9WgXcQ/",
      "https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
      "youtube.com/e/dQw4w9WgXc",
      "youtube.com/a/b/c?v=dQw4w9WgXcQ"
    ],
    max: 8
  },
  { fn: "oracle:call", source: String.raw`(s) => { const m = /, [0-9]+x[0-9]+/.exec(s); return m === null ? null : m.index; }`, tokens: [", ", ",", " ", "1920", "x", "1080", "0", "X", "yuv420p", "Stream #0:0", ", 640x480 [SAR 1:1]", "kb/s, "], edges: [", 1x1", ",, 1x1", ", x1", ", 1x", ", 12x34, 5x6", "a, 1920x1080, 30 fps"], max: 10 },
  { fn: "oracle:call", source: String.raw`(s) => { const m = /[^0-9]/.exec(s); return m === null ? null : m.index; }`, tokens: ["0", "12", "9", "x", " ", ",", "٠", "１"], edges: ["", "0", "123", "123 ", "1080 [SAR"] },
  { fn: "oracle:call", source: String.raw`(s) => /^\d/.test(s)`, tokens: ["0", "2024 KitBot", "KitBot", " 1", "١", "９"], edges: ["", "0", "a0", " 0"] },
  { fn: "oracle:call", source: String.raw`(s) => s.replace(/(^\w|\s\w|\/\w)/g, (m) => m.toUpperCase())`, tokens: ["meters", " per ", "/", "second", "degrees", " ", "  ", "\t", " ", "　", "_", "a", "é", "ß", "1", "/s", "//"], edges: ["", "a", " a", "meters per second", "m/s", "/a", "a  b", " ß", "ab/cd ef"], max: 8 }
];

async function genSites() {
  for (const site of SITES) {
    const call = async (s: string) => (site.source === undefined ? record(site.fn, [s]) : record(site.fn, [site.source, s]));
    for (const edge of site.edges) await call(edge);
    for (const g of GENERIC) await call(g);
    for (let i = 0; i < count; i++) await call(build(site.tokens, site.max ?? 7));
  }
}

const EXEC = String.raw`(p, f, s) => { const r = new RegExp(p, f); const out = []; for (let i = 0; i < 40; i++) { const m = r.exec(s); if (m === null) { out.push(r.lastIndex); break; } out.push([m.index, r.lastIndex, ...m]); if (!r.global && !r.sticky) break; if (m[0].length === 0) r.lastIndex++; } return out; }`;
const SPLIT = String.raw`(p, f, s, n) => s.split(new RegExp(p, f), n)`;
const REPLACE = String.raw`(p, f, s, t) => s.replace(new RegExp(p, f), t)`;
const REPLACE_FN = String.raw`(p, f, s) => s.replace(new RegExp(p, f), (...a) => "<" + a.slice(0, -2).map(String).join(",") + "@" + a[a.length - 2] + ">")`;
const MATCH = String.raw`(p, f, s) => { const m = s.match(new RegExp(p, f)); return m === null || m.index === undefined ? m : [m.index, ...m]; }`;

const LITERALS = ["a", "b", "c", "/", ":", "-", "x", "0", "1", " ", "é", ",", "<", ">"];
const ESCAPED = ["\\.", "\\/", "\\-", "\\\\", "\\n", "\\r", "\\t", "\\uD83D", "\\uDE00", "\\x61", "\\u00a0", "\\$", "\\^", "\\?", "\\(", "\\[", "\\]", "\\{", "\\|"];
const CLASS_ESCAPES = ["\\d", "\\w", "\\s", "\\D", "\\W", "\\S"];
const QUANTS = ["*", "+", "?", "{0,2}", "{1}", "{2}", "{2,}", "{1,3}", "{0}", "{0,1}"];

function randomClass(): string {
  let s = "[";
  if (chance(0.3)) s += "^";
  const n = 1 + below(3);
  for (let i = 0; i < n; i++) {
    const r = below(6);
    if (r === 0) s += pick(CLASS_ESCAPES);
    else if (r === 1) s += pick(["a-c", "0-9", "x-z", "\\uD800-\\uDBFF", "\\u0000-\\u001f", "\\d-z", "a-\\w"]);
    else if (r === 2) s += pick(["\\b", "\\-", "\\]", "\\\\", "\\/", "\\n"]);
    else s += pick(["a", "b", "/", ":", "-", ".", " ", "$", "^x", "|", "("]);
  }
  if (chance(0.15)) s += "-";
  return s + "]";
}

function randomAtom(depth: number): string {
  const r = below(depth > 0 ? 12 : 9);
  if (r < 3) return pick(LITERALS);
  if (r === 3) return pick(ESCAPED);
  if (r === 4) return pick(CLASS_ESCAPES);
  if (r === 5) return ".";
  if (r < 8) return randomClass();
  if (r === 8) return pick(["a", "b"]);
  if (r === 9) return "(?:" + randomDisjunction(depth - 1) + ")";
  return "(" + randomDisjunction(depth - 1) + ")";
}

function randomTerm(depth: number): string {
  const r = below(14);
  if (r === 0) return pick(["^", "$", "\\b", "\\B"]);
  if (r === 1 && depth > 0) return (chance(0.5) ? "(?=" : "(?!") + randomDisjunction(depth - 1) + ")";
  let atom = randomAtom(depth);
  if (chance(0.4)) atom += pick(QUANTS) + (chance(0.3) ? "?" : "");
  return atom;
}

function randomDisjunction(depth: number): string {
  const alternatives: string[] = [];
  const n = chance(0.7) ? 1 : 2 + below(2);
  for (let i = 0; i < n; i++) {
    let seq = "";
    const terms = below(4) + (i === 0 ? 1 : 0);
    for (let t = 0; t < terms; t++) seq += randomTerm(depth);
    alternatives.push(seq);
  }
  return alternatives.join("|");
}

const INPUT_TOKENS = ["a", "b", "c", "ab", "aa", "/", ":", "-", ".", "x", "0", "1", "12", " ", "\n", "\r", " ", " ", "é", "_", ",", "<", ">", "\ud83d", "\ude00", "😀", "\\", "$"];

const FLAGS = ["", "", "g", "g", "y", "gy", "m", "gm", "s", "gs", "ms", "gmsy"];
const TEMPLATES = ["", "-", "[$&]", "<$1>", "$2$1", "$$", "$", "$0", "$01", "$10", "$99", "$`|$'", "$<n>", "$&$&", "x$3y", "$1$", "$001"];
const LIMITS: unknown[] = [undefined, undefined, undefined, 0, 1, 2, 3, 5, -1, 4294967296, 4294967297, 2.7, NaN];

const UPSTREAM_PATTERNS = [
  String.raw`^0+`,
  String.raw`.{1,2}`,
  String.raw`\\(?:033|x1b|x1B|e)(?=\[[0-9;:]*[a-zA-Z])`,
  String.raw`\\r\\n|\\n`,
  String.raw`(<[^>]*>)`,
  String.raw`(?:^|\/)\.schema\/(.*)`,
  String.raw`\/|:`,
  String.raw`\/|:|_`,
  String.raw`^\/?Log\d+`,
  String.raw`^([0123456789abcdef]{2}-)+[0123456789abcdef]{2}(-\.\.\.)?$`,
  String.raw`^[0123456789abcdef]{2}$`,
  String.raw`(?:youtube\.com\/(?:[^\/]+\/.+\/|(?:v|e(?:mbed)?)\/|.*[?&]v=)|youtu\.be\/)([^"&?\/\s]{11})`,
  String.raw`, [0-9]+x[0-9]+`,
  String.raw`[^0-9]`,
  String.raw`^\d`,
  String.raw`(^\w|\s\w|\/\w)`,
  String.raw`[ -香<>\&]`
];
const UPSTREAM_TOKENS = [
  ...INPUT_TOKENS, "0a-", "ff", "-...", "Log1", "/Log", ".schema/", "youtube.com/", "youtu.be/", "watch?v=", "dQw4w9WgXcQ", "embed/", ", 12x3",
  "\\033[0m", "\\e[", "\\r\\n", "\\n", "<b>", "香", "&"
];

function compiles(pattern: string, flags: string): boolean {
  try {
    new RegExp(pattern, flags);
    return true;
  } catch {
    return false;
  }
}

async function genEngine() {
  for (let i = 0; i < count; i++) {
    const upstream = chance(0.25);
    const flags = pick(FLAGS);
    let pattern = upstream ? pick(UPSTREAM_PATTERNS) : randomDisjunction(2);
    while (!compiles(pattern, flags)) pattern = randomDisjunction(2);
    const input = upstream ? build(UPSTREAM_TOKENS, 10) : build(INPUT_TOKENS, 8);
    const op = below(10);
    if (op < 4) await record("oracle:call", [EXEC, pattern, flags, input]);
    else if (op < 6) await record("oracle:call", [SPLIT, pattern, flags, input, pick(LIMITS)]);
    else if (op < 8) await record("oracle:call", [REPLACE, pattern, flags, input, pick(TEMPLATES)]);
    else if (op < 9) await record("oracle:call", [REPLACE_FN, pattern, flags, input]);
    else await record("oracle:call", [MATCH, pattern, flags, input]);
  }
}

const run = mode === "sites" ? genSites : mode === "engine" ? genEngine : undefined;
if (run === undefined) throw new Error("usage: genRegex sites|engine <count> <seed>");
run().then(
  () => process.stdout.write("", () => process.exit(0)),
  (error) => {
    console.error(error);
    process.exit(2);
  }
);
