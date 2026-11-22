import type { Platform } from "./bundle";

export type Fetch = { reject: true } | { status: number; body: Uint8Array };
export type Op = { check: Fetch } | { shouldPrompt: true } | { prompt: number };

export interface CheckerCase {
  name: string;
  platform: Platform;
  arch: string;
  translation: boolean;
  packaged: boolean;
  version: string;
  beta: boolean;
  repository: string | null;
  ops: Op[];
}

export class Rng {
  private state: number;
  constructor(seed: number) {
    this.state = seed >>> 0 || 0x2545f491;
  }
  next(): number {
    this.state ^= this.state << 13;
    this.state ^= this.state >>> 17;
    this.state ^= this.state << 5;
    return this.state >>> 0;
  }
  below(n: number): number {
    return this.next() % n;
  }
  chance(p: number): boolean {
    return this.next() / 4294967296 < p;
  }
  pick<T>(items: readonly T[]): T {
    return items[this.below(items.length)];
  }
}

const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));
const json = (value: unknown) => utf8(JSON.stringify(value));
const ok = (value: unknown): Fetch => ({ status: 200, body: json(value) });

const API = "https://api.github.com/repos/Mechanical-Advantage/AdvantageScope/releases/";
const DL = "https://github.com/Mechanical-Advantage/AdvantageScope/releases/download/";

function assetsFor(version: string, names: string[]) {
  return names.map((name, i) => ({
    url: API + "assets/" + (1000 + i),
    id: 1000 + i,
    name,
    content_type: "application/octet-stream",
    size: 100000 + i,
    browser_download_url: DL + "v" + version + "/" + name
  }));
}

function standardAssets(version: string) {
  return assetsFor(version, [
    `advantagescope-linux-arm64-v${version}.AppImage`,
    `advantagescope-linux-arm64-v${version}.deb`,
    `advantagescope-linux-x64-v${version}.AppImage`,
    `advantagescope-linux-x64-v${version}.deb`,
    `advantagescope-mac-arm64-v${version}.dmg`,
    `advantagescope-mac-x64-v${version}.dmg`,
    `advantagescope-win-arm64-v${version}.exe`,
    `advantagescope-win-x64-v${version}.exe`
  ]);
}

function release(tag: string, published: unknown, prerelease: unknown, assets?: unknown, extra: object = {}) {
  const r: any = {
    url: API + tag,
    html_url: "https://github.com/Mechanical-Advantage/AdvantageScope/releases/tag/" + tag,
    id: tag.length * 7919,
    tag_name: tag,
    name: tag,
    draft: false,
    prerelease,
    created_at: "2026-11-19T12:00:00Z",
    published_at: published,
    assets: assets === undefined ? standardAssets(tag.slice(1)) : assets,
    ...extra
  };
  if (published === undefined) delete r.published_at;
  if (prerelease === undefined) delete r.prerelease;
  return r;
}

const ALPHA6 = release("v27.0.0-alpha-6", "2026-11-20T18:30:00Z", true);
const ALPHA7 = release("v27.0.0-alpha-7", "2027-01-08T23:30:00Z", true);
const STABLE26 = release("v26.2.1", "2026-04-05T09:15:00Z", false);
const STABLE27 = release("v27.0.0", "2027-01-09T00:00:00Z", false);
const MAC_ONLY = release("v27.1.0", "2027-02-01T10:00:00Z", false, assetsFor("27.1.0", ["advantagescope-mac-arm64-v27.1.0.dmg"]));

type Arch = [Platform, string, boolean];
const DARWIN_ARM: Arch = ["darwin", "arm64", false];
const DARWIN_X64: Arch = ["darwin", "x64", false];
const DARWIN_ROSETTA: Arch = ["darwin", "x64", true];
const LINUX_X64: Arch = ["linux", "x64", false];
const WIN_X64: Arch = ["win32", "x64", false];
const WIN_EMULATED: Arch = ["win32", "x64", true];
const WIN_IA32: Arch = ["win32", "ia32", false];

function hand(
  name: string,
  [platform, arch, translation]: Arch,
  ops: Op[],
  opts: { packaged?: boolean; version?: string; beta?: boolean; repository?: string | null } = {}
): CheckerCase {
  return {
    name,
    platform,
    arch,
    translation,
    packaged: opts.packaged ?? true,
    version: opts.version ?? "27.0.0-alpha-6",
    beta: opts.beta ?? true,
    repository: opts.repository ?? null,
    ops
  };
}

const check = (fetch: Fetch): Op => ({ check: fetch });
const should: Op = { shouldPrompt: true };
const prompt = (response: number): Op => ({ prompt: response });
const all = (fetch: Fetch, responses: number[]): Op[] => [check(fetch), should, ...responses.map(prompt)];

export function handCheckerCases(): CheckerCase[] {
  const newer = ok([ALPHA7, ALPHA6, STABLE26]);
  const same = ok([ALPHA6, STABLE26]);
  return [
    hand("prompt before any check", DARWIN_ARM, [should, prompt(0)]),
    hand("development build", DARWIN_ARM, all(newer, [0]), { packaged: false }),
    hand("development build stable", WIN_X64, all(newer, [0, 1]), { packaged: false, beta: false }),
    hand("network failure", DARWIN_ARM, all({ reject: true }, [0])),
    hand("invalid json", LINUX_X64, all({ status: 200, body: utf8("<!doctype html><title>Unicorn!</title>") }, [0])),
    hand("empty body", LINUX_X64, all({ status: 200, body: new Uint8Array(0) }, [0])),
    hand("bom before array", DARWIN_ARM, all({ status: 200, body: utf8("﻿" + JSON.stringify([ALPHA7])) }, [0])),
    hand("invalid utf-8 in a field", DARWIN_ARM, all({ status: 200, body: new Uint8Array([...json([ALPHA7]).slice(0, 40), 0xff, ...json([ALPHA7]).slice(40)]) }, [0])),
    hand("rate limited object", DARWIN_ARM, [...all({ status: 403, body: json({ message: "API rate limit exceeded", documentation_url: "https://docs.github.com" }) }, [0]), ...all(newer, [1]), ...all({ status: 403, body: json({ message: "API rate limit exceeded" }) }, [0])]),
    hand("json null", DARWIN_ARM, all(ok(null), [0])),
    hand("json string", DARWIN_ARM, all(ok("v27.0.0"), [0])),
    hand("empty list", DARWIN_ARM, all(ok([]), [0])),
    hand("only other majors", WIN_X64, all(ok([release("v28.0.0", "2027-06-01T00:00:00Z", false), STABLE26]), [0])),
    hand("same version", DARWIN_ARM, all(same, [0])),
    hand("same version rosetta", DARWIN_ROSETTA, all(same, [0, 1, 2])),
    hand("same version windows emulated", WIN_EMULATED, all(same, [0, 1, 2])),
    hand("newer darwin arm", DARWIN_ARM, all(newer, [0, 1, 2])),
    hand("newer darwin x64", DARWIN_X64, all(newer, [0])),
    hand("newer rosetta", DARWIN_ROSETTA, all(newer, [0, 1, 2])),
    hand("newer linux two matches", LINUX_X64, all(newer, [0, 0, 1, 2])),
    hand("newer windows", WIN_X64, all(newer, [0, 1, 2])),
    hand("newer windows ia32", WIN_IA32, all(newer, [0, 1, 2])),
    hand("newer windows emulated", WIN_EMULATED, all(newer, [0, 2])),
    hand("only mac asset", WIN_X64, all(ok([MAC_ONLY]), [0]), { version: "27.0.0", beta: false }),
    hand("stable ignores prereleases", DARWIN_ARM, all(ok([ALPHA7, STABLE27, STABLE26]), [0]), { version: "26.2.1", beta: false }),
    hand("stable same major prerelease only", DARWIN_ARM, all(ok([ALPHA7, ALPHA6, STABLE26]), [0]), { version: "27.0.0", beta: false }),
    hand("beta sees prerelease", DARWIN_ARM, all(ok([ALPHA7, STABLE27]), [0]), { version: "27.0.0" }),
    hand("first entry wins unsorted", DARWIN_ARM, all(ok([STABLE26, STABLE27, ALPHA7]), [0]), { version: "26.0.0", beta: false }),
    hand("no assets field", DARWIN_ARM, [...all(ok([release("v27.0.0-alpha-7", "2027-01-08T23:30:00Z", true, null)]), [0, 1]), should]),
    hand("asset without url", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "2027-01-08T23:30:00Z", true, [{ name: "advantagescope-mac-arm64.dmg" }])]), [0])),
    hand("asset url null", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "2027-01-08T23:30:00Z", true, [{ name: "x-mac-arm64", browser_download_url: null }])]), [0])),
    hand("asset name array", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "2027-01-08T23:30:00Z", true, [{ name: ["mac", "arm64"], browser_download_url: 7 }])]), [0])),
    hand("asset without name", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "2027-01-08T23:30:00Z", true, [{ browser_download_url: "u" }])]), [0, 1])),
    hand("tag without v", DARWIN_ARM, all(ok([release("27.0.0", "2027-01-09T00:00:00Z", false), ALPHA6]), [0]), { version: "7.0.0" }),
    hand("tag number", DARWIN_ARM, all(ok([{ tag_name: 27, prerelease: false }]), [0])),
    hand("release null", DARWIN_ARM, all(ok([null, ALPHA7]), [0])),
    hand("release null stable", DARWIN_ARM, all(ok([null, ALPHA7]), [0]), { beta: false }),
    hand("missing published_at", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", undefined, true)]), [0])),
    hand("published_at null", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", null, true)]), [0])),
    hand("published_at number", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", 1799999999999, true)]), [0])),
    hand("published_at legacy", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "Fri, 08 Jan 2027 23:30:00 GMT", true)]), [0])),
    hand("published_at local", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "2027-01-08T23:30:00", true)]), [0])),
    hand("published_at date only", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "2027-01-08", true)]), [0])),
    hand("published_at offset", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "2027-01-08T23:30:00-08:00", true)]), [0])),
    hand("published_at garbage", DARWIN_ARM, all(ok([release("v27.0.0-alpha-7", "soon", true)]), [0])),
    hand("published_at same version", DARWIN_ARM, all(ok([release("v27.0.0-alpha-6", "2026-11-20T23:59:59.999+05:30", true)]), [0])),
    hand("version without dots", DARWIN_ARM, all(ok([release("v27", "2027-01-09T00:00:00Z", false)]), [0]), { version: "27" }),
    hand("empty version", DARWIN_ARM, all(ok([release("v", "2027-01-09T00:00:00Z", false), release("v.1", "2027-01-10T00:00:00Z", false)]), [0]), { version: "" }),
    hand("stale state after failure", DARWIN_ARM, [...all(newer, [0]), ...all({ reject: true }, [0]), ...all(newer, [2]), ...all(same, [0])]),
    hand("fallback url persists", LINUX_X64, [...all(newer, [0, 0]), ...all(ok([MAC_ONLY]), [0])], { version: "27.0.0", beta: false }),
    hand("release repository", DARWIN_ARM, all(newer, [0, 1, 2]), { repository: "AdamEXu/asrs" }),
    hand("release repository windows", WIN_X64, all(ok([MAC_ONLY]), [0, 1]), { repository: "AdamEXu/asrs", version: "27.0.0", beta: false }),
    hand("release repository network failure", LINUX_X64, all({ reject: true }, [0]), { repository: "AdamEXu/asrs" }),
    hand("release repository empty", DARWIN_ARM, all(ok([]), [0]), { repository: "AdamEXu/asrs", beta: false }),
    hand("out of range response", WIN_X64, all(newer, [3, 7]))
  ];
}

const TAGS = [
  "v27.0.0",
  "v27.0.0-alpha-6",
  "v27.0.0-alpha-7",
  "v27.0.0-beta-1",
  "v27.1.0",
  "v27.0.1",
  "v26.2.1",
  "v26.0.0",
  "v28.0.0",
  "v4.1.0",
  "27.0.0",
  "V27.2.0",
  "v027.0.0",
  "v27",
  "v",
  "",
  "x27.0.1",
  "v27.0.0 ",
  "v٢٧.0.0",
  "vv27.0.0"
];
const PUBLISHED: unknown[] = [
  "2026-11-20T18:30:00Z",
  "2027-01-08T23:30:00Z",
  "2027-01-09T00:00:00Z",
  "2026-03-08T10:00:00Z",
  "2026-11-01T09:59:59Z",
  "2026-12-31T23:59:59.999Z",
  "2025-12-31T12:00:00+14:00",
  "2026-04-05T02:30:00",
  "2026-06-15",
  "2026-06",
  "Jan 9 2027 10:00 PST",
  "Sat, 09 Jan 2027 07:59:59 GMT",
  "2027/01/09",
  "1/9/27",
  null,
  undefined,
  1700000000000,
  -1,
  "",
  "garbage",
  true,
  ["2026-01-01"],
  {}
];
const PRERELEASE: unknown[] = [true, false, false, undefined, "false", 0, null, 1];
const VERSIONS = ["27.0.0-alpha-6", "27.0.0", "27.1.0", "27.0.0-alpha-7", "26.2.1", "28.0.0-beta-1", "4.1.0", "27", ""];
const ARCHES: Arch[] = [DARWIN_ARM, DARWIN_X64, DARWIN_ROSETTA, LINUX_X64, ["linux", "arm64", false], WIN_X64, WIN_EMULATED, WIN_IA32, ["win32", "arm64", false]];
const ASSET_NAMES = [
  "advantagescope-mac-arm64.dmg",
  "advantagescope-mac-x64.dmg",
  "advantagescope-mac-universal.dmg",
  "advantagescope-linux-x64.AppImage",
  "advantagescope-linux-x64.deb",
  "advantagescope-linux-arm64.deb",
  "advantagescope-win-x64.exe",
  "advantagescope-win-arm64.exe",
  "advantagescope-win-ia32.exe",
  "AdvantageScope-darwin-arm64.zip",
  "checksums.txt"
];

function randomAsset(rng: Rng, index: number): unknown {
  const roll = rng.below(40);
  if (roll === 0) return null;
  if (roll === 1) return { browser_download_url: "https://example.com/" + index };
  const a: any = { id: index, name: rng.pick(ASSET_NAMES) };
  if (roll === 2) a.name = [rng.pick(["mac", "win", "linux"]), rng.pick(["arm64", "x64"])];
  if (roll === 3) a.name = 5;
  const url = rng.below(12);
  if (url === 0) {
  } else if (url === 1) a.browser_download_url = null;
  else if (url === 2) a.browser_download_url = index;
  else a.browser_download_url = DL + "asset/" + index + "/" + a.name;
  return a;
}

function randomRelease(rng: Rng): unknown {
  const roll = rng.below(60);
  if (roll === 0) return null;
  if (roll === 1) return 5;
  if (roll === 2) return { prerelease: false };
  const tag = rng.pick(TAGS);
  let assets: unknown;
  const shape = rng.below(20);
  if (shape === 0) assets = undefined;
  else if (shape === 1) assets = null;
  else if (shape === 2) assets = {};
  else if (shape < 8) assets = standardAssets(tag.slice(1));
  else assets = Array.from({ length: rng.below(5) }, (_, i) => randomAsset(rng, i));
  const r = release(tag, rng.pick(PUBLISHED), rng.pick(PRERELEASE), assets === undefined ? [] : assets);
  if (assets === undefined) delete r.assets;
  if (rng.below(30) === 0) r.tag_name = rng.pick([null, 27, ["v27.0.0"]]);
  return r;
}

function randomFetch(rng: Rng): Fetch {
  const roll = rng.below(30);
  if (roll === 0) return { reject: true };
  if (roll === 1) return { status: 200, body: utf8("[") };
  if (roll === 2) return ok({ message: "Not Found" });
  return ok(Array.from({ length: rng.below(6) }, () => randomRelease(rng)));
}

export function randomCheckerCases(count: number, seed: number): CheckerCase[] {
  const rng = new Rng(seed);
  const out: CheckerCase[] = [];
  for (let i = 0; i < count; i++) {
    const [platform, arch, translation] = rng.pick(ARCHES);
    const ops: Op[] = [];
    const checks = 1 + rng.below(3);
    for (let c = 0; c < checks; c++) {
      ops.push(check(randomFetch(rng)), should);
      for (let p = rng.below(3); p > 0; p--) ops.push(prompt(rng.below(3)));
    }
    out.push({
      name: "random " + i,
      platform,
      arch,
      translation,
      packaged: rng.below(15) !== 0,
      version: rng.pick(VERSIONS),
      beta: rng.chance(0.5),
      repository: rng.below(6) === 0 ? "AdamEXu/asrs" : null,
      ops
    });
  }
  return out;
}

const HAND_DATES: unknown[] = [
  "2026-11-20T18:30:00Z",
  "2026-11-20T18:30:00.000Z",
  "2026-11-20T18:30:00.5Z",
  "2026-11-20T18:30:00.12Z",
  "2026-11-20T18:30:00.1234Z",
  "2026-11-20T18:30:00.123456789123Z",
  "2026-11-20T18:30Z",
  "2026-11-20T18:30",
  "2026-11-20T18:30:00",
  "2026-11-20T18:30:00.999",
  "2026-11-20",
  "2026-11",
  "2026",
  "+002026-11-20T18:30:00Z",
  "-000001-01-01T00:00:00Z",
  "-000000-01-01T00:00:00Z",
  "+000000-01-01T00:00:00Z",
  "+275760-09-13T00:00:00Z",
  "+275760-09-13T00:00:00.001Z",
  "-271821-04-20T00:00:00Z",
  "-271821-04-19T23:59:59.999Z",
  "+275760-09-13T00:00:00",
  "-271821-04-20T00:00:00",
  "+275760-09-13",
  "2026-11-20T24:00:00Z",
  "2026-11-20T24:00:01Z",
  "2026-11-20T24:00",
  "2026-11-20T25:00Z",
  "2026-11-20T23:60Z",
  "2026-11-20T23:59:60Z",
  "2026-13-01",
  "2026-00-10",
  "2026-02-30",
  "2026-02-31T00:00Z",
  "2026-04-31",
  "2026-11-32",
  "2026-11-00",
  "2026-11-20T18:30:00+05:30",
  "2026-11-20T18:30:00-08:00",
  "2026-11-20T18:30:00+0530",
  "2026-11-20T18:30:00+05",
  "2026-11-20T18:30:00+24:00",
  "2026-11-20T18:30:00+23:59",
  "2026-11-20T18:30:00+05:60",
  "2026-11-20T18:30:00z",
  "2026-11-20t18:30:00Z",
  "2026-11-20 18:30:00Z",
  "2026-11-20 18:30:00",
  "2026-11-20 18:30",
  "2026-11-20T18:30:00ZZ",
  "2026-11-20T18:30:00Z ",
  " 2026-11-20T18:30:00Z",
  "2026-11-20T18:30:00 Z",
  "2026-11-20T18:30:00 GMT",
  "2026-11-20T18:30:00 UTC+5",
  "2026-11-20T8:30:00Z",
  "2026-11-20T18:3:00Z",
  "2026-1-20",
  "2026-01-2",
  "026-11-20",
  "20261-11-20",
  "02026-11-20",
  "2026/11/20",
  "2026/11/20 18:30",
  "11/20/2026",
  "11/20/26",
  "11/20/49",
  "11/20/50",
  "11/20/99",
  "11/20/100",
  "11/20",
  "20/11/2026",
  "11-20-2026",
  "2026.11.20",
  "20.11.2026",
  "Nov 20 2026",
  "Nov 20, 2026",
  "20 Nov 2026",
  "November 20, 2026 18:30:00",
  "novEMBER 20 2026",
  "Novemberish 20 2026",
  "No 20 2026",
  "Fri, 20 Nov 2026 18:30:00 GMT",
  "Fri, 20 Nov 2026 18:30:00 +0000",
  "Fri, 20 Nov 2026 18:30:00 -0800",
  "Fri, 20 Nov 2026 18:30:00 PST",
  "Fri, 20 Nov 2026 18:30:00 PDT",
  "Fri, 20 Nov 2026 18:30:00 EST",
  "Fri, 20 Nov 2026 18:30:00 EDT",
  "Fri, 20 Nov 2026 18:30:00 CST",
  "Fri, 20 Nov 2026 18:30:00 CDT",
  "Fri, 20 Nov 2026 18:30:00 MST",
  "Fri, 20 Nov 2026 18:30:00 MDT",
  "Fri, 20 Nov 2026 18:30:00 UT",
  "Fri, 20 Nov 2026 18:30:00 UTC",
  "Fri, 20 Nov 2026 18:30:00 Z",
  "Fri, 20 Nov 2026 18:30:00 BST",
  "Fri Nov 20 2026 18:30:00 GMT-0800 (Pacific Standard Time)",
  "Fri Nov 20 2026 18:30:00 GMT+0530 (India Standard Time)",
  "Fri Nov 20 2026 18:30:00 GMT+1300 (New Zealand Daylight Time)",
  "Fri Nov 20 2026 18:30:00 GMT+13:00",
  "Fri Nov 20 2026 18:30:00 GMT+13",
  "Fri Nov 20 2026 18:30:00 GMT+130",
  "Fri Nov 20 2026 18:30:00 GMT+13000",
  "Fri Nov 20 2026 18:30:00 GMT-",
  "Fri Nov 20 2026 18:30:00 (comment (nested)) GMT",
  "Fri Nov 20 2026 (unclosed",
  "Nov 20 2026 6:30 PM",
  "Nov 20 2026 6:30 pm",
  "Nov 20 2026 12:30 AM",
  "Nov 20 2026 12:30 PM",
  "Nov 20 2026 13:30 PM",
  "Nov 20 2026 0:30 AM",
  "Nov 20 2026 PM 6:30",
  "Nov 20 2026 6 PM",
  "Nov 20 2026 18:30:00.250",
  "Nov 20 2026 18:30:00.2",
  "Nov 20 2026 18:30:00.2500000001",
  "Nov 20 2026 18:30:",
  "Nov 20 2026 18::30",
  "Nov 20 2026 18:30:00:10",
  "Nov 20 2026 24:00",
  "Nov 20 2026 24:00:00.001",
  "Nov 20 2026 T18:30",
  "Nov 20 2026 18.30",
  "Nov 20 2026 18.30.15",
  "2026 Nov 20",
  "20 2026 Nov",
  "Nov 2026",
  "Nov",
  "Nov 20 2026 18:30 +05",
  "Nov 20 2026 18:30 +0530",
  "Nov 20 2026 18:30 -5:30",
  "Nov 20 2026 18:30 UTC+05:30",
  "Nov 20 2026 18:30 GMT+5 30",
  "Nov 20 2026 UTC",
  "UTC Nov 20 2026",
  "Nov 20 2026 -08:00",
  "Nov 20 2026 Z",
  "Nov 20 2026 Zulu",
  "Mon Nov 20 2026",
  "Monday, November 20, 2026",
  "Friday 20 2026 Nov",
  "Nov 20 2026 xyz",
  "abc Nov 20 2026",
  "Nov 20 2026 )",
  "Nov 20 2026 - 3",
  "Nov-20-2026",
  "20-Nov-2026",
  "2026-Nov-20",
  "Nov 20 -2026",
  "Nov 20 +2026",
  "Nov 20 2026 18:30 am",
  "1 2 3",
  "1 2 3 4",
  "1",
  "12",
  "123",
  "2026",
  "99",
  "0",
  "00:00",
  "18:30",
  "T18:30",
  "2026-11-20T",
  "2026-11-20TZ",
  "2026-11-20T18",
  "2026-11-20T18:",
  "2026-11-20T18:30:",
  "2026-11-20T18:30:00.",
  "2026-11-20T18:30:00.Z",
  "2026-11-20T18:30:00+",
  "2026-11-20T18:30:00+05:",
  "2026-11-20T18:30:00+5:30",
  "2026-11-20T18:30:00+053",
  "2026-11-20T18:30:00 +05:30",
  "2026-11-20T18:30:00 PST",
  "2026-11-20 PST",
  "2026-11-20 garbage",
  "2026-11-20xyz",
  "2026-11-20T18:30:00.123456",
  "2026-11-20T18:30:00,123Z",
  "1970-01-01T00:00:00Z",
  "1969-12-31T23:59:59.999Z",
  "0000-01-01T00:00:00Z",
  "0001-01-01",
  "0049-01-01",
  "0050-01-01",
  "0099-12-31T23:59:59",
  "1900-02-29",
  "2000-02-29",
  "2100-02-29",
  "9999-12-31T23:59:59.999Z",
  "2026-03-08T02:30:00",
  "2026-03-08T02:00:00",
  "2026-11-01T01:30:00",
  "2026-04-05T02:30:00",
  "2026-09-27T02:30:00",
  "2026-04-05T01:30:00",
  "Mar 8 2026 2:30",
  "Nov 1 2026 1:30",
  "Sep 27 2026 2:30 AM",
  "Apr 5 2026 2:30 AM",
  "",
  " ",
  "\t2026-11-20\n",
  " 2026-11-20",
  "2026-11-20　",
  "2026 Nov 20",
  "Nov 20 2026",
  "Nov 20 2026",
  "﻿Nov 20 2026",
  "2026-11-20\u0000garbage",
  "Nov 20\u0000 2026",
  "Ｎov 20 2026",
  "Nov ２０ 2026",
  "٢٠٢٦-11-20",
  "Növ 20 2026",
  "[Nov] 20 2026",
  "Nov_20_2026",
  "Nov 20 2026 ~",
  "Nov 20 2026 @ 18:30",
  "Nov 20 2026 at 18:30",
  "Nov 20 2026 a 18:30",
  "Nov 20 2026 t 18:30",
  "Nov 20 2026 18:30 utc+5",
  "Nov 20 2026 18:30 gmt-0800 (PST)",
  "1000000000",
  "999999999 1 1",
  "Nov 20 1000000000",
  "Nov 20 123456789012",
  "Nov 20 2026 18:30 +99999",
  "Nov 20 2026 18:30 +9999",
  "Nov 20 2026 18:30 +2400",
  "Nov 20 2026 18:30 +999999999:59",
  "Nov 20 2026 18:30 -16777216:00",
  "Nov 20 2026 18:30 +0596523:00",
  "Nov 20 2026 18:30 +596524:00",
  "Nov 20 2026 18:30 +1193046:28",
  "Jan 1 275760",
  "Sep 13 275760",
  "Sep 14 275760",
  "Apr 20 -271821",
  "Dec 31 1969 23:59:59.999 GMT",
  "😀 Nov 20 2026",
  "Nov 20 2026 \ud800",
  "dec 25 2026",
  "DEC 25 2026",
  "decimal 25 2026",
  "Ma 25 2026",
  "Mayo 25 2026",
  "june 1 2026",
  "jun 1 2026",
  "sept 1 2026",
  "a 1 2026",
  "p 1 2026",
  "am 1 2026",
  "pm 6 1 2026",
  "1 2026 Jan 6:00 PM PST",
  null,
  true,
  false,
  0,
  -0,
  1.5,
  -1.5,
  1e20,
  8.64e15,
  8.64e15 + 1,
  -8.64e15,
  -8.64e15 - 1,
  1799999999999.9,
  ["2026-11-20"],
  ["2026", "11", "20"],
  [],
  [5],
  [null],
  [[]],
  {},
  { published_at: "2026-11-20" }
];

const FIELD_ALT = [
  ["2026", "1999", "2049", "0000", "9999", "+002026", "-000042", "26", "20260"],
  ["01", "02", "11", "12", "13", "00", "1", "001"],
  ["01", "20", "28", "29", "30", "31", "32", "00", "5"],
  ["00", "09", "12", "18", "23", "24", "25", "7"],
  ["00", "30", "59", "60", "5"],
  ["00", "45", "59", "60", "9"],
  ["", "0", "5", "05", "123", "1234", "999999999999"],
  ["Z", "", "+00:00", "-08:00", "+05:30", "+13:00", "+0530", "+14", "z", " GMT", " PST", "+24:00", "-00:00"]
];
const LEGACY_TOKENS = [
  "Nov", "January", "feb", "Sep", "dec", "20", "1", "31", "2026", "99", "49", "0", "12", "13", "6", "18:30", "18:30:45",
  "6:30:00.5", "24:00", "AM", "PM", "am", "pm", "GMT", "UTC", "UT", "Z", "PST", "EDT", "+0800", "-05:30", "+5",
  "(note)", "Mon", "Friday", "T", "-", ".", "/", ",", ":", ")", "xyz"
];
const SEPARATORS = [" ", " ", " ", "", ",", ", ", "-", "/", "  "];

export function randomDateInputs(count: number, seed: number): unknown[] {
  const rng = new Rng(seed);
  const out: unknown[] = [];
  for (let i = 0; i < count; i++) {
    if (rng.chance(0.55)) {
      const [y, mo, d, h, mi, s, ms, tz] = FIELD_ALT.map((alts) => rng.pick(alts));
      const level = rng.below(6);
      let text = y;
      if (level >= 1) text += "-" + mo;
      if (level >= 2) text += "-" + d;
      if (level >= 3) text += (rng.chance(0.9) ? "T" : " ") + h + ":" + mi;
      if (level >= 4) text += ":" + s;
      if (level >= 5 && ms !== "") text += "." + ms;
      if (level >= 3) text += tz;
      out.push(text);
    } else {
      const n = 1 + rng.below(6);
      let text = "";
      for (let t = 0; t < n; t++) {
        if (t > 0) text += rng.pick(SEPARATORS);
        text += rng.pick(LEGACY_TOKENS);
      }
      out.push(text);
    }
  }
  return out;
}

export function dateInputs(count: number, seed: number): unknown[] {
  return [...HAND_DATES, ...randomDateInputs(count, seed)];
}

export interface LicenseEntry {
  kind: "file" | "dir";
  path: string;
  data?: Uint8Array;
}

export type SpdxReply = { reject: true } | { status: number; body: Uint8Array };

export interface LicenseCase {
  name: string;
  files: LicenseEntry[];
  spdx: { [id: string]: SpdxReply };
}

const file = (p: string, text: string | Uint8Array): LicenseEntry => ({
  kind: "file",
  path: p,
  data: typeof text === "string" ? utf8(text) : text
});
const dir = (p: string): LicenseEntry => ({ kind: "dir", path: p });

function lock(packages: string[]): LicenseEntry {
  const obj: any = { name: "advantagescope", version: "27.0.0-alpha-6", lockfileVersion: 3, requires: true, packages: {} };
  for (const p of packages) obj.packages[p] = p === "" ? { name: "advantagescope", version: "27.0.0-alpha-6", license: "BSD-3-Clause" } : { version: "1.0.0" };
  return file("package-lock.json", JSON.stringify(obj, null, 2));
}

const spdxOk = (text: unknown): SpdxReply => ({ status: 200, body: json({ licenseId: "X", licenseText: text, name: "X" }) });
const BSD = "Copyright (c) 2021-2026 Littleton Robotics\nAll rights reserved.\n";
const MIT = "MIT License\n\nPermission is hereby granted, free of charge...\n";

export function handLicenseCases(): LicenseCase[] {
  return [
    {
      name: "upstream shape",
      files: [
        lock(["", "node_modules/three", "node_modules/@mcap/core", "node_modules/fuse.js", "node_modules/zlib-thing", "node_modules/missing-one"]),
        file("LICENSE", BSD),
        file("README.md", "# AdvantageScope\n"),
        file("file-header.txt", "// header\n"),
        file("license-check-config.json", "{}"),
        dir("licenses"),
        file("licenses/The Blue Alliance.txt", "TBA terms\n"),
        file("licenses/ACME Robotics.txt", "ACME terms\n"),
        file("licenses/Google Material Symbols.txt", "Apache-2.0 text\n"),
        file("licenses/notes.md", "not a license\n"),
        dir("node_modules/three"),
        file("node_modules/three/LICENSE", "The MIT License\n\nCopyright © 2010-2026 three.js authors\n"),
        file("node_modules/three/package.json", '{"license":"MIT"}'),
        dir("node_modules/@mcap/core"),
        file("node_modules/@mcap/core/package.json", '{"license":"MIT"}'),
        dir("node_modules/fuse.js"),
        file("node_modules/fuse.js/LICENSE", "Apache License\nVersion 2.0\n"),
        file("node_modules/fuse.js/LICENSE.js", "module.exports = 1;"),
        dir("node_modules/zlib-thing"),
        file("node_modules/zlib-thing/package.json", '{"license":"(MIT OR Apache-2.0)"}')
      ],
      spdx: { MIT: spdxOk(MIT), "Apache-2.0": spdxOk("Apache text\n") }
    },
    {
      name: "license file filters",
      files: [
        lock(["", "node_modules/a"]),
        file("LICENSE.md", "root md\n"),
        file("license", "root lower\n"),
        dir("node_modules/a"),
        file("node_modules/a/LICENSE-MIT", "mit part\n"),
        file("node_modules/a/LICENSE-APACHE", "apache part\n"),
        file("node_modules/a/License.txt", "txt part\n"),
        file("node_modules/a/license.json", "{}"),
        file("node_modules/a/LICENSE-header.txt", "excluded\n"),
        file("node_modules/a/LICENSE.js", "excluded"),
        dir("node_modules/a/licenses"),
        file("node_modules/a/package.json", '{"license":"MIT"}'),
        file("node_modules/a/COPYING", "not matched\n")
      ],
      spdx: {}
    },
    {
      name: "spdx edge cases",
      files: [
        lock(["", "node_modules/a", "node_modules/b", "node_modules/c", "node_modules/d", "node_modules/e", "node_modules/f", "node_modules/g", "node_modules/h", "node_modules/i"]),
        file("LICENSE", BSD),
        ...["a", "b", "c", "d", "e", "f", "g", "h", "i"].map((m) => dir("node_modules/" + m)),
        file("node_modules/a/package.json", '{"license":"ISC"}'),
        file("node_modules/b/package.json", '{"license":{"type":"MIT"}}'),
        file("node_modules/c/package.json", '{"name":"c"}'),
        file("node_modules/d/package.json", '{"license":"  (BSD-2-Clause OR MIT)  "}'),
        file("node_modules/e/package.json", '{"license":"SEE LICENSE IN LICENSE.txt"}'),
        file("node_modules/f/package.json", '{"license":"Unknown-1.0"}'),
        file("node_modules/g/package.json", '{"license":"0BSD"}'),
        file("node_modules/h/package.json", '{"license":"WTFPL"}'),
        file("node_modules/i/package.json", '{"license":"Zlib"}')
      ],
      spdx: {
        ISC: spdxOk("ISC text\n"),
        "[object Object]": spdxOk("object text\n"),
        undefined: spdxOk(null),
        "BSD-2-Clause": spdxOk(42),
        "SEE LICENSE IN LICENSE.txt": { status: 404, body: utf8("404: Not Found") },
        "Unknown-1.0": { reject: true },
        "0BSD": { status: 200, body: utf8("not json") },
        WTFPL: { status: 200, body: json({ licenseId: "WTFPL" }) },
        Zlib: spdxOk({ nested: ["text"] })
      }
    },
    {
      name: "sort order",
      files: [
        lock(["", ...["Zeta", "alpha", "Alpha", "@scope/pkg", "_private", "10-thing", "9-thing", "émile", "Éclair", "a-b", "a_b", "ab"].map((m) => "node_modules/" + m)]),
        file("LICENSE", BSD),
        ...["Zeta", "alpha", "Alpha", "@scope/pkg", "_private", "10-thing", "9-thing", "émile", "Éclair", "a-b", "a_b", "ab"].flatMap((m) => [
          dir("node_modules/" + m),
          file("node_modules/" + m + "/LICENSE", m + " license\n")
        ]),
        dir("licenses"),
        file("licenses/AdvantageScope.txt", "duplicate root name\n"),
        file("licenses/zz.txt", "zz\n"),
        file("licenses/AA.txt", "AA\n")
      ],
      spdx: {}
    },
    {
      name: "no root license and nested modules",
      files: [
        lock(["", "node_modules/a", "node_modules/a/node_modules/b", "node_modules/c"]),
        file("README.md", "readme\n"),
        dir("node_modules/a"),
        file("node_modules/a/LICENSE", "a\n"),
        dir("node_modules/a/node_modules/b"),
        file("node_modules/a/node_modules/b/LICENSE", new Uint8Array([0x62, 0xff, 0xfe, 0x0a])),
        dir("node_modules/c"),
        file("node_modules/c/LICENSE", "c with ünïcode and 😀\r\n")
      ],
      spdx: {}
    },
    {
      name: "only root",
      files: [lock([""]), file("LICENSE", BSD)],
      spdx: {}
    },
    {
      name: "nothing installed",
      files: [lock(["", "node_modules/a"])],
      spdx: { "BSD-3-Clause": spdxOk("unused\n") }
    }
  ];
}

const MODULE_NAMES = ["three", "Three", "fuse.js", "@mcap/core", "@msgpack/msgpack", "zstd", "a", "B", "c", "électron", "_x", "1x", "AdvantageScope", "advantagescope", "z", "Z"];
const LICENSE_FILE_NAMES = ["LICENSE", "LICENSE.md", "license", "License.txt", "LICENSE-MIT", "LICENSE.js", "license.json", "LICENSE-header", "LICENCE", "README"];
const SPDX_IDS = ["MIT", "ISC", "Apache-2.0", "(MIT OR Apache-2.0)", "BSD-3-Clause", "Unlicense", "Bogus"];

export function randomLicenseCases(count: number, seed: number): LicenseCase[] {
  const rng = new Rng(seed);
  const out: LicenseCase[] = [];
  for (let i = 0; i < count; i++) {
    const modules = [...new Set(Array.from({ length: rng.below(6) }, () => rng.pick(MODULE_NAMES)))];
    const files: LicenseEntry[] = [lock(["", ...modules.map((m) => "node_modules/" + m)])];
    if (rng.chance(0.8)) files.push(file("LICENSE", "root " + i + "\n"));
    for (const m of modules) {
      if (rng.chance(0.15)) continue;
      files.push(dir("node_modules/" + m));
      const n = rng.below(3);
      for (let k = 0; k < n; k++) {
        const name = rng.pick(LICENSE_FILE_NAMES);
        if (!files.some((f) => f.path === "node_modules/" + m + "/" + name)) files.push(file("node_modules/" + m + "/" + name, m + " " + name + "\n"));
      }
      if (rng.chance(0.7)) files.push(file("node_modules/" + m + "/package.json", JSON.stringify({ name: m, license: rng.pick(SPDX_IDS) })));
    }
    if (rng.chance(0.6)) {
      files.push(dir("licenses"));
      for (let k = rng.below(4); k > 0; k--) {
        const name = rng.pick(["Extra", "TBA", "Material", "a", "Z", "AdvantageScope"]) + rng.pick([".txt", ".txt", ".md"]);
        if (!files.some((f) => f.path === "licenses/" + name)) files.push(file("licenses/" + name, name + " text\n"));
      }
    }
    const spdx: { [id: string]: SpdxReply } = {};
    for (const id of ["MIT", "ISC", "Apache-2.0", "BSD-3-Clause", "Unlicense"]) {
      const roll = rng.below(10);
      spdx[id] = roll === 0 ? { reject: true } : roll === 1 ? { status: 404, body: utf8("404") } : spdxOk(id + " text\n");
    }
    out.push({ name: "random " + i, files, spdx });
  }
  return out;
}
