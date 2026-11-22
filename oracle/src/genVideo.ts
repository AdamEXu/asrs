import "./environment";
import { toTree } from "./cve1";
import { buildBundles } from "./video/bundle";
import { Case, Op, Sample, flowCases, loadSamples, local, youtube, ytRun } from "./video/cases";
import { Platform, ProcScript, World } from "./video/world";

// Ground truth for asrs-services' video processor: upstream's VideoProcessor.ts and
// getElectronPlatform.ts, unmodified, against the scripted world in video/world.ts.
//
//   genVideo platform
//     getElectronPlatform() over process.platform x process.arch.
//   genVideo video-id <count> <seed>
//     VideoProcessor.getVideoId on hand-picked and generated URLs.
//   genVideo flows
//     The hand-written prepare/cleanup scenarios in video/cases.ts.
//   genVideo stderr <count> <seed>
//     Local-file conversions replaying the recorded ffmpeg stderr in video/samples, re-chunked
//     and mutated.
//   genVideo youtube <count> <seed>
//     YouTube conversions over generated yt-dlp output.
//
// Flow records: a header {case, name, config} per case, then one record per op or delivered
// event, each with the effects upstream made during it (see world.ts).

const [mode, countArg, seedArg] = process.argv.slice(2);
const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const settle = async () => {
  for (let i = 0; i < 40; i++) await new Promise((resolve) => setImmediate(resolve));
};

let failure: unknown = null;
process.on("unhandledRejection", (reason) => {
  failure = reason;
});

const realPlatform = process.platform;
const realArch = process.arch;
function setProcess(platform: string, arch: string) {
  Object.defineProperty(process, "platform", { value: platform, configurable: true });
  Object.defineProperty(process, "arch", { value: arch, configurable: true });
}

const bundles = await buildBundles();
let importCount = 0;

async function load(world: World) {
  (globalThis as any).__asrsVideo = world.glue;
  setProcess(world.script.platform, world.script.arch);
  return await import(bundles[world.script.platform] + "?instance=" + importCount++);
}

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const MAX_STEPS = 20000;

async function runCase(index: number, c: Case) {
  const world = new World(c.script);
  const mod = await load(world);
  const VP = mod.VideoProcessor;
  VP.readTimerText = (p: string, w: number, h: number) => world.readTimerText(p, w, h);
  const ytName = c.script.platform === "win32" ? "yt-dlp.exe" : c.script.platform === "darwin" ? "yt-dlp_macos" : "yt-dlp";
  const ytDlpBinary = world.path.join(world.paths.dirname, "..", "..", "app.asar.unpacked", "node_modules", "youtube-dl-exec", "bin", ytName);
  const staticEffects = world.takeEffects();
  if (staticEffects.length > 0) throw new Error(`${c.name}: effects while loading`);
  out({
    case: index,
    name: c.name,
    config: {
      platform: c.script.platform,
      arch: c.script.arch,
      userData: world.paths.userData,
      temp: world.paths.temp,
      ytDlpBinary,
      windowIcon: toTree(mod.Constants.WINDOW_ICON)
    }
  });
  let step = 0;
  const check = () => {
    if (failure !== null) throw new Error(`${c.name}: unhandled rejection ${String(failure)}`);
  };
  const deliver = async (limit: number) => {
    for (let n = 0; n < limit; n++) {
      const event = world.deliverOne();
      if (event === null) return;
      await settle();
      check();
      out({ case: index, step: step++, event, effects: world.takeEffects() });
      if (step > MAX_STEPS) throw new Error(`${c.name}: runaway`);
    }
  };
  const ops: Op[] = c.ops;
  for (const op of ops) {
    if ("prepare" in op) {
      const p = op.prepare;
      VP.prepare(world.window, p.uuid, p.source, p.matchInfo ?? null, p.menuCoordinates ?? null, (data: unknown) =>
        world.effect({ k: "data", v: toTree(data) })
      );
      await settle();
      check();
      out({
        case: index,
        step: step++,
        op: "prepare",
        uuid: p.uuid,
        source: toTree(p.source),
        matchInfo: toTree(p.matchInfo ?? null),
        menuCoordinates: toTree(p.menuCoordinates ?? null),
        effects: world.takeEffects()
      });
    } else if ("cleanup" in op) {
      VP.cleanup();
      await settle();
      check();
      out({ case: index, step: step++, op: "cleanup", effects: world.takeEffects() });
    } else {
      await deliver(op.deliver);
    }
  }
  await deliver(MAX_STEPS);
  if (world.pending() !== 0) throw new Error(`${c.name}: events left`);
}

async function runCases(cases: Case[]) {
  const names = new Set<string>();
  for (const [i, c] of cases.entries()) {
    if (names.has(c.name)) throw new Error("duplicate case " + c.name);
    names.add(c.name);
    await runCase(i, c);
  }
}

function rechunk(text: string, rng: () => number): string[] {
  const chunks: string[] = [];
  let rest = text;
  while (rest.length > 0) {
    const take = rng() < 0.3 ? rest.length : 1 + Math.floor(rng() * Math.min(rest.length, rng() < 0.5 ? 40 : 400));
    chunks.push(rest.slice(0, take));
    rest = rest.slice(take);
  }
  return chunks;
}

const MUTATIONS: ((text: string, rng: () => number) => string)[] = [
  (t) => t,
  (t) => t.replace(/ (\d+(\.\d+)?|1k) fps, /g, (m) => m.replace(/\d/, "x")),
  (t) => t.replace(/(\d{2,5})x(\d{2,5})/, "$1 x $2"),
  (t) => t.replace(/Duration: [^,]*/, "Duration: N/A"),
  (t) => t.replace(/Duration: \d\d/, "Duration: 1"),
  (t) => t.replace(/frame=\s*(\d+)/g, (_m, n) => "frame=" + n),
  (t) => t.replace(/frame=/, "\rframe="),
  (t) => t.replace(/ fps, /, " fps,  , 12x34 "),
  (t, rng) => t.slice(0, Math.floor(rng() * t.length)),
  (t) => t + "av_interleaved_write_frame(): No space left on device\n",
  (t) => t.replace(/(\d+) fps, /g, "$1.5 fps, "),
  (t) => t.replace(/Stream #0:0/g, "Stream #0:0, 7x8")
];

function stderrCases(samples: Sample[], count: number, seed: number): Case[] {
  const rng = mulberry32(seed);
  const usable = samples.filter((s) => s.chunks.join("").includes("Stream"));
  const cases: Case[] = [];
  for (let i = 0; i < count; i++) {
    const sample = usable[Math.floor(rng() * usable.length)];
    const mutation = Math.floor(rng() * MUTATIONS.length);
    const text = MUTATIONS[mutation](sample.chunks.join(""), rng);
    const chunks = rng() < 0.25 ? sample.chunks.map((c) => MUTATIONS[mutation](c, rng)) : rechunk(text, rng);
    const exitRoll = rng();
    const exit = exitRoll < 0.6 ? sample.exitCode : exitRoll < 0.85 ? 1 : exitRoll < 0.95 ? 0 : 69;
    const platform: Platform = (["darwin", "linux", "win32"] as Platform[])[Math.floor(rng() * 3)];
    const main: ProcScript = { kind: "run", events: chunks.map((c) => ["stderr", c] as ["stderr", string]), exit };
    const retry = (n: number): ProcScript => (n === 0 ? main : { kind: "run", events: rechunk(text, rng).map((c) => ["stderr", c] as ["stderr", string]), exit: 0 });
    cases.push(
      local(`stderr-${i}-${sample.name}-m${mutation}`, platform, "x64", Math.floor(rng() * 1e9), retry, {
        timerText: (frame) => (frame % 7 === 0 ? "04" : frame % 7 === 1 ? "03" : "")
      })
    );
  }
  return cases;
}

const VCODECS: unknown[] = ["avc1.640028", "vp9", "none", "", null, undefined, 0, "av01.0.08M.08", "None"];
const HEIGHTS: unknown[] = [144, 240, 360, 480, 720, 1080, 1440, 2160, 1081, null, undefined, "720", "1080", "abc", 0, -1, 1080.5, true];
const URLS: unknown[] = ["https://rr3.example.invalid/videoplayback?itag=", undefined, null, 7, "", "https://cdn.example.invalid/a b.mp4?id="];
const HEADERS: unknown[] = [
  { "User-Agent": "Mozilla/5.0 (compatible)", Accept: "*/*" },
  undefined,
  null,
  {},
  { "User-Agent": "yt", "Accept-Language": "en-us,en;q=0.5", Cookie: "a=b; c=d" },
  "plain",
  ["Accept: x"],
  { n: 5, b: false, o: { x: 1 } }
];

function youtubeCases(count: number, seed: number): Case[] {
  const rng = mulberry32(seed);
  const pick = <T>(list: T[]) => list[Math.floor(rng() * list.length)];
  const cases: Case[] = [];
  for (let i = 0; i < count; i++) {
    const n = Math.floor(rng() * 7);
    const formats: any[] = [];
    for (let j = 0; j < n; j++) {
      const f: any = { format_id: String(j) };
      const vcodec = pick(VCODECS);
      if (vcodec !== undefined) f.vcodec = vcodec;
      const height = pick(HEIGHTS);
      if (height !== undefined) f.height = height;
      const url = pick(URLS);
      if (url !== undefined) f.url = typeof url === "string" && url.endsWith("=") ? url + j : url;
      formats.push(f);
    }
    const doc: any = { id: "x", formats };
    const headers = pick(HEADERS);
    if (headers !== undefined) doc.http_headers = headers;
    if (rng() < 0.08) delete doc.formats;
    const platform: Platform = (["darwin", "linux", "win32"] as Platform[])[Math.floor(rng() * 3)];
    const ids = ["dQw4w9WgXcQ", "abcdefghijk", "A-_09zZaQ12"];
    const clip = pick([
      `https://www.youtube.com/watch?v=${pick(ids)}`,
      `https://youtu.be/${pick(ids)}`,
      `https://m.youtube.com/watch?app=desktop&v=${pick(ids)}#t=1`,
      `https://www.youtube.com/shorts/${pick(ids)}`,
      `https://www.youtube.com/v/${pick(ids)}?version=3`
    ]);
    cases.push(youtube(`youtube-${i}`, platform, Math.floor(rng() * 1e9), clip, ytRun(JSON.stringify(doc))));
  }
  return cases;
}

const VIDEO_ID_URLS = [
  "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
  "https://youtube.com/watch?v=dQw4w9WgXcQ&t=10s",
  "https://youtu.be/dQw4w9WgXcQ",
  "youtu.be/dQw4w9WgXcQ",
  "https://www.youtube.com/embed/dQw4w9WgXcQ",
  "https://www.youtube.com/e/dQw4w9WgXcQ",
  "https://www.youtube.com/v/dQw4w9WgXcQ",
  "https://www.youtube.com/shorts/dQw4w9WgXcQ",
  "https://www.youtube.com/user/someone/a/dQw4w9WgXcQ",
  "https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
  "https://www.youtube.com/watch?v=dQw4w9WgXc",
  "https://www.youtube.com/watch?v=dQw4w9WgXcQQQ",
  "https://www.youtube.com/watch?v=dQw4 9WgXcQ",
  "https://www.youtube.com/watch?v=dQw4 9WgXcQ",
  "https://www.youtube.com/watch?v=dQw4 9WgXcQ",
  'https://www.youtube.com/watch?v=dQw4"9WgXcQ',
  "https://www.youtube.com/watch?v=😀😀😀😀😀x",
  "https://www.youtube.com/watch?v=\ud800bcdefghijk",
  "https://www.youtube.com/watch?a=1\n&v=dQw4w9WgXcQ",
  "https://www.youtube.com/watch\n?v=dQw4w9WgXcQ",
  "https://www.youtube.com/channel/x/\n/dQw4w9WgXcQ",
  "YOUTUBE.COM/watch?v=dQw4w9WgXcQ",
  "https://youtu.be/",
  "",
  "https://www.youtube.com/watch?v=dQw4w9WgXcQ https://youtu.be/abcdefghijk",
  "https://youtu.be/abcdefghijk https://www.youtube.com/watch?v=dQw4w9WgXcQ",
  "https://youtube.com/embedd/dQw4w9WgXcQ",
  "https://youtube.com/embe/dQw4w9WgXcQ",
  "https://youtube.com/vv/dQw4w9WgXcQ",
  "https://youtube.com/?&v=dQw4w9WgXcQ",
  "https://youtube.com/??v=dQw4w9WgXcQ",
  "https://youtube.com/a//b/dQw4w9WgXcQ",
  "https://youtube.com/a/b/c/d/dQw4w9WgXcQ",
  "https://youtube.com//b/dQw4w9WgXcQ"
];
const ID_PIECES = [
  "youtube.com/", "youtu.be/", "youtube.com/watch?v=", "youtube.com/embed/", "youtube.com/v/", "youtube.com/e/", "/", "?", "&", "v=",
  "dQw4w9WgXcQ", "abcdefghijk", "a", "b/", "x?y", "\"", " ", "\t", "\n", "　", "é", "😀", "embed", "e", "v", "=", "watch"
];

async function videoIds(count: number, seed: number) {
  const world = new World({ platform: "darwin", arch: "arm64", seed: 1 });
  const VP = (await load(world)).VideoProcessor;
  const rng = mulberry32(seed);
  const urls = [...VIDEO_ID_URLS];
  for (let i = 0; i < count; i++) {
    let url = "";
    const parts = 1 + Math.floor(rng() * 8);
    for (let j = 0; j < parts; j++) url += ID_PIECES[Math.floor(rng() * ID_PIECES.length)];
    urls.push(url);
  }
  for (const url of urls) out({ url, result: toTree(VP.getVideoId(url)) });
  if (world.takeEffects().length > 0) throw new Error("getVideoId made effects");
}

async function platforms() {
  const world = new World({ platform: "darwin", arch: "arm64", seed: 1 });
  const fn = (await load(world)).getElectronPlatform;
  for (const platform of ["darwin", "linux", "win32", "freebsd", "openbsd", "sunos", "aix", "android", "cygwin", "netbsd", ""]) {
    for (const arch of ["x64", "arm64", "arm", "ia32", "ppc64", "s390x", "mips", "mipsel", "riscv64", "loong64", ""]) {
      setProcess(platform, arch);
      out({ platform, arch, result: fn() });
    }
  }
}

if (mode === "platform") await platforms();
else if (mode === "video-id") await videoIds(Number(countArg), Number(seedArg));
else if (mode === "flows") await runCases(flowCases(await loadSamples()));
else if (mode === "stderr") await runCases(stderrCases(await loadSamples(), Number(countArg), Number(seedArg)));
else if (mode === "youtube") await runCases(youtubeCases(Number(countArg), Number(seedArg)));
else throw new Error("usage: genVideo platform | video-id <count> <seed> | flows | stderr <count> <seed> | youtube <count> <seed>");
setProcess(realPlatform, realArch);
