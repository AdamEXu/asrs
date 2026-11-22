import { install as installGlobals } from "./fakes/globals";
import * as fakeFs from "./fakes/mainFs";
import * as electronFake from "./fakes/mainElectron";
import { toTree } from "./cve1";
import { UPSTREAM_DIR } from "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Ground truth for asrs-app's menu model: the application menu template upstream main.ts builds
// in setupMenu, unmodified, against the fake `electron` of the `files` harness, and the same
// template after main.ts's sendAllPreferences rewrites it in place.
//
//   genMenus template <darwin|win32|linux> <fresh|prefs>
//     fresh   no preferences file; then three `update-preferences` hub messages
//     prefs   a preferences file that sets the live mode and Systemcore address before startup
//   genMenus electron
//     the darwin `fresh` template handed to Electron's real Menu.buildFromTemplate in an Electron
//     main process (genMenusMain.mjs), which fills in role defaults; records Electron's items

const [mode, ...rest] = process.argv.slice(2);
const usage = () => {
  console.error("usage: genMenus template <darwin|win32|linux> <fresh|prefs> | genMenus electron");
  process.exit(2);
};
let platform: string;
let scenario: string;
if (mode === "template") {
  [platform, scenario] = rest;
  if (!["darwin", "win32", "linux"].includes(platform) || !["fresh", "prefs"].includes(scenario)) usage();
} else if (mode === "electron") {
  [platform, scenario] = ["darwin", "fresh"];
} else {
  usage();
}

installGlobals();
Object.defineProperty(process, "platform", { value: platform! });

const Constants = await import("@upstream/main/electron/ElectronConstants");
const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const flush = async (times = 12) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};

if (scenario! === "prefs") {
  const prefs = { liveMode: "rlog", systemcoreStaticAddress: "usb" };
  const text = JSON.stringify(prefs);
  out({ op: "putPrefs", file: Constants.PREFS_FILENAME, text });
  fakeFs.putFile(Constants.PREFS_FILENAME, new Uint8Array(Buffer.from(text, "utf8")));
}

await import("@upstream/main/electron/main");
electronFake.fireReady();
await flush();

const template = () => {
  const items = electronFake.menuTemplate();
  if (items === null) throw new Error("main.ts set no application menu");
  return items;
};

function hubPort(): electronFake.FakePort {
  for (const window of electronFake.windowList()) {
    for (const posted of window.webContents?.posted ?? []) {
      if (posted.channel !== "port") continue;
      const channel = electronFake.channels.find((c) => c.port1 === posted.ports?.[0]);
      if (channel !== undefined) return channel.port2;
    }
  }
  throw new Error("no hub window port");
}

if (mode === "template") {
  out({ op: "config", platform: platform!, scenario: scenario! });
  out({ op: "template", step: 0, template: toTree(template()) });
  if (scenario! === "fresh") {
    const updates = [
      { liveMode: "phoenix", userAssetsFolder: "/fake/custom-assets", systemcoreStaticAddress: "wifi" },
      { liveMode: "ftcdashboard", systemcoreStaticAddress: "usb" },
      { liveMode: "nt4", systemcoreStaticAddress: "" }
    ];
    const port = hubPort();
    for (let i = 0; i < updates.length; i++) {
      port.emit("message", { data: { name: "update-preferences", data: updates[i] } });
      await flush();
      out({ op: "template", step: i + 1, update: toTree(updates[i]), template: toTree(template()) });
    }
  }
  process.exit(0);
}

const CLICK = "__genMenusClick";
const plain = (value: any): any => {
  if (typeof value === "function") return { [CLICK]: value.name };
  if (Array.isArray(value)) return value.map(plain);
  if (value !== null && typeof value === "object") {
    const copy: any = {};
    for (const key of Object.keys(value)) if (value[key] !== undefined) copy[key] = plain(value[key]);
    return copy;
  }
  return value;
};

const upstreamPackage = JSON.parse(fs.readFileSync(path.join(UPSTREAM_DIR, "package.json"), "utf8"));
const here = path.dirname(fileURLToPath(import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-menus-"));
let run;
try {
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(
    requestPath,
    JSON.stringify({ appName: upstreamPackage.productName, click: CLICK, template: plain(template()), userData: path.join(scratch, "userData") })
  );
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  run = spawnSync(process.execPath, ["--lang=en-US", "--log-level=3", path.join(here, "genMenusMain.mjs"), requestPath], {
    env,
    encoding: "utf8",
    maxBuffer: 1 << 26,
    stdio: ["ignore", "pipe", "inherit"]
  });
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
if (run.status !== 0) {
  process.stderr.write(`genMenus: Electron exited with ${run.status ?? run.signal}\n`);
  process.exit(1);
}
out({ op: "config", platform: "darwin", scenario: "electron" });
process.stdout.write(run.stdout, () => process.exit(0));
