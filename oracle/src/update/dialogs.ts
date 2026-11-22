import { install as installGlobals, setNow } from "../fakes/globals";
import * as fakeFs from "../fakes/mainFs";
import * as electronFake from "../fakes/mainElectron";
import { toTree } from "../cve1";
import { buildMainBundle } from "./bundle";
import type { Fetch } from "./cases";

// Ground truth for P07's main-process flows: main.ts (with the real UpdateChecker) built by
// update/bundle.ts against the `files` fakes. Covered: the "About AdvantageScope" menu item, the
// beta block of a hub's dom-ready handler (expired alert, welcome window, survey prompt), the
// hub's "open-feedback" and "prompt-update" messages, the startup update check and the "Check
// for Updates..." menu item. Each case first sets the platform, packaging, clock, BETA_CONFIG
// fields and beta file ({op: "setup", args}), then runs ops, each recorded with what main did:
//   boxes      showMessageBox calls: {parent: window id or null, options}; `icon` is
//              "WINDOW_ICON" when it is ElectronConstants' WINDOW_ICON
//   quits      app.quit() calls
//   opened     shell.openExternal URLs
//   clipboard  clipboard.writeText texts
//   created    new windows: {id, parent, options} with the size/modality options main passed
//   posted     messages main posted on ports it created during the op
//   feedback   "show-feedback-button" values sent to the hub
//   update     "show-update-button" values sent to the hub
//   fetches    net.fetch calls: {url, init}; an op that fetches carries the scripted reply in
//              args.fetch ({reject} or {status, body})
//   betaFile   the beta state file's bytes afterwards (null when absent), betaWrites how many
//              times it was written
// BETA_CONFIG's `isAlpha` and `surveyUrl` are config values each release edits; cases set them
// on upstream's own object so betaUtil and main.ts run unchanged against every variant.

const BETA_NAME = "/fake/userData/beta-2027.json";
const SURVEY_URL = "https://forms.gle/asrs-survey";

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const flush = async (times = 12) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};

const boxes: unknown[] = [];
const fetches: unknown[] = [];
let scriptedFetch: Fetch | null = null;
const responses: number[] = [];
const quits: number[] = [];
const clipboard: string[] = [];
let windowIcon: unknown;

let caseIndex = -1;
let stepIndex = 0;

function describeOptions(options: any): unknown {
  const described: any = {};
  for (const key of Object.keys(options)) {
    described[key] = key === "icon" && options.icon === windowIcon ? "WINDOW_ICON" : options[key];
  }
  return toTree(described);
}

function install(constants: any) {
  windowIcon = constants.WINDOW_ICON;
  electronFake.dialog.showMessageBox = async (...args: any[]): Promise<any> => {
    const [parent, options] = args.length >= 2 ? [args[0], args[1]] : [null, args[0]];
    boxes.push({ parent: parent === null ? null : parent.id, options: describeOptions(options) });
    return { response: responses.shift() ?? 0, checkboxChecked: false };
  };
  (electronFake.app as any).quit = () => {
    quits.push(1);
  };
  electronFake.clipboard.writeText = (text: string) => {
    clipboard.push(text);
  };
  electronFake.net.fetch = (url: unknown, init: unknown): any => {
    fetches.push({ url: toTree(url), init: toTree(init) });
    const scripted = scriptedFetch;
    if (scripted === null) throw new Error("fetch without a scripted reply");
    if ("reject" in scripted) return Promise.reject(new TypeError("fetch failed"));
    return Promise.resolve(new Response(scripted.body, { status: scripted.status }));
  };
}

function describeFetch(fetch: Fetch): unknown {
  return "reject" in fetch ? { reject: true } : { status: fetch.status, body: fetch.body };
}

function hubPort(windowId: number): electronFake.FakePort | null {
  const window = electronFake.FakeBrowserWindow.fromId(windowId);
  const posted = window?.webContents.posted.filter((p) => p.channel === "port") ?? [];
  const port1 = posted[posted.length - 1]?.ports?.[0];
  return electronFake.channels.find((c) => c.port1 === port1)?.port2 ?? null;
}

async function record(op: string, args: unknown, run: () => unknown) {
  const r: any = { case: caseIndex, step: stepIndex++, op };
  if (args !== undefined) r.args = toTree(args);
  const channelsBefore = electronFake.channels.length;
  try {
    r.result = toTree(await run());
  } catch (error) {
    r.throw = (error as Error).name;
  }
  await flush();
  r.boxes = boxes.splice(0, boxes.length);
  r.quits = quits.splice(0, quits.length).length;
  r.opened = electronFake.externalUrls.splice(0, electronFake.externalUrls.length).map((u) => toTree(u));
  r.clipboard = clipboard.splice(0, clipboard.length);
  r.created = electronFake.takeCreated().map(({ id }) => {
    const options = electronFake.FakeBrowserWindow.fromId(id)?.options ?? {};
    return {
      id,
      parent: options.parent?.id ?? null,
      options: toTree({
        width: options.width,
        height: options.height,
        useContentSize: options.useContentSize,
        resizable: options.resizable,
        modal: options.modal,
        show: options.show
      })
    };
  });
  r.posted = electronFake.channels.slice(channelsBefore).flatMap((c) => c.port2.sent.map((m) => toTree(m)));
  const sent = electronFake.takeSent();
  r.feedback = sent.filter((m) => m?.name === "show-feedback-button").map((m) => toTree(m.data));
  r.update = sent.filter((m) => m?.name === "show-update-button").map((m) => toTree(m.data));
  r.fetches = fetches.splice(0, fetches.length);
  r.betaWrites = fakeFs.takeWrites().filter((name) => name === BETA_NAME).length;
  const file = fakeFs.dumpFiles()[BETA_NAME];
  r.betaFile = file === undefined ? null : toTree(file);
  responses.splice(0, responses.length);
  out(r);
  return r;
}

type Config = { isAlpha: boolean; surveyUrl: string | null };
type Setup = {
  platform: string;
  arch: string;
  packaged: boolean;
  now: number;
  config: Config;
  betaFile: string | null;
};

let betaConfig: any;

async function setup(s: Setup) {
  caseIndex++;
  stepIndex = 0;
  await record("setup", s, () => {
    Object.defineProperty(process, "platform", { value: s.platform, configurable: true });
    Object.defineProperty(process, "arch", { value: s.arch, configurable: true });
    electronFake.setPackaged(s.packaged);
    setNow(s.now);
    betaConfig.isAlpha = s.config.isAlpha;
    betaConfig.surveyUrl = s.config.surveyUrl;
    if (s.betaFile === null) fakeFs.removeFile(BETA_NAME);
    else fakeFs.putFile(BETA_NAME, new Uint8Array(Buffer.from(s.betaFile, "utf8")));
    return null;
  });
}

function clickAppMenu(label: string) {
  const template = electronFake.menuTemplate();
  if (template === null) throw new Error("no application menu");
  for (let i = 0; i < template.length; i++) {
    const submenu = (template[i] as any)?.submenu;
    if (!Array.isArray(submenu)) continue;
    const item = submenu.find((candidate: any) => candidate?.label === label);
    if (item !== undefined) {
      item.click.call(item, item, undefined);
      return;
    }
  }
  throw new Error(`no ${label} item`);
}

async function about(response: number) {
  await record("about", { response }, () => {
    responses.push(response);
    clickAppMenu("About AdvantageScope");
    return null;
  });
}

async function menuCheck(fetch: Fetch, response: number) {
  await record("menuCheck", { fetch: describeFetch(fetch), response }, () => {
    scriptedFetch = fetch;
    responses.push(response);
    clickAppMenu("Check for Updates...");
    return null;
  });
}

async function promptUpdate(hub: number, response: number) {
  await record("promptUpdate", { window: hub, response }, () => {
    responses.push(response);
    hubPort(hub)!.emit("message", { data: { name: "prompt-update", data: null } });
    return null;
  });
}

async function domReady(hub: number, response: number | null) {
  const r = await record("domReady", { window: hub, response }, () => {
    if (response !== null) responses.push(response);
    electronFake.FakeBrowserWindow.fromId(hub)!.webContents.emit("dom-ready");
    return null;
  });
  return r;
}

async function welcomeReady(id: number) {
  await record("welcomeReady", { window: id }, () => {
    electronFake.FakeBrowserWindow.fromId(id)!.webContents.emit("dom-ready");
    return null;
  });
}

async function welcomeDone(id: number) {
  await record("welcomeDone", { window: id }, () => {
    const port = hubPort(id);
    if (port === null) throw new Error("welcome window has no port");
    port.emit("message", { data: true });
    return null;
  });
}

async function welcomeClose(id: number) {
  await record("welcomeClose", { window: id }, () => {
    electronFake.FakeBrowserWindow.fromId(id)!.emit("close", { preventDefault: () => undefined });
    return null;
  });
}

async function feedback(hub: number) {
  await record("feedback", { window: hub }, () => {
    hubPort(hub)!.emit("message", { data: { name: "open-feedback", data: null } });
    return null;
  });
}

const CONFIGS: Config[] = [
  { isAlpha: true, surveyUrl: null },
  { isAlpha: false, surveyUrl: null },
  { isAlpha: true, surveyUrl: SURVEY_URL },
  { isAlpha: false, surveyUrl: SURVEY_URL }
];

const DAY = 24 * 3600 * 1000;
const T0 = Date.UTC(2026, 10, 21, 12);

const BETA_FILES: (string | null)[] = [
  null,
  "{}",
  '{"welcomeComplete":false,"surveyStatus":false}',
  '{"welcomeComplete":true,"surveyStatus":false}',
  '{"welcomeComplete":true,"surveyStatus":true}',
  `{"welcomeComplete":true,"surveyStatus":${T0 - DAY}}`,
  `{"welcomeComplete":true,"surveyStatus":${T0 + DAY}}`,
  `{"welcomeComplete":true,"surveyStatus":${T0}}`,
  '{"welcomeComplete":1,"surveyStatus":0}',
  '{"welcomeComplete":true,"surveyStatus":"soon"}',
  '{"welcomeComplete":true}',
  "{",
  "null"
];

// Local midnight of 2027-01-09 (BETA_CONFIG.expiration) falls at a different instant in each
// vector zone; the last instants straddle all four, and T0 + 30 days is past the point where a
// two-week survey date would still fall before expiry.
const NOWS = [
  T0,
  T0 + 30 * DAY,
  Date.UTC(2027, 0, 8, 10, 59, 59, 999),
  Date.UTC(2027, 0, 8, 11),
  Date.UTC(2027, 0, 8, 18, 30),
  Date.UTC(2027, 0, 9),
  Date.UTC(2027, 0, 9, 8),
  Date.UTC(2027, 0, 9, 8, 0, 0, 1)
];

const PLATFORMS: [string, string][] = [
  ["darwin", "arm64"],
  ["darwin", "x64"],
  ["linux", "x64"],
  ["linux", "arm64"],
  ["win32", "x64"],
  ["win32", "ia32"],
  ["win32", "arm64"]
];

const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));
const releases = (tag: string, published: string) =>
  utf8(
    JSON.stringify([
      {
        tag_name: tag,
        prerelease: true,
        published_at: published,
        assets: [{ name: "advantagescope-mac-arm64.dmg", browser_download_url: "https://example.com/mac-arm64.dmg" }]
      }
    ])
  );

export async function run() {
  // UpdateChecker logs every failed fetch.
  console.error = () => undefined;
  (globalThis as any).__asrsMain = { electron: electronFake, fs: fakeFs.default };
  installGlobals();
  electronFake.setPackaged(false);
  Object.defineProperty(process, "platform", { value: "darwin", configurable: true });
  Object.defineProperty(process, "arch", { value: "arm64", configurable: true });
  const mod = await import(await buildMainBundle());
  await mod.load();
  betaConfig = mod.BETA_CONFIG;
  install(mod.Constants);
  out({ op: "config", result: toTree({ userData: "/fake/userData", betaFile: BETA_NAME }) });

  caseIndex++;
  electronFake.setPackaged(true);
  setNow(T0);
  const startupFetch: Fetch = { status: 200, body: releases("v27.0.0-alpha-7", "2027-01-08T23:30:00Z") };
  await record("ready", { fetch: describeFetch(startupFetch) }, async () => {
    scriptedFetch = startupFetch;
    electronFake.fireReady();
    await flush(12);
    return null;
  });
  const hub = electronFake.windowList()[0]!.id;
  await promptUpdate(hub, 1);

  for (const [platform, arch] of PLATFORMS) {
    for (const packaged of [false, true]) {
      await setup({ platform, arch, packaged, now: T0, config: CONFIGS[0], betaFile: '{"welcomeComplete":true,"surveyStatus":true}' });
      await about(0);
      await about(1);
    }
  }

  for (const config of CONFIGS) {
    for (const betaFile of BETA_FILES) {
      for (const now of NOWS) {
        for (const response of [0, 1]) {
          const [platform, arch] = PLATFORMS[(caseIndex + response) % PLATFORMS.length];
          await setup({ platform, arch, packaged: true, now, config, betaFile });
          const r = await domReady(hub, response);
          const welcome = r.created.find((c: any) => c.parent === hub)?.id;
          if (welcome !== undefined) {
            await welcomeReady(welcome);
            if (response === 0) await welcomeDone(welcome);
            else await welcomeClose(welcome);
            if (electronFake.FakeBrowserWindow.fromId(welcome) !== null) electronFake.FakeBrowserWindow.fromId(welcome)!.destroy();
          }
        }
      }
    }
  }

  for (const config of CONFIGS) {
    await setup({ platform: "darwin", arch: "arm64", packaged: true, now: T0, config, betaFile: null });
    await feedback(hub);
  }

  const MENU_FETCHES: Fetch[] = [
    { status: 200, body: releases("v27.0.0-alpha-6", "2026-11-20T18:30:00Z") },
    { status: 200, body: releases("v27.0.0-alpha-8", "2027-01-09T00:30:00+13:00") },
    { reject: true },
    { status: 403, body: utf8('{"message":"API rate limit exceeded"}') },
    { status: 200, body: utf8("[]") }
  ];
  for (const [platform, arch] of [PLATFORMS[0], PLATFORMS[2], PLATFORMS[4]]) {
    for (const packaged of [true, false]) {
      for (const fetch of MENU_FETCHES) {
        for (const response of [0, 1, 2]) {
          await setup({ platform, arch, packaged, now: T0, config: CONFIGS[0], betaFile: '{"welcomeComplete":true,"surveyStatus":true}' });
          await menuCheck(fetch, response);
          await promptUpdate(hub, response);
          await domReady(hub, null);
        }
      }
    }
  }
}
