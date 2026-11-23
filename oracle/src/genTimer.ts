import "./environment";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { Worker } from "node:worker_threads";
import { toTree } from "./cve1";
import { buildBundles } from "./video/bundle";
import { World } from "./video/world";

// Ground truth for asrs-ocr: upstream's VideoProcessor.ts and the real tesseract.js, over the JPEG
// frames ffmpeg wrote for the clips oracle/tools/timerClips.mjs rendered.
//
//   genTimer clip <file.clip>
//     One Local conversion. The recorded ffmpeg stderr is delivered chunk by chunk, each after
//     every OCR job the previous one started has settled, so the last status carries the
//     autoEndFrame the search ends on. tesseract's results are delivered in the order the jobs
//     were added (eight workers race otherwise, and the record order would not be reproducible).
//   genTimer rects <file.clip>...
//     TIMER_RECTS for single frames at other video dimensions, rectangles upstream cannot produce,
//     and damaged images: each read by one worker that keeps its history, and by a fresh one.
//     The two differ only for rectangles outside the image, where the core reads freed memory.

const [mode, ...args] = process.argv.slice(2);
const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

declare const __UPSTREAM_DIR__: string;
const upstreamDir = __UPSTREAM_DIR__;
const requireUpstream = createRequire(path.join(upstreamDir, "package.json"));
const Tesseract = requireUpstream("tesseract.js");

// Node's getCore compares the boolean `lstmOnly` with OEM numbers, so it loads the core with the
// legacy engine; the file compared is the one it loads for the options createWorker sends.
async function checkVendoredData() {
  let lstmOnly: unknown;
  const postMessage = Worker.prototype.postMessage;
  Worker.prototype.postMessage = function (packet: any, ...rest: any[]) {
    if (packet.action === "load") lstmOnly = packet.payload.options.lstmOnly;
    return postMessage.call(this, packet, ...rest);
  };
  const worker = await Tesseract.createWorker("eng", undefined, LANG_OPTIONS);
  await worker.terminate();
  Worker.prototype.postMessage = postMessage;
  if (lstmOnly === undefined) throw new Error("createWorker sent no load options");
  await requireUpstream("tesseract.js/src/worker-script/node/getCore")(lstmOnly, undefined, { progress() {} });
  const cores = Object.keys(requireUpstream.cache).filter((file) => path.basename(path.dirname(file)) === "tesseract.js-core");
  if (cores.length !== 1) throw new Error(`one core expected: ${cores}`);
  const core = path.basename(cores[0], ".js") + ".wasm";
  const pairs = [
    [path.join(path.dirname(cores[0]), core), core],
    [path.join(upstreamDir, "eng.traineddata.gz"), "eng.traineddata.gz"]
  ];
  for (const [upstream, vendored] of pairs) {
    if (!fs.readFileSync(upstream).equals(fs.readFileSync(path.join("crates/asrs-ocr/data", vendored)))) {
      throw new Error(`crates/asrs-ocr/data/${vendored} is not upstream's`);
    }
  }
}

interface Clip {
  name: string;
  stderr: string[];
  exit: number;
  frames: Map<number, Buffer>;
}

function readClip(file: string): Clip {
  const data = fs.readFileSync(file);
  const headerEnd = 4 + data.readUInt32LE(0);
  const header = JSON.parse(data.toString("utf8", 4, headerEnd));
  const frames = new Map<number, Buffer>();
  let at = headerEnd;
  for (const [frame, length] of header.frames) {
    frames.set(frame, data.subarray(at, at + length));
    at += length;
  }
  if (at !== data.length) throw new Error(`${file}: trailing bytes`);
  return { name: header.name, stderr: header.stderr, exit: header.exit, frames };
}

const LANG_OPTIONS = { langPath: upstreamDir, cacheMethod: "readOnly", gzip: true };

const settle = async () => {
  for (let i = 0; i < 40; i++) await new Promise((resolve) => setImmediate(resolve));
};

const RANDOM = Array.from({ length: 32 }, (_, i) => (i * 7 + 0.5) / 224);

async function loadVideoProcessor(world: World) {
  const bundles = await buildBundles();
  (globalThis as any).__asrsVideo = world.glue;
  Object.defineProperty(process, "platform", { value: "darwin", configurable: true });
  Object.defineProperty(process, "arch", { value: "arm64", configurable: true });
  return (await import(bundles.darwin)).VideoProcessor;
}

async function runClip(clip: Clip) {
  const world = new World({
    platform: "darwin",
    arch: "arm64",
    seed: 1,
    random: RANDOM,
    files: { "/Users/frc/Library/Application Support/AdvantageScope/ffmpeg": "binary" },
    openDialog: () => ({ canceled: false, filePaths: ["/Users/frc/Movies/match.mp4"] }),
    process: () => ({ kind: "run", events: clip.stderr.map((chunk) => ["stderr", chunk] as ["stderr", string]), exit: clip.exit })
  });
  const lines: object[] = [];

  const frameOf = new Map<Buffer, number>();
  world.glue.fs.promises.readFile = async (p: string) => {
    const frame = Number(p.slice(-12, -4));
    const data = clip.frames.get(frame);
    if (data === undefined) throw new Error(`${clip.name}: frame ${frame} is not in the clip`);
    const buffer = Buffer.from(data);
    frameOf.set(buffer, frame);
    return buffer;
  };

  const rectTexts = new Map<number, string[]>();
  const ready: Promise<unknown>[] = [];
  let lastDelivery: Promise<unknown> = Promise.resolve();
  world.glue.tesseract = {
    createScheduler() {
      const scheduler = Tesseract.createScheduler();
      return {
        addWorker: (worker: unknown) => scheduler.addWorker(worker),
        addJob(action: string, image: Buffer, options: unknown) {
          const result = scheduler.addJob(action, image, options);
          const delivered = lastDelivery.then(() => result);
          lastDelivery = delivered;
          return delivered.then((value: any) => {
            rectTexts.get(frameOf.get(image)!)!.push(value.data.text);
            return value;
          });
        }
      };
    },
    createWorker(langs: unknown, oem: unknown, options: any) {
      if (langs !== "eng" || oem !== undefined || JSON.stringify(Object.keys(options)) !== '["langPath","cacheMethod","gzip"]') {
        throw new Error("createWorker arguments changed");
      }
      const worker = Tesseract.createWorker(langs, oem, { ...options, langPath: upstreamDir });
      ready.push(worker);
      return worker;
    }
  };

  const VP = await loadVideoProcessor(world);
  await Promise.all(ready);
  await settle();

  const readTimerText = VP.readTimerText;
  let reads = 0;
  let pending = 0;
  VP.readTimerText = async (p: string, width: number, height: number) => {
    const id = reads++;
    const frame = Number(p.slice(-12, -4));
    if (rectTexts.has(frame)) throw new Error(`${clip.name}: frame ${frame} is already being read`);
    rectTexts.set(frame, []);
    lines.push({ k: "read", id, frame, width: toTree(width), height: toTree(height) });
    pending++;
    const text = await readTimerText.call(VP, p, width, height);
    pending--;
    lines.push({ k: "text", id, rects: rectTexts.get(frame), text });
    rectTexts.delete(frame);
    return text;
  };

  out({ k: "clip", name: clip.name, random: toTree(RANDOM) });
  VP.prepare(world.window, "timer", 0, null, null, (data: unknown) => lines.push({ k: "data", v: toTree(data) }));
  await settle();
  while (true) {
    const at = lines.length;
    const event = world.deliverOne();
    if (event === null) break;
    do await settle();
    while (pending > 0 && failure === null);
    lines.splice(at, 0, { k: event.e, ...("data" in event ? { data: event.data } : {}), ...("code" in event ? { code: event.code } : {}) });
    if (failure !== null) throw failure;
  }
  for (const line of lines) out(line);
}

let failure: unknown = null;
process.on("unhandledRejection", (reason) => {
  failure = reason;
});

async function recognize(worker: any, image: Buffer, rectangle: object) {
  try {
    return { text: (await worker.recognize(image, { rectangle })).data.text as string };
  } catch (error) {
    return { error: String(error) };
  }
}

interface Edit {
  keep?: number;
  zeros?: number;
  insert?: string;
}
const NO_EDIT: Edit = {};
// Each aborted job leaks some of the core's 64 KiB stack; this many overflow it.
const ABORTS = 80;

function applyEdit(image: Buffer, edit: Edit): Buffer {
  if (edit.zeros !== undefined) return Buffer.alloc(edit.zeros);
  if (edit.insert !== undefined) return Buffer.concat([image.subarray(0, 2), Buffer.from(edit.insert, "hex"), image.subarray(2)]);
  return image.subarray(0, edit.keep ?? image.length);
}

function exifSegment(orientation: number): string {
  const tiff = Buffer.from([0x4d, 0x4d, 0, 42, 0, 0, 0, 8, 0, 1, 1, 18, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0, 0, 0, 0, 0]);
  const body = Buffer.concat([Buffer.from("Exif\0\0", "latin1"), tiff]);
  return Buffer.concat([Buffer.from([0xff, 0xe1, 0, body.length + 2]), body]).toString("hex");
}

async function runRects(clips: Clip[]) {
  // tesseract.js rejects a failed job and also throws its message from its message handler; the
  // rejection is only seen once something else wakes the event loop.
  process.on("uncaughtException", (error) => {
    if (!/^(Error|RuntimeError|RangeError|TypeError): /.test(error.message)) {
      console.error(error);
      process.exit(1);
    }
  });
  setInterval(() => {}, 10);
  const VP = await loadVideoProcessor(new World({ platform: "darwin", arch: "arm64", seed: 1 }));
  const rects: { left: number; top: number; width: number; height: number }[] = VP.TIMER_RECTS;
  let worker: any = null;
  let workerIndex = -1;
  const nextWorker = async () => {
    await worker?.terminate();
    worker = await Tesseract.createWorker("eng", undefined, LANG_OPTIONS);
    workerIndex++;
  };

  const run = async (clip: Clip, frame: number, edit: Edit, rectangle: { [key: string]: number }) => {
    const image = applyEdit(clip.frames.get(frame)!, edit);
    const result = await recognize(worker, image, rectangle);
    const freshWorker = await Tesseract.createWorker("eng", undefined, LANG_OPTIONS);
    const fresh = await recognize(freshWorker, image, rectangle);
    await freshWorker.terminate();
    const { left, top, width, height } = rectangle;
    out({ k: "rect", worker: workerIndex, clip: clip.name, frame, edit, rectangle: toTree([left, top, width, height]), result, fresh });
  };
  const scaled = (rect: (typeof rects)[number], w: number, h: number) => ({
    left: rect.left * w,
    top: rect.top * h,
    width: rect.width * w,
    height: rect.height * h
  });

  out({ k: "timerRects", v: toTree(rects.map((r) => [r.left, r.top, r.width, r.height])) });
  for (const clip of clips) {
    const numbers = [...clip.frames.keys()];
    const frame = numbers[numbers.length >> 1];
    await nextWorker();
    for (const [w, h] of [[1920, 1080], [1280, 720], [3840, 2160], [1440, 1080], [1000, 563]]) {
      for (const rect of rects) await run(clip, frame, NO_EDIT, scaled(rect, w, h));
    }
  }

  const clip = clips[0];
  const frame = [...clip.frames.keys()][10];
  const image = clip.frames.get(frame)!;
  const timer = scaled(rects[4], 1920, 1080);
  const odd = [
    [865, 980, 0, 0],
    [865, 980, 1, 1],
    [865, 980, 2, 40],
    [865.9, 980.9, 199.9, 89.9],
    [-50, -50, 400, 300],
    [1900, 1060, 200, 200],
    [NaN, NaN, NaN, NaN],
    [Infinity, 0, 100, 100],
    [4294967296 + 865, 980, 200, 90],
    [0, 890, 1920, 190]
  ];
  for (const [left, top, width, height] of odd) {
    await nextWorker();
    await run(clip, frame, NO_EDIT, { left, top, width, height });
    await run(clip, frame, NO_EDIT, timer);
  }
  await nextWorker();
  const edits: Edit[] = [
    ...[1, 3, 6, 9, 0].map((orientation) => ({ insert: exifSegment(orientation) })),
    { keep: image.length >> 1 },
    { keep: image.length - 2 },
    { keep: 300 },
    { keep: 4 },
    { keep: 0 },
    { zeros: 1000 }
  ];
  for (const edit of edits) {
    await run(clip, frame, edit, timer);
    await run(clip, frame, NO_EDIT, timer);
  }
  await nextWorker();
  for (let i = 0; i < ABORTS; i++) await run(clip, frame, { keep: 300 }, timer);
  await run(clip, frame, NO_EDIT, timer);
  await worker.terminate();
}

await checkVendoredData();
if (mode === "clip" && args.length === 1) {
  await runClip(readClip(args[0]));
} else if (mode === "rects" && args.length > 0) {
  await runRects(args.map(readClip));
} else {
  throw new Error("usage: genTimer clip <file.clip> | rects <file.clip>...");
}
process.exit(0);
