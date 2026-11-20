import "./shims";
import { toTree } from "./cve1";
import { invoke } from "./registry";

// Emits fn records for the R10 console port.
//   genConsole ansi <count> <seed>       oracle:ansiUp over the ANSI corpus
//   genConsole ansiAppend <count> <seed> oracle:ansiUpAppend over randomly chunked input
//   genConsole format <count> <seed>     oracle:consoleFormatValue over (value, filter)
//   genConsole filter <count> <seed>     oracle:call row-filter predicate over (value, filter)
//   genConsole export <count> <seed>     oracle:consoleExport over raw line arrays

const [mode, countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
let seed = Number(seedArg) >>> 0 || 0x5eedc0de;

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function next(): number {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return seed >>> 0;
}
const below = (n: number) => next() % n;
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

const ESC = "\x1b";

// The row-filter predicate inlined in updateValueAvailability; the same source text
// is registered in asrs-cli's tabs_console.rs.
const LINE_VISIBLE =
  '(value, input) => { const filter = input.toLowerCase(); if (!(filter.startsWith("!") ? filter.length > 1 : filter.length > 0)) return true; return filter.startsWith("!") ? !value.toLowerCase().includes(filter.slice(1).toLowerCase()) : value.toLowerCase().includes(filter.toLowerCase()); }';

const sgr = (params: string) => `${ESC}[${params}m`;

const SGR_PARAMS = [
  ...Array.from({ length: 110 }, (_, i) => String(i)),
  "38;5;0", "38;5;1", "38;5;7", "38;5;8", "38;5;15", "38;5;16", "38;5;17",
  "38;5;51", "38;5;196", "38;5;215", "38;5;231", "38;5;232", "38;5;243",
  "38;5;255", "38;5;256", "38;5;999", "38;5;", "38;5", "38;05;196",
  "48;5;0", "48;5;9", "48;5;16", "48;5;123", "48;5;255", "48;5;256",
  "38;2;0;0;0", "38;2;255;255;255", "38;2;10;20;30", "38;2;256;0;0",
  "38;2;1;2", "38;2;1", "38;2;", "38;2", "38;2;0;0;0;1",
  "48;2;0;0;0", "48;2;200;100;50", "48;2;300;0;0",
  ";", ";;", "0;", ";1", "1;2", "4;38;5;196", "38;5;196;1",
  "1;38;2;10;20;30;4", "31;42", "00", "0001", " 1", "1 ", "1x",
];

const CSI_JUNK = [
  `${ESC}[`,
  `${ESC}[1`,
  `${ESC}[12;`,
  `${ESC}[A`,
  `${ESC}[1A`,
  `${ESC}[999999999999999999999D`,
  `${ESC}[?25h`,
  `${ESC}[?25l`,
  `${ESC}[>4;2m`,
  `${ESC}[<0;1;2M`,
  `${ESC}[ m`,
  `${ESC}[1 m`,
  `${ESC}[12:34m`,
  `${ESC}[31:m`,
  `${ESC}[a`,
  `${ESC}[abc`,
  `${ESC}[38;5;-1m`,
  `${ESC}[38;5;+1m`,
  `${ESC}[\x00`,
  `${ESC}[\x01`,
  `${ESC}[\x08`,
  `${ESC}[\x0f`,
  `${ESC}[\x1f`,
  `${ESC}[\t`,
  `${ESC}X`,
  `${ESC}]`,
  `${ESC}]x`,
  `${ESC}]0`,
  `${ESC}]0;title\x07`,
  `${ESC}]0;title${ESC}\\`,
  `${ESC}]8`,
  `${ESC}]8;`,
  `${ESC}]8;;`,
  `${ESC}]8;;http://a`,
  `${ESC}]8;;http://a\x07`,
  `${ESC}]8;;http://a\x07T`,
  `${ESC}]8;;http://a\x07T\x07tail`,
  `${ESC}]8;;http://a\x01bad\x07T\x07${ESC}]8;;\x07`,
  `${ESC}(`,
  `${ESC}(B`,
  `${ESC}(0`,
  `${ESC}( `,
  `${ESC}(`,
  `${ESC}#8`,
  `${ESC}c`,
  `${ESC}M`,
  `${ESC}7`,
  `${ESC}8`,
  `${ESC}=`,
  `${ESC}>`,
  `${ESC}`,
  `${ESC}\x00`,
];

const OSC8 = (params: string, url: string, text: string, st: string) =>
  `${ESC}]8;${params};${url}${st}${text}${ESC}]8;;${st}`;

const OSC_TOKENS = [
  OSC8("", "http://example.com", "click", "\x07"),
  OSC8("", "http://example.com", "click", `${ESC}\\`),
  OSC8("", "https://a.b/c?d=e&f=g", "T&<>\"'", "\x07"),
  OSC8("id=x", "http://a", "T", "\x07"),
  OSC8("id=1;foo=bar", "https://a", "T", "\x07"),
  OSC8("id=", "https://a", "T", "\x07"),
  OSC8("", "ftp://a", "T", "\x07"),
  OSC8("", "javascript:x", "T", "\x07"),
  OSC8("", "HTTP://UPPER", "T", "\x07"),
  OSC8("", "Http://mixed", "T", "\x07"),
  OSC8("", "noscheme", "T", "\x07"),
  OSC8("", "", "T", "\x07"),
  OSC8("", "constructor:x", "T", "\x07"),
  OSC8("", "hasOwnProperty:x", "T", "\x07"),
  OSC8("", "valueOf:x", "T", "\x07"),
  OSC8("", "toString:x", "T", "\x07"),
  OSC8("", "toLocaleString:x", "T", "\x07"),
  OSC8("", "isPrototypeOf:x", "T", "\x07"),
  OSC8("", "propertyIsEnumerable:x", "T", "\x07"),
  OSC8("", "__defineGetter__:x", "T", "\x07"),
  OSC8("", "__defineSetter__:x", "T", "\x07"),
  OSC8("", "__lookupGetter__:x", "T", "\x07"),
  OSC8("", "__lookupSetter__:x", "T", "\x07"),
  OSC8("", "__proto__:x", "T", "\x07"),
  OSC8("", "http://a b", "T", "\x07"),
  OSC8("", "http://a" + "x".repeat(503), "T", "\x07"),
  OSC8("", "http://a" + "x".repeat(504), "T", "\x07"),
  OSC8("", "http://a" + "x".repeat(505), "T", "\x07"),
  OSC8("", "http://a", "T", "\x07") + "after",
  OSC8("", "http://a", "T\x1b[31m", "\x07"),
  `${ESC}]8;;http://a\x07T${ESC}]8;;\x07T2`,
  `${ESC}]8;;http://a${ESC}\\T${ESC}]8;;${ESC}\\`,
  `${ESC}]8;;http://a\x07T${ESC}]8;;${ESC}\\`,
  `${ESC}]8;;http://a${ESC}\\T${ESC}]8;;\x07`,
  `${ESC}]8;;http://a\x07T${ESC}]8;;`,
  `${ESC}]8;;http://a${ESC}\\T`,
  `${ESC}]8;;\x07T\x07`,
  `${ESC}]8;\x7f;http://a\x07T\x07${ESC}]8;;\x07`,
];

const TEXT_TOKENS = [
  "hello", "foo", "bar", "x", "0", "9", " ", "  ", "\t", "\n", "\r\n", "\r",
  "\x00", "\x01", "\x02", "\x06", "\x08", "\x0b", "\x0c", "\x0e", "\x1f",
  "a&b", "<", ">", "<tag>", "&amp;", "\"", "'", "&<>\"'",
  "é", "ß", "ǅ", "İ", "ı", "ſ", "K", "Å", "Σσς", "ﬀ", "ﬃ",
  "", "😀", "🙂🙃", "\ud800", "\udfff", "\ud83d", "\ude00", "\ud83d\ude00",
  "line one", "long text segment with words",
  "[31m", "[0m",
];

// Literal spellings ConsoleRenderer normalizes before ansi_to_html sees them.
const SPELLINGS = [
  "\\x1b[31m", "\\x1B[31m", "\\033[1m", "\\e[4m", "\\x1b[38;5;42m",
  "\\x1b[", "\\x1bZ", "\\x1b[;", "\\x1b[31", "\\x1b]8;;http://a\\x07T\\x1b]8;;\\x07",
  "\\033X", "\\x1bbad", "\\e", "\\x1b[m", "\\x1b[1;m",
  "\\r\\n", "\\n", "a\\nb", "\\r", "\\\\n", "\\\\x1b[31m",
];

const FILTERS = [
  "", "x", "o", "a", "bar", "foo", "ba", "A", "AB", "ab", "O", "oo",
  " ", ".", "<", ">", "&", "\"", ";", "span", "highlight", "lt", "gt", "amp",
  "!", "!x", "!foo", "!!", "! ", "!0",
  "i", "İ", "ı", "k", "K", "ſ", "s", "ss", "ß", "ﬀ", "ﬃ", "σ", "Σ", "ς",
  "é", "", "😀", "\ud800", "\n", "\t", "m", "rgb", "0", "00",
];

const loneSurrogate = () => String.fromCharCode(0xd800 + below(0x800));

function ansiToken(): string {
  const r = below(100);
  if (r < 38) return sgr(pick(SGR_PARAMS));
  if (r < 55) return pick(CSI_JUNK);
  if (r < 70) return pick(OSC_TOKENS);
  if (r < 72) return loneSurrogate();
  if (r < 76) return `${ESC}${pick("ABCDHKSTcfhlmnrsu")}`;
  return pick(TEXT_TOKENS);
}

function ansiString(maxTokens: number): string {
  let s = "";
  const n = below(maxTokens + 1);
  for (let i = 0; i < n; i++) s += ansiToken();
  return s;
}

function formatValue(): string {
  let s = "";
  const n = below(10) + 1;
  for (let i = 0; i < n; i++) {
    const r = below(100);
    if (r < 30) s += pick(SPELLINGS);
    else if (r < 60) s += ansiToken();
    else if (r < 64) s += loneSurrogate();
    else s += pick(TEXT_TOKENS);
  }
  return s;
}

const ANSI_EDGES = [
  "",
  ESC,
  `${ESC}${ESC}`,
  `${ESC}[m`,
  `${ESC}[;m`,
  `${ESC}m`,
  "plain text only",
  "\x07",
  `${ESC}[0m`,
  `${ESC}[0m${ESC}[0m`,
  sgr("31") + "red" + sgr("0") + "plain",
  sgr("38;5;196") + "x",
  "x" + sgr("38;2;255;0;128") + "y",
  "a" + sgr("1") + "b" + sgr("22") + "c",
  sgr("1;2;3;4;31;42") + "all",
  `${ESC}[31;42m` + "both" + `${ESC}[39;49m` + "none",
  `${ESC}[1m${ESC}[38;5;999m` + "oob",
  `${ESC}[38;2;1;2;3m${ESC}[38;2m` + "true",
  `${ESC}[38;5;5m${ESC}[48;5;9m` + "fgbg",
  `${ESC}]8;;http://a\x07L\x07${ESC}]8;;\x07` + "nested text",
  "text" + `${ESC}` + "more",
  "text" + `${ESC}[`,
  `${ESC}[31mtext${ESC}`,
  `${ESC}[31mtext${ESC}[`,
  `${ESC}[31mtext${ESC}[0;`,
  "end" + `${ESC}]8;;http://a\x07`,
  `${ESC}[31m` + "x".repeat(5000),
  "x".repeat(20000),
  "\ud800\udfff\ud800",
  "İKſßﬀΣσς",
  `&<>"'` + sgr("31") + `&<>"'`,
  `${ESC}[31m` + "&<>\"'",
  `${ESC}[38;5;255m${ESC}[48;2;1;2;3mT${ESC}[39m${ESC}[49mU`,
];

const FORMAT_EDGES: [string, string][] = [
  ["", ""],
  ["", "x"],
  ["plain", ""],
  ["plain", "plain"],
  ["foo bar foo", "foo"],
  ["foo bar foo", "!foo"],
  ["foo", "!"],
  ["foo", "!!"],
  ["\\x1b[31mred", "red"],
  ["\\x1b[31mred", "span"],
  ["\\e[1mbold\\e[0m", "m"],
  ["\\033[38;5;42mfoo", "foo"],
  ["a\\nb", "b"],
  ["a\\r\\nb", "b"],
  ["x&<>\"'y", "&"],
  ["<span>x</span>", "span"],
  ["<a href=\"u\">t</a>", "a href"],
  ["İKſßﬀ", "i"],
  ["İKſßﬀ", "k"],
  ["İKſßﬀ", "ss"],
  ["İKſßﬀ", "ﬀ"],
  ["i̇", "i"],
  ["Σσς", "σ"],
  ["Σσς", "ς"],
  ["abababab", "ab"],
  ["aaaa", "aa"],
  ["aaaa", "aaa"],
  ["x", "xx"],
  ["x", "x"],
  ["\\x1b[31m" + "a".repeat(1000), "a"],
  ["😀😀", "😀"],
  ["\ud800x\ud800", "x"],
  ["\ud800\ud800", "\ud800"],
  ["", "!"],
  ["filter at end", "end"],
  ["START filter", "start"],
  [`${ESC}[31mfoo${ESC}[0mbar`, "ob"],
  [`${ESC}[31mfoo${ESC}[0mbar`, "ar"],
  [`${ESC}]8;;http://a\x07LINK${ESC}]8;;\x07`, "link"],
  [`${ESC}]8;;http://a\x07LINK${ESC}]8;;\x07`, "href"],
  [`${ESC}]8;;http://a\x07LINK${ESC}]8;;\x07`, "http"],
];

const FILTER_EDGES: [string, string][] = [
  ["", ""],
  ["", "x"],
  ["x", ""],
  ["x", "!"],
  ["x", "!!"],
  ["Foo Bar", "foo"],
  ["Foo Bar", "FOO"],
  ["Foo Bar", "!foo"],
  ["Foo Bar", "!baz"],
  ["Foo Bar", "BAZ"],
  ["Foo Bar", "!BAZ"],
  ["anything", "!"],
  ["!bang", "!bang"],
  ["!bang", "bang"],
  ["İstanbul", "i"],
  ["istanbul", "İ"],
  ["\ud800\ud800", "x"],
  ["x", "\ud800"],
  ["K", "k"],
  ["k", "K"],
  ["ſ", "s"],
  ["s", "ſ"],
];

async function main() {
  if (mode === "ansi") {
    for (const s of ANSI_EDGES) await record("oracle:ansiUp", [s]);
    for (let i = 0; i < count; i++) await record("oracle:ansiUp", [ansiString(24)]);
  } else if (mode === "ansiAppend") {
    for (let i = 0; i < count; i++) {
      const s = ansiString(24);
      const n = Math.min(1 + below(3), s.length + 1);
      const cuts = new Set<number>();
      while (cuts.size < n) cuts.add(below(s.length + 1));
      const points = [0, ...[...cuts].sort((a, b) => a - b), s.length];
      const chunks: string[] = [];
      for (let j = 0; j + 1 < points.length; j++) chunks.push(s.slice(points[j], points[j + 1]));
      await record("oracle:ansiUpAppend", [chunks]);
    }
  } else if (mode === "format") {
    for (const [v, f] of FORMAT_EDGES) await record("oracle:consoleFormatValue", [v, f]);
    for (let i = 0; i < count; i++) {
      await record("oracle:consoleFormatValue", [formatValue(), pick(FILTERS)]);
    }
  } else if (mode === "filter") {
    for (const [v, f] of FILTER_EDGES) await record("oracle:call", [LINE_VISIBLE, v, f]);
    for (let i = 0; i < count; i++) {
      await record("oracle:call", [LINE_VISIBLE, formatValue(), pick(FILTERS)]);
    }
  } else if (mode === "export") {
    for (let i = 0; i < count; i++) {
      const lines: string[] = [];
      const n = below(8);
      for (let j = 0; j < n; j++) lines.push(formatValue());
      await record("oracle:consoleExport", [lines]);
    }
  } else {
    throw new Error(`unknown mode ${mode}`);
  }
}

main().then(
  () => process.stdout.write("", () => process.exit(0)),
  (error) => {
    console.error(error);
    process.exit(2);
  }
);
