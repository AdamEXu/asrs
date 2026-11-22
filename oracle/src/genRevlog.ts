import { UPSTREAM_DIR } from "./shims";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { Writable } from "node:stream";
import { pathToFileURL } from "node:url";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";

// Ground truth for asrs_formats::revlog from the real @rev-robotics/revlog-converter 2.0.0 in
// upstream's node_modules, loaded as the ES module upstream's main process loads.
//
//   genRevlog dbc
//     {"kind":"resource","name","text"}             a DBC file shipped in the package, as
//                                                    revlogParser reads it (encoding "ascii")
//     {"kind":"load","load","source"|"text","dump"} `new Dbc().load(text)`, the Dbc as CVE-1
//     {"kind":"decode","load","id","data","result"} a CanDecoder over that Dbc: `decode(
//                                                    createFrame(id, data))` as CVE-1
//   genRevlog convert
//     {"kind":"blob","blob","bytes"}                an input other cases refer to
//     {"kind":"case","case","name","path","setup","input"?,"output","error","uncaught"}
//       main.ts's call: `parseREVLOG(path, writable)` with a Writable that collects chunks.
//       setup is "file" (input written to path), "missing", "directory" (path is a
//       directory), "unreadable" (mode 000) or "none" (path is never created). output is the
//       concatenated chunks when the promise resolves, else null; error is [name, message] of
//       the rejection, else null; uncaught counts uncaught exceptions until the case settles.
//       Paths are relative to a scratch directory the process runs in.
//       An input is {"bytes"} or {"blob","len"?,"flips"?: [[offset, xor]...]}.

const PKG = path.join(UPSTREAM_DIR, "node_modules", "@rev-robotics", "revlog-converter", "dist");
const RESOURCES = ["spark.public.dbc", "servo_hub.public.dbc", "encoder.public.dbc", "a301.public.dbc"];

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString("hex");

async function genDbc(): Promise<void> {
  const { Dbc, CanDecoder } = await import(pathToFileURL(path.join(PKG, "lib", "revDBC.js")).href);
  const rng = new Rng(0x5eed0f14, "revlog/dbc");
  let loads = 0;

  const load = (record: object, text: string) => {
    const dbc = new Dbc().load(text);
    const id = loads++;
    out({ kind: "load", load: id, ...record, dump: toTree(dbc) });
    const decoder = new CanDecoder();
    decoder.database = dbc;
    return { id, dbc, decode: (frameId: number, data: Uint8Array) => {
      const result = decoder.decode(decoder.createFrame(frameId, Buffer.from(data)));
      out({ kind: "decode", load: id, id: toTree(frameId), data: hex(data), result: toTree(result) });
    } };
  };

  for (const name of RESOURCES) {
    const text = fs.readFileSync(path.join(PKG, "resources", name), { encoding: "ascii" });
    out({ kind: "resource", name, text });
    const { dbc, decode } = load({ source: name }, text);
    for (const message of dbc.messagesById.values()) {
      for (const length of [message.dlc, 0, 1 + rng.below(12)]) decode(message.id, rng.bytes(length));
    }
    decode(0, rng.bytes(8));
    decode(12345, rng.bytes(8));
  }

  for (const text of SYNTHETIC_DBCS) {
    const { dbc, decode } = load({ text }, text);
    const ids = [...dbc.messagesById.keys()] as number[];
    for (const id of ids) {
      const message = dbc.messagesById.get(id);
      for (const length of new Set([0, 1, message.dlc, 8, 9, 12, 16, 20])) {
        decode(id, rng.bytes(length));
        decode(id, new Uint8Array(length).fill(0xff));
        decode(id, new Uint8Array(length));
      }
      for (const special of SPECIAL_FRAMES) decode(id, special);
      for (let i = 0; i < 24; i++) decode(id, rng.bytes(rng.below(18)));
    }
    decode(999999, rng.bytes(8));
  }
}

const f32 = (v: number) => {
  const b = Buffer.alloc(4);
  b.writeFloatLE(v);
  return [...b];
};
const f64 = (v: number) => {
  const b = Buffer.alloc(8);
  b.writeDoubleLE(v);
  return [...b];
};
const SPECIAL_FRAMES: Uint8Array[] = [
  [...f32(NaN), ...f32(-0)],
  [...f32(Infinity), ...f32(-Infinity)],
  [...f32(1e-45), ...f32(3.4e38)],
  f64(-0),
  f64(NaN),
  f64(1.7976931348623157e308),
  [...f64(-Infinity), ...f64(5e-324)],
  [0x80, 0, 0, 0, 0, 0, 0, 0x80, 0x80, 0, 0, 0, 0, 0, 0, 0x80],
  [0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0, 0, 0, 0, 0xe0, 0xff, 0xff, 0xff, 0xff, 0xff],
  [1, 0, 0, 0, 0, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0x20, 0],
  [0, 0, 0, 0, 0, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0x20, 0],
  [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0],
  [1, 0, 0, 0, 0, 0, 0xe0, 0xff, 1, 0, 0, 0, 0, 0, 0xe0, 0xff]
].map((a) => new Uint8Array(a));

const SYNTHETIC_DBCS: string[] = [
  // Little-endian integers around the 53-bit and 64-bit edges, signed and unsigned.
  [
    'VERSION ""',
    "BO_ 100 INTS: 16 Node",
    ' SG_ U1 : 0|1@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ U7 : 1|7@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ S13 : 3|13@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ U32 : 8|32@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ S32 : 8|32@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ U53 : 0|53@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ U54 : 0|54@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ S54 : 0|54@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ U63 : 1|63@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ U64 : 0|64@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ S64 : 0|64@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ U65 : 0|65@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ S70 : 4|70@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ HIGH : 60|8@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ FD : 64|16@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ FAR : 200|8@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ ZERO : 5|0@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ S1 : 9|1@1- (1,0) [0|0] "" Vector__XXX',
    ""
  ].join("\n"),
  // Scaling, offsets and clamping.
  [
    "BO_ 200 SCALED: 2 Node",
    ' SG_ HALF : 0|8@1+ (0.5,0) [0|0] "deg" Vector__XXX',
    ' SG_ SHIFT : 8|8@1- (1,-3.25) [0|0] "V" Vector__XXX',
    ' SG_ ZEROF : 0|16@1+ (0,7) [0|0] "" Vector__XXX',
    ' SG_ CLAMP : 0|16@1+ (0.01,-100) [-50|50] "A" Vector__XXX',
    ' SG_ LOWONLY : 16|16@1- (1,0) [-5|0] "" Vector__XXX',
    ' SG_ HIGHONLY : 16|16@1- (1,0) [0|7] "" Vector__XXX',
    ' SG_ BIGCLAMP : 0|64@1+ (1,0) [0|1000] "" Vector__XXX',
    ' SG_ SMALLF : 0|64@1+ (0.001,0) [0|0] "" Vector__XXX',
    ' SG_ NEGZERO : 0|8@1+ (-0,-0) [-0|-0] "" Vector__XXX',
    ' SG_ ODDNUM : 0|8@1+ (1.5.3,1-2) [--1|.5] "" Vector__XXX',
    ' SG_ DASH : 0|8@1+ (-,.) [5.|007] "" Vector__XXX',
    ""
  ].join("\n"),
  // Motorola (big-endian) signals: the saw-tooth walk, running off the frame.
  [
    "BO_ 300 MOTOROLA: 8 Node",
    ' SG_ B8 : 7|8@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B12 : 7|12@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B16 : 23|16@0- (1,0) [0|0] "" Vector__XXX',
    ' SG_ B1 : 0|1@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B33 : 39|33@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B64 : 7|64@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ BSCALED : 15|10@0+ (0.25,1) [0|200] "" Vector__XXX',
    ' SG_ BOFF : 63|16@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ BMID : 3|6@0+ (1,0) [0|0] "" Vector__XXX',
    ""
  ].join("\n"),
  // IEEE floats: single, double, a 64-bit "float", misaligned and out-of-range starts.
  [
    "BO_ 400 FLOATS: 8 Node",
    ' SG_ F0 : 0|32@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ F32 : 32|32@1- (1,0) [0|0] "" Vector__XXX',
    ' SG_ FMIS : 33|32@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ FOUT : 40|32@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ F64AS32 : 0|64@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ D0 : 0|64@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ D8 : 8|64@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ FCLAMP : 0|32@1+ (1,0) [-1|1] "" Vector__XXX',
    ' SG_ FBE : 7|32@0+ (2,0) [0|0] "" Vector__XXX',
    ' SG_ FINT : 0|32@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 401 MOREFLOATS: 16 Node",
    ' SG_ FD64 : 64|64@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ F96 : 96|32@1+ (1,0) [0|0] "" Vector__XXX',
    "",
    "SIG_VALTYPE_ 400 F0 : 1;",
    "SIG_VALTYPE_ 400 F32 : 1;",
    "SIG_VALTYPE_ 400 FMIS : 1;",
    "SIG_VALTYPE_ 400 FOUT : 1;",
    "SIG_VALTYPE_ 400 F64AS32 : 1;",
    "SIG_VALTYPE_ 400 D0 : 2;",
    "SIG_VALTYPE_ 400 D8 : 2;",
    "SIG_VALTYPE_ 400 FCLAMP : 1;",
    "SIG_VALTYPE_ 400 FBE : 1;",
    "SIG_VALTYPE_ 400 FINT : 0;",
    "SIG_VALTYPE_ 401 FD64 : 2;",
    "SIG_VALTYPE_ 401 F96 : 1;",
    "SIG_VALTYPE_ 401 NOSUCH : 1;",
    "SIG_VALTYPE_ 402 F0 : 1;",
    "SIG_VALTYPE_ 400 F0 : 3;",
    ""
  ].join("\n"),
  // What the line regexes accept and drop.
  [
    'SG_ ORPHAN : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 500 FIRST: 4 Node\r",
    ' SG_ CRLF : 0|8@1+ (1,0) [0|0] "u" Vector__XXX\r',
    '\tSG_ TABBED:8|8@1-(2,1)[0|0]"tab" X',
    ' SG_ MUX M : 0|4@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ MUXED m1 : 4|4@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ SPACED  :  16 | 8 @1+ ( 1 , 0 ) [ 0 | 0 ] "" Vector__XXX',
    ' SG_ SPACED2 :  16|8@1+ ( 1 , 0 ) [ 0 | 0 ] "" Vector__XXX',
    ' SG_ QUOTE : 24|8@1+ (1,0) [0|0] "a"b" Vector__XXX',
    ' SG_ NOUNIT : 24|8@1+ (1,0) [0|0] Vector__XXX',
    ' SG_ CRUNIT : 24|8@1+ (1,0) [0|0] "a\rb" Vector__XXX',
    " SG_ NBSP : 24|8@1+ (1,0) [0|0] \"\" Vector__XXX",
    " SG_ LS : 24|8@1+ (1,0) [0|0] \"x y\" Vector__XXX",
    ' SG_ café : 24|8@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ U NIT : 24|8@1+ (1,0) [0|0] "°C" Vector__XXX',
    ' SG_ CRLF : 8|4@1+ (1,0) [0|0] "again" Vector__XXX',
    " BO_ 501 INDENTED: 8 Node",
    ' SG_ AFTERINDENT : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 502 SECOND: 3 Node extra",
    ' SG_ ONLY : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 503 FIRST: 6 Other",
    ' SG_ REPLACED : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 502 THIRD: 2 Node",
    ' SG_ T : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 99999999999999999999 HUGE: 8 Node",
    ' SG_ H : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 100000000000000000000 HUGE2: 8 Node",
    "BO_ 4294967296 WIDE: 8 Node",
    ' SG_ W : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 0007 LEADING: 008 Node",
    ' SG_ L : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    "BO_ 600 : 8 Node",
    "BO_ 601 NOSENDER: 8",
    "BO_ 602 X:8 Y",
    ' SG_ NOSPACE : 0|8@1+ (1,0) [0|0] "" Vector__XXX',
    'CM_ SG_ 500 CRLF "first comment";',
    'CM_ SG_ 500 CRLF "second; with "quotes"" ;";',
    'CM_ SG_ 500 TABBED "tabbed";  trailing',
    'CM_ SG_ 500 SPACED "multi',
    'line";',
    'CM_ SG_ 501 AFTERINDENT "lost";',
    'CM_ SG_ 503 REPLACED "on the replacement";',
    'CM_ SG_ 500 NOSUCH "none";',
    'CM_ BO_ 500 "message comment";',
    '   CM_ SG_ 602 NOSPACE "";',
    'CM_ SG_ 602 NOSPACE "x y";',
    "SIG_VALTYPE_ 500 CRLF : 1 ;",
    "SIG_VALTYPE_ 500 TABBED: 2;",
    "SIG_VALTYPE_ 100000000000000000000 H : 2;",
    "SIG_VALTYPE_ 0007 L : 1;"
  ].join("\n"),
  "",
  "\n\r\n  \n",
  // A Spark-like status frame with a fault/warning mix, as the converter names them.
  [
    "BO_ 2181412864 STATUS_0: 8 Vector__XXX",
    ' SG_ APPLIED_OUTPUT : 0|16@1- (3.082369457075716E-5,0) [0|0] "duty cycle" Vector__XXX',
    ' SG_ VOLTAGE : 16|12@1+ (0.0073260073260073,0) [0|0] "V" Vector__XXX',
    ' SG_ CURRENT : 28|12@1+ (0.03663003663003663,0) [0|0] "A" Vector__XXX',
    ' SG_ MOTOR_TEMPERATURE : 40|8@1+ (1,0) [0|0] "degC" Vector__XXX',
    ' SG_ HARD_FORWARD_LIMIT_REACHED : 48|1@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ OTHER_FAULT : 49|1@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ OTHER_WARNING : 50|1@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ INVERTED : 51|1@1+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ PRIMARY_HEARTBEAT_LOCK : 52|1@1+ (1,0) [0|0] "" Vector__XXX',
    'CM_ SG_ 2181412864 APPLIED_OUTPUT "Applied output";',
    ""
  ].join("\n"),
  // Motorola signals of 127 bits and more: the BigInt grows past 128 bits.
  [
    "BO_ 700 WIDEMOTOROLA: 40 Node",
    ' SG_ B127 : 7|127@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B128 : 7|128@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B129 : 6|129@0- (1,0) [0|0] "" Vector__XXX',
    ' SG_ B200 : 7|200@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B300 : 63|300@0+ (1,0) [0|0] "" Vector__XXX',
    ' SG_ B150S : 7|150@0+ (0.5,-1) [0|0] "" Vector__XXX',
    ' SG_ B140C : 7|140@0+ (1,0.5) [-10|9999999999] "" Vector__XXX',
    ' SG_ B330 : 7|330@0+ (1,0) [0|0] "" Vector__XXX',
    ""
  ].join("\n")
];

// --- convert ---

const le = (value: number | bigint, bytes: number): number[] => {
  let v = BigInt.asUintN(64, BigInt(value));
  const o: number[] = [];
  for (let i = 0; i < bytes; i++) {
    o.push(Number(v & 0xffn));
    v >>= 8n;
  }
  return o;
};
const utf8 = (text: string) => [...Buffer.from(text, "utf-8")];
const shortString = (text: string | number[]) => {
  const bytes = typeof text === "string" ? utf8(text) : text;
  return [bytes.length & 0xff, ...bytes];
};

interface RecordOptions {
  idLen?: number;
  sizeLen?: number;
  high?: number;
  size?: number;
}
const minLen = (v: number) => (v <= 0xff ? 1 : v <= 0xffff ? 2 : v <= 0xffffff ? 3 : 4);
function rec(id: number, payload: number[], o: RecordOptions = {}): number[] {
  const size = o.size ?? payload.length;
  const idLen = o.idLen ?? minLen(id);
  const sizeLen = o.sizeLen ?? minLen(size);
  const bitfield = (idLen - 1) | ((sizeLen - 1) << 2) | ((o.high ?? 0) << 4);
  return [bitfield, ...le(id, idLen), ...le(size, sizeLen), ...payload];
}

const R = { persistent: 1, periodic: 2, header: 3, busTable: 4, firmware: 5, frames: 6, robotState: 7, matchInfo: 8, event: 9, diagnostics: 10 };

const v1Persistent = (entries: { id: number; data: number[] }[]) =>
  rec(R.persistent, entries.flatMap((e) => [...le(e.id, 4), ...[...e.data, 0, 0, 0, 0, 0, 0].slice(0, 6)]));
const v1Periodic = (entries: { ms: number; id: number; data: number[] }[]) =>
  rec(R.periodic, entries.flatMap((e) => [...le(e.ms, 4), ...le(e.id, 4), ...[...e.data, 0, 0, 0, 0, 0, 0, 0, 0].slice(0, 8)]));
const header = (o: { major?: number; minor?: number; startUs?: bigint | number; magic?: string; tail?: boolean } = {}) =>
  rec(R.header, [
    ...utf8(o.magic ?? "REVLOG"),
    o.major ?? 2,
    o.minor ?? 0,
    ...(o.startUs === undefined ? [] : le(o.startUs, 8)),
    ...(o.tail === false || o.startUs === undefined ? [] : [...le(0, 8), ...shortString("REVLib 2027.0.0")])
  ]);
const busTable = (buses: { id: number; name: string | number[] }[]) => rec(R.busTable, buses.flatMap((b) => [b.id, ...shortString(b.name)]));
const firmware = (entries: { bus: number; id: number; data: number[] }[]) =>
  rec(R.firmware, entries.flatMap((e) => [e.bus, ...le(e.id, 4), e.data.length, ...e.data]));
const frames = (bus: number, baseUs: bigint | number, entries: { offsetUs: number; id: number; data: number[] | Uint8Array }[]) =>
  rec(R.frames, [...le(baseUs, 8), bus, ...entries.flatMap((e) => [...le(e.offsetUs, 2), ...le(e.id, 4), e.data.length, ...e.data])]);
const controlWord = (o: { enabled?: boolean; mode?: number; extra?: bigint }) =>
  (BigInt(o.mode ?? 0) << 56n) | (o.enabled ? 1n << 58n : 0n) | (o.extra ?? 0n);
const robotState = (timeUs: bigint | number, word: bigint) => rec(R.robotState, [...le(timeUs, 8), ...le(word, 8)]);
const matchInfo = (timeUs: bigint | number, type: number, number: number, replay: number, event: string | number[]) =>
  rec(R.matchInfo, [...le(timeUs, 8), type, ...le(number, 2), replay, ...shortString(event)]);
const loggerEvent = (timeUs: bigint | number, code: number, detail: string | number[] = "") =>
  rec(R.event, [...le(timeUs, 8), code, ...shortString(detail)]);
const diagnostics = (timeUs: bigint | number, entries: { bus: number; kind: number; count: number }[]) =>
  rec(R.diagnostics, [...le(timeUs, 8), ...entries.flatMap((e) => [e.bus, e.kind, ...le(e.count, 4)])]);

const canId = (deviceType: number, manufacturer: number, apiClass: number, apiIndex: number, device: number) =>
  (((deviceType & 0x1f) << 24) | ((manufacturer & 0xff) << 16) | ((apiClass & 0x3f) << 10) | ((apiIndex & 0xf) << 6) | (device & 0x3f)) >>> 0;
const SPARK = { type: 2, manufacturer: 5, statusClass: 0x2e, frames: 10 };
const SERVO_HUB = { type: 12, manufacturer: 5, statusClass: 0x2e, frames: 5 };
const ENCODER = { type: 7, manufacturer: 5, statusClass: 0x16, frames: 5 };
const A301 = { type: 2, manufacturer: 3, statusClass: 0x2e, frames: 4 };
const DEVICES = [SPARK, SERVO_HUB, ENCODER, A301];
type Device = typeof SPARK;
const status = (d: Device, index: number, device: number) => canId(d.type, d.manufacturer, d.statusClass, index, device);
const firmwareId = (d: Device, device: number) => canId(d.type, d.manufacturer, 0x09, 8, device);

const cat = (...parts: number[][]) => parts.flat();

async function genConvert(): Promise<void> {
  const { parseREVLOG } = await import(pathToFileURL(path.join(PKG, "index.js")).href);
  const rng = new Rng(0x5eed0f14, "revlog/convert");

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-genrevlog-"));
  process.chdir(scratch);
  let uncaught = 0;
  let inCase = false;
  process.on("uncaughtException", (error) => {
    if (!inCase) {
      console.error(error);
      process.exit(1);
    }
    uncaught++;
  });

  const blobs: Uint8Array[] = [];
  const blob = (bytes: number[] | Uint8Array): number => {
    blobs.push(Uint8Array.from(bytes));
    out({ kind: "blob", blob: blobs.length - 1, bytes: hex(blobs[blobs.length - 1]) });
    return blobs.length - 1;
  };

  type Input = { bytes: number[] | Uint8Array } | { blob: number; len?: number; flips?: [number, number][] };
  const materialize = (input: Input): Uint8Array => {
    if ("bytes" in input) return Uint8Array.from(input.bytes);
    const bytes = Uint8Array.from(blobs[input.blob].subarray(0, input.len ?? blobs[input.blob].length));
    for (const [offset, xor] of input.flips ?? []) bytes[offset] ^= xor;
    return bytes;
  };
  const describe = (input: Input) => {
    if ("bytes" in input) return { bytes: hex(Uint8Array.from(input.bytes)) };
    const d: Record<string, unknown> = { blob: input.blob };
    if (input.len !== undefined) d.len = input.len;
    if (input.flips !== undefined) d.flips = input.flips;
    return d;
  };

  let caseIndex = 0;
  const settle = async () => {
    for (let i = 0; i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
  };
  const run = async (name: string, setup: "file" | "missing" | "directory" | "unreadable" | "none", input?: Input, given?: string) => {
    const index = caseIndex++;
    const p = given ?? `case-${index}.revlog`;
    if (setup === "file" || setup === "unreadable") fs.writeFileSync(p, materialize(input!));
    if (setup === "directory") fs.mkdirSync(p);
    if (setup === "unreadable") fs.chmodSync(p, 0);
    await settle();
    uncaught = 0;
    inCase = true;
    const chunks: Buffer[] = [];
    const writable = new Writable({
      write(chunk, _encoding, callback) {
        chunks.push(chunk);
        callback();
      }
    });
    let output: Buffer | null = null;
    let error: [string, string] | null = null;
    try {
      await parseREVLOG(p, writable);
      output = Buffer.concat(chunks);
    } catch (err: any) {
      error = [err.name, err.message];
    }
    await settle();
    inCase = false;
    const record: Record<string, unknown> = { kind: "case", case: index, name, path: p, setup };
    if (input !== undefined) record.input = describe(input);
    record.output = output === null ? null : hex(output);
    record.error = error;
    record.uncaught = uncaught;
    out(record);
    if (setup === "unreadable") fs.chmodSync(p, 0o600);
  };
  const file = (name: string, input: Input) => run(name, "file", input);
  const bytes = (b: number[] | Uint8Array): Input => ({ bytes: b });

  // Opening the file.
  await run("missing file", "missing", undefined, "missing.revlog");
  await run("directory", "directory", undefined, "dir.revlog");
  await run("unreadable file", "unreadable", bytes(cat(header({ startUs: 1 }))), "locked.revlog");
  await run("NUL in path", "none", undefined, "nul\u0000.revlog");
  await run("NUL and quote in path", "none", undefined, "it's\u0000.revlog");
  await run("NUL and both quotes in path", "none", undefined, "\"it's\"\u0000\n\t\\.revlog");
  await run("NUL and every quote in path", "none", undefined, "`\"'\u0000.revlog");
  await run("long path with NUL", "none", undefined, "x".repeat(140) + "\u0000\u0001\u007fé\u{1f600}.revlog");
  await run("NUL path inspected to 128 units", "none", undefined, "a".repeat(115) + "\u0000.revlog");
  await run("NUL path inspected to 129 units", "none", undefined, "a".repeat(116) + "\u0000.revlog");
  await run("control characters with NUL", "none", undefined, "\b\f\r\u000b\u001b\u0000.revlog");
  await run("lone surrogates and C1 controls with NUL", "none", undefined, "\ud800a\udc00😀\u0085\u009f '\u0000.revlog");
  await run("long path, lone surrogates and C1 controls with NUL", "none", undefined, "y".repeat(90) + "\ud800a\udc00\u0085\u009f\\'\u0000\u007f" + "z".repeat(40) + ".revlog");

  // Nothing to convert.
  await file("empty file", bytes([]));
  await file("one byte", bytes([0]));
  await file("v1 with no v1 records", bytes(cat(rec(5, [1, 2, 3]), rec(9, []))));
  await file("v1 persistent record, empty", bytes(rec(R.persistent, [])));
  await file("v2 header only", bytes(header()));
  await file("v2 header with start time only", bytes(header({ startUs: 0x0102030405060708n, tail: false })));
  await file("header magic wrong", bytes(header({ magic: "REVLOX", startUs: 1 })));
  await file("header magic with high bits set", bytes(cat(rec(R.header, [0xd2, 0xc5, 0x56, 0xcc, 0x4f, 0xc7, 2, 0, ...le(3, 8)]), loggerEvent(4, 2))));
  await file("header payload of 7 bytes", bytes(rec(R.header, [...utf8("REVLOG"), 2])));
  await file("header payload empty", bytes(rec(R.header, [])));
  await file("header newer major", bytes(header({ major: 3, minor: 1 })));
  await file("header major 255", bytes(header({ major: 255, minor: 255, startUs: 5 })));
  await file("header minor ignored", bytes(cat(header({ major: 2, minor: 200, startUs: 1 }), loggerEvent(2, 1))));
  await file("unrealistic payload size first", bytes(rec(R.header, [], { size: 5 * 1024 * 1024 + 1, sizeLen: 3 })));
  await file("payload size exactly 5 MiB, truncated", bytes(cat(rec(R.header, [], { size: 5 * 1024 * 1024, sizeLen: 3 }), [1, 2, 3])));

  // v2: every record type.
  const sparkFw = firmwareId(SPARK, 3);
  const basic = blob(
    cat(
      header({ startUs: 1_000_000 }),
      busTable([
        { id: 0, name: "rio" },
        { id: 1, name: "CANivore Main" }
      ]),
      firmware([
        { bus: 0, id: sparkFw, data: [25, 1, 0, 4] },
        { bus: 0, id: firmwareId(SERVO_HUB, 1), data: [26, 2, 3] },
        { bus: 1, id: firmwareId(ENCODER, 9), data: [0, 1, 2, 3, 1, 26] },
        { bus: 1, id: firmwareId(A301, 4), data: [1, 2, 0x01, 0x02] }
      ]),
      robotState(1_000_100, controlWord({ mode: 0 })),
      matchInfo(1_000_150, 2, 42, 1, "2027cmptx"),
      loggerEvent(1_000_200, 1),
      loggerEvent(1_000_210, 2, "/U/logs"),
      frames(0, 1_000_300, [
        { offsetUs: 0, id: status(SPARK, 0, 3), data: [0x10, 0x27, 0xc8, 0x00, 0x96, 0x11, 0x2c, 0x01] },
        { offsetUs: 5, id: status(SPARK, 1, 3), data: [1, 0, 0, 0, 0, 0, 0, 0] },
        { offsetUs: 9, id: status(SPARK, 2, 3), data: [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x20, 0x41] },
        { offsetUs: 12, id: status(SERVO_HUB, 0, 1), data: [0x30, 0x11, 0x05, 0x00, 0x00] }
      ]),
      frames(1, 1_000_310, [
        { offsetUs: 0, id: status(ENCODER, 2, 9), data: [0, 0, 0x80, 0x3f, 0, 0, 0, 0x40] },
        { offsetUs: 1, id: status(A301, 0, 4), data: [1, 2, 3, 4, 5, 6, 7, 8] }
      ]),
      robotState(1_000_400, controlWord({ enabled: true, mode: 1 })),
      robotState(1_000_500, controlWord({ enabled: true, mode: 1 })),
      robotState(1_000_600, controlWord({ enabled: false, mode: 2 })),
      robotState(900, controlWord({ enabled: true, mode: 3 })),
      matchInfo(1_000_700, 2, 42, 0, "2027cmptx"),
      matchInfo(1_000_800, 3, 42, 0, "2027cmptx"),
      firmware([{ bus: 0, id: sparkFw, data: [25, 1, 0, 4] }]),
      firmware([{ bus: 0, id: sparkFw, data: [25, 2, 0, 1] }]),
      diagnostics(1_000_900, [
        { bus: 0, kind: 1, count: 3 },
        { bus: 1, kind: 2, count: 9 },
        { bus: 0, kind: 1, count: 4 },
        { bus: 7, kind: 1, count: 1 }
      ]),
      loggerEvent(1_001_000, 3),
      loggerEvent(1_001_100, 4, "after pause"),
      loggerEvent(1_001_200, 5),
      loggerEvent(1_001_300, 6, "/media/sda1"),
      loggerEvent(1_001_400, 0),
      loggerEvent(1_001_500, 7, "x"),
      loggerEvent(1_001_600, 255, "é\u{1f600}")
    )
  );
  await file("v2 every record type", { blob: basic });
  for (let len = 0; len < blobs[basic].length; len++) await file(`v2 every record type cut at ${len}`, { blob: basic, len });
  for (let i = 0; i < 48; i++) {
    const offset = rng.below(blobs[basic].length);
    await file(`v2 every record type flip ${i}`, { blob: basic, flips: [[offset, 1 << rng.below(8)]] });
  }

  // v1: firmware tables and periodic frames.
  const v1Firmware = v1Persistent([
    { id: firmwareId(SPARK, 3), data: [25, 0, 0, 4, 0, 0x45] },
    { id: firmwareId(SERVO_HUB, 1), data: [2, 1, 25, 0, 1, 0] },
    { id: firmwareId(ENCODER, 9), data: [0, 1, 2, 3, 1, 26] },
    { id: firmwareId(A301, 2), data: [1, 0, 0, 9, 0, 0] },
    { id: canId(9, 4, 1, 1, 1), data: [1, 2, 3, 4, 5, 6] }
  ]);
  const v1Parts: number[][] = [];
  for (let loop = 0; loop < 4; loop++) {
    const ms = 1000 + loop * 25;
    v1Parts.push(v1Firmware, v1Firmware);
    v1Parts.push(
      v1Periodic([
        { ms, id: status(SPARK, 0, 3), data: [0x10, 0x27, 0xc8, 0x00, 0x96, 0x11, 0x2c, loop] },
        { ms: ms + 1, id: status(SPARK, 2, 3), data: [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x20, 0x41] },
        { ms: ms + 2, id: status(SERVO_HUB, 0, 1), data: [0x30, 0x11, 0x05, 0x00, loop] },
        { ms: ms + 3, id: status(ENCODER, 2, 9), data: [0, 0, 0x80, 0x3f, 0, 0, 0, 0x40] },
        { ms: ms + 4, id: status(A301, 1, 2), data: [loop, 1, 2, 3, 4, 5, 6, 7] }
      ])
    );
  }
  v1Parts.push(rec(R.header, [...utf8("REVLOG"), 2, 0]), rec(R.frames, [1, 2, 3, 4, 5, 6, 7, 8, 9]));
  v1Parts.push(v1Periodic([{ ms: 0xffffffff, id: status(SPARK, 0, 3), data: [1, 2, 3, 4, 5, 6, 7, 8] }]));
  v1Parts.push(rec(R.persistent, [...le(firmwareId(SPARK, 3), 4), 26, 0, 0, 1, 0, 0, 9, 9, 9]));
  v1Parts.push(rec(R.periodic, [...le(5, 4), ...le(status(SPARK, 1, 3), 4), 1, 2, 3, 4, 5, 6, 7, 8, 9, 9]));
  const v1 = blob(cat(...v1Parts));
  await file("v1 firmware and frames", { blob: v1 });
  for (let len = 0; len < blobs[v1].length; len++) await file(`v1 cut at ${len}`, { blob: v1, len });
  for (let i = 0; i < 24; i++) {
    const offset = rng.below(blobs[v1].length);
    await file(`v1 flip ${i}`, { blob: v1, flips: [[offset, 1 << rng.below(8)]] });
  }
  await file("v1 short firmware tables", bytes(cat(
    v1Persistent([{ id: firmwareId(SERVO_HUB, 2), data: [1, 2, 3, 4, 5, 6] }]),
    rec(R.persistent, [...le(firmwareId(ENCODER, 1), 4), 1, 2, 3, 4, 5, 6, ...le(firmwareId(SPARK, 1), 4), 1])
  )));

  // Record framing: id and size widths, ignored high bits, unknown ids.
  for (const idLen of [1, 2, 3, 4]) {
    for (const sizeLen of [1, 2, 3, 4]) {
      await file(`framing id ${idLen} size ${sizeLen}`, bytes(cat(
        rec(R.header, [...utf8("REVLOG"), 2, 0, ...le(77, 8)], { idLen, sizeLen }),
        rec(R.event, [...le(80, 8), 1, 0], { idLen, sizeLen, high: idLen * 4 + sizeLen - 1 })
      )));
    }
  }
  await file("v2 unknown record ids", bytes(cat(
    header({ startUs: 10 }),
    rec(0, [1]),
    rec(R.persistent, v1Firmware.slice(3)),
    rec(R.periodic, [1, 2, 3, 4]),
    rec(11, [9, 9]),
    rec(0x103, [1, 2, 3]),
    rec(0xffffffff, []),
    loggerEvent(12, 1)
  )));
  await file("v2 unrealistic size stops parsing", bytes(cat(
    header({ startUs: 10 }),
    loggerEvent(11, 1),
    rec(R.event, [], { size: 5 * 1024 * 1024 + 1, sizeLen: 4 }),
    loggerEvent(12, 2)
  )));
  await file("v2 payload past end", bytes(cat(header({ startUs: 10 }), loggerEvent(11, 1), rec(R.event, [1, 2, 3], { size: 40 }))));

  // Header records after the first.
  await file("second header switches to v1", bytes(cat(
    header({ startUs: 10 }),
    frames(0, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    header({ major: 1, startUs: 30 }),
    v1Periodic([{ ms: 1, id: status(SPARK, 0, 1), data: [8, 7, 6, 5, 4, 3, 2, 1] }]),
    v1Persistent([{ id: firmwareId(SPARK, 1), data: [1, 2, 3, 4, 0, 0] }]),
    loggerEvent(40, 1)
  )));
  await file("second header major 0 redecides", bytes(cat(
    header({ startUs: 10 }),
    header({ major: 0, startUs: 30 }),
    loggerEvent(40, 1),
    header({ major: 2, startUs: 50 }),
    loggerEvent(60, 3)
  )));
  await file("second header major 0 then v1", bytes(cat(
    header({ startUs: 10 }),
    header({ major: 0 }),
    v1Persistent([{ id: firmwareId(SPARK, 1), data: [1, 2, 3, 4, 0, 0] }]),
    loggerEvent(40, 1)
  )));
  await file("invalid second header", bytes(cat(header({ startUs: 10 }), loggerEvent(11, 1), header({ magic: "revlog" }))));
  await file("newer second header", bytes(cat(header({ startUs: 10 }), loggerEvent(11, 1), header({ major: 9, minor: 4 }))));
  await file("header in a v1 file is ignored", bytes(cat(
    v1Persistent([{ id: firmwareId(SPARK, 1), data: [1, 2, 3, 4, 0, 0] }]),
    header({ major: 7 }),
    v1Persistent([{ id: firmwareId(SPARK, 1), data: [1, 2, 3, 5, 0, 0] }])
  )));
  await file("v2 v1-prefixed names after major 1 header", bytes(cat(
    header({ major: 1, startUs: 99 }),
    firmware([{ bus: 3, id: firmwareId(SPARK, 1), data: [1, 2, 3, 4] }]),
    frames(3, 100, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }])
  )));

  // Bus table.
  await file("bus table edge cases", bytes(cat(
    header({ startUs: 10 }),
    busTable([
      { id: 0, name: "first" },
      { id: 0, name: "renamed" },
      { id: 2, name: "" },
      { id: 3, name: [0xff, 0xfe, 0x41, 0xc3] },
      { id: 255, name: "été \u{1f600}" }
    ]),
    rec(R.busTable, [4, 200, ...utf8("short")]),
    rec(R.busTable, [5]),
    frames(0, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    frames(2, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    frames(3, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    frames(4, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    frames(255, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    frames(9, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    busTable([{ id: 0, name: "late" }, { id: 9, name: "late9" }]),
    frames(0, 30, [
      { offsetUs: 0, id: status(SPARK, 0, 1), data: [8, 7, 6, 5, 4, 3, 2, 1] },
      { offsetUs: 0, id: status(SPARK, 1, 1), data: [8, 7, 6, 5, 4, 3, 2, 1] }
    ]),
    firmware([{ bus: 0, id: firmwareId(SPARK, 1), data: [1, 2, 3, 4] }]),
    diagnostics(40, [{ bus: 9, kind: 1, count: 2 }, { bus: 6, kind: 1, count: 0 }])
  )));
  await file("long names widen the control record size", bytes(cat(
    header({ startUs: 10 }),
    busTable([{ id: 0, name: "B".repeat(255) }]),
    frames(0, 20, [{ offsetUs: 0, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    loggerEvent(30, 2, "D".repeat(255)),
    matchInfo(31, 1, 2, 0, "E".repeat(255))
  )));

  // Firmware records.
  await file("firmware edge cases", bytes(cat(
    header({ startUs: 0xfedcba9876543210n }),
    firmware([
      { bus: 0, id: firmwareId(SPARK, 0), data: [] },
      { bus: 0, id: firmwareId(SPARK, 63), data: [1, 2, 3] },
      { bus: 0, id: firmwareId(SPARK, 62), data: [1, 2, 0xff, 0xff, 9, 9] },
      { bus: 0, id: firmwareId(SERVO_HUB, 5), data: [1, 2] },
      { bus: 0, id: firmwareId(SERVO_HUB, 6), data: [1, 2, 3] },
      { bus: 0, id: firmwareId(ENCODER, 5), data: [1, 2, 3, 4, 5] },
      { bus: 0, id: firmwareId(ENCODER, 6), data: [9, 9, 9, 4, 5, 6, 7] },
      { bus: 0, id: firmwareId(A301, 7), data: [3, 4, 0x12, 0x34] },
      { bus: 0, id: canId(2, 4, 0, 0, 1), data: [1, 2, 3, 4] },
      { bus: 0, id: canId(1, 5, 0, 0, 1), data: [1, 2, 3, 4] },
      { bus: 0, id: (firmwareId(SPARK, 8) | 0xe0000000) >>> 0, data: [5, 6, 7, 8] },
      { bus: 1, id: firmwareId(SPARK, 0), data: [1, 2, 3, 4] }
    ]),
    rec(R.firmware, [0, ...le(firmwareId(SPARK, 9), 4), 4, 1, 2, 3, 4, 0, ...le(firmwareId(SPARK, 10), 4), 9, 1, 2]),
    rec(R.firmware, [0, ...le(firmwareId(SPARK, 11), 4), 0, 7, 7, 7, 7, 7]),
    frames(0, 0xfedcba9876543300n, [{ offsetUs: 0xffff, id: status(SPARK, 0, 1), data: [1] }]),
    firmware([{ bus: 0, id: firmwareId(SPARK, 0), data: [] }]),
    firmware([{ bus: 0, id: firmwareId(SPARK, 0), data: [0, 0, 0, 0] }]),
    robotState(5, controlWord({})),
    firmware([{ bus: 0, id: firmwareId(SPARK, 0), data: [0, 0, 0, 1] }])
  )));

  // Frames records.
  await file("frames edge cases", bytes(cat(
    header({ startUs: 10 }),
    rec(R.frames, [1, 2, 3, 4, 5, 6, 7, 8]),
    rec(R.frames, [...le(100, 8), 0]),
    rec(R.frames, [...le(100, 8), 0, 1, 0, ...le(status(SPARK, 0, 2), 4), 8, 1, 2, 3]),
    frames(0, 200, [
      { offsetUs: 0, id: status(SPARK, 0, 2), data: [] },
      { offsetUs: 1, id: status(SPARK, 0, 2), data: [0xff] },
      { offsetUs: 2, id: status(SPARK, 0, 2), data: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] },
      { offsetUs: 3, id: status(SPARK, 3, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 4, id: status(SPARK, 10, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 5, id: status(SPARK, 15, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 6, id: canId(2, 5, 0x00, 4, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 7, id: canId(2, 5, 0x3f, 5, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 8, id: status(SERVO_HUB, 5, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 9, id: status(ENCODER, 0, 2), data: [] },
      { offsetUs: 10, id: status(ENCODER, 1, 2), data: [7] },
      { offsetUs: 11, id: canId(3, 5, 0x2e, 0, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 12, id: canId(2, 6, 0x2e, 0, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 0xffff, id: status(SPARK, 1, 2), data: [0, 0, 0, 0, 0, 0, 0, 0] }
    ]),
    frames(0, 100, [{ offsetUs: 0, id: status(SPARK, 1, 2), data: [1, 0, 0, 0, 0, 0, 0, 0] }]),
    firmware([{ bus: 0, id: firmwareId(SPARK, 2), data: [1, 2, 3, 4] }]),
    frames(0, 300, [
      { offsetUs: 0, id: status(SPARK, 2, 2), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 1, id: status(SPARK, 2, 2), data: [1, 2, 3, 4] }
    ]),
    rec(R.frames, [...le(400, 8), 0, ...le(0, 2), ...le(status(SPARK, 0, 2), 4), 8, 1, 2, 3, 4, 5, 6, 7, 8, ...le(1, 2), ...le(status(SPARK, 0, 2), 4), 8, 1, 2]),
    rec(R.frames, [...le(500, 8), 0, ...le(0, 2), ...le(status(SPARK, 0, 2), 4), 2, 1, 2, 9, 9, 9, 9, 9, 9])
  )));
  await file("frames fast-cache collision across buses", bytes(cat(
    header({ startUs: 10 }),
    busTable([{ id: 0, name: "zero" }, { id: 1, name: "one" }]),
    frames(1, 20, [{ offsetUs: 0, id: status(SPARK, 0, 5), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    frames(0, 30, [{ offsetUs: 0, id: (status(SPARK, 0, 5) | 0x20000000) >>> 0, data: [8, 7, 6, 5, 4, 3, 2, 1] }]),
    frames(0, 40, [{ offsetUs: 0, id: (status(SPARK, 0, 5) | 0xc0000000) >>> 0, data: [8, 7, 6, 5, 4, 3, 2, 1] }]),
    frames(2, 50, [{ offsetUs: 0, id: (status(SPARK, 0, 5) | 0xa0000000) >>> 0, data: [8, 7, 6, 5, 4, 3, 2, 1] }]),
    frames(3, 60, [{ offsetUs: 0, id: (status(SPARK, 0, 5) | 0x80000000) >>> 0, data: [8, 7, 6, 5, 4, 3, 2, 1] }])
  )));
  await file("same entry names from different CAN ids and buses", bytes(cat(
    header({ startUs: 10 }),
    busTable([{ id: 0, name: "same" }, { id: 1, name: "same" }]),
    frames(0, 20, [
      { offsetUs: 0, id: status(SPARK, 0, 7), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 1, id: canId(2, 5, 0x01, 0, 7), data: [2, 3, 4, 5, 6, 7, 8, 9] },
      { offsetUs: 2, id: status(SPARK, 0, 7), data: [3, 4, 5, 6, 7, 8, 9, 10] }
    ]),
    frames(1, 30, [{ offsetUs: 0, id: status(SPARK, 0, 7), data: [4, 5, 6, 7, 8, 9, 10, 11] }]),
    firmware([{ bus: 0, id: firmwareId(SPARK, 7), data: [1, 2, 3, 4] }, { bus: 1, id: firmwareId(SPARK, 7), data: [1, 2, 3, 4] }])
  )));
  await file("timestamp widths", bytes(cat(
    header({ startUs: 0 }),
    ...[0n, 0xffn, 0x100n, 0xffffn, 0x10000n, 0xffffffn, 0x1000000n, 0xffffffffn, 0x100000000n, 0xffffffffffn, 0x10000000000n,
      0xffffffffffffn, 0x1000000000000n, 0xffffffffffffffn, 0x100000000000000n, 0xffffffffffffffffn].map((t, i) => loggerEvent(t, i % 7))
  )));
  await file("frame timestamp overflow, then nothing", bytes(cat(
    header({ startUs: 5 }),
    frames(0, 0xffffffffffffff00n, [
      { offsetUs: 0xff, id: status(SPARK, 1, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 0x100, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] },
      { offsetUs: 0x101, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 9] }
    ]),
    loggerEvent(7, 1)
  )));
  await file("frame timestamp overflow, then firmware", bytes(cat(
    header({ startUs: 5 }),
    frames(0, 0xffffffffffffffffn, [{ offsetUs: 9, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    firmware([{ bus: 0, id: firmwareId(SPARK, 1), data: [1, 2, 3, 4] }])
  )));
  await file("frame timestamp overflow, then robot state resets it", bytes(cat(
    header({ startUs: 5 }),
    frames(0, 0xffffffffffffffffn, [{ offsetUs: 1, id: status(SPARK, 0, 1), data: [1, 2, 3, 4, 5, 6, 7, 8] }]),
    robotState(6, controlWord({ enabled: true })),
    firmware([{ bus: 0, id: firmwareId(SPARK, 1), data: [1, 2, 3, 4] }])
  )));

  // Robot state, match info, logger events, diagnostics.
  await file("robot state words", bytes(cat(
    header({ startUs: 10 }),
    rec(R.robotState, [...le(11, 8), 1, 2, 3, 4, 5, 6, 7]),
    ...[0n, 1n << 58n, (1n << 58n) | (1n << 56n), 2n << 56n, 3n << 56n, (1n << 58n) | (3n << 56n), 0xffffffffffffffffn, 1n << 57n,
      (1n << 59n) | (1n << 60n) | (1n << 61n), 0n, 0n].map((w, i) => robotState(20 + i * 10, w)),
    rec(R.robotState, [...le(500, 8), ...le(1n << 58n, 8), 1, 2, 3])
  )));
  await file("match info", bytes(cat(
    header({ startUs: 10 }),
    rec(R.matchInfo, [...le(11, 8), 1, 2, 3, 4]),
    rec(R.matchInfo, [...le(12, 8), 1, 2, 0, 0, 0]),
    matchInfo(13, 1, 2, 0, ""),
    matchInfo(14, 1, 2, 1, ""),
    matchInfo(15, 255, 0xffff, 255, "x"),
    matchInfo(16, 255, 0xffff, 0, "x"),
    rec(R.matchInfo, [...le(17, 8), 0, 0, 0, 0, 50, ...utf8("cut")]),
    matchInfo(18, 0, 0, 0, [0xc3, 0x28, 0xe2, 0x82]),
    matchInfo(19, 0, 1, 0, "\u{1f600}")
  )));
  await file("logger events", bytes(cat(
    header({ startUs: 10 }),
    rec(R.event, [...le(11, 8), 1]),
    rec(R.event, [...le(12, 8), 1, 0]),
    rec(R.event, [...le(13, 8), 2, 9, ...utf8("abc")]),
    rec(R.event, [...le(14, 8), 3, 0, 1, 2, 3]),
    loggerEvent(15, 1, "same"),
    loggerEvent(15, 1, "same"),
    loggerEvent(16, 6, [0xed, 0xa0, 0x80, 0x41]),
    loggerEvent(17, 99, "\u0000"),
    loggerEvent(18, 0, "")
  )));
  await file("diagnostics", bytes(cat(
    header({ startUs: 10 }),
    rec(R.diagnostics, [1, 2, 3, 4, 5, 6, 7]),
    rec(R.diagnostics, le(11, 8)),
    diagnostics(12, [{ bus: 0, kind: 1, count: 0xffffffff }]),
    diagnostics(13, [{ bus: 0, kind: 1, count: 0xffffffff }, { bus: 0, kind: 0, count: 5 }, { bus: 0, kind: 255, count: 5 }]),
    diagnostics(14, [{ bus: 0, kind: 1, count: 1 }]),
    rec(R.diagnostics, [...le(15, 8), 1, 1, 5, 0, 0, 0, 2, 1, 7]),
    busTable([{ id: 1, name: "named" }]),
    diagnostics(16, [{ bus: 1, kind: 1, count: 1 }])
  )));

  // Every reachable signal of every device type, with random and extreme payloads.
  const sweep: number[][] = [header({ startUs: 1 }), busTable([{ id: 0, name: "rio" }, { id: 1, name: "fd" }])];
  for (const d of DEVICES) {
    for (let index = 0; index < 16; index++) {
      const entries: { offsetUs: number; id: number; data: number[] | Uint8Array }[] = [];
      for (const fill of [0x00, 0xff, 0x80, 0x7f]) entries.push({ offsetUs: entries.length, id: status(d, index, 1), data: new Uint8Array(8).fill(fill) });
      for (let i = 0; i < 12; i++) entries.push({ offsetUs: entries.length, id: status(d, index, 1 + (i % 3)), data: rng.bytes(rng.pick([0, 3, 8, 8, 8, 12])) });
      sweep.push(frames(rng.below(2), 1000 + index * 1000 + d.type * 100000, entries));
    }
  }
  const sweepBlob = blob(cat(...sweep));
  await file("signal sweep", { blob: sweepBlob });

  // Larger logs: many records across 64 KiB read chunks and output flushes.
  const robot = (seconds: number, v1Format: boolean) => {
    const parts: number[][] = [];
    const devices: { d: Device; n: number; bus: number }[] = [];
    for (let n = 1; n <= 8; n++) devices.push({ d: SPARK, n, bus: n <= 4 ? 0 : 1 });
    devices.push({ d: SERVO_HUB, n: 1, bus: 0 }, { d: ENCODER, n: 9, bus: 1 }, { d: ENCODER, n: 10, bus: 1 }, { d: A301, n: 12, bus: 0 });
    const t0 = 18_000_000n;
    if (!v1Format) {
      parts.push(header({ startUs: t0 }), busTable([{ id: 0, name: "rio" }, { id: 1, name: "canivore" }]));
      parts.push(firmware(devices.map((x) => ({ bus: x.bus, id: firmwareId(x.d, x.n), data: [26, 1, 0, x.n] }))));
      parts.push(loggerEvent(t0, 1), matchInfo(t0, 2, 17, 0, "2027txhou"));
    }
    const smooth = (x: { n: number }, index: number, tick: number) => {
      const base = (tick * (x.n + 1) + index * 37) & 0xffff;
      return [base & 0xff, base >> 8, (tick >> 3) & 0xff, x.n, index, 0x40, tick & 0x0f, 0];
    };
    for (let tick = 0; tick < seconds * 50; tick++) {
      const us = t0 + BigInt(tick) * 20_000n;
      if (v1Format) {
        if (tick % 5 === 0) parts.push(v1Persistent(devices.map((x) => ({ id: firmwareId(x.d, x.n), data: [26, 1, 0, x.n, 0, 0] }))));
        parts.push(v1Periodic(devices.flatMap((x) => [0, 1, 2].map((index) => ({ ms: Number(us / 1000n) + index, id: status(x.d, index, x.n), data: smooth(x, index, tick) })))));
      } else {
        for (const bus of [0, 1]) {
          parts.push(frames(bus, us, devices.filter((x) => x.bus === bus).flatMap((x) =>
            [0, 1, 2, 3].filter((index) => index < x.d.frames && (index < 2 || tick % 5 === 0)).map((index) => ({ offsetUs: index * 97 + x.n, id: status(x.d, index, x.n), data: smooth(x, index, tick) })))));
        }
        if (tick % 10 === 0) parts.push(robotState(us, controlWord({ enabled: tick > 100, mode: tick > 100 ? 1 + ((tick / 250) | 0) % 2 : 0 })));
        if (tick % 100 === 50) parts.push(diagnostics(us, [{ bus: tick % 200 === 50 ? 0 : 1, kind: 1, count: 1 + (tick % 7) }]));
        if (tick % 150 === 0) parts.push(firmware([{ bus: 0, id: firmwareId(SPARK, 1), data: [26, 1, 0, (tick / 150) & 3] }]));
      }
    }
    if (!v1Format) parts.push(loggerEvent(t0 + BigInt(seconds) * 1_000_000n, 3));
    return cat(...parts);
  };
  await file("robot v2 log", bytes(robot(8, false)));
  await file("robot v1 log", bytes(robot(5, true)));
  const robotBlob = blob(robot(3, false));
  await file("robot v2 log cut mid-record", { blob: robotBlob, len: blobs[robotBlob].length - 5 });

  fs.rmSync(scratch, { recursive: true, force: true });
}

const mode = process.argv[2];
if (mode === "dbc") await genDbc();
else if (mode === "convert") await genConvert();
else throw new Error(`genRevlog: unknown mode ${mode}`);
