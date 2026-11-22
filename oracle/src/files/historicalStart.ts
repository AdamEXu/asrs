import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import path from "node:path";
import { crc32 } from "node:zlib";
import lzmaStub from "lzma-native";
import * as fakeFs from "../fakes/mainFs";
import * as electronFake from "../fakes/mainElectron";
import { toTree } from "../cve1";

// Ground truth for asrs-services' file-open pipeline: main.ts's real `historical-start` and
// `historical-start-raw` handlers, reached through the hub port, against the fake fs, with the
// real lzma-native addon (main.ts's `require("lzma-native")` gets the build's stub object, which
// is filled with the real module's exports before any case runs).
//
// Records, in order:
//   {"kind":"blob","id","bytes"}         an input other records refer to
//   {"kind":"case", ...}                  one hub message and everything main did in reply
// An input is {"bytes"} or {"blob", "len", "flips": [[offset, xor]...]}: the blob's first `len`
// bytes with each listed byte XORed. Reply files and dumped files are {"bytes"} up to
// INLINE_LIMIT bytes, otherwise {"len", "sha256"}.

declare const __UPSTREAM_DIR__: string;

const INLINE_LIMIT = 4096;
const TEMP = "/fake/temp";
const ROOT = "/fake/logs";

const realLzma = createRequire(path.join(__UPSTREAM_DIR__, "package.json"))("lzma-native");
Object.assign(lzmaStub, realLzma);

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

let rngState = 0x9e3779b9;
function nextU32(): number {
  rngState ^= rngState << 13;
  rngState ^= rngState >>> 17;
  rngState ^= rngState << 5;
  return rngState >>> 0;
}
const below = (n: number) => nextU32() % n;

const drawn: number[] = [];
let scriptedState = 0x1234567;
Math.random = () => {
  scriptedState ^= scriptedState << 13;
  scriptedState ^= scriptedState >>> 17;
  scriptedState ^= scriptedState << 5;
  const value = (scriptedState >>> 0) / 4294967296;
  drawn.push(value);
  return value;
};

let uncaught = 0;
process.on("uncaughtException", () => {
  uncaught++;
});

type Input = { bytes: Uint8Array } | { blob: number; len?: number; flips?: [number, number][] };

const blobs: Uint8Array[] = [];
function blob(bytes: Uint8Array): number {
  blobs.push(bytes);
  out({ kind: "blob", id: blobs.length - 1, bytes: toTree(bytes) });
  return blobs.length - 1;
}

function materialize(input: Input): Uint8Array {
  if ("bytes" in input) return input.bytes;
  const source = blobs[input.blob];
  const bytes = new Uint8Array(source.subarray(0, input.len ?? source.length));
  for (const [offset, xor] of input.flips ?? []) bytes[offset] ^= xor;
  return bytes;
}

function describeInput(input: Input): unknown {
  if ("bytes" in input) return { bytes: toTree(input.bytes) };
  const d: any = { blob: input.blob };
  if (input.len !== undefined) d.len = input.len;
  if (input.flips !== undefined) d.flips = input.flips;
  return d;
}

function describeBytes(bytes: Uint8Array | null | undefined): unknown {
  if (bytes === null || bytes === undefined) return null;
  if (bytes.length <= INLINE_LIMIT) return { bytes: toTree(new Uint8Array(bytes)) };
  return { len: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
}

const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));

async function compress(coder: string, options: object, data: Uint8Array): Promise<Uint8Array> {
  return await new Promise((resolve, reject) => {
    const stream = realLzma.createStream(coder, options);
    const chunks: Buffer[] = [];
    stream.on("data", (chunk: Buffer) => chunks.push(chunk));
    stream.on("error", reject);
    stream.on("end", () => resolve(new Uint8Array(Buffer.concat(chunks))));
    stream.end(Buffer.from(data));
  });
}
const xz = (data: Uint8Array, check: number, preset = 6) => compress("easyEncoder", { preset, check }, data);
const concat = (...parts: Uint8Array[]) => new Uint8Array(Buffer.concat(parts.map((p) => Buffer.from(p))));

function plaintext(size: number, randomRuns: boolean): Uint8Array {
  const words = ["/RealOutputs/Drive/", "Pose2d", "YZ", "timestamp", "\u0000\u0001", "Odometry", "YYZZ", "battery"];
  const bytes: number[] = [];
  while (bytes.length < size) {
    if (randomRuns && below(4) === 0) {
      const run = 16 + below(200);
      for (let i = 0; i < run; i++) bytes.push(below(256));
    } else {
      bytes.push(...Buffer.from(words[below(words.length)], "utf8"));
    }
  }
  return new Uint8Array(bytes.slice(0, size));
}

function findPort(windowId: number): electronFake.FakePort | null {
  const window = electronFake.FakeBrowserWindow.fromId(windowId);
  const posted = window?.webContents.posted.filter((p: any) => p.channel === "port") ?? [];
  const port1 = posted[posted.length - 1]?.ports?.[0];
  return electronFake.channels.find((c) => c.port1 === port1)?.port2 ?? null;
}

const immediate = () => new Promise((resolve) => setImmediate(resolve));

async function settle(hasReply: () => boolean): Promise<void> {
  const start = performance.now();
  while (!hasReply() && performance.now() - start < 5000) await immediate();
  for (let i = 0; i < 20; i++) await immediate();
}

interface Case {
  files?: [string, Input][];
  failures?: [string, "read" | "write" | "readOpened"][];
  missingDirs?: string[];
  request:
    | { name: "historical-start"; path: string }
    | { name: "historical-start-raw"; data: Input; extension: string };
}

let caseIndex = 0;
let hubPort: electronFake.FakePort | null = null;
const recent: string[] = [];

async function runCase(c: Case): Promise<void> {
  fakeFs.reset();
  electronFake.takeSent();
  drawn.splice(0, drawn.length);
  recent.splice(0, recent.length);
  uncaught = 0;
  for (const [name, input] of c.files ?? []) fakeFs.putFile(name, materialize(input));
  for (const [name, kind] of c.failures ?? []) fakeFs.setFailure(name, kind);
  for (const dir of c.missingDirs ?? []) fakeFs.setMissingDir(dir, true);

  const uuid = `case-${caseIndex}`;
  const record: any = { kind: "case", case: caseIndex++ };
  if (c.files !== undefined) record.files = c.files.map(([name, input]) => [name, describeInput(input)]);
  if (c.failures !== undefined) record.failures = c.failures;
  if (c.missingDirs !== undefined) record.missingDirs = c.missingDirs;
  let data: any;
  if (c.request.name === "historical-start") {
    record.request = { name: c.request.name, uuid, path: c.request.path };
    data = { uuid, path: c.request.path };
  } else {
    record.request = {
      name: c.request.name,
      uuid,
      data: describeInput(c.request.data),
      extension: c.request.extension
    };
    data = { uuid, data: materialize(c.request.data), extension: c.request.extension };
  }

  const replies: any[] = [];
  const collect = () => {
    for (const m of electronFake.takeSent()) if (m?.name === "historical-data") replies.push(m.data);
    return replies.length > 0;
  };
  hubPort!.emit("message", { data: { name: c.request.name, data } });
  await settle(collect);
  collect();

  record.random = drawn.slice();
  record.recent = recent.slice();
  record.replies = replies.map((reply) => ({
    uuid: reply.uuid,
    error: reply.error,
    files: reply.files.map((file: Uint8Array | null) => describeBytes(file))
  }));
  record.uncaught = uncaught;
  record.writes = fakeFs.takeWrites();
  record.after = Object.entries(fakeFs.dumpFiles()).map(([name, bytes]) => [name, describeBytes(bytes)]);
  out(record);
}

const start = (p: string): Case["request"] => ({ name: "historical-start", path: p });
const raw = (d: Input, extension: string): Case["request"] => ({ name: "historical-start-raw", data: d, extension });

export async function run(): Promise<void> {
  electronFake.app.addRecentDocument = (p: string) => {
    recent.push(p);
  };
  electronFake.fireReady();
  for (let i = 0; i < 12; i++) await new Promise((resolve) => setImmediate(resolve));
  const windowId = electronFake.windowList()[0]?.id ?? 0;
  electronFake.FakeBrowserWindow.fromId(windowId)?.webContents.emit("dom-ready");
  hubPort = findPort(windowId);
  if (hubPort === null) throw new Error("historicalStart: no hub port");

  const { CHECK_NONE, CHECK_CRC32, CHECK_CRC64, CHECK_SHA256 } = realLzma;
  const small = plaintext(1200, true);
  const large = plaintext(150000, true);
  const text = utf8("hello YZ world\n");

  const plainBlob = blob(plaintext(5000, true));
  const xzSmall = blob(await xz(small, CHECK_CRC64));
  const xzSmallCrc32 = blob(await xz(small, CHECK_CRC32, 1));
  const xzSmallNone = blob(await xz(small, CHECK_NONE, 0));
  const xzSmallSha = blob(await xz(small, CHECK_SHA256, 9));
  const xzLarge = blob(await xz(large, CHECK_CRC32));
  const xzLargeCrc64 = blob(await xz(large, CHECK_CRC64, 1));
  const xzEmpty = blob(await xz(new Uint8Array(0), CHECK_CRC64));
  const xzText = blob(await xz(text, CHECK_CRC32));
  const xzMulti = blob(await compress("easyEncoder", { preset: 1, check: CHECK_CRC32, threads: 2, blockSize: 20000 }, large));
  const alone = blob(await compress("aloneEncoder", { preset: 6 }, small));
  const aloneLarge = blob(await compress("aloneEncoder", { preset: 1 }, large));
  const zeros = (n: number) => new Uint8Array(n);
  const garbage = new Uint8Array(Array.from({ length: 100 }, () => below(256)));
  const xzMagicGarbage = concat(new Uint8Array([0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]), garbage);
  const concatenated = blob(concat(blobs[xzText], blobs[xzSmall]));
  const padded = blob(concat(blobs[xzText], zeros(4), blobs[xzSmall], zeros(8)));
  const oddPadded = blob(concat(blobs[xzText], zeros(3), blobs[xzSmall]));
  const trailingGarbage = blob(concat(blobs[xzSmall], utf8("abc")));
  const trailingZeros = blob(concat(blobs[xzSmall], zeros(7)));
  const xzThenAlone = blob(concat(blobs[xzText], blobs[alone]));
  const aloneThenXz = blob(concat(blobs[alone], blobs[xzText]));

  // Stream flags with a reserved check ID of the same size (liblzma skips the check) and an
  // invalid one; the header and footer CRCs are recomputed so only the ID differs.
  const withCheckId = (id: number): Uint8Array => {
    const bytes = new Uint8Array(blobs[xzSmallCrc32]);
    const view = new DataView(bytes.buffer);
    const end = bytes.length;
    bytes[7] = id;
    view.setUint32(8, crc32(bytes.subarray(6, 8)), true);
    bytes[end - 3] = id;
    view.setUint32(end - 12, crc32(bytes.subarray(end - 8, end - 2)), true);
    return bytes;
  };
  const reservedCheck = blob(withCheckId(0x02));
  const invalidCheck = blob(withCheckId(0x10));

  const xzFile = (name: string, input: Input): Case => ({
    files: [[`${ROOT}/${name}.wpilogxz`, input]],
    request: start(`${ROOT}/${name}.wpilogxz`)
  });

  // Plain files, the path file and recent documents.
  await runCase({ files: [[`${ROOT}/a.wpilog`, { blob: plainBlob }]], request: start(`${ROOT}/a.wpilog`) });
  await runCase({ files: [[`${ROOT}/a.rlog`, { bytes: utf8("") }]], request: start(`${ROOT}/a.rlog`) });
  await runCase({ request: start(`${ROOT}/missing.wpilog`) });
  await runCase({
    files: [[`${ROOT}/locked.wpilog`, { bytes: text }]],
    failures: [[`${ROOT}/locked.wpilog`, "read"]],
    request: start(`${ROOT}/locked.wpilog`)
  });
  await runCase({
    files: [[`${ROOT}/dir.csv`, { bytes: text }]],
    failures: [[`${ROOT}/dir.csv`, "readOpened"]],
    request: start(`${ROOT}/dir.csv`)
  });
  await runCase({ files: [[`${ROOT}/café \u{1f600}.csv`, { bytes: text }]], request: start(`${ROOT}/café \u{1f600}.csv`) });
  await runCase({ files: [[`${ROOT}/lone\ud800.csv`, { bytes: text }]], request: start(`${ROOT}/lone\ud800.csv`) });
  await runCase({ files: [[`${ROOT}/nul\u0000.csv`, { bytes: text }]], request: start(`${ROOT}/nul\u0000.csv`) });
  await runCase({ files: [[`${ROOT}/nul\u0000.wpilogxz`, { bytes: text }]], request: start(`${ROOT}/nul\u0000.wpilogxz`) });
  await runCase({ files: [[`${ROOT}/nul\u0000.dslog`, { bytes: text }]], request: start(`${ROOT}/nul\u0000.dslog`) });
  await runCase({ files: [[`${ROOT}/noext`, { bytes: text }]], request: start(`${ROOT}/noext`) });
  await runCase({ files: [[`${ROOT}/UPPER.DSLOG`, { bytes: text }]], request: start(`${ROOT}/UPPER.DSLOG`) });
  await runCase({
    files: [[`${ROOT}/a.wpilog`, { bytes: text }]],
    missingDirs: [TEMP],
    request: start(`${ROOT}/a.wpilog`)
  });
  await runCase({
    files: [[`${ROOT}/a.wpilog`, { bytes: text }]],
    failures: [[`${TEMP}/akit-log-path.txt`, "write"]],
    request: start(`${ROOT}/a.wpilog`)
  });

  // DSLog pairing.
  const dslog = utf8("dslog bytes");
  const dsevents = utf8("dsevents bytes");
  await runCase({
    files: [
      [`${ROOT}/m.dslog`, { bytes: dslog }],
      [`${ROOT}/m.dsevents`, { bytes: dsevents }]
    ],
    request: start(`${ROOT}/m.dslog`)
  });
  await runCase({ files: [[`${ROOT}/m.dslog`, { bytes: dslog }]], request: start(`${ROOT}/m.dslog`) });
  await runCase({ files: [[`${ROOT}/m.dsevents`, { bytes: dsevents }]], request: start(`${ROOT}/m.dslog`) });
  await runCase({ request: start(`${ROOT}/m.dslog`) });
  await runCase({
    files: [
      [`${ROOT}/m.dslog`, { bytes: dslog }],
      [`${ROOT}/m.dsevents`, { bytes: dsevents }]
    ],
    failures: [[`${ROOT}/m.dsevents`, "readOpened"]],
    request: start(`${ROOT}/m.dslog`)
  });
  await runCase({ files: [[`${ROOT}/m.dsevents`, { bytes: dsevents }]], request: start(`${ROOT}/m.dsevents`) });
  await runCase({ files: [[`${ROOT}/.dslog`, { bytes: dslog }], [`${ROOT}/.dsevents`, { bytes: dsevents }]], request: start(`${ROOT}/.dslog`) });

  // wpilogxz: whole files.
  for (const id of [xzSmall, xzSmallCrc32, xzSmallNone, xzSmallSha, xzLarge, xzLargeCrc64, xzEmpty, xzText, xzMulti]) {
    await runCase(xzFile(`whole-${id}`, { blob: id }));
  }
  for (const id of [alone, aloneLarge, concatenated, padded, oddPadded, trailingGarbage, trailingZeros, xzThenAlone, aloneThenXz, reservedCheck, invalidCheck]) {
    await runCase(xzFile(`whole-${id}`, { blob: id }));
  }
  await runCase(xzFile("empty", { bytes: new Uint8Array(0) }));
  await runCase(xzFile("zeros", { bytes: zeros(16) }));
  await runCase(xzFile("garbage", { bytes: garbage }));
  await runCase(xzFile("magic-garbage", { bytes: xzMagicGarbage }));
  await runCase(xzFile("plain", { blob: plainBlob }));
  await runCase({ request: start(`${ROOT}/missing.wpilogxz`) });
  await runCase({
    files: [[`${ROOT}/locked.wpilogxz`, { blob: xzSmall }]],
    failures: [[`${ROOT}/locked.wpilogxz`, "read"]],
    request: start(`${ROOT}/locked.wpilogxz`)
  });
  await runCase({
    files: [[`${ROOT}/dir.wpilogxz`, { blob: xzSmall }]],
    failures: [[`${ROOT}/dir.wpilogxz`, "readOpened"]],
    request: start(`${ROOT}/dir.wpilogxz`)
  });

  // wpilogxz: truncated at every length of the small file and of the two-stream file.
  for (const id of [xzSmall, concatenated]) {
    for (let len = 0; len < blobs[id].length; len++) await runCase(xzFile(`cut-${id}-${len}`, { blob: id, len }));
  }
  // Sampled truncations of the files whose output crosses lzma-native's 64 KiB buffers.
  for (const id of [xzLarge, xzMulti, aloneLarge, alone, padded]) {
    const size = blobs[id].length;
    const lengths = new Set<number>([1, 12, 13, 24, size - 1, size - 12, size - 13, size - 24]);
    while (lengths.size < 40) lengths.add(1 + below(size - 1));
    for (const len of [...lengths].filter((l) => l > 0 && l < size).sort((a, b) => a - b)) {
      await runCase(xzFile(`cut-${id}-${len}`, { blob: id, len }));
    }
  }
  // Corruption: one flipped byte, so the error surfaces mid-stream.
  for (const id of [xzSmall, xzLarge, xzMulti, alone, aloneLarge, concatenated]) {
    const size = blobs[id].length;
    for (let i = 0; i < 24; i++) {
      const offset = below(size);
      await runCase(xzFile(`flip-${id}-${i}`, { blob: id, flips: [[offset, 1 << below(8)]] }));
    }
  }

  // historical-start-raw: child logs through a temporary file.
  await runCase({ request: raw({ blob: plainBlob }, "wpilog") });
  await runCase({ request: raw({ bytes: text }, "csv") });
  await runCase({ request: raw({ bytes: new Uint8Array(0) }, "rlog") });
  await runCase({ request: raw({ bytes: dslog }, "dslog") });
  await runCase({ request: raw({ blob: xzSmall }, "wpilogxz") });
  await runCase({ request: raw({ blob: xzLarge, len: 9000 }, "wpilogxz") });
  await runCase({ request: raw({ bytes: text }, "") });
  await runCase({ request: raw({ bytes: text }, "a.b") });
  await runCase({ request: raw({ bytes: text }, "é\ud800") });
  for (const extension of ["a/b", "a\\b", "..", "x..y", "../wpilog", "/", "\\"]) {
    await runCase({ request: raw({ bytes: text }, extension) });
  }
  await runCase({ request: raw({ bytes: text }, "wpi\u0000log") });
  await runCase({ missingDirs: [TEMP], request: raw({ bytes: text }, "wpilog") });
}
