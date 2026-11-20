import "./shims";

// Emits TextDecoder/TextEncoder ground truth for the jstring vectors:
//   {"d":"<hex>","s":"<decoded>"}              default utf-8 decode
//   {"d":"<hex>","l":"<label>",...}            decode with a label
//   {"d":"<hex>","f":1,"s"|"e":...}            decode with {fatal:true}
//   {"d":"<hex>","b":1,"s":...}                decode with {ignoreBOM:true}
//   {"l":"<label>","s":""}                     constructor accepted the label
//   {"l":"<label>","e":[name,message]}         constructor threw
//   {"e":"<input>","h":"<hex>"}                TextEncoder output
// "s" is the decoded string; "e" is [Error.name, Error.message].

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const hex = (bytes: number[] | Uint8Array) =>
  [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");

let seed = 0x51ed1e5;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};

function decodeRec(bytes: number[], label = "utf-8", fatal = false, ignoreBOM = false) {
  const rec: Record<string, unknown> = { d: hex(bytes) };
  if (label !== "utf-8") rec.l = label;
  if (fatal) rec.f = 1;
  if (ignoreBOM) rec.b = 1;
  try {
    rec.s = new TextDecoder(label, { fatal, ignoreBOM }).decode(new Uint8Array(bytes));
  } catch (e) {
    const err = e as Error;
    rec.e = [err.name, err.message];
  }
  out(rec);
}

function labelRec(label: string) {
  try {
    new TextDecoder(label);
    out({ l: label, s: "" });
  } catch (e) {
    const err = e as Error;
    out({ l: label, e: [err.name, err.message] });
  }
}

function encodeRec(text: string) {
  out({ e: text, h: hex(new TextEncoder().encode(text)) });
}

// Every single byte, non-fatal and fatal.
for (let b = 0; b <= 0xff; b++) {
  decodeRec([b]);
  decodeRec([b], "utf-8", true);
}

// Every lead byte against boundary-ish continuation bytes, both modes.
const CONT_BOUNDARIES = [0x00, 0x7f, 0x80, 0x8f, 0x90, 0x9f, 0xa0, 0xbf, 0xc0, 0xe0, 0xfe, 0xff];
for (let lead = 0x80; lead <= 0xff; lead++) {
  for (const cont of CONT_BOUNDARIES) {
    decodeRec([lead, cont]);
    decodeRec([lead, cont], "utf-8", true);
  }
}

// Every valid two-byte sequence.
for (let lead = 0xc2; lead <= 0xdf; lead++) {
  for (let cont = 0x80; cont <= 0xbf; cont++) {
    decodeRec([lead, cont]);
  }
}

// Every three-byte lead/second combination at third-byte extremes.
for (let lead = 0xe0; lead <= 0xef; lead++) {
  for (let second = 0x80; second <= 0xbf; second++) {
    decodeRec([lead, second, 0x80]);
    decodeRec([lead, second, 0xbf]);
  }
}

// Four-byte sequences across all lead/boundary combinations.
for (let lead = 0xf0; lead <= 0xf5; lead++) {
  for (const second of CONT_BOUNDARIES) {
    decodeRec([lead, second, 0x80, 0x80]);
    decodeRec([lead, second, 0xbf, 0xbf]);
    decodeRec([lead, second, 0x80, 0x80], "utf-8", true);
  }
}

// Truncation at every prefix of valid sequences.
for (const seq of [
  [0xc2, 0x80],
  [0xe0, 0xa0, 0x80],
  [0xe1, 0x80, 0x80],
  [0xed, 0x9f, 0xbf],
  [0xf0, 0x90, 0x80, 0x80],
  [0xf4, 0x8f, 0xbf, 0xbf],
  [0xef, 0xbb, 0xbf],
  [0xef, 0xbf, 0xbd],
]) {
  for (let end = 1; end < seq.length; end++) {
    decodeRec(seq.slice(0, end));
    decodeRec(seq.slice(0, end), "utf-8", true);
  }
}

// BOM and replacement-char edge cases.
for (const bytes of [
  [0xef, 0xbb, 0xbf],
  [0xef, 0xbb, 0xbf, 0x61],
  [0x61, 0xef, 0xbb, 0xbf, 0x62],
  [0xef, 0xbb, 0xbf, 0xef, 0xbb, 0xbf, 0x61],
  [0xef, 0xbb, 0x61],
  [0xef, 0xbf, 0xbf, 0x61],
  [0xef, 0xbf, 0xbd, 0x61],
  [0xfe, 0xff, 0x61],
  [0xef, 0xbb],
]) {
  decodeRec(bytes);
  decodeRec(bytes, "utf-8", false, true);
}

// Resync and malformed classics.
for (const bytes of [
  [0xc0, 0xaf],
  [0xc1, 0xbf],
  [0xe0, 0x80, 0x80],
  [0xe0, 0x9f, 0x80],
  [0xed, 0xa0, 0x80],
  [0xed, 0xbf, 0xbf],
  [0xf0, 0x80, 0x80, 0x80],
  [0xf0, 0x8f, 0xbf, 0xbf],
  [0xf4, 0x90, 0x80, 0x80],
  [0xf5, 0x80, 0x80, 0x80],
  [0xf8, 0x88, 0x80, 0x80, 0x80],
  [0x61, 0xc3, 0x28, 0x62],
  [0xc2, 0x41],
  [0xe2, 0x41, 0x80],
  [0xf0, 0x41, 0x80, 0x80],
  [0xf0, 0x9f, 0x98, 0x80],
  [0xdf, 0xbf],
  [0xe0, 0xa0, 0x80],
  [0xf4, 0x8f, 0xbf, 0xbf],
]) {
  decodeRec(bytes);
  decodeRec(bytes, "utf-8", true);
}

// UTF-16 little and big endian: unpaired surrogates, odd tails, BOMs, both modes.
const UTF16_CASES: number[][] = [
  [0x00, 0xd8],
  [0x00, 0xdc],
  [0x00, 0xd8, 0x62],
  [0x00, 0xdc, 0x62],
  [0x00, 0xd8, 0x62, 0x63],
  [0x00, 0xdb, 0xff, 0xdc],
  [0x00, 0xd8, 0x00, 0xd8],
  [0x00, 0xdc, 0x00, 0xd8],
  [0x00, 0xd8, 0x00, 0xdc],
  [0x00, 0xd8, 0x00, 0xdc, 0x62],
  [0x00, 0xd8, 0x61, 0x00, 0x62],
  [0x61, 0x00, 0x00, 0xd8, 0x62],
  [0x61, 0x00, 0x00, 0xd8],
  [0x00, 0xd8, 0x00, 0xd8, 0x61],
  [0x3d, 0xd8, 0x00, 0xde],
  [0xff, 0xfe, 0x61, 0x00],
  [0xff, 0xfe],
  [0x61, 0x00, 0xff, 0xfe, 0x62, 0x00],
  [0xff, 0xfe, 0x00, 0xd8],
  [0x61],
  [0x62, 0x00, 0x61],
  [],
];
for (const bytes of UTF16_CASES) {
  decodeRec(bytes, "utf-16le");
  decodeRec(bytes, "utf-16le", true);
  decodeRec(bytes, "utf-16le", false, true);
  decodeRec(swapUtf16(bytes), "utf-16be");
  decodeRec(swapUtf16(bytes), "utf-16be", true);
  decodeRec(swapUtf16(bytes), "utf-16be", false, true);
}
for (const bytes of [
  [0xd8, 0x3d, 0xde, 0x00],
  [0xfe, 0xff, 0x00, 0x61],
  [0xfe, 0xff],
]) {
  decodeRec(bytes, "utf-16be");
}

function swapUtf16(bytes: number[]): number[] {
  const out = [...bytes];
  for (let i = 0; i + 1 < out.length; i += 2) {
    [out[i], out[i + 1]] = [out[i + 1], out[i]];
  }
  return out;
}

// Label normalization and errors.
for (const label of [
  "utf-8",
  "utf8",
  "UTF8",
  "unicode-1-1-utf-8",
  "unicode11utf8",
  "unicode20utf8",
  "x-unicode20utf8",
  "ucs-2",
  "iso-10646-ucs-2",
  "unicode",
  "unicodefeff",
  "utf-16",
  "utf-16le",
  "utf-16be",
  "unicodefffe",
  "csunicode",
  "utf-7",
  "x-user-defined",
  "replacement",
  "hz-gb-2312",
  "iso-2022-cn",
  "iso-2022-kr",
  "bogus",
  "",
  " UTF-8 ",
  "\tutf-8\n",
  "utf_8",
  "UTF-16",
  "utf-16LE",
  "null",
]) {
  labelRec(label);
}

// Seeded fuzz: random byte strings, non-fatal and fatal.
for (let i = 0; i < 4000; i++) {
  const len = Math.floor(rng() * 21);
  const bytes = [...Array(len)].map(() => Math.floor(rng() * 256));
  decodeRec(bytes);
  if (i % 5 === 0) decodeRec(bytes, "utf-8", true);
}

// TextEncoder: ASCII, BMP, astral, lone surrogates in every position.
for (const text of [
  "",
  "a",
  "hello world",
  "é",
  "€",
  "\u{1f600}",
  "😀",
  "\ud800",
  "\udc00",
  "\ud800x",
  "x\udc00y",
  "\ud800\ud800",
  "\udfff\ud800",
  "a\ud800\udc00b",
  "abc\udfff€",
  "ab\ud800",
  "\ud800",
  "ࠀ",
  "\uffff",
  "\ud7ff\ue000",
  "a%b",
]) {
  encodeRec(text);
}
