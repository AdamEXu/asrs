import "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import v8 from "node:v8";
import { DEFAULT_PREFS } from "@upstream/shared/Preferences";
import { toTree } from "./cve1";
import { UPSTREAM_DIR } from "./shims";

// Ground truth for the download window (asrs_services::download::page and
// asrs_app::views::download): upstream's real www/download.html and its rolldown bundle in an
// offscreen Electron window (Blink), driven the way main.ts's openDownload drives it: a "port"
// message carrying a MessageChannel port, then main's messages on it. The user's steps are real
// input sent over the DevTools protocol. "resize" gives the window another content height,
// "clock" sets the page's Date, and "scroll" sets the list's scrollTop. After the open and after
// every step it records the rows, the alert, the progress bar and its text, the list's overflow
// and scroll position, the focused element, the buttons' "blurred" class, and every message posted
// on the port, alert and uncaught error since the previous record. The "loading" records hold each
// bar's vertical scale with its animation paused at a time, and where the bars are.

const WIDTH = 500;
const HEIGHT = 472;

type File = { name: string; size: number | undefined; isFolder: boolean };
type Step = { op: "post" | "row" | "click" | "key" | "resize" | "clock" | "scroll"; name?: string; data?: any; index?: number; shift?: boolean; value?: any };
type Flow = { name: string; platform: string; steps: Step[] };

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const post = (name: string, data: any): Step => ({ op: "post", name, data });
const list = (files: File[]): Step => post("set-list", files);
const row = (index: number, shift = false): Step => ({ op: "row", index, shift });
const click = (name: string): Step => ({ op: "click", name });
const key = (value: string): Step => ({ op: "key", value });
const resize = (value: number): Step => ({ op: "resize", value });
const clock = (value: number): Step => ({ op: "clock", value });
const scroll = (value: number): Step => ({ op: "scroll", value });
const file = (name: string, size: number | undefined = 1e6): File => ({ name, size, isFolder: false });
const folder = (name: string, size: number | undefined = 1e6): File => ({ name, size, isFolder: true });
const prefs = (fields: object = {}): Step => post("set-preferences", { ...DEFAULT_PREFS, robotAddress: "10.63.28.2", ...fields });
const logs = (count: number, prefix = "log"): File[] => Array.from({ length: count }, (_, i) => file(`${prefix}${String(i).padStart(2, "0")}.wpilog`, i * 333333));

const EXTENSIONS = ["wpilog", "rlog", "wpilogxz", "revlog", "hoot"];
const MESSAGES = [
  "No such file",
  "no such file",
  "Timed out while waiting for handshake",
  "Fetch failed",
  "All configured authentication methods failed",
  "Not connected",
  "Error while looking up IPv4 address for 'robot.local': Error: getaddrinfo ENOTFOUND robot.local",
  "Error while looking up IPv4 address for 'robot.local': Error: getaddrinfo EAI_AGAIN robot.local",
  "Socket is closed",
  "connect ECONNREFUSED 10.63.28.2:22",
  "ENOENT: no such file or directory, open '/Users/a/R&D <x>/log.wpilog'",
  "",
  "a  b\t c\n d ",
  "<b>&amp;</b> &lt;u&gt; &#39;&#x41;&quot;&apos;&nbsp;x",
  "1 < 2 && 3 > 2",
  "a <3 b </ c>d <!-- e -->f <?g>h"
];
const ALERTS = [
  "No new logs found.",
  "Saved 1 log to <u>/Users/a/logs</u>",
  "Saved 2 new logs (1 skipped) to <u>/Users/a/R&D  logs</u>",
  "Saved 3 logs to <u>C:\\Users\\a\\x</u>",
  "Saved 3 logs to <u>/Users/a/<b>x</b></u>",
  "Saved 3 logs to <u>/Users/a/</u></u> b",
  "x <U>y</U> <u >z</u > <u/>w"
];

function flows(): Flow[] {
  const result: Flow[] = [];
  const add = (name: string, steps: Step[], platform = "darwin") => result.push({ name, platform, steps });
  const opened = (...steps: Step[]): Step[] => [prefs(), ...steps];

  for (const platform of ["darwin", "win32", "linux", "freebsd"]) {
    const names = [...EXTENSIONS.map((e) => `a.${e}`), "a.b.rlog", "noext", "TRAILING.", ".wpilog", "a.WPILOG"];
    add(`icons ${platform}`, opened(list([...names.map((n) => file(n)), folder("folder"), folder("folder.wpilog")])), platform);
    add(`addresses ${platform}`, [
      ...["10.00.028.02", "robot.local", "", "172.22.11.2", "0.0"].map((robotAddress) => prefs({ robotAddress })),
      ...["usb", "wifi"].map((systemcoreStaticAddress) => prefs({ systemcoreStaticAddress, remotePath: "/media/sda1/logs" }))
    ], platform);
  }
  add("no preferences", [post("show-error", "No such file"), post("show-error", "Fetch failed"), list(logs(2))]);

  const sizes = [0, 1, 99999, 99999.9, 1e5, 149999, 150000, 949999, 950000, 999999, 1e6, 1049999, 1050000, 1.5e6, 12345678, 99949999, 99950000, 123456789, 1e9, 1e12, 1e15, 1e21, 1e26, 1e27, -1, -1e6, NaN, Infinity, undefined];
  for (let i = 0; i < sizes.length; i += 10) {
    add(`sizes ${i}`, opened(list(sizes.slice(i, i + 10).map((size, j) => ({ name: `s${String(j).padStart(2, "0")}.wpilog`, size, isFolder: j % 3 === 0 })))));
  }

  const sortNames = [
    "FRC_20250301_120000_Q1.wpilog", "FRC_20250301_120000_Q10.wpilog", "FRC_20250301_120000_Q2.wpilog", "FRC_TBD_1234abcd.wpilog", "FRC_TBD_99.wpilog", "TBD",
    "Log_1234abcd.wpilog", "Log_25-03-01_12-00-00.wpilog", "Log_25-03-01_12-00-00_q1.wpilog", "akit_1234abcd.wpilog", "akit_25-03-01.wpilog", "Akit_1.wpilog", "log_1.wpilog",
    "a.rlog", "A.rlog", "á.rlog", "b.rlog", "B.rlog", "10.hoot", "9.hoot", "_x.hoot", "-x.hoot", "x y.hoot", "x_y.hoot", "x-y.hoot", "日本.revlog", "é.revlog", "e\u0301.revlog", "Z", "z", "ß.rlog", "ss.rlog",
    "rio_0001.hoot", "CANivore_0001.hoot", "REV_20250101_000000.revlog", "x.wpilog", "x.wpilogxz", "xTBDy.rlog", "Log_.rlog", "Log_-.rlog"
  ];
  add("sort all", opened(list(sortNames.map((n) => file(n)))));
  add("sort all reversed", opened(list([...sortNames].reverse().map((n) => file(n)))));
  const r = random(20261123);
  const pick = <T>(values: T[]): T => values[Math.floor(r() * values.length)];
  for (let i = 0; i < 150; i++) {
    const count = 1 + Math.floor(r() * 30);
    const files = Array.from({ length: count }, () => {
      const name = r() < 0.5 ? pick(sortNames) : pick(["Log_", "akit_", "", "FRC_", "TBD"]) + pick(["", "a", "A", "-", "1", "02", "é", " "]) + pick(["b", "B", "_", "10", "9"]) + "." + pick(EXTENSIONS);
      return { name, size: pick(sizes.slice(0, 20)), isFolder: r() < 0.15 };
    });
    add(`sort ${i}`, opened(list(files)), pick(["darwin", "win32", "linux"]));
  }

  MESSAGES.forEach((message, i) => {
    add(`error ${i}`, opened(post("show-error", message), list(logs(1)), post("show-alert", message)));
    add(`error ${i} preferences`, [prefs({ robotAddress: "10.<b>.01", remotePath: "/a b/<c>&\"d'" }), list(logs(3)), post("show-error", message)]);
  });
  ALERTS.forEach((message, i) => add(`alert ${i}`, opened(list(logs(1)), post("show-alert", message), list(logs(2)))));
  add("transitions", opened(
    post("show-alert", "a"), list(logs(3)), post("set-progress", 0), list(logs(3)), post("set-progress", 1), post("show-error", "Not connected"), list(logs(2)), list(logs(2)),
    post("set-progress", 0), post("show-alert", "b"), post("show-error", "Not connected"), post("show-alert", "c"), list(logs(2)), post("show-error", "x"), post("set-progress", 0), list(logs(2)), post("show-error", "y"), post("show-error", "z")
  ));

  const transfer = (current: any, total: any): Step => post("set-progress", { current, total });
  add("progress", opened(
    clock(1000), post("set-progress", 0), transfer(0, 12345678), transfer(32768, 12345678), clock(1400), transfer(2e6, 12345678), clock(1500), transfer(2e6, 12345678), clock(1501), transfer(1e6, 12345678), transfer(1000001, 12345678),
    clock(2000), transfer(5e6, 12345678), clock(61500), transfer(6e6, 12345678), clock(6000000), transfer(6000001, 1e15), transfer(12345678, 12345678), post("set-progress", 1), post("set-progress", 0), transfer(2e6, 4e6), post("show-alert", "x"), transfer(2e6, 4e6),
    clock(6010000), transfer(2e6, 4e6), post("show-error", "x"), transfer(2e6, 4e6), clock(6005000), transfer(3e6, 4e6)
  ));
  const odd: [any, any][] = [[0, 0], [5, 0], [0, NaN], [5e6, NaN], [NaN, 5e6], [5e6, 4e6], [-5, 10], [5, -10], [5e6, Infinity], [Infinity, 5e6], [0, undefined], [2e6, undefined], [1e21, 1e27], [undefined, 5]];
  add("odd progress", opened(clock(0), transfer(1, 2), clock(10000), ...odd.flatMap(([current, total]) => [transfer(current, total), transfer(3, 4)])));
  for (let i = 0; i < 12; i++) {
    let now = Math.floor(r() * 1e6);
    let current = 0;
    const total = Math.floor(r() * 10 ** (5 + r() * 6));
    const steps: Step[] = [clock(now), post("set-progress", 0)];
    for (let j = 0; j < 20; j++) {
      now += Math.floor(r() * 10 ** (1 + r() * 4));
      current = Math.min(total, current + Math.floor((r() * total) / 10));
      steps.push(clock(now), transfer(current, total));
    }
    steps.push(post("set-progress", 1));
    add(`progress ${i}`, opened(...steps));
  }

  add("select", opened(list(logs(6)), row(1), row(3), row(1), row(1), click("download"), list(logs(8)), row(0), click("download"), list(logs(2)), click("download")));
  add("shift without a click", opened(list(logs(6)), row(2, true), row(4, true), row(4, true), row(1, true), click("download")));
  add("shift ranges", opened(list(logs(8)), row(1), row(4, true), row(6, true), row(0, true), row(3), row(5, true), row(0, true), click("download")));
  add("shift over a mix", opened(list(logs(8)), row(2), row(5), row(0, true), row(5), row(7, true), click("download")));
  add("shift past a shorter list", opened(list(logs(8)), row(7), list(logs(3)), row(1, true), click("download"), row(0, true), key("Meta+a"), key("Meta+a"), click("download")));
  add("shift past a shorter list deselects", opened(list(logs(8)), row(7), row(0, true), row(7), list(logs(3)), row(1, true), click("download")));
  add("shift past every row", opened(list(logs(30)), scroll(1000), row(29), list(logs(3)), row(1, true), click("download"), row(1, true), row(2), click("download")));
  add("empty list", opened(list([]), click("download"), key("Meta+a"), click("download"), key("Enter")));
  add("duplicate names", opened(list([file("a.wpilog"), file("a.wpilog"), file("b.wpilog")]), row(0), row(1), row(1), key("Meta+a"), click("download"), row(2), row(2), click("download")));
  add("select all", opened(list(logs(5)), key("Meta+a"), click("download"), key("Meta+a"), click("download"), row(2), key("Meta+a"), click("download"), list(logs(6)), key("Meta+a"), key("Meta+a"), click("download")));
  add("select all by platform", opened(list(logs(2)), key("Control+a"), key("a"), key("Alt+a"), key("Shift+Meta+A"), key("Meta+b"), key("Control+Meta+a"), click("download")));
  for (const platform of ["win32", "linux"]) {
    add(`select all on ${platform}`, opened(list(logs(2)), key("Meta+a"), click("download"), key("Control+a"), click("download"), key("Control+Meta+a"), click("download")), platform);
  }
  add("select all then an error", opened(list(logs(3)), row(1), post("show-error", "x"), click("download"), key("Meta+a"), click("download"), key("Meta+a"), list(logs(3)), key("Meta+a"), post("show-error", "x"), list(logs(3))));
  add("buttons while loading", [click("download"), key("Enter"), click("exit"), prefs(), click("exit")]);

  add("keys", opened(list(logs(3)), key("Enter"), key("NumpadEnter"), key("Escape"), key(" "), row(0), key("Enter"), key("Shift+Enter"), key("Control+Enter"), key("NumpadEnter"), key(" ")));
  add("tab order", opened(list(logs(3)), key("Tab"), key("Tab"), key("Shift+Tab"), key("Tab")));
  add("tab order of a scrolling list", opened(list(logs(40)), key("Tab"), key("Tab"), key("Tab"), key("Shift+Tab"), key("Shift+Tab"), key("Tab")));
  add("click on a scrolling list", opened(list(logs(40)), row(2), key("Tab"), key("Tab"), key("Shift+Tab"), key("Shift+Tab")));
  add("tab from a button to a list that starts scrolling", opened(list(logs(3)), key("Tab"), list(logs(40)), key("Shift+Tab"), key("Tab"), click("download"), key("Shift+Tab"), key("Shift+Tab")));
  add("a focused list stops scrolling", opened(list(logs(40)), key("Tab"), list(logs(3)), key("Tab")));
  add("a focused list and an error", opened(list(logs(40)), key("Tab"), post("show-error", "x"), key("Tab")));
  for (const [button, tabs] of [["exit", 1], ["download", 2]] as [string, number][]) {
    for (const press of ["Enter", "NumpadEnter", " ", "Shift+Enter"]) {
      add(`${press} on ${button}`, opened(list(logs(3)), row(1), ...Array.from({ length: tabs }, () => key("Tab")), key(press)));
      add(`${press} on ${button} with nothing selected`, opened(list(logs(3)), ...Array.from({ length: tabs }, () => key("Tab")), key(press)));
    }
  }
  add("keys after a button click", opened(list(logs(3)), row(1), click("download"), key("Enter"), key(" "), row(0), key("Enter"), click("exit"), key("Enter")));
  add("focus", opened(list(logs(3)), post("set-focused", false), post("set-focused", false), post("set-focused", true)));

  for (const count of [0, 1, 15, 16, 17, 18, 40]) {
    add(`filler ${count}`, [resize(500), prefs(), list(logs(count)), ...[472, 473, 471, 455, 454, 453, 452, 451, 446, 430, 500, 900, 100, 80, 60, 472].map(resize), post("show-error", "x"), resize(600), list(logs(count))]);
  }
  add("scroll and refresh", opened(list(logs(40)), scroll(300), list(logs(40)), scroll(10000), list(logs(38)), resize(700), list(logs(20)), list(logs(40)), scroll(50), post("show-error", "x"), list(logs(40)), scroll(60), resize(2000), resize(472)));

  for (let i = 0; i < 60; i++) {
    let count = 3 + Math.floor(r() * 12);
    const platform = pick(["darwin", "darwin", "win32", "linux"]);
    const all = platform === "darwin" ? "Meta+a" : "Control+a";
    const steps: Step[] = [prefs(), list(logs(count))];
    let shown = true;
    for (let j = 0; j < 25; j++) {
      const roll = r();
      if (roll < 0.35 && shown && count > 0) steps.push(row(Math.floor(r() * count)));
      else if (roll < 0.65 && shown && count > 0) steps.push(row(Math.floor(r() * count), true));
      else if (roll < 0.75) {
        count = Math.floor(r() * 16);
        shown = true;
        steps.push(list(logs(count, pick(["log", "log", "other"]))));
      } else if (roll < 0.82) steps.push(key(all));
      else if (roll < 0.86) {
        steps.push(post("show-error", pick(MESSAGES)));
        shown = false;
      } else if (roll < 0.9) steps.push(key("Enter"));
      else if (roll < 0.94) steps.push(resize(150 + Math.floor(r() * 500)), resize(HEIGHT));
      else steps.push(click("download"));
    }
    add(`walk ${i}`, steps, platform);
  }
  return result;
}

const SETUP_SCRIPT = `(() => {
  window.__rec = { sent: [], alerts: [], errors: [] };
  window.__now = 0;
  const RealDate = Date;
  window.Date = class extends RealDate {
    constructor(...args) { if (args.length === 0) super(window.__now); else super(...args); }
  };
  window.alert = (message) => { window.__rec.alerts.push(String(message)); };
  window.addEventListener("error", (event) => {
    const e = event.error;
    window.__rec.errors.push(e instanceof Error ? { name: e.name, message: e.message } : { name: null, message: String(event.message) });
  });
  const channel = new MessageChannel();
  channel.port2.onmessage = (event) => { window.__rec.sent.push(event.data); };
  window.__port = channel.port2;
  window.postMessage("port", "*", [channel.port1]);
})()`;

const STATE_SCRIPT = `(() => {
  const rec = window.__rec;
  const list = document.querySelector("div.file-list");
  const alert = document.querySelector("div.alert-text");
  const progress = document.querySelector("progress");
  const details = document.querySelector("div.progress-details");
  const rows = Array.from(list.children[0].children).map((el) => {
    const selected = el.classList.contains("selected");
    if (el.childElementCount === 0) return { selected };
    const [img, name, size] = el.children;
    const src = img.getAttribute("src");
    return { selected, name: name.textContent, size: size.textContent, src: src === null ? null : src.replace("../icons/download/", ""), imgClass: img.className };
  });
  const active = document.activeElement;
  return {
    rows,
    loading: !list.children[1].hidden,
    overflow: list.style.overflow || "hidden",
    scrollTop: list.scrollTop,
    alert: alert.hidden ? null : { text: alert.innerText, underlined: Array.from(alert.querySelectorAll("u"), (u) => u.innerText) },
    progress: progress.hidden ? null : { value: progress.value, details: details.hidden ? null : details.innerText },
    active: active === null || active === document.body ? null : active.id !== "" ? active.id : active.className,
    blurred: Array.from(document.getElementsByTagName("button"), (b) => b.classList.contains("blurred")),
    sent: rec.sent.splice(0),
    alerts: rec.alerts.splice(0),
    errors: rec.errors.splice(0)
  };
})()`;

const LOADING_SCRIPT = `(() => {
  const bars = Array.from(document.querySelectorAll(".loading-bar"));
  const animations = bars.map((bar) => bar.getAnimations()[0]);
  const frames = [];
  for (let time = 0; time <= 2500; time += 7) {
    for (const animation of animations) {
      animation.pause();
      animation.currentTime = time;
    }
    frames.push({ time, scales: bars.map((bar) => { const t = getComputedStyle(bar).transform; return t === "none" ? 1 : new DOMMatrix(t).d; }) });
  }
  for (const animation of animations) animation.cancel();
  const rect = (el) => { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; };
  return { frames, container: rect(document.querySelector("div.loading")), bars: bars.map(rect) };
})()`;

const ELECTRON_MAIN = `
import { app, BrowserWindow, net, protocol } from "electron";
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
  " ": { code: "Space", keyCode: 32, text: " " }
};
app.setPath("userData", request.userData);
app.dock?.hide();
app.on("window-all-closed", () => {});
app.whenReady().then(async () => {
  const root = path.dirname(path.dirname(request.www));
  protocol.handle("file", (req) => {
    const file = fileURLToPath(req.url);
    if (!fs.existsSync(file) || !fs.realpathSync(file).startsWith(root + path.sep)) return new Response("", { status: 404 });
    return net.fetch(req, { bypassCustomProtocolHandlers: true });
  });
  const records = [];
  const win = new BrowserWindow({ show: false, width: request.width, height: request.height, useContentSize: true, webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false } });
  win.webContents.debugger.attach("1.3");
  const send = (method, params) => win.webContents.debugger.sendCommand(method, params);
  const js = (code) => win.webContents.executeJavaScript(code, true);
  const settle = () => js("new Promise((resolve) => setTimeout(resolve, 0)).then(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))))");
  const height = async (value) => {
    win.setContentSize(request.width, value);
    await js("new Promise((resolve) => { const check = () => (window.innerHeight === " + value + " ? requestAnimationFrame(resolve) : setTimeout(check, 1)); check(); })");
  };
  const press = async (name) => {
    const parts = name.length > 1 ? name.split("+") : [name];
    const key = parts.pop();
    const modifiers = parts.reduce((bits, part) => bits | { Alt: 1, Control: 2, Meta: 4, Shift: 8 }[part], 0);
    const letter = /^[a-zA-Z]$/.test(key) ? { key, code: "Key" + key.toUpperCase(), keyCode: key.toUpperCase().charCodeAt(0) } : undefined;
    const k = KEYS[key] ?? letter;
    if (k === undefined) throw new Error("unknown key " + name);
    const text = (modifiers & 6) === 0 ? k.text ?? (letter && key) : undefined;
    const down = { key: k.key ?? key, code: k.code, windowsVirtualKeyCode: k.keyCode, nativeVirtualKeyCode: k.keyCode, modifiers };
    await send("Input.dispatchKeyEvent", text === undefined && k.text === undefined ? { type: "rawKeyDown", ...down } : { type: "keyDown", text: text ?? k.text, ...down });
    await send("Input.dispatchKeyEvent", { type: "keyUp", ...down });
  };
  const apply = async (step) => {
    switch (step.op) {
      case "post":
        await js("window.__port.postMessage({ name: " + JSON.stringify(step.name) + ", data: " + step.literal + " })");
        break;
      case "row":
      case "click": {
        const el = step.op === "row" ? "document.querySelector('div.file-list').children[0].children[" + step.index + "]" : "document.getElementById(" + JSON.stringify(step.name) + ")";
        const [x, y, hit] = await js("(() => { const el = " + el + "; const r = el.getBoundingClientRect(); const x = r.x + r.width / 2, y = r.y + r.height / 2; const at = document.elementFromPoint(x, y); return [x, y, at !== null && el.contains(at)]; })()");
        if (!hit) throw new Error("nothing to click");
        const mouse = { x, y, button: "left", clickCount: 1, modifiers: step.shift ? 8 : 0 };
        await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, modifiers: mouse.modifiers });
        await send("Input.dispatchMouseEvent", { type: "mousePressed", ...mouse });
        await send("Input.dispatchMouseEvent", { type: "mouseReleased", ...mouse });
        break;
      }
      case "key":
        await press(step.value);
        break;
      case "resize":
        await height(step.value);
        break;
      case "clock":
        await js("window.__now = " + step.value);
        break;
      case "scroll":
        await js("document.querySelector('div.file-list').scrollTop = " + step.value);
        break;
    }
  };
  await win.loadFile(path.join(request.www, "download.html"));
  await height(request.height);
  records.push({ kind: "loading", ...(await js(request.loading)) });
  for (const flow of request.flows) {
    win.setContentSize(request.width, request.height);
    await win.loadFile(path.join(request.www, "download.html"));
    win.webContents.focus();
    await height(request.height);
    await js(request.setup);
    await settle();
    await js("window.__port.postMessage({ name: 'set-platform', data: " + JSON.stringify(flow.platform) + " })");
    await settle();
    records.push({ kind: "open", flow: flow.name, state: await js(request.state) });
    for (let i = 0; i < flow.steps.length; i++) {
      try {
        await apply(flow.steps[i]);
      } catch (e) {
        throw new Error(flow.name + " step " + i + " " + JSON.stringify(flow.steps[i]) + ": " + e.message);
      }
      await settle();
      records.push({ kind: "step", flow: flow.name, index: i, state: await js(request.state) });
    }
  }
  win.destroy();
  fs.writeFileSync(request.output, v8.serialize(records));
  app.exit(0);
}).catch((e) => {
  process.stderr.write("genDownloadPage main: " + (e && e.stack ? e.stack : e) + "\\n");
  app.exit(1);
});
`;

const jsLiteral = (value: any): string => {
  if (typeof value === "number") return Object.is(value, -0) ? "-0" : String(value);
  if (Array.isArray(value)) return "[" + value.map(jsLiteral).join(",") + "]";
  if (value === null || typeof value !== "object") return value === undefined ? "undefined" : JSON.stringify(value);
  return "{" + Object.keys(value).map((k) => JSON.stringify(k) + ":" + jsLiteral(value[k])).join(",") + "}";
};

const all = flows();
const byName = new Map(all.map((f) => [f.name, f]));
if (byName.size !== all.length) {
  process.stderr.write("genDownloadPage: duplicate flow names\n");
  process.exit(1);
}
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-download-"));
try {
  const requestPath = path.join(scratch, "request.bin");
  const mainPath = path.join(scratch, "main.mjs");
  const output = path.join(scratch, "records.bin");
  fs.writeFileSync(mainPath, ELECTRON_MAIN);
  fs.writeFileSync(
    requestPath,
    v8.serialize({
      flows: all.map((f) => ({ ...f, steps: f.steps.map((s) => ({ ...s, literal: s.op === "post" ? jsLiteral(s.data) : undefined })) })),
      width: WIDTH,
      height: HEIGHT,
      setup: SETUP_SCRIPT,
      state: STATE_SCRIPT,
      loading: LOADING_SCRIPT,
      www: fs.realpathSync(path.join(UPSTREAM_DIR, "www")),
      userData: path.join(scratch, "userData"),
      output
    })
  );
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  const run = spawnSync(process.execPath, ["--lang=en-US", "--log-level=3", mainPath, requestPath], {
    env,
    encoding: "utf8",
    stdio: ["ignore", "inherit", "inherit"]
  });
  if (run.status !== 0) {
    process.stderr.write(`genDownloadPage: Electron exited with ${run.status ?? run.signal}\n`);
    process.exit(1);
  }
  for (const r of v8.deserialize(fs.readFileSync(output)) as any[]) {
    if (r.kind === "loading") {
      for (const { time, scales } of r.frames) out({ kind: "loading", time, scales: toTree(scales) });
      out({ kind: "bars", container: toTree(r.container), bars: toTree(r.bars) });
      continue;
    }
    const flow = byName.get(r.flow)!;
    if (r.kind === "open") out({ kind: "open", flow: r.flow, platform: flow.platform, steps: flow.steps.length, state: toTree(r.state) });
    else out({ kind: "step", flow: r.flow, step: toTree(flow.steps[r.index]), state: toTree(r.state) });
  }
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
