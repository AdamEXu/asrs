import "./environment";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

// What the user can insert into an en-US <input type="number">, in Blink in an offscreen Electron
// window. Each case is the text the editor shows, the selection, the inserted text, the text shown
// afterwards and the value then read. The texts are inserted with execCommand("insertText"), which
// takes the route of typing and pasting (BeforeTextInsertedEvent); the starting texts are built by
// insertions too, so every one is a text the filter lets through. The shown text is the control's
// selection after select().

function mulberry32(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const rng = mulberry32(263);
const below = (n: number) => Math.floor(rng() * n);
const NUMBER = "0123456789..eE+-+-";
const OTHER = " ,a$xI\t\n½٣−‒ー－＋．。｡，ｅＥ０５９\u{1d7d8}";
const others = [...OTHER];
const random = (length: number, other: number) => {
  let out = "";
  for (let i = 0; i < length; i++) out += rng() < other ? others[below(others.length)] : NUMBER[below(NUMBER.length)];
  return out;
};

// [start, text]: the caret fractions are resolved against the shown text in the page.
type Case = { build: string[]; from: number; to: number; insert: string };
const cases: Case[] = [];
const STARTS = [[], ["5"], ["-5"], ["+5"], ["1.5"], ["-1.5"], ["1e5"], ["1e-5"], ["-1.5e-5"], ["1.5e5"], ["."], ["-"], ["e"], ["1e"], ["1e+"], ["-."], ["5."]];
const INSERTS = ["1", ".", "-", "+", "e", "E", "12a3", "1.2.3", "-5", "--5", "1-2", "1e5e5", "1,234", " 12 ", "5\n6", ...others];
for (const build of STARTS) {
  const length = build.join("").length;
  for (const insert of INSERTS) {
    for (let at = 0; at <= length; at++) cases.push({ build, from: at, to: at, insert });
    if (length > 0) cases.push({ build, from: 0, to: length, insert });
  }
}
for (let i = 0; i < 4000; i++) {
  const build = [random(1 + below(6), 0), random(below(4), 0.1)];
  const [a, b] = [rng(), rng()];
  const caret = rng() < 0.6;
  cases.push({ build, from: -a, to: caret ? -a : -b, insert: random(1 + below(6), 0.25) });
}

const PAGE = `((cases) => {
  const input = document.querySelector("input");
  const selection = getSelection();
  const out = [];
  const shown = () => { input.select(); return selection.toString(); };
  for (const c of cases) {
    input.value = "";
    input.focus();
    for (const text of c.build) if (text !== "") document.execCommand("insertText", false, text);
    const before = shown();
    const resolve = (at) => at < 0 ? Math.min(before.length, Math.floor(-at * (before.length + 1))) : at;
    const ends = [resolve(c.from), resolve(c.to)].sort((x, y) => x - y);
    if (ends[1] > before.length) throw new Error("caret outside " + JSON.stringify(before));
    selection.modify("move", "backward", "lineboundary");
    for (let i = 0; i < ends[0]; i++) selection.modify("move", "forward", "character");
    for (let i = ends[0]; i < ends[1]; i++) selection.modify("extend", "forward", "character");
    if (selection.toString() !== before.slice(ends[0], ends[1])) throw new Error("selected " + JSON.stringify(selection.toString()) + " in " + JSON.stringify(before) + " " + ends);
    document.execCommand("insertText", false, c.insert);
    const value = input.value;
    out.push({ shown: before, start: ends[0], end: ends[1], insert: c.insert, result: shown(), value });
  }
  return out;
})`;

const ELECTRON_MAIN = `
import { app, BrowserWindow } from "electron";
import fs from "node:fs";
const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
app.setPath("userData", request.userData);
app.dock?.hide();
app.on("window-all-closed", () => {});
app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, width: 400, height: 100, webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false } });
  await win.loadURL("data:text/html,<!doctype html><input type=number>");
  win.webContents.focus();
  const records = await win.webContents.executeJavaScript(request.page + "(" + JSON.stringify(request.cases) + ")", true);
  win.destroy();
  fs.writeFileSync(request.output, JSON.stringify(records));
  app.exit(0);
}).catch((e) => {
  process.stderr.write("genNumberInsert main: " + (e && e.stack ? e.stack : e) + "\\n");
  app.exit(1);
});
`;

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-number-insert-"));
try {
  const requestPath = path.join(scratch, "request.json");
  const mainPath = path.join(scratch, "main.mjs");
  const output = path.join(scratch, "records.json");
  fs.writeFileSync(mainPath, ELECTRON_MAIN);
  fs.writeFileSync(requestPath, JSON.stringify({ cases, page: PAGE, userData: path.join(scratch, "userData"), output }));
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
  const run = spawnSync(process.execPath, ["--lang=en-US", "--log-level=3", mainPath, requestPath], {
    env,
    encoding: "utf8",
    stdio: ["ignore", "inherit", "inherit"]
  });
  if (run.status !== 0) {
    process.stderr.write(`genNumberInsert: Electron exited with ${run.status ?? run.signal}\n`);
    process.exit(1);
  }
  const records: object[] = JSON.parse(fs.readFileSync(output, "utf8"));
  process.stdout.write(records.map((record) => JSON.stringify(record) + "\n").join(""));
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
