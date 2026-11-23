import "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import v8 from "node:v8";
import Field2dController_Config from "@upstream/hub/controllers/Field2dController_Config";
import Field3dController_Config from "@upstream/hub/controllers/Field3dController_Config";
import { LineGraphController_DiscreteConfig, LineGraphController_NumericConfig } from "@upstream/hub/controllers/LineGraphController_Config";
import MechanismController_Config from "@upstream/hub/controllers/MechanismController_Config";
import PointsController_Config from "@upstream/hub/controllers/PointsController_Config";
import StatisticsController_Config from "@upstream/hub/controllers/StatisticsController_Config";
import SwerveController_Config from "@upstream/hub/controllers/SwerveController_Config";
import { Rng } from "./corpus/rng";
import { toTree } from "./cve1";
import { buildLicensesPage } from "./info/bundle";
import { UPSTREAM_DIR } from "./shims";

// Ground truth for the windows without a hub renderer behind them (asrs_tabs::info and
// asrs_app::views::info): upstream's real www/<page>.html in an offscreen Electron window (Blink).
//
//   genInfoWindows licenses
//     src/licenses.ts over lists of texts ({page, case, data: [{module, text}], ...}).
//   genInfoWindows sourceListHelp
//     the rolldown bundle, sent main's "set-config" as openSourceListHelp sends it.
//   genInfoWindows betaWelcome
//     the rolldown bundle, sent isAlpha() as openBetaWelcome sends it, then real input over the
//     DevTools protocol.
//
// A "dump" record is the whole body after the page settled: [tag, class, box, colour, extra,
// children] per element, with client boxes, and [text, line boxes] per text node. `extra` is the
// element's own state: hidden, an input's checked, a button's disabled, an object's data and its
// SVG's colour. `layout: false` cases leave the boxes out. A "step" record is the state of
// the beta page after one scripted step, with the replies it posted on its port.

const [mode, ...rest] = process.argv.slice(2);
if (rest.length > 0 || !["licenses", "sourceListHelp", "betaWelcome"].includes(mode)) {
  process.stderr.write("usage: genInfoWindows licenses|sourceListHelp|betaWelcome\n");
  process.exit(1);
}

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

interface Case {
  name: string;
  layout: boolean;
  width: number;
  height: number;
  scale: number;
  dark: boolean;
  /** The licenses list, or the messages main posts. */
  data: any;
  steps: string[];
}

const sizes = (widths: number[], height: number): Pick<Case, "width" | "height" | "scale">[] =>
  widths.flatMap((width) => [1, 1.5, 2].map((scale) => ({ width, height, scale })));

function licenseCases(): Case[] {
  const cases: Case[] = [];
  const add = (name: string, layout: boolean, data: [string, string][], at: Pick<Case, "width" | "height" | "scale">) =>
    cases.push({ name, layout, ...at, dark: false, data: data.map(([module, text]) => ({ module, text })), steps: [] });
  const one = { width: 600, height: 597, scale: 1 };

  const texts = [
    "", " ", "  ", "\n", "\n\n", "\n\n\n", "\n\n\n\n", "\t", "\r\n", "\r\n\r\n", "\r", "\r\r\n\n", "a", " a", "  a", "a ", "a  ", " a b ", "a  b   c    d",
    "a\nb", "a\n\nb", "a\n\n\nb", "a\n\n\n\nb", "a\n \nb", "a \n\n b", "a\n\n  b", "\n\na", " \n\na", "a\n\n", "a\n\n ", "a\n \n\n b", "a\n>b", "a\n> b", "a\n>\n> b", "a\n\n> b", "a\n>\n>\n>",
    "a\n-b", "a\n- b\n- c", "a\n\n- b", "a\n--\nb", "a -b", "a\n -b", "a\n\t-b", "a\tb", "a\t\tb", "\ta", "a\r\nb\r\n\r\nc", "a\rb", "a\n\rb", "a\r\n>b", "a\r\n-b",
    "a<br>b", "a<br> b", "a<br>  b", "<br>", "<br><br>", "a<br/>b", "a <br>\n\n<br> b", "a\n\n<br>b", "&amp;<b>x</b>", "a > b", "> a", "- a", " a  b ", "a b c", "a​b", "café 丝刀 👋", "👋\n\n👋", "a\fb", "a\vb", "a\u0085b", "﻿a", "a\u0000b", "\ud800a\udc00", "$&\n\n$1$$"
  ];
  add("texts", false, texts.map((t, i) => [`module ${i}`, t]), one);
  const modules = ["AdvantageScope", "AdvantageScope Lite", "", " ", "  a  b ", "a\nb", "a\r\nb", "a\rb", "a\n", "\n", "a\tb", "<b>a</b>", "&amp;", "@scope/name", "café 👋", "a<br>b"];
  add("modules", false, modules.map((m) => [m, "text"]), one);
  add("empty", false, [], one);

  const rng = new Rng(22, "licenses");
  const pieces = ["a", "b", "word", " ", " ", "  ", "\t", "\n", "\n", "\n\n", "\r\n", "\r\n\r\n", "\r", ">", "-", "\n>", "\n-", "<br>", "<br> ", "<", "br>", " ", "丝", "👋"];
  add("random", false, Array.from({ length: 600 }, (_, i) => [`random ${i}`, Array.from({ length: rng.range(1, 24) }, () => rng.pick(pieces)).join("")]), one);

  const read = (file: string) => fs.readFileSync(path.join(UPSTREAM_DIR, file), "utf8");
  const real: [string, string][] = [["AdvantageScope", read("LICENSE")]];
  for (const name of ["ACME Robotics", "Google Material Symbols", "The Blue Alliance"]) real.push([name, read(`licenses/${name}.txt`)]);
  for (const file of ["@mcap/core/LICENSE", "bignumber.js/LICENCE.md", "camera-controls/LICENSE", "chart.js/LICENSE.md", "mathjs/LICENSE", "mathjs/NOTICE", "protobufjs/LICENSE", "simple-statistics/LICENSE", "ssh2/LICENSE", "three/LICENSE", "lzma-native/LICENSE", "fuse.js/LICENSE", "argparse/LICENSE", "typescript/LICENSE.txt", "caniuse-lite/LICENSE"]) {
    const full = path.join(UPSTREAM_DIR, "node_modules", file);
    if (fs.existsSync(full)) real.push([file.split("/").slice(0, -1).join("/"), read(`node_modules/${file}`)]);
    else throw new Error(`no ${file}`);
  }
  for (const at of sizes([600, 731, 1200], 597)) add(`real ${at.width} ${at.scale}x`, true, real, at);

  const words = ["the", "Software", "WARRANTIES", "copyright-holder", "(c)", "http://example.com/a/very/long/path/that/does/not/fit?query=string&more=parameters#fragment-identifier", "\"AS-IS\"", "and/or", "2.0.", "A", "To", "Va", "WA", "i", "fi", "3-clause", "-", "--", "—", "“Licensor”", "©", "Straße", "x.y.z", "[name]", "{year}", "50%", "$5", "don't", "'q'", "a:b", "e.g.,", " ", "Björn", "1,000", "#1", "*", "***", "=====", "a=b", "~/x", "C++", "<you@example.com>"];
  const random: [string, string][] = Array.from({ length: 40 }, (_, i) => [`Random ${words[i % words.length]} ${i}`, Array.from({ length: rng.range(1, 160) }, () => rng.pick(words)).join(rng.pick([" ", " ", "\n", " \n"]))]);
  random.push(["wide", "supercalifragilisticexpialidocious".repeat(6) + " after"], ["spaces", "a " + "b".repeat(80) + " c"], ["@electron/a-module-name-that-does-not-fit-on-one-line-of-the-window/with/a-path", "x"], ["cjk", "丝刀口 and text after it that wraps over several lines of the page because it is long enough to do so twice, at least"]);
  for (const at of sizes([600, 731], 597)) add(`words ${at.width} ${at.scale}x`, true, random, at);
  return cases;
}

const HELP_CONFIGS: [string, any][] = [
  ["numeric", LineGraphController_NumericConfig],
  ["discrete", LineGraphController_DiscreteConfig],
  ["field2d", Field2dController_Config],
  ["field3d", Field3dController_Config],
  ["mechanism", MechanismController_Config],
  ["points", PointsController_Config],
  ["statistics", StatisticsController_Config],
  ["swerve", SwerveController_Config]
];

function syntheticConfig(): any {
  const type = (key: string, extra: any = {}) => ({
    key,
    display: `Type ${key}`,
    symbol: "circle",
    showInTypeName: false,
    color: "#123456",
    sourceTypes: ["Number"],
    showDocs: true,
    options: [],
    ...extra
  });
  const option = (key: string, display: string, values: [string, string][]) => ({
    key,
    display,
    showInTypeName: false,
    values: values.map(([k, d]) => ({ key: k, display: d }))
  });
  const colours: [string, string][] = [["#ff0000", "Red"], ["#ffff00", "Yellow"], ["#000000", "Black"], ["#ffffff", "White"]];
  return {
    title: "Synthetic",
    autoAdvance: true,
    allowChildrenFromDrag: false,
    types: [
      type("a", { sourceTypes: [] }),
      type("ab", { sourceTypes: ["NumberArray", "struct:Pose2d", "Pose2d[]"], showDocs: false }),
      type("abc", { sourceTypes: ["proto:Thing", "BooleanArray", "StringArray", "Raw", "Empty", "Boolean", "String"], childOf: "parents", darkColor: "#654321" }),
      type("parent", { parentKey: "parents", display: "Parent One" }),
      type("parent2", { parentKey: "parents", display: "Parent Two", symbol: "target" }),
      type("parent2again", { parentKey: "parents", display: "Parent Two", showDocs: false }),
      type("parent3", { parentKey: "parents", display: "Parent Three", childOf: "nobody" }),
      type("orphan", { childOf: "parents2", sourceTypes: ["a:b:c", "Mechanism2d"] }),
      type("onlyChild", { childOf: "single" }),
      type("singleParent", { parentKey: "single", display: "Single" }),
      type("colour", { color: "color", options: [option("color", "Color", colours), option("size", "Size", [["bold", "Bold"]]), option("none", "None", []), option("two", "Two", [["x", "One"], ["y", "Other"]])] }),
      type("colour2", { color: "color", options: [option("color", "Color", colours)] }),
      type("missing", { color: "nothing", options: [option("color", "Color", colours)] }),
      type("long", {
        display: "A title that is much too long for the header of a window this narrow, so it ends early",
        sourceTypes: ["A type name that is long", "another", "Number", "a-hyphenated-type-name", "x".repeat(60)],
        options: [option("long", "A long option name", [["a", "Value with several words"], ["b", "y".repeat(70)], ["#00ff00", "Green and more words"], ["c", "Last"]])]
      })
    ]
  };
}

function helpCases(): Case[] {
  const cases: Case[] = [];
  const add = (name: string, data: any[], at: Pick<Case, "width" | "height" | "scale">, dark = false) => cases.push({ name, layout: true, ...at, dark, data, steps: [] });
  const config = (data: any) => ({ name: "set-config", data });
  for (const [name, value] of [...HELP_CONFIGS, ["synthetic", syntheticConfig()] as [string, any]]) {
    for (const at of sizes([275, 350, 600], 472)) add(`${name} ${at.width} ${at.scale}x`, [config(value)], at);
    add(`${name} dark`, [config(value)], { width: 350, height: 472, scale: 1 }, true);
  }
  const at = { width: 350, height: 472, scale: 1 };
  const numeric = LineGraphController_NumericConfig;
  add("no message", [], at);
  add("other messages", [{ name: "set-preferences", data: { theme: "dark" } }, config(numeric), { name: "set-focused", data: true }], at);
  add("two configs", [config(numeric), config(MechanismController_Config)], at);
  const exhausted = syntheticConfig();
  exhausted.types = Array.from({ length: 6 }, (_, i) => ({ ...exhausted.types[10], key: `t${i}`, display: `T${i}` }));
  add("colours run out", [config(exhausted)], at);
  const empty = syntheticConfig();
  empty.types = [];
  add("no types", [config(empty)], at);
  const entities = syntheticConfig();
  entities.types = [{ ...entities.types[10], options: [{ key: "x", display: "A &amp; B", values: [{ key: "a", display: "R&D" }, { key: "b", display: "&lt;b&gt; &#x1F44B; &#65;&quot;&nbsp;&copy;" }, { key: "c", display: "1 < 2 & 3 > 2 &unknown; &" }] }] }];
  add("entities", [config(entities)], at);
  return cases;
}

function betaCases(): Case[] {
  const cases: Case[] = [];
  for (const alpha of [true, false]) {
    for (const at of sizes([450], 490)) cases.push({ name: `${alpha ? "alpha" : "beta"} ${at.scale}x`, layout: true, ...at, dark: false, data: [alpha], steps: [] });
  }
  const one = { width: 450, height: 490, scale: 1 };
  cases.push({ name: "no message", layout: true, ...one, dark: true, data: [], steps: [] });
  const flow = (name: string, steps: string[], data: any[] = [true]) => cases.push({ name, layout: false, ...one, dark: false, data, steps });
  const all = ["click 0", "click 1", "click 2"];
  flow("clicks", [...all, "click button"]);
  flow("other order", ["click 2", "click 0", "click 1", "click button"], [false]);
  flow("disabled button", ["click button", "click 0", "click button", "click 1", "click button", "click 2", "click button"]);
  flow("unchecking", [...all, "click 1", "click button", "click 1", "click button"]);
  flow("unchecking all", [...all, ...all, "click button"]);
  flow("two clicks", [...all, "click button", "click button"]);
  flow("tab order", Array.from({ length: 6 }, () => "key Tab"));
  flow("tab order enabled", [...all, ...Array.from({ length: 6 }, () => "key Tab")]);
  flow("tab order back", ["key Shift+Tab", "key Shift+Tab", ...all, "key Tab", "key Shift+Tab", "key Shift+Tab"]);
  flow("space", ["key Tab", "key Space", "key Tab", "key Space", "key Tab", "key Space", "key Tab", "key Space"]);
  flow("space twice", ["key Tab", "key Space", "key Space", "key Space"]);
  flow("enter", ["key Tab", "key Enter", "key Space", "key Enter", "key Tab", "key Space", "key Tab", "key Space", "key Tab", "key Enter"]);
  flow("numpad enter", [...all, "key Tab", "key NumpadEnter"]);
  flow("shift enter", [...all, "key Tab", "key Shift+Enter"]);
  flow("escape", [...all, "key Escape", "key Enter", "key Space"]);
  flow("keys without focus", ["key Enter", "key Space", "key Escape"]);
  flow("clicks before any message", [...all, "click button"], []);
  flow("focus lost", ["key Tab", "click text", "key Space", "key Tab", "key Space"]);
  flow("disabling the focused button", [...all, "key Tab", "click 2", "key Enter", "key Space", "key Tab"]);
  return cases;
}

const STATE_SCRIPT = `(() => {
  const inputs = Array.from(document.getElementsByTagName("input"));
  const button = document.getElementsByTagName("button")[0];
  const active = document.activeElement;
  const rec = window.__rec;
  return {
    checked: inputs.map((i) => i.checked),
    disabled: button.disabled,
    active: active === null || active === document.body ? null : active === button ? "button" : inputs.indexOf(active),
    replies: rec.replies.splice(0),
    errors: rec.errors.splice(0)
  };
})()`;

const DUMP_SCRIPT = `((layout) => {
  const box = (r) => layout ? [r.x, r.y, r.width, r.height] : [];
  const range = document.createRange();
  const extra = (el) => {
    switch (el.tagName) {
      case "INPUT": return { checked: el.checked, disabled: el.disabled };
      case "BUTTON": return { disabled: el.disabled, title: el.title };
      case "OBJECT": {
        const svg = el.contentDocument === null ? undefined : el.contentDocument.getElementsByTagName("svg")[0];
        return { data: el.getAttribute("data"), type: el.type, color: svg === undefined ? null : svg.style.color };
      }
      default: return {};
    }
  };
  const visit = (node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      range.selectNodeContents(node);
      return [node.data, Array.from(range.getClientRects(), box)];
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return null;
    const el = node;
    return [el.tagName.toLowerCase(), el.className, box(el.getBoundingClientRect()), getComputedStyle(el).color, { hidden: el.hidden, ...extra(el) }, Array.from(el.childNodes, visit).filter((n) => n !== null)];
  };
  return {
    title: document.title,
    scroll: [document.documentElement.scrollWidth, document.documentElement.scrollHeight],
    body: visit(document.body),
    errors: window.__rec.errors.splice(0)
  };
})`;

const ELECTRON_MAIN = `
import { app, BrowserWindow, nativeTheme, net, protocol } from "electron";
import fs from "node:fs";
import path from "node:path";
import v8 from "node:v8";
import { fileURLToPath } from "node:url";
const request = v8.deserialize(fs.readFileSync(process.argv[process.argv.length - 1]));
const KEYS = {
  Enter: { code: "Enter", keyCode: 13, text: "\\r" },
  NumpadEnter: { key: "Enter", code: "NumpadEnter", keyCode: 13, text: "\\r" },
  Tab: { code: "Tab", keyCode: 9 },
  Escape: { code: "Escape", keyCode: 27 },
  Space: { key: " ", code: "Space", keyCode: 32, text: " " }
};
app.setPath("userData", request.userData);
app.dock?.hide();
app.on("window-all-closed", () => {});
let current = null;
app.whenReady().then(async () => {
  const root = path.dirname(request.www);
  protocol.handle("file", (req) => {
    const file = fs.realpathSync(fileURLToPath(req.url));
    if (!file.startsWith(root + path.sep)) return new Response("", { status: 404 });
    if (request.licensesBundle !== null && file === path.join(root, "bundles", "licenses.js")) {
      const source = "globalThis.__asrsLicenses = " + JSON.stringify(current.data) + ";\\n" + fs.readFileSync(request.licensesBundle, "utf8");
      return new Response(source, { headers: { "content-type": "text/javascript" } });
    }
    return net.fetch(req, { bypassCustomProtocolHandlers: true });
  });
  const records = [];
  const win = new BrowserWindow({ show: false, width: 600, height: 600, useContentSize: true, webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false } });
  await win.loadURL("about:blank");
  win.webContents.debugger.attach("1.3");
  const send = (method, params) => win.webContents.debugger.sendCommand(method, params);
  const js = (code) => win.webContents.executeJavaScript(code, true);
  const settle = () => js("document.fonts.ready.then(() => new Promise((resolve) => setTimeout(resolve, 0))).then(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))))");
  await send("Page.enable");
  await send("Page.addScriptToEvaluateOnNewDocument", { source: "window.__rec = { replies: [], errors: [] }; window.addEventListener('error', (event) => { const e = event.error; if (e instanceof Error) window.__rec.errors.push({ name: e.name, message: e.message }); else if (!(event.target instanceof Element)) window.__rec.errors.push({ name: null, message: String(event.message) }); });" });
  const press = async (name) => {
    const parts = name.split("+");
    const key = parts.pop();
    const modifiers = parts.reduce((bits, part) => bits | { Shift: 8 }[part], 0);
    const k = KEYS[key];
    const down = { key: k.key ?? key, code: k.code, windowsVirtualKeyCode: k.keyCode, nativeVirtualKeyCode: k.keyCode, modifiers };
    await send("Input.dispatchKeyEvent", k.text === undefined ? { type: "rawKeyDown", ...down } : { type: "keyDown", text: k.text, ...down });
    await send("Input.dispatchKeyEvent", { type: "keyUp", ...down });
  };
  const apply = async (step) => {
    const [op, what] = step.split(" ");
    if (op === "key") return press(what);
    const el = what === "button" ? "document.getElementsByTagName('button')[0]" : what === "text" ? "document.getElementsByTagName('p')[0]" : "document.getElementsByTagName('input')[" + what + "]";
    const [x, y] = await js("(() => { const r = " + el + ".getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()");
    const mouse = { x, y, button: "left", clickCount: 1 };
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await send("Input.dispatchMouseEvent", { type: "mousePressed", ...mouse });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", ...mouse });
  };
  for (current of request.cases) {
    nativeTheme.themeSource = current.dark ? "dark" : "light";
    win.setContentSize(current.width, current.height);
    await win.loadFile(path.join(request.www, request.page + ".html"));
    win.webContents.focus();
    if (request.licensesBundle === null) {
      await js("(() => { const channel = new MessageChannel(); channel.port2.onmessage = (event) => { window.__rec.replies.push(event.data); }; window.__port = channel.port2; window.postMessage('port', '*', [channel.port1]); })()");
      await settle();
      for (const post of current.data) await js("window.__port.postMessage(" + JSON.stringify(post) + ")");
    }
    await settle();
    await js("Promise.all(Array.from(document.querySelectorAll('object'), (o) => o.contentDocument !== null && o.contentDocument.querySelector('svg') !== null ? null : new Promise((r) => o.addEventListener('load', r))))");
    await settle();
    const [width, height, ratio] = await js("[innerWidth, innerHeight, devicePixelRatio]");
    if (width !== current.width || height !== current.height || ratio !== current.scale) throw new Error(current.name + ": the viewport is " + [width, height, ratio]);
    records.push({ kind: "dump", case: current.name, state: await js(request.dump + "(" + current.layout + ")") });
    for (const step of current.steps) {
      await apply(step);
      await settle();
      records.push({ kind: "step", case: current.name, step, state: await js(request.state) });
    }
    records.push({ kind: "end", case: current.name });
  }
  win.destroy();
  fs.writeFileSync(request.output, v8.serialize(records));
  app.exit(0);
}).catch((e) => {
  process.stderr.write("genInfoWindows main: " + (e && e.stack ? e.stack : e) + "\\n");
  app.exit(1);
});
`;

async function main() {
  const cases = mode === "licenses" ? licenseCases() : mode === "sourceListHelp" ? helpCases() : betaCases();
  const byName = new Map(cases.map((c) => [c.name, c]));
  if (byName.size !== cases.length) throw new Error("duplicate case names");
  const licensesBundle = mode === "licenses" ? await buildLicensesPage() : null;
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-info-"));
  try {
    const requestPath = path.join(scratch, "request.bin");
    const mainPath = path.join(scratch, "main.mjs");
    const output = path.join(scratch, "records.bin");
    fs.writeFileSync(mainPath, ELECTRON_MAIN);
    const www = fs.realpathSync(path.join(UPSTREAM_DIR, "www"));
    const request = { page: mode, licensesBundle, dump: DUMP_SCRIPT, state: STATE_SCRIPT, www, userData: path.join(scratch, "userData"), output };
    const env: NodeJS.ProcessEnv = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
    const recorded = new Map<string, any[]>();
    for (const scale of new Set(cases.map((c) => c.scale))) {
      fs.writeFileSync(requestPath, v8.serialize({ ...request, cases: cases.filter((c) => c.scale === scale) }));
      const switches = ["--lang=en-US", "--log-level=3", `--force-device-scale-factor=${scale}`];
      const run = spawnSync(process.execPath, [...switches, mainPath, requestPath], { env, encoding: "utf8", stdio: ["ignore", "inherit", "inherit"] });
      if (run.status !== 0) throw new Error(`Electron exited with ${run.status ?? run.signal}`);
      for (const r of v8.deserialize(fs.readFileSync(output))) recorded.set(r.case, [...(recorded.get(r.case) ?? []), r]);
    }
    for (const r of cases.flatMap((c) => recorded.get(c.name)!)) {
      if (r.kind === "dump") {
        const { layout, width, height, scale, dark, data } = byName.get(r.case)!;
        out({ kind: "dump", case: r.case, layout, width, height, scale, dark, data: toTree(data), state: toTree(r.state) });
      } else if (r.kind === "step") out({ kind: "step", case: r.case, step: r.step, state: toTree(r.state) });
      else out({ kind: "end", case: r.case });
    }
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

main().catch((e) => {
  process.stderr.write(`genInfoWindows: ${e.stack ?? e}\n`);
  process.exit(1);
});
