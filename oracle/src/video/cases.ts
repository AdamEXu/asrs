import { GENUINE_PREFIX, PLATFORM_PATHS, Platform, ProcScript, Script, pathFor } from "./world";

export type Op =
  | { prepare: { uuid: string; source: unknown; matchInfo?: unknown; menuCoordinates?: unknown } }
  | { cleanup: true }
  | { deliver: number };

export interface Case {
  name: string;
  script: Script;
  ops: Op[];
}

export interface Sample {
  name: string;
  file: string;
  chunks: string[];
  exitCode: number;
}

export const SAMPLE_NAMES = [
  "h264-720p30-aac",
  "h264-long-progress",
  "h264-1080p2997",
  "vp9-360p25",
  "prores-480p60",
  "hevc-portrait-23976",
  "mpeg4-240p15",
  "h264-2160p-downscale",
  "h264-raw-no-duration",
  "mjpeg-1000fps",
  "audio-only",
  "missing-input"
];

export async function loadSamples(): Promise<Sample[]> {
  const out: Sample[] = [];
  for (const name of SAMPLE_NAMES) out.push((await import(`./samples/${name}.json`)).default);
  return out;
}

const UUID_A = "k3j4h5g6f7d8s9a0q1w2e3r4t5y6u7i8";
const UUID_B = "zz0099yy8877xx6655ww4433vv2211uu";
const SHA: { [platform: string]: string } = {
  "mac-x64": "cfe20936c83ecf5d68e424b87e8cc45b24dd6be81787810123bb964a0df686f9",
  "mac-arm64": "a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584",
  "win-x64": "e9fd5e711debab9d680955fc1e38a2c1160fd280b144476cc3f62bc43ef49db1",
  "win-arm64": "fb3766af5cc193ca863e15cd4554a33732973209dad5e3c1433b5e291bceb16c"
};
const genuine = (key: string) => Buffer.from(GENUINE_PREFIX + SHA[key], "latin1");

export const VERSION_OK: ProcScript = {
  kind: "run",
  events: [["stdout", "ffmpeg version 6.0 Copyright (c) 2000-2023 the FFmpeg developers\n"]],
  exit: 0
};

export function sampleRun(sample: Sample): ProcScript {
  return { kind: "run", events: sample.chunks.map((c) => ["stderr", c] as ["stderr", string]), exit: sample.exitCode };
}

export function userDataFfmpeg(platform: Platform) {
  return pathFor(platform).join(PLATFORM_PATHS[platform].userData, platform === "win32" ? "ffmpeg.exe" : "ffmpeg");
}

const prefsPath = (platform: Platform) => pathFor(platform).join(PLATFORM_PATHS[platform].userData, "prefs.json");

const isYtDlp = (cmd: string) => cmd.includes("yt-dlp");

function progressStream(opts: { fps: string; duration: string; width: number; height: number; frames: number[]; tail?: string }) {
  const events: ["stderr", string][] = [];
  events.push(["stderr", "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'match.mp4':\n"]);
  events.push(["stderr", `  Duration: ${opts.duration}, start: 0.000000, bitrate: 2400 kb/s\n`]);
  events.push([
    "stderr",
    `  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), ${opts.width}x${opts.height} [SAR 1:1 DAR 16:9], 2263 kb/s, ${opts.fps} fps, ${opts.fps} tbr, 90k tbn (default)\n`
  ]);
  for (const frame of opts.frames) {
    events.push(["stderr", `frame=${String(frame).padStart(5)} fps=212 q=2.0 size=N/A time=00:00:10.00 bitrate=N/A speed=7.1x    \r`]);
  }
  if (opts.tail) events.push(["stderr", opts.tail]);
  return events;
}

const localOps = (uuid = UUID_A): Op[] => [{ prepare: { uuid, source: 0, matchInfo: null, menuCoordinates: null } }];

function local(
  name: string,
  platform: Platform,
  arch: string,
  seed: number,
  main: ProcScript | ((index: number) => ProcScript),
  extra: Partial<Script> = {},
  ops: Op[] = localOps()
): Case {
  let mainIndex = 0;
  return {
    name,
    script: {
      platform,
      arch,
      seed,
      files: { [userDataFfmpeg(platform)]: "binary" },
      openDialog: () => ({ canceled: false, filePaths: [platform === "win32" ? "D:\\Videos\\Match 12.mp4" : "/Users/frc/Movies/Match 12.mp4"] }),
      process: (cmd, args) => {
        if (args[0] === "-version") return VERSION_OK;
        return typeof main === "function" ? main(mainIndex++) : main;
      },
      ...extra
    },
    ops
  };
}

const YT_JSON = JSON.stringify({
  id: "dQw4w9WgXcQ",
  title: "Qualification 12 - 2024 Silicon Valley Regional",
  formats: [
    { format_id: "18", vcodec: "avc1.42001E", height: 360, url: "https://rr1.example.invalid/videoplayback?itag=18" },
    { format_id: "140", vcodec: "none", acodec: "mp4a.40.2", url: "https://rr1.example.invalid/videoplayback?itag=140" },
    { format_id: "137", vcodec: "avc1.640028", height: 1080, url: "https://rr1.example.invalid/videoplayback?itag=137" },
    { format_id: "22", vcodec: "avc1.64001F", height: 720, url: "https://rr1.example.invalid/videoplayback?itag=22" },
    { format_id: "313", vcodec: "vp9", height: 2160, url: "https://rr1.example.invalid/videoplayback?itag=313" }
  ],
  http_headers: {
    "User-Agent": "com.google.android.youtube/19.09.37 (Linux; U; Android 11) gzip",
    Accept: "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    "Accept-Language": "en-us,en;q=0.5",
    "Sec-Fetch-Mode": "navigate"
  }
});

export function ytRun(stdout: string, exit = 0, stderr = ""): ProcScript {
  const events: ["stdout" | "stderr", string][] = [];
  if (stdout !== "") events.push(["stdout", stdout]);
  if (stderr !== "") events.push(["stderr", stderr]);
  return { kind: "run", events, exit };
}

function youtube(
  name: string,
  platform: Platform,
  seed: number,
  clipboard: string,
  yt: ProcScript | ((args: string[], index: number) => ProcScript),
  main: ProcScript = sampleFallback(),
  extra: Partial<Script> = {}
): Case {
  let ytIndex = 0;
  return {
    name,
    script: {
      platform,
      arch: "arm64",
      seed,
      clipboard,
      files: { [userDataFfmpeg(platform)]: "binary" },
      process: (cmd, args) => {
        if (args[0] === "-version") return VERSION_OK;
        if (isYtDlp(cmd)) return typeof yt === "function" ? yt(args, ytIndex++) : yt;
        return main;
      },
      ...extra
    },
    ops: [{ prepare: { uuid: UUID_A, source: 1, matchInfo: null, menuCoordinates: null } }]
  };
}

function sampleFallback(): ProcScript {
  return {
    kind: "run",
    events: progressStream({ fps: "30", duration: "00:02:40.12", width: 1280, height: 720, frames: [120, 480] }),
    exit: 0
  };
}

const MATCH_QUAL = { year: 2024, event: "CASJ", matchType: 2, matchNumber: 12 };
const MATCH_ELIM = { year: 2024, event: "casj", matchType: 3, matchNumber: 14 };
const EVENTS_2024 = JSON.stringify([
  { key: "2024cada", first_event_code: "CADA", name: "Sacramento Regional" },
  { key: "2024week0", first_event_code: null, name: "Week 0" },
  { key: "2024casj", first_event_code: "CASJ", name: "Silicon Valley Regional" }
]);

function tba(
  name: string,
  seed: number,
  opts: {
    prefs?: string | null;
    matchInfo?: unknown;
    menuCoordinates?: unknown;
    fetch?: Script["fetch"];
    yt?: (args: string[], index: number) => ProcScript;
    menu?: Script["menu"];
    platform?: Platform;
    extra?: Partial<Script>;
  }
): Case {
  const platform = opts.platform ?? "darwin";
  const files: { [p: string]: string } = { [userDataFfmpeg(platform)]: "binary" };
  if (opts.prefs !== null) files[prefsPath(platform)] = opts.prefs ?? JSON.stringify({ tbaApiKey: "tba-key-0123456789" });
  let ytIndex = 0;
  return {
    name,
    script: {
      platform,
      arch: "arm64",
      seed,
      files,
      fetch: opts.fetch,
      menu: opts.menu,
      process: (cmd, args) => {
        if (args[0] === "-version") return VERSION_OK;
        if (isYtDlp(cmd)) return opts.yt ? opts.yt(args, ytIndex++) : ytRun(YT_JSON);
        return sampleFallback();
      },
      ...opts.extra
    },
    ops: [
      {
        prepare: {
          uuid: UUID_A,
          source: 2,
          matchInfo: opts.matchInfo === undefined ? MATCH_QUAL : opts.matchInfo,
          menuCoordinates: opts.menuCoordinates === undefined ? [412.5, 230.49] : opts.menuCoordinates
        }
      }
    ]
  };
}

const tbaFetch =
  (videos: unknown, eventsBody = EVENTS_2024, playoff: unknown = 10): Script["fetch"] =>
  (url) => {
    if (url.includes("/events/")) return { status: 200, body: eventsBody };
    if (url.includes("/event/")) return { status: 200, body: JSON.stringify({ key: "2024casj", playoff_type: playoff }) };
    return { status: 200, body: JSON.stringify({ key: "2024casj_qm12", videos }) };
  };

const titleRun = (titles: (string | ProcScript)[]) => (args: string[], index: number) => {
  if (args.includes("--dump-single-json")) return ytRun(YT_JSON);
  const t = titles[index % titles.length];
  return typeof t === "string" ? ytRun(t + "\n") : t;
};

export function flowCases(samples: Sample[]): Case[] {
  const sample = (name: string) => samples.find((s) => s.name === name)!;
  const cases: Case[] = [];

  // FFmpeg location and download.
  cases.push(local("userdata-ffmpeg-darwin", "darwin", "arm64", 1, sampleRun(sample("h264-720p30-aac"))));
  cases.push({
    ...local("system-ffmpeg-darwin", "darwin", "arm64", 2, sampleRun(sample("vp9-360p25"))),
    script: { ...local("", "darwin", "arm64", 2, sampleRun(sample("vp9-360p25"))).script, files: {} }
  });
  cases.push(local("linux-ignores-userdata", "linux", "x64", 3, sampleRun(sample("prores-480p60"))));
  const downloadCase = (name: string, platform: Platform, arch: string, seed: number, version: ProcScript, extra: Partial<Script>) => {
    const base = local(name, platform, arch, seed, sampleRun(sample("mpeg4-240p15")));
    return {
      ...base,
      script: {
        ...base.script,
        files: {},
        process: (_cmd: string, args: string[]) => (args[0] === "-version" ? version : sampleRun(sample("mpeg4-240p15"))),
        ...extra
      }
    };
  };
  const enoent: ProcScript = { kind: "enoent" };
  const versionFails: ProcScript = { kind: "run", events: [["stderr", "dyld: Library not loaded\n"]], exit: 1 };
  cases.push(downloadCase("download-ok-mac-arm64", "darwin", "arm64", 4, enoent, { download: () => ({ ok: true, data: genuine("mac-arm64") }) }));
  cases.push(downloadCase("download-ok-mac-x64", "darwin", "x64", 5, versionFails, { download: () => ({ ok: true, data: genuine("mac-x64") }) }));
  cases.push(downloadCase("download-ok-win-x64", "win32", "x64", 6, enoent, { download: () => ({ ok: true, data: genuine("win-x64") }) }));
  cases.push(downloadCase("download-ok-win-arm64", "win32", "arm64", 7, enoent, { download: () => ({ ok: true, data: genuine("win-arm64") }) }));
  cases.push(downloadCase("download-win-ia32-unknown", "win32", "ia32", 8, enoent, {}));
  cases.push(downloadCase("download-declined", "darwin", "arm64", 9, enoent, { messageBox: () => 1 }));
  cases.push(downloadCase("download-failed", "darwin", "arm64", 10, versionFails, { download: () => ({ ok: false }) }));
  cases.push(
    downloadCase("download-hash-mismatch", "darwin", "arm64", 11, enoent, {
      download: () => ({ ok: true, data: Buffer.from("not really ffmpeg\n", "latin1") })
    })
  );
  cases.push(
    downloadCase("download-hash-wrong-platform", "win32", "x64", 12, enoent, { download: () => ({ ok: true, data: genuine("mac-x64") }) })
  );
  cases.push(
    downloadCase("download-rename-fails", "darwin", "arm64", 13, enoent, {
      download: () => ({ ok: true, data: genuine("mac-arm64") }),
      fail: [{ op: "rename", path: pathFor("darwin").join(PLATFORM_PATHS.darwin.userData, "ffmpeg-download"), code: "EPERM" }]
    })
  );
  cases.push(
    downloadCase("download-readfile-fails", "darwin", "arm64", 14, enoent, {
      download: () => ({ ok: true, data: genuine("mac-arm64") }),
      fail: [{ op: "readFile", path: pathFor("darwin").join(PLATFORM_PATHS.darwin.userData, "ffmpeg-download"), code: "EACCES" }]
    })
  );
  cases.push(
    downloadCase("download-chmod-fails", "darwin", "x64", 15, enoent, {
      download: () => ({ ok: true, data: genuine("mac-x64") }),
      fail: [{ op: "chmod", path: pathFor("darwin").join(PLATFORM_PATHS.darwin.userData, "ffmpeg"), code: "EPERM" }]
    })
  );
  cases.push(
    downloadCase("download-unlink-fails", "darwin", "arm64", 16, enoent, {
      download: () => ({ ok: true, data: Buffer.from("tampered", "latin1") }),
      fail: [{ op: "unlink", path: pathFor("darwin").join(PLATFORM_PATHS.darwin.userData, "ffmpeg-download"), code: "EBUSY" }]
    })
  );
  cases.push(downloadCase("linux-no-ffmpeg", "linux", "arm64", 17, enoent, {}));
  cases.push(downloadCase("linux-armv7l-no-ffmpeg", "linux", "arm", 18, versionFails, {}));

  // Local source.
  cases.push(local("local-canceled", "darwin", "arm64", 19, sampleRun(sample("h264-720p30-aac")), { openDialog: () => ({ canceled: true, filePaths: [] }) }));
  cases.push(local("local-no-paths", "win32", "x64", 20, sampleRun(sample("h264-720p30-aac")), { openDialog: () => ({ canceled: false, filePaths: [] }) }));
  for (const [i, s] of samples.entries()) {
    const platform: Platform = i % 3 === 0 ? "win32" : i % 3 === 1 ? "darwin" : "linux";
    cases.push(local("local-sample-" + s.name, platform, "x64", 100 + i, sampleRun(s)));
  }
  cases.push(
    local("local-exit-other-code", "darwin", "arm64", 21, {
      kind: "run",
      events: progressStream({ fps: "30", duration: "00:00:10.00", width: 1920, height: 1080, frames: [90] }),
      exit: 255
    })
  );
  const noSpace = (exit: number): ProcScript => ({
    kind: "run",
    events: progressStream({
      fps: "29.97",
      duration: "00:02:31.45",
      width: 1920,
      height: 1080,
      frames: [300],
      tail: "[image2 @ 0x7f8] Could not write header for output file #0: No space left on device\n"
    }),
    exit
  });
  cases.push(local("local-no-space-fallback", "darwin", "arm64", 22, (n) => (n === 0 ? noSpace(1) : sampleRun(sample("h264-1080p2997")))));
  cases.push(local("local-no-space-twice", "win32", "x64", 23, () => noSpace(1)));
  cases.push({
    ...local("local-no-space-version-again", "darwin", "arm64", 24, (n) => (n === 0 ? noSpace(1) : sampleRun(sample("vp9-360p25")))),
    script: {
      ...local("", "darwin", "arm64", 24, () => noSpace(1)).script,
      files: {},
      process: (() => {
        let n = 0;
        return (_cmd: string, args: string[]) => (args[0] === "-version" ? VERSION_OK : n++ === 0 ? noSpace(1) : sampleRun(sample("vp9-360p25")));
      })()
    }
  });
  cases.push(local("local-no-space-code-0", "darwin", "arm64", 25, noSpace(0)));
  cases.push(
    local("local-mkdir-fails", "darwin", "arm64", 26, sampleRun(sample("h264-720p30-aac")), {
      random: [0],
      fail: [{ op: "mkdir", path: pathFor("darwin").join(PLATFORM_PATHS.darwin.temp, "advantagescope-video-cache", "a".repeat(32)) + "/", code: "EACCES" }]
    })
  );
  cases.push(
    local("local-cache-collision", "linux", "x64", 27, sampleRun(sample("mpeg4-240p15")), {
      random: [0.5],
      files: {
        [pathFor("linux").join(PLATFORM_PATHS.linux.temp, "advantagescope-video-cache", "s".repeat(32), "00000001.jpg")]: "old frame"
      },
      process: (_cmd: string, args: string[]) => (args[0] === "-version" ? VERSION_OK : sampleRun(sample("mpeg4-240p15")))
    })
  );

  // Parse failures in hand-made streams.
  const bad = (name: string, seed: number, events: ["stderr", string][], exit = 0) =>
    cases.push(local("parse-" + name, "darwin", "arm64", seed, { kind: "run", events, exit }));
  bad("fps-nan", 30, [["stderr", "  Duration: 00:01:00.00, start: 0.0\n  Stream #0:0: Video: h264, yuv420p, 1280x720, 3O fps, 30 tbr\n"]]);
  bad("fps-empty", 31, [["stderr", "Stream #0:0: Video: h264, yuv420p, 1280x720,  fps, 30 tbr\n"]]);
  bad("no-dimensions", 32, [["stderr", "  Stream #0:0: Video: h264, yuv420p, 30 fps, 30 tbr\n"]]);
  bad("height-at-end", 33, [["stderr", "Video: h264, 30 fps, 1280x720"]]);
  bad("width-overflow", 34, [["stderr", "Video: h264, 99999999999999999999999x720 , 30 fps,\n"]]);
  bad("duration-nan", 35, [["stderr", "  Duration: N/A, start: 0.000000, bitrate: N/A\n"]]);
  bad("frame-before-info", 36, [["stderr", "frame=   12 fps=0.0 q=2.0 size=N/A time=00:00:00.40 bitrate=N/A speed=0.8x    \r"]]);
  bad("frame-count-nan", 37, [
    ...progressStream({ fps: "25", duration: "00:00:30.00", width: 640, height: 480, frames: [] }),
    ["stderr", "frame=   1a fps=0.0 q=2.0 size=N/A time=00:00:00.40 bitrate=N/A speed=0.8x    \r"]
  ]);
  bad("frame-no-fps-field", 38, [
    ...progressStream({ fps: "25", duration: "00:00:30.00", width: 640, height: 480, frames: [] }),
    ["stderr", "frame=  250 q=2.0 size=N/A\r"],
    ["stderr", "frame=  75 fps=\r"]
  ]);
  bad("all-in-one-chunk", 39, [
    [
      "stderr",
      "  Duration: 01:02:03.45, start: 0.000000, bitrate: 1 kb/s\n  Stream #0:0: Video: h264, yuv420p, 1920x1080 [SAR 1:1], 59.94 fps, 59.94 tbr\nOutput #0, image2:\n  Stream #0:0: Video: mjpeg, 1920x1080, q=2-31, 59.94 fps, 59.94 tbn\nframe=  100 fps=0.0\r"
    ],
    ["stderr", "frame=  4000 fps=50 q=2.0\r"],
    ["stderr", "frame=  9000 fps=51 q=2.0\r"],
    ["stderr", "error after fail\n"]
  ], 1);
  bad("fps-then-frame-in-one-chunk", 40, [
    ["stderr", "frame=   10 fps=0.0 Duration: 00:00:05.00, x Video: h264, 320x240, 10 fps, \r"],
    ["stderr", "frame=   50 fps=10\r"]
  ]);
  bad("duration-short-slice", 41, [["stderr", "Duration: 1:2:3"], ["stderr", "Video: h264, 640x360, 24 fps, 24 tbr\n"], ["stderr", "frame=  48 fps=24\r"]]);
  bad("duration-hex", 42, [["stderr", "Duration: 0x:00:10.00,"], ["stderr", "Video: h264, 640x360, 24 fps,\n"], ["stderr", "frame=  48 fps=24\r"]]);
  bad("fps-infinity", 43, [["stderr", "Duration: 00:00:10.00\nVideo: h264, 640x360, Infinity fps, 24 tbr\n"], ["stderr", "frame=  48 fps=24\r"]]);
  bad("fps-negative-zero", 44, [["stderr", "Duration: 00:00:10.00\nVideo: h264, 640x360, -0 fps,\n"], ["stderr", "frame=  48 fps=24\r"]]);
  bad("frame-zero-completed", 45, [["stderr", "Duration: 00:00:04.00\nVideo: h264, 640x360, 30 fps,\n"], ["stderr", "frame= fps=0\r"], ["stderr", "frame=0x10 fps=1\r"]]);
  bad("dimension-leading-zero", 46, [["stderr", "Duration: 00:00:04.00\nVideo: h264, 0640x0360, 30 fps,\n"], ["stderr", "frame=  30 fps=30\r"]]);
  bad("data-after-close-code-1", 47, [["stderr", "garbage\n"]], 1);

  // Timer search through scripted timer text.
  const timerCase = (name: string, seed: number, fps: string, frames: number[], text: (frame: number) => string, platform: Platform = "darwin") =>
    cases.push(
      local(
        "timer-" + name,
        platform,
        "arm64",
        seed,
        {
          kind: "run",
          events: progressStream({ fps, duration: "00:02:40.00", width: 1920, height: 1080, frames: [...frames, ...Array.from({ length: 40 }, (_, i) => frames[frames.length - 1] + 30 * (i + 1))] }),
          exit: 0
        },
        { timerText: (frame) => text(frame) }
      )
    );
  for (const variant of [0, 1, 2]) {
    const v = (name: string) => name + (variant === 0 ? "" : "-" + variant);
    const seed = 50 + 1000 * variant;
    timerCase(v("transition-30fps"), seed, "30", [150, 300, 600, 900], (f) => (f < 230 ? "0:05" : f < 260 ? "0:04" : "0:03"));
    timerCase(v("transition-2997"), seed + 1, "29.97", [200, 450, 900], (f) => (f < 300 ? "2:15\n0:06" : f < 322 ? "0:04" : "0:03"));
    timerCase(v("transition-25-win"), seed + 2, "25", [100, 400], (f) => (f < 70 ? "00:05" : f < 95 ? "00:04" : "00:03"), "win32");
    timerCase(v("never-four"), seed + 3, "30", [150, 400], () => "0:15");
    timerCase(v("three-without-four"), seed + 4, "30", [150, 400], (f) => (f < 61 ? "0:05" : "0:03"));
    timerCase(v("both-04-and-03"), seed + 5, "24", [200, 300], (f) => (f < 49 ? "04" : f < 73 ? "04 03" : "03"));
    timerCase(v("four-again-later"), seed + 6, "60", [600, 1200], (f) => (f % 120 < 60 ? "0:04" : "0:03"));
    timerCase(v("exact-frame-none"), seed + 7, "30", [300], (f) => (f === 61 ? "0:03" : f < 61 ? "0:04" : "0:02"));
    timerCase(v("fractional-fps-window"), seed + 8, "23.976", [240, 480], (f) => (f < 40 ? "0:05" : f < 60 ? "0:04" : "0:03"));
  }

  // Process map, kill and cleanup.
  cases.push(
    local("same-uuid-twice", "darwin", "arm64", 60, () => ({ kind: "hang", events: [["stderr", "Duration: 00:00:10.00\nVideo: h264, 640x360, 30 fps,\n"]] }), {}, [
      ...localOps(),
      { deliver: 8 },
      ...localOps(),
      { deliver: 50 },
      { cleanup: true }
    ])
  );
  cases.push(
    local("cleanup-orders-integer-keys", "linux", "x64", 61, () => ({ kind: "hang" }), {}, [
      ...localOps("7"),
      ...localOps(UUID_B),
      ...localOps("2"),
      { deliver: 200 },
      { cleanup: true }
    ])
  );
  cases.push(
    local("cleanup-rm-fails", "win32", "x64", 62, sampleRun(sample("h264-720p30-aac")), {
      dirs: [pathFor("win32").join(PLATFORM_PATHS.win32.userData, "video-cache", "old")],
      fail: [{ op: "rm", path: pathFor("win32").join(PLATFORM_PATHS.win32.temp, "advantagescope-video-cache"), code: "EBUSY" }]
    }, [...localOps(), { deliver: 200 }, { cleanup: true }])
  );
  cases.push(local("cleanup-nothing", "darwin", "arm64", 63, sampleRun(sample("h264-720p30-aac")), {}, [{ cleanup: true }]));
  cases.push(
    local("cleanup-after-finish", "darwin", "x64", 64, sampleRun(sample("h264-1080p2997")), {}, [...localOps(), { deliver: 200 }, { cleanup: true }])
  );
  cases.push(
    local("two-tabs-interleaved", "darwin", "arm64", 65, (n) => sampleRun(sample(n === 0 ? "h264-long-progress" : "hevc-portrait-23976")), {
      timerText: (f) => (f < 200 ? "0:04" : "0:03")
    }, [...localOps(UUID_A), ...localOps(UUID_B)])
  );
  cases.push({
    name: "unknown-source",
    script: { platform: "darwin", arch: "arm64", seed: 66 },
    ops: [
      { prepare: { uuid: UUID_A, source: 3, matchInfo: null, menuCoordinates: null } },
      { prepare: { uuid: UUID_A, source: "0", matchInfo: null, menuCoordinates: null } },
      { prepare: { uuid: UUID_A, source: null, matchInfo: null, menuCoordinates: null } }
    ]
  });

  // YouTube.
  cases.push(youtube("yt-no-url", "darwin", 70, "https://example.com/watch?v=dQw4w9WgXcQ", ytRun(YT_JSON)));
  cases.push(youtube("yt-ok-youtu-be", "darwin", 71, "https://youtu.be/dQw4w9WgXcQ?t=42", ytRun(YT_JSON)));
  cases.push(youtube("yt-ok-win", "win32", 72, "  https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=1  ", ytRun(YT_JSON)));
  cases.push(youtube("yt-ok-linux", "linux", 73, "youtube.com/embed/dQw4w9WgXcQ", ytRun(YT_JSON)));
  cases.push(youtube("yt-bad-id", "darwin", 74, "https://www.youtube.com/watch?v=short", ytRun(YT_JSON)));
  cases.push(youtube("yt-enoent", "darwin", 75, "https://youtu.be/dQw4w9WgXcQ", { kind: "enoent" }));
  cases.push(youtube("yt-enoent-destroyed", "win32", 76, "https://youtu.be/dQw4w9WgXcQ", { kind: "enoent" }, undefined, { destroyed: true }));
  cases.push(youtube("yt-python3-missing", "linux", 77, "https://youtu.be/dQw4w9WgXcQ", ytRun("", 127, "/usr/bin/env: 'python3': No such file or directory\n")));
  cases.push(youtube("yt-python-version", "darwin", 78, "https://youtu.be/dQw4w9WgXcQ", ytRun("", 1, "ERROR: Python 3.9 is no longer supported\n")));
  cases.push(youtube("yt-enoent-text", "darwin", 79, "https://youtu.be/dQw4w9WgXcQ", ytRun("", 1, "spawn ENOENT\n")));
  cases.push(youtube("yt-other-error", "darwin", 80, "https://youtu.be/dQw4w9WgXcQ", ytRun("partial", 1, "ERROR: [youtube] dQw4w9WgXcQ: Video unavailable\n")));
  cases.push(youtube("yt-empty-error", "darwin", 81, "https://youtu.be/dQw4w9WgXcQ", ytRun("", 2)));
  cases.push(youtube("yt-not-json", "darwin", 82, "https://youtu.be/dQw4w9WgXcQ", ytRun("https://direct.example.invalid/v.mp4\n")));
  cases.push(youtube("yt-bad-json", "darwin", 83, "https://youtu.be/dQw4w9WgXcQ", ytRun('{"formats": [}\n')));
  cases.push(youtube("yt-bom-json", "darwin", 84, "https://youtu.be/dQw4w9WgXcQ", ytRun("\ufeff" + YT_JSON)));
  cases.push(youtube("yt-padded-json", "darwin", 85, "https://youtu.be/dQw4w9WgXcQ", ytRun("\n  " + YT_JSON + "  \n\n")));
  cases.push(youtube("yt-formats-object", "darwin", 86, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: { a: 1 } }))));
  cases.push(youtube("yt-formats-empty", "darwin", 87, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: [] }))));
  cases.push(youtube("yt-formats-null", "darwin", 88, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: null, http_headers: { A: "b" } }))));
  cases.push(
    youtube("yt-url-undefined", "darwin", 89, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 720 }] })))
  );
  cases.push(
    youtube(
      "yt-url-null-byte",
      "darwin",
      90,
      "https://youtu.be/dQw4w9WgXcQ",
      ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 720, url: "https://a.invalid/\u0000x" }] }))
    )
  );
  cases.push(
    youtube(
      "yt-url-number-headers-odd",
      "win32",
      91,
      "https://youtu.be/dQw4w9WgXcQ",
      ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 720, url: 12345 }], http_headers: { "X-A": 1, "X-B": null, "X-C": [1, "b"], "X-D": { e: 1 }, "X-E": true } }))
    )
  );
  cases.push(
    youtube("yt-headers-string", "darwin", 92, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 1, url: "u" }], http_headers: "ab" })))
  );
  cases.push(
    youtube("yt-headers-array", "darwin", 93, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 1, url: "u" }], http_headers: ["x", "y"] })))
  );
  cases.push(
    youtube("yt-headers-falsy", "darwin", 94, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 1, url: "u" }], http_headers: 0 })))
  );
  cases.push(
    youtube("yt-headers-empty-object", "darwin", 95, "https://youtu.be/dQw4w9WgXcQ", ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 1, url: "u" }], http_headers: {} })))
  );
  cases.push(
    youtube(
      "yt-headers-integer-keys",
      "linux",
      96,
      "https://youtu.be/dQw4w9WgXcQ",
      ytRun(JSON.stringify({ formats: [{ vcodec: "h264", height: 1, url: "u" }], http_headers: { b: "2", "10": "x", a: "1", "2": "y" } }))
    )
  );
  cases.push(youtube("yt-title-json-object", "darwin", 97, "https://youtu.be/dQw4w9WgXcQ", ytRun('{"a": 1}')));
  cases.push(
    youtube("yt-main-mkdir-fails", "darwin", 98, "https://youtu.be/dQw4w9WgXcQ", ytRun(YT_JSON), undefined, {
      random: [0],
      fail: [{ op: "mkdir", path: pathFor("darwin").join(PLATFORM_PATHS.darwin.temp, "advantagescope-video-cache", "a".repeat(32)) + "/", code: "EACCES" }]
    })
  );
  cases.push(youtube("yt-clipboard-empty", "linux", 99, "", ytRun(YT_JSON)));

  // The Blue Alliance.
  const oneVideo = [{ type: "youtube", key: "dQw4w9WgXcQ" }, { type: "tba", key: "ignored" }];
  const threeVideos = [
    { type: "youtube", key: "aaaaaaaaaaa" },
    { type: "youtube", key: "bbbbbbbbbbb" },
    { type: "vimeo", key: "1" },
    { type: "youtube", key: "ccccccccccc" }
  ];
  cases.push(tba("tba-no-prefs", 200, { prefs: null }));
  cases.push(tba("tba-bad-prefs", 201, { prefs: "{tbaApiKey" }));
  cases.push(tba("tba-bom-prefs", 202, { prefs: "\ufeff" + JSON.stringify({ tbaApiKey: "k" }), fetch: tbaFetch(oneVideo) }));
  cases.push(tba("tba-no-key", 203, { prefs: JSON.stringify({ tbaApiKey: "" }) }));
  cases.push(tba("tba-key-number", 204, { prefs: JSON.stringify({ tbaApiKey: 6328 }), fetch: tbaFetch(oneVideo) }));
  cases.push(tba("tba-network-error", 205, { fetch: () => ({ network: true }) }));
  cases.push(tba("tba-events-401", 206, { fetch: () => ({ status: 401, body: '{"Error":"X-TBA-Auth-Key is invalid"}' }) }));
  cases.push(tba("tba-events-bad-json", 207, { fetch: () => ({ status: 200, body: "<html>" }) }));
  cases.push(tba("tba-events-bom-json", 208, { fetch: tbaFetch(oneVideo, "\ufeff" + EVENTS_2024) }));
  cases.push(tba("tba-event-missing", 209, { fetch: tbaFetch(oneVideo, JSON.stringify([{ key: "2024x", first_event_code: "cada" }])) }));
  cases.push(tba("tba-event-code-number", 210, { fetch: tbaFetch(oneVideo, JSON.stringify([{ key: "2024x", first_event_code: 5 }])) }));
  cases.push(tba("tba-events-not-array", 211, { fetch: tbaFetch(oneVideo, JSON.stringify({ key: "x" })) }));
  cases.push(tba("tba-one-video", 212, { fetch: tbaFetch(oneVideo) }));
  cases.push(tba("tba-one-video-win", 213, { fetch: tbaFetch(oneVideo), platform: "win32" }));
  cases.push(tba("tba-no-youtube", 214, { fetch: tbaFetch([{ type: "tba", key: "x" }]) }));
  cases.push(tba("tba-videos-missing", 215, { fetch: (url) => (url.includes("/match/") ? { status: 200, body: "{}" } : tbaFetch(oneVideo)!(url, 0)) }));
  cases.push(tba("tba-match-404", 216, { fetch: (url) => (url.includes("/match/") ? { status: 404, body: "{}" } : tbaFetch(oneVideo)!(url, 0)) }));
  cases.push(tba("tba-elim-double", 217, { matchInfo: MATCH_ELIM, fetch: tbaFetch(oneVideo, EVENTS_2024, 10) }));
  cases.push(tba("tba-elim-bracket8", 218, { matchInfo: { ...MATCH_ELIM, matchNumber: 7 }, fetch: tbaFetch(oneVideo, EVENTS_2024, 0) }));
  cases.push(tba("tba-elim-unknown-playoff", 219, { matchInfo: { ...MATCH_ELIM, matchNumber: 30 }, fetch: tbaFetch(oneVideo, EVENTS_2024, 10) }));
  cases.push(tba("tba-elim-event-error", 220, { matchInfo: MATCH_ELIM, fetch: (url) => (url.includes("/event/") ? { status: 500, body: "" } : tbaFetch(oneVideo)!(url, 0)) }));
  cases.push(tba("tba-practice", 221, { matchInfo: { ...MATCH_QUAL, matchType: 1, matchNumber: 3 }, fetch: tbaFetch(oneVideo) }));
  cases.push(tba("tba-match-info-null", 222, { matchInfo: null, fetch: tbaFetch(oneVideo) }));
  cases.push(
    tba("tba-menu-click", 223, {
      fetch: tbaFetch(threeVideos),
      yt: titleRun(["Qualification 12", "  Qual 12 (alt angle)  ", ytRun("", 1, "ERROR: unavailable\n")]),
      menu: () => [{ close: true }, { click: 1 }]
    })
  );
  cases.push(
    tba("tba-menu-click-first", 224, {
      fetch: tbaFetch(threeVideos),
      yt: titleRun(["", '{"t": 1}', "{broken"]),
      menu: () => [{ click: 0 }, { close: true }],
      menuCoordinates: [-10.5, 2.5]
    })
  );
  cases.push(tba("tba-menu-dismissed", 225, { fetch: tbaFetch(threeVideos), yt: titleRun(["A", "B", "C"]), menu: () => [{ close: true }] }));
  for (const seed of [226, 227, 228, 229, 236, 237, 238, 239]) {
    cases.push(
      tba("tba-menu-race-" + seed, seed, { fetch: tbaFetch(threeVideos), yt: titleRun(["A", "B", "C"]), menu: () => [{ close: true }, { click: 2 }] })
    );
  }
  cases.push(tba("tba-titles-python-missing", 230, { fetch: tbaFetch(threeVideos), yt: (args) => (args.includes("--get-title") ? { kind: "enoent" } : ytRun(YT_JSON)) }));
  cases.push(
    tba("tba-titles-python-one", 231, {
      fetch: tbaFetch(threeVideos),
      yt: titleRun(["A", ytRun("", 1, "python3 not found"), "C"]),
      menu: () => [{ click: 0 }]
    })
  );
  cases.push(
    tba("tba-direct-url-fails", 232, {
      fetch: tbaFetch(oneVideo),
      yt: () => ytRun(JSON.stringify({ formats: [] }))
    })
  );
  cases.push(
    tba("tba-direct-url-python", 233, {
      fetch: tbaFetch(oneVideo),
      yt: () => ({ kind: "enoent" })
    })
  );
  cases.push(tba("tba-menu-coords-null", 234, { fetch: tbaFetch(oneVideo), menuCoordinates: null }));
  cases.push(
    tba("tba-menu-coords-odd", 235, {
      fetch: tbaFetch(threeVideos),
      yt: titleRun(["A", "B", "C"]),
      menu: () => [{ click: 2 }],
      menuCoordinates: ["3.5", null]
    })
  );
  return cases;
}

export function randomInt(rng: () => number, n: number) {
  return Math.floor(rng() * n);
}

export { UUID_A, UUID_B, YT_JSON, progressStream, local, youtube, localOps };
