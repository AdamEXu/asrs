import "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import v8 from "node:v8";
import jsonfile from "jsonfile";
import { DEFAULT_PREFS } from "@upstream/shared/Preferences";
import { toTree } from "./cve1";
import { UPSTREAM_DIR } from "./shims";

const args = process.argv.slice(2);
if (args.length > 1 || (args.length === 1 && args[0] !== "preferences")) {
  process.stderr.write("usage: genPrompts [preferences]\n");
  process.exit(1);
}
const preferences = args.length === 1;

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

// Ground truth for the prompt windows (asrs_tabs::prompts and asrs_app::views::prompts):
// upstream's real www/<page>.html and its rolldown bundle in an offscreen Electron window (Blink),
// driven the way main.ts's create*Window drives it: a "port" message carrying a MessageChannel
// port, then the messages main posts on it. The scripted user steps are real input sent over the
// DevTools protocol (typed characters, keys, clicks at an element's centre), so focus changes and
// "change" events come in Chromium's order; "insert" is execCommand("insertText"), which
// inserts a text at once and leaves the caret after it as pasting does, without the clipboard
// (the protocol's Input.insertText counts the caret from the unfiltered text); "select" is script focus with the whole text selected,
// "pick" is what choosing an option in a select's native popup does (the new value, then "input"
// and "change"), and "post" is another message from main. After the open and after every step it
// records every control, the focused element, the buttons' "blurred" class, and every reply posted
// on the port, alert and uncaught error since the previous record. A flow ends at the reply after
// which main.ts destroys the window, which is the first one except for export.html's help URL.
// The "preferences" set is preferences.html alone: its records also hold each option's "hidden"
// and each input's placeholder, and an end record the bytes prefs.json is left with when jsonfile
// writes each reply, as openPreferences does (Enter on a button posts two).

interface Step {
  op: "select" | "text" | "insert" | "key" | "click" | "pick" | "post";
  id?: string;
  value?: any;
}
interface Flow {
  name: string;
  page: string;
  posts: any[];
  steps: Step[];
}

const select = (id: string): Step => ({ op: "select", id });
const text = (value: string): Step => ({ op: "text", value });
const insert = (value: string): Step => ({ op: "insert", value });
const key = (value: string): Step => ({ op: "key", value });
const click = (id: string): Step => ({ op: "click", id });
const pick = (id: string, value: string): Step => ({ op: "pick", id, value });
const focused = (isFocused: boolean): Step => ({ op: "post", value: { isFocused } });

function flows(): Flow[] {
  const list: Flow[] = [];
  const add = (page: string, name: string, posts: any[], steps: Step[]) => list.push({ page, name: `${page} ${name}`, posts, steps });

  const numberTexts = ["5", "-5", "0", "-0", "1.5", ".5", "5.", "1e3", "1E-2", "1e", "-", "+5", "e", "", "00012", "1.0000000001", "9007199254740993", "1e400", "1e-400", "1a2", "1 2", "0x10", "-5.", "+", ".", "-.", "+.5", "-.5", "+5.", "+1e3", "1e+3", "5.5e2", "-1e-2", "+-5", "-+5", "1.2.3", "1..2", "--5", "1-2", "1e2e3", "1e2.5", "1e-5-2", "1ee2", "1e+-2", "e5", "5.e3", "1e5.", "1,000", "- 5", "\u00bd", "\uff15"];
  const inserts = ["12", "1.2.3", "12a3", " 12 ", "1,234", "-5", "--5", "1-2", "1e5", "1e5e5", "abc", "5\n6", "\t7", "$5", "\u22125", "0x10", "Infinity"];

  const ranges: [number, number][] = [
    [-10, 10],
    [0, 1],
    [0, 0],
    [1, 1 + 1e-9],
    [0.1 + 0.2, 1 / 3 + 1],
    [-1e21, 1e21],
    [1e-7, 2e-7],
    [-0, 5],
    [-5, -0],
    [123456.7891234, 123456.7891236],
    [1e300, 1e308],
    [5, -5],
    [NaN, 1],
    [0, Infinity],
    [-Infinity, 0]
  ];
  ranges.forEach((range, i) => {
    add("editRange", `confirm ${i}`, [range], [click("confirm")]);
    add("editRange", `exit ${i}`, [range], [click("exit")]);
  });
  add("editRange", "enter", [[-10, 10]], [key("Enter")]);
  add("editRange", "shift enter", [[-10, 10]], [key("Shift+Enter")]);
  add("editRange", "other keys", [[-10, 10]], [key("Escape"), key("ArrowLeft"), key("Backspace"), key("Enter")]);
  add("editRange", "tab order", [[2, 8]], [key("Tab"), key("Tab"), key("Tab"), key("Shift+Tab"), key("Shift+Tab"), key("Shift+Tab"), key("Enter")]);
  add("editRange", "space on confirm", [[2, 8]], [key("Tab"), key("Tab"), key("Tab"), key(" ")]);
  add("editRange", "space on exit", [[2, 8]], [text("9"), key("Tab"), key("Tab"), key(" ")]);
  add("editRange", "type over max", [[-10, 10]], [text("25.5"), click("confirm")]);
  add("editRange", "type both", [[-10, 10]], [text("3"), select("min"), text("-3.25"), key("Enter")]);
  add("editRange", "typed then exit", [[0.1 + 0.2, 10]], [text("3"), select("min"), text("1"), click("exit")]);
  add("editRange", "max below min", [[-10, 10]], [text("-20"), key("Enter"), text("5"), click("confirm"), select("max"), text("5"), click("confirm")]);
  add("editRange", "equal alerts", [[3, 3]], [click("confirm"), key("Enter"), select("min"), text("2"), key("Enter")]);
  add("editRange", "cleared min", [[2, 8]], [select("min"), key("Backspace"), click("confirm")]);
  add("editRange", "cleared max", [[-8, -2]], [key("Backspace"), key("Enter")]);
  add("editRange", "focus messages", [[2, 8]], [focused(false), focused(false), focused(true), focused(false), key("Enter")]);
  numberTexts.forEach((t, i) => {
    add("editRange", `max ${JSON.stringify(t)}`, [[-100, 100]], [text(t), i % 2 === 0 ? key("Enter") : click("confirm")]);
    add("editRange", `min ${JSON.stringify(t)}`, [[-100, 1e308]], [select("min"), text(t), i % 2 === 0 ? click("confirm") : key("Enter")]);
    add("editRange", `min below max ${JSON.stringify(t)}`, [[-100, 1e12]], [select("min"), text(t), i % 2 === 0 ? key("Enter") : click("confirm")]);
  });
  add("export", "insert in the prefixes", [{ supportsAkit: true }], [select("prefixes"), insert("/A,\n/B\n"), click("confirm")]);
  inserts.forEach((t, i) => {
    const confirm = i % 2 === 0 ? click("confirm") : key("Enter");
    add("editRange", `insert ${JSON.stringify(t)}`, [[-100, 1e12]], [select("min"), insert(t), confirm]);
    add("editRange", `insert at the start ${JSON.stringify(t)}`, [[-100.5, 1e12]], [select("min"), key("ArrowLeft"), insert(t), confirm]);
    add("editRange", `insert after the point ${JSON.stringify(t)}`, [[-1.5, 1e12]], [select("min"), key("ArrowRight"), key("ArrowLeft"), insert(t), confirm]);
    add("editRange", `insert at the end ${JSON.stringify(t)}`, [[-0.25, 1e12]], [select("min"), key("ArrowRight"), insert(t), confirm]);
  });
  add("editRange", "insert beside the same digits", [[11, 1e12]], [select("min"), key("ArrowLeft"), insert("1a"), text("2"), click("confirm")]);
  add("editRange", "insert over the same text", [[100, 1e12]], [select("min"), insert("100a"), text("5"), click("confirm")]);
  add("editRange", "insert then type", [[-100, 1e12]], [select("min"), insert("1.2.3"), text(".4"), key("ArrowLeft"), text("."), text("-"), text("5"), key("Backspace"), key("Enter")]);
  add("editRange", "enter on exit", [[2, 8]], [key("Tab"), key("Tab"), key("Enter")]);
  add("editRange", "enter on confirm", [[2, 8]], [key("Tab"), key("Tab"), key("Tab"), key("Enter")]);
  add("editRange", "enter on exit alerts", [[3, 3]], [key("Tab"), key("Tab"), key("Enter")]);
  add("editRange", "enter on confirm alerts", [[3, 3]], [key("Tab"), key("Tab"), key("Tab"), key("Enter"), key(" "), click("exit")]);
  add("editRange", "numpad enter", [[2, 8]], [text("9"), key("NumpadEnter"), key("Enter")]);
  add("editRange", "numpad enter on exit", [[2, 8]], [text("9"), key("Tab"), key("Tab"), key("NumpadEnter")]);
  add("editRange", "numpad enter on confirm", [[2, 8]], [text("9"), key("Tab"), key("Tab"), key("Tab"), key("NumpadEnter")]);

  for (const fov of [50, 0, -0, 1.5, 0.1 + 0.2, 1e21, 1e-7, 1 / 3, NaN, Infinity]) {
    add("editFov", `confirm ${String(Object.is(fov, -0) ? "-0" : fov)}`, [fov], [click("confirm")]);
    add("editFov", `exit ${String(Object.is(fov, -0) ? "-0" : fov)}`, [fov], [click("exit")]);
  }
  add("editFov", "enter", [50], [key("Enter")]);
  add("editFov", "typed enter", [50], [text("75.25"), key("Enter")]);
  add("editFov", "typed exit", [50], [text("75.25"), click("exit")]);
  add("editFov", "cleared", [50], [key("Backspace"), click("confirm")]);
  add("editFov", "tab order", [50], [key("Tab"), key("Tab"), key("Shift+Tab"), key(" ")]);
  add("editFov", "focus messages", [50], [focused(false), focused(true), click("confirm")]);
  numberTexts.forEach((t, i) => add("editFov", `text ${JSON.stringify(t)}`, [60], [text(t), i % 2 === 0 ? click("confirm") : key("Enter")]));

  add("editFov", "numpad enter", [50], [text("75"), key("NumpadEnter"), key("NumpadEnter"), click("exit")]);

  const names = ["Line Graph", "", " ", "  padded  ", "café 日本", "a\nb\r\nc", "📈 Graph", "x".repeat(60), "<b>&amp;</b>", "\"quoted\""];
  for (const name of names) {
    add("renameTab", `confirm ${JSON.stringify(name)}`, [name], [click("confirm")]);
    add("renameTab", `exit ${JSON.stringify(name)}`, [name], [click("exit")]);
  }
  add("renameTab", "enter", ["Line Graph"], [key("Enter")]);
  add("renameTab", "type over", ["Line Graph"], [text("My Tab"), key("Enter")]);
  add("renameTab", "append", ["Line Graph"], [key("ArrowRight"), text(" 2"), click("confirm")]);
  add("renameTab", "typed exit", ["Line Graph"], [text("My Tab"), click("exit")]);
  add("renameTab", "clear", ["Line Graph"], [key("Backspace"), key("Enter")]);
  add("renameTab", "tab order", ["Line Graph"], [key("Tab"), key("Tab"), key("Shift+Tab"), key("Shift+Tab"), text("Z"), key("Enter")]);
  add("renameTab", "other keys", ["Line Graph"], [key("Escape"), key("ArrowLeft"), text("!"), key("Enter")]);
  add("renameTab", "focus messages", ["Line Graph"], [focused(false), focused(true), click("confirm")]);

  add("renameTab", "numpad enter", ["Line Graph"], [text("A"), key("NumpadEnter"), text("B"), key("Enter")]);
  add("renameTab", "insert", ["Line Graph"], [insert("a\nb\r\n\rc\td\n\r\n"), key("ArrowLeft"), insert(" 1.2.3 "), insert("\n"), key("Enter")]);

  const akit = [{ supportsAkit: true }];
  add("export", "defaults", akit, [click("confirm")]);
  add("export", "defaults without akit", [{ supportsAkit: false }], [click("confirm")]);
  add("export", "before any message", [], [click("confirm")]);
  add("export", "enter", akit, [key("Enter")]);
  add("export", "exit", akit, [click("exit")]);
  add("export", "help then exit", akit, [click("help"), click("help"), click("exit")]);
  add("export", "enter on exit", akit, [key("Shift+Tab"), key("Shift+Tab"), key("Shift+Tab"), key("Enter")]);
  add("export", "enter on confirm", akit, [key("Shift+Tab"), key("Shift+Tab"), key("Enter")]);
  add("export", "enter on help", akit, [key("Shift+Tab"), key("Enter")]);
  add("export", "enter on select", akit, [key("Tab"), key("Enter")]);
  add("export", "space and numpad enter on help", akit, [key("Shift+Tab"), key(" "), key("NumpadEnter"), click("exit")]);
  add("export", "numpad enter on exit", akit, [key("Shift+Tab"), key("Shift+Tab"), key("Shift+Tab"), key("NumpadEnter")]);
  add("export", "numpad enter commits the period", akit, [pick("samplingMode", "fixed"), select("samplingPeriod"), text("0"), key("NumpadEnter"), text("0"), key("Enter")]);
  add("export", "focus messages", akit, [focused(false), focused(true), focused(false), key("Enter")]);
  add("export", "akit toggles", [{ supportsAkit: false }], [{ op: "post", value: { supportsAkit: true } }, pick("samplingMode", "akit"), click("confirm")]);
  add("export", "focused and akit in one message", akit, [{ op: "post", value: { isFocused: false, supportsAkit: false } }, click("confirm")]);
  add("export", "tab order", akit, ["Tab", "Tab", "Tab", "Tab", "Tab", "Tab", "Tab"].map(key).concat([key("Shift+Tab"), key(" ")]));
  add("export", "tab order with period", akit, [pick("samplingMode", "fixed"), ...["Tab", "Tab", "Tab", "Tab", "Tab", "Tab"].map(key), click("exit")]);
  for (const format of ["csv-table", "csv-list", "wpilog", "mcap"]) {
    for (const mode of ["changes", "fixed", "akit"]) {
      for (const generated of ["true", "false"]) {
        add("export", `${format} ${mode} ${generated}`, akit, [pick("format", format), pick("samplingMode", mode), pick("includeGenerated", generated), click("confirm")]);
      }
    }
  }
  for (const prefixes of ["/RealOutputs", "a, b ,c", " ", "日本"]) {
    add("export", `prefixes ${JSON.stringify(prefixes)}`, akit, [select("prefixes"), text(prefixes), key("Enter")]);
  }
  add("export", "mode back and forth", akit, [pick("samplingMode", "fixed"), pick("samplingMode", "changes"), pick("samplingMode", "fixed"), pick("samplingMode", "akit"), click("confirm")]);
  [...numberTexts, "-1", "0.0", "-0.5", "100"].forEach((t) => {
    add("export", `period ${JSON.stringify(t)} enter`, akit, [pick("samplingMode", "fixed"), select("samplingPeriod"), ...(t === "" ? [key("Backspace")] : [text(t)]), key("Enter")]);
    add("export", `period ${JSON.stringify(t)} click`, akit, [pick("samplingMode", "fixed"), select("samplingPeriod"), ...(t === "" ? [key("Backspace")] : [text(t)]), click("confirm")]);
    add("export", `period ${JSON.stringify(t)} tab`, akit, [pick("samplingMode", "fixed"), select("samplingPeriod"), ...(t === "" ? [key("Backspace")] : [text(t)]), key("Tab"), key("Enter")]);
  });
  add("export", "period retyped", akit, [pick("samplingMode", "fixed"), select("samplingPeriod"), text("0"), key("Tab"), select("samplingPeriod"), text("-4"), select("prefixes"), select("samplingPeriod"), text("7"), click("confirm")]);
  add("export", "select type-ahead", akit, [select("format"), text("w"), click("confirm")]);
  add("export", "type-ahead skips disabled akit", [{ supportsAkit: false }], [select("samplingMode"), text("adv"), click("confirm")]);
  add("export", "type-ahead changes the mode", akit, [select("samplingMode"), text("f"), select("samplingPeriod"), text("8"), click("confirm")]);

  const noop = { type: null, factor: 1 };
  const length = { type: "length", from: "meters", to: "feet", factor: 2 };
  add("unitConversion", "confirm", [length], [click("confirm")]);
  add("unitConversion", "exit", [length], [click("exit")]);
  add("unitConversion", "tab order none", [noop], [key("Tab"), key("Tab"), key("Tab"), key("Tab"), click("exit")]);
  add("unitConversion", "tab order", [length], [...["Tab", "Tab", "Tab", "Tab", "Tab", "Tab"].map(key), key(" ")]);
  add("unitConversion", "type-ahead", [noop], [key("Tab"), text("le"), key("Tab"), text("f"), key("Tab"), text("y"), select("extraFactor"), text("1/2"), key("Enter")]);
  add("unitConversion", "picks", [noop], [pick("unitType", "angle"), pick("fromUnit", "degrees"), pick("toUnit", "rotations"), click("confirm")]);
  add("unitConversion", "alert then fix", [length], [select("extraFactor"), text("oops"), key("Enter"), select("extraFactor"), text("4/3"), click("confirm")]);
  add("unitConversion", "enter on a select alerts", [length], [select("extraFactor"), text("oops"), select("fromUnit"), key("Enter"), text("c"), key("Enter"), key("NumpadEnter"), click("exit")]);
  add("unitConversion", "numpad enter", [length], [select("extraFactor"), text("3"), key("NumpadEnter"), click("confirm")]);
  add("unitConversion", "type change", [length], [pick("unitType", "none"), pick("unitType", "length"), click("confirm")]);
  add("unitConversion", "focus messages", [length], [focused(false), focused(true), click("confirm")]);

  const arrows = (...names: string[]) => names.map((name) => key(`Arrow${name}`));
  add("editRange", "arrow keys", [[-10, 10]], [...arrows("Up", "Up", "Down"), text("5"), key("Tab"), ...arrows("Down"), click("confirm")]);
  add("editRange", "arrow keys on fractions", [[-0.25, 1e21]], [...arrows("Down"), select("min"), ...arrows("Up", "Down", "Down"), key("Enter")]);
  add("editRange", "arrow keys on bad input", [[0, 1]], [text("1e"), ...arrows("Up"), key("Backspace"), ...arrows("Up"), key("Enter")]);
  add("editFov", "arrow keys", [72.5], [...arrows("Up", "Down"), text("1"), key("ArrowLeft"), ...arrows("Up"), text("2"), click("confirm")]);
  const fixed = [pick("samplingMode", "fixed"), select("samplingPeriod")];
  add("export", "arrow keys", akit, [...fixed, ...arrows("Up", "Up", "Down"), text("0"), ...arrows("Down"), click("confirm")]);
  add("export", "arrow keys to zero", akit, [...fixed, text("2"), ...arrows("Down", "Down", "Down"), key("Tab"), key("Enter")]);
  add("export", "arrow keys on fractions", akit, [...fixed, text("0.5"), ...arrows("Down"), text(".5"), ...arrows("Up"), key("Enter")]);
  return list;
}

function preferenceFlows(): Flow[] {
  const list: Flow[] = [];
  const add = (name: string, posts: any[], steps: Step[]) => list.push({ page: "preferences", name, posts, steps });
  const post = (prefs: any, platform: any = "darwin") => [{ platform, prefs }];
  const defaults: Record<string, any> = { ...DEFAULT_PREFS };
  const full: Record<string, any> = {
    ...DEFAULT_PREFS,
    theme: "dark",
    robotAddress: "10.63.28.2",
    remotePath: "/media/sda1/logs",
    liveMode: "rlog",
    liveSubscribeMode: "logging",
    liveDiscard: -1,
    publishFilter: "/Tuning,/Choosers",
    rlogPort: 5801,
    coordinateSystem: "center-rotated",
    field3dModeAc: "cinematic",
    field3dModeBattery: "low-power",
    field3dAntialiasing: false,
    tbaApiKey: "key",
    userAssetsFolder: "/Users/a/assets",
    skipHootNonProWarning: true,
    skipFRCCoordinateSystemWarning: true,
    skipFTCCoordinateSystemWarning: true,
    ctreLicenseAccepted: true,
    systemcoreStaticAddress: "usb",
    hasScrolledLineGraph: true,
    hasScrolledTimeline: true,
    prefersFTC: true,
    timestamps: "original"
  };
  const reordered = Object.fromEntries(Object.entries(full).reverse());
  const extended = { zeta: [1, { b: null }], ...reordered, 7: "seven", address: "legacy" };

  for (const platform of ["darwin", "win32", "linux", "freebsd", undefined]) {
    for (const [name, prefs] of [["defaults", defaults], ["full", full], ["extended", extended]] as [string, any][]) {
      add(`${platform} ${name} confirm`, post(prefs, platform), [click("confirm")]);
      add(`${platform} ${name} exit`, post(prefs, platform), [click("exit")]);
      add(`${platform} ${name} enter`, post(prefs, platform), [key("Enter")]);
    }
  }
  add("shift enter", post(full), [key("Shift+Enter")]);
  add("other keys", post(full), [key("Escape"), key("ArrowLeft"), key("Backspace"), text("x"), key(" "), key("Enter")]);
  add("no message", [], [click("exit"), click("confirm"), key("Enter"), focused(false), focused(true)]);
  add("null message", [null], [click("confirm")]);
  add("string message", ["prefs"], [click("confirm")]);
  add("empty message", [{}], [click("confirm")]);
  add("null prefs", post(null), [click("confirm")]);
  add("focus messages", post(full), [focused(false), focused(false), focused(true), focused(false), key("Enter")]);
  add("focus key in prefs message", [{ platform: "darwin", prefs: full, isFocused: false }], [click("confirm")]);
  add("two messages", [{ platform: "darwin", prefs: full }, { platform: "linux", prefs: defaults }], [click("confirm")]);
  add("two messages exit", [{ platform: "darwin", prefs: full }, { platform: "darwin", prefs: defaults }], [click("exit")]);
  add("second message throws", [{ platform: "darwin", prefs: full }, { platform: "linux", prefs: {} }], [key("Enter")]);

  add("second message without prefs", [{ platform: "darwin", prefs: full }, { platform: "linux", prefs: null }], [click("confirm")]);
  add("second message without prefs then exit", [{ platform: "darwin", prefs: { ...full, theme: "other" } }, { platform: "linux" }], [click("exit")]);

  const shown = ["theme", "robotAddress", "remotePath", "liveSubscribeMode", "liveDiscard", "publishFilter", "coordinateSystem", "timestamps", "field3dModeAc", "field3dModeBattery", "field3dAntialiasing", "tbaApiKey"];
  for (const k of Object.keys(full)) {
    const { [k]: _, ...rest } = full;
    add(`without ${k}`, post(rest), [click("confirm")]);
    add(`without ${k} exit`, post(rest), [click("exit")]);
    for (const value of shown.includes(k) ? [null, 12.5, true, "", "other", ["a", 1], { a: 1 }, "a\r\nb\n"] : [null, "other"]) {
      add(`${k} ${JSON.stringify(value)}`, post({ ...full, [k]: value }), [click("confirm")]);
    }
  }
  for (const value of [60, 300, 1200, -1, 500, 0, -0, 1.5, 1e21, NaN, "60", "060", " 60"]) {
    add(`liveDiscard is ${typeof value} ${Object.is(value, -0) ? "-0" : value}`, post({ ...full, liveDiscard: value }), [click("confirm")]);
  }
  for (const value of ["", "true", "false", 1]) add(`antialiasing is ${JSON.stringify(value)}`, post({ ...full, field3dAntialiasing: value }), [key("Enter")]);

  const options: Record<string, string[]> = {
    theme: ["system", "light", "dark"],
    coordinateSystem: ["automatic", "center-red", "wall-blue", "wall-alliance", "center-rotated"],
    timestamps: ["start-zero", "original"],
    liveSubscribeMode: ["low-bandwidth", "logging"],
    liveDiscard: ["60", "300", "1200", "-1"],
    field3dModeAc: ["cinematic", "standard", "low-power"],
    field3dModeBattery: ["", "cinematic", "standard", "low-power"],
    field3dAntialiasing: ["true", "false"]
  };
  for (const [id, values] of Object.entries(options)) {
    for (const value of values) {
      add(`pick ${id} ${JSON.stringify(value)} from defaults`, post(defaults), [pick(id, value), click("confirm")]);
      add(`pick ${id} ${JSON.stringify(value)} from full`, post(full), [pick(id, value), key("Enter")]);
      add(`pick ${id} ${JSON.stringify(value)} then exit`, post(full), [pick(id, value), click("exit")]);
    }
    add(`pick ${id} over another value`, post({ ...full, [id]: "other" }), [pick(id, values[values.length - 1]), click("confirm")]);
  }
  add("pick on linux", post(defaults, "linux"), [pick("theme", "dark"), pick("theme", "light"), click("confirm")]);
  const typed: [string, string, string][] = [
    ["theme", "a", "always d"],
    ["coordinateSystem", "c", "b"],
    ["timestamps", "o", "s"],
    ["liveSubscribeMode", "lo", "log"],
    ["liveDiscard", "n", "after 2"],
    ["field3dModeAc", "c", "low"],
    ["field3dModeBattery", "d", "s"],
    ["field3dAntialiasing", "of", "x"]
  ];
  for (const [id, ...texts] of typed) {
    for (const t of texts) add(`type ${JSON.stringify(t)} in ${id}`, post(defaults), [select(id), text(t), click("confirm")]);
  }
  for (const t of ["l", "d", "m", "a"]) add(`type ${JSON.stringify(t)} in theme on linux`, post({ ...defaults, theme: "dark" }, "linux"), [select("theme"), text(t), click("confirm")]);
  add("linux system theme", post(defaults, "linux"), [select("theme"), text("d"), text("d"), key("Enter")]);

  const texts = ["10.0.0.2", "  padded ", "a\"b\\c", "caf\u00e9 \u65e5\u672c", "<b>&amp;</b>", "\u{1f4c8}", "x".repeat(80)];
  for (const id of ["robotAddress", "remotePath", "publishFilter", "tbaApiKey"]) {
    texts.forEach((t, i) => add(`type ${JSON.stringify(t)} in ${id}`, post(defaults), [select(id), text(t), i % 2 === 0 ? key("Enter") : click("confirm")]));
    add(`clear ${id}`, post(full), [select(id), key("Backspace"), click("confirm")]);
    add(`append to ${id}`, post(full), [select(id), key("ArrowRight"), text("!"), key("Enter")]);
    add(`insert in ${id}`, post(full), [select(id), insert("a\nb\r\n\rc\td\n"), key("ArrowLeft"), insert(" 1.2 "), click("confirm")]);
    add(`type in ${id} then exit`, post(full), [select(id), text("typed"), click("exit")]);
    add(`numpad enter in ${id}`, post(full), [select(id), text("typed"), key("NumpadEnter"), click("confirm")]);
  }
  add("edit everything", post(defaults), [
    ...Object.entries(options).map(([id, values]) => pick(id, values[values.length - 1])),
    ...["robotAddress", "remotePath", "publishFilter", "tbaApiKey"].flatMap((id) => [select(id), text(`new ${id}`)]),
    click("confirm")
  ]);
  const tabs = (n: number, back = false) => Array.from({ length: n }, () => key(back ? "Shift+Tab" : "Tab"));
  add("tab order", post(full), [...tabs(14), key(" ")]);
  add("tab order back", post(full), [...tabs(14, true), text("l"), key("Enter")]);
  add("tab and type", post(defaults), [...tabs(4), text("k"), ...tabs(1), text("h"), ...tabs(1), text("p"), ...tabs(1), text("log"), ...tabs(2), text("f"), key("Enter")]);
  add("space on exit", post(full), [...tabs(2, true), key(" ")]);
  add("enter on exit", post(full), [...tabs(2, true), key("Enter")]);
  add("enter on confirm", post(full), [...tabs(1, true), key("Enter")]);
  for (const [name, n] of [["exit", 2], ["confirm", 1]] as [string, number][]) {
    for (const press of ["Enter", "Shift+Enter", "Control+Enter", "Alt+Enter", "Meta+Enter", "NumpadEnter", "Shift+NumpadEnter", " "]) {
      add(`${press} on ${name} after an edit`, post(full), [pick("theme", "light"), select("robotAddress"), text("typed"), select("field3dAntialiasing"), ...tabs(3 - n), key(press)]);
    }
  }
  add("numpad enter on exit", post(full), [...tabs(2, true), key("NumpadEnter")]);
  add("numpad enter on confirm", post(full), [...tabs(1, true), key("NumpadEnter")]);
  add("enter on a select", post(full), [select("coordinateSystem"), key("NumpadEnter"), key("Enter")]);
  return list;
}

const STATE_SCRIPT = `(() => {
  const rec = window.__rec;
  const PREFERENCES = ${preferences};
  const controls = {};
  for (const el of document.querySelectorAll("input, select")) {
    controls[el.id] = el.tagName === "SELECT"
      ? { options: Array.from(el.options).map((o) => ({ text: o.text, value: o.value, disabled: o.disabled, ...(PREFERENCES ? { hidden: o.hidden } : {}) })), value: el.value, selectedIndex: el.selectedIndex, disabled: el.disabled }
      : { value: el.value, disabled: el.disabled, ...(PREFERENCES ? { placeholder: el.placeholder } : {}) };
  }
  const active = document.activeElement;
  return {
    controls,
    active: active === null || active === document.body ? null : active.id !== "" ? active.id : active.tagName === "A" ? "help" : active.tagName,
    blurred: Array.from(document.getElementsByTagName("button")).map((b) => b.classList.contains("blurred")),
    replies: rec.replies.splice(0),
    alerts: rec.alerts.splice(0),
    errors: rec.errors.splice(0)
  };
})()`;

const SETUP_SCRIPT = `(() => {
  window.__rec = { replies: [], alerts: [], errors: [] };
  window.alert = (message) => { window.__rec.alerts.push(String(message)); };
  window.addEventListener("error", (event) => {
    const e = event.error;
    window.__rec.errors.push(e instanceof Error ? { name: e.name, message: e.message } : { name: null, message: String(event.message) });
  });
  const channel = new MessageChannel();
  channel.port2.onmessage = (event) => { window.__rec.replies.push(event.data); };
  window.__port = channel.port2;
  window.postMessage("port", "*", [channel.port1]);
})()`;

const ELECTRON_MAIN = `
import { app, BrowserWindow, net, protocol } from "electron";
import fs from "node:fs";
import path from "node:path";
import v8 from "node:v8";
import { fileURLToPath } from "node:url";
const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
const KEYS = {
  Enter: { code: "Enter", keyCode: 13, text: "\\r" },
  NumpadEnter: { key: "Enter", code: "NumpadEnter", keyCode: 13, text: "\\r" },
  Tab: { code: "Tab", keyCode: 9 },
  Escape: { code: "Escape", keyCode: 27 },
  Backspace: { code: "Backspace", keyCode: 8 },
  ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
  ArrowRight: { code: "ArrowRight", keyCode: 39 },
  ArrowUp: { code: "ArrowUp", keyCode: 38 },
  ArrowDown: { code: "ArrowDown", keyCode: 40 },
  " ": { code: "Space", keyCode: 32, text: " " }
};
app.setPath("userData", request.userData);
app.dock?.hide();
app.on("window-all-closed", () => {});
app.whenReady().then(async () => {
  const root = path.dirname(path.dirname(request.www));
  protocol.handle("file", (req) => {
    const file = fs.realpathSync(fileURLToPath(req.url));
    if (!file.startsWith(root + path.sep)) return new Response("", { status: 404 });
    return net.fetch(req, { bypassCustomProtocolHandlers: true });
  });
  const records = [];
  const win = new BrowserWindow({ show: false, width: request.width, height: request.height, useContentSize: true, webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false } });
  win.webContents.debugger.attach("1.3");
  const send = (method, params) => win.webContents.debugger.sendCommand(method, params);
  const js = (code) => win.webContents.executeJavaScript(code, true);
  const settle = () => js("new Promise((resolve) => setTimeout(resolve, 0)).then(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))))");
  const press = async (name) => {
    const parts = name.length > 1 ? name.split("+") : [name];
    const key = parts.pop();
    const modifiers = parts.reduce((bits, part) => bits | { Alt: 1, Control: 2, Meta: 4, Shift: 8 }[part], 0);
    const k = KEYS[key];
    if (k === undefined) throw new Error("unknown key " + name);
    const down = { key: k.key ?? key, code: k.code, windowsVirtualKeyCode: k.keyCode, nativeVirtualKeyCode: k.keyCode, modifiers };
    await send("Input.dispatchKeyEvent", k.text === undefined ? { type: "rawKeyDown", ...down } : { type: "keyDown", text: k.text, ...down });
    await send("Input.dispatchKeyEvent", { type: "keyUp", ...down });
  };
  const apply = async (step) => {
    const id = JSON.stringify(step.id);
    switch (step.op) {
      case "select":
        await js("(() => { const el = document.getElementById(" + id + "); if (el.tagName === 'SELECT') el.focus(); else el.select(); })()");
        break;
      case "text":
        for (const c of step.value) {
          await send("Input.dispatchKeyEvent", { type: "keyDown", key: c, text: c, unmodifiedText: c });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: c });
        }
        break;
      case "insert":
        await js("document.execCommand('insertText', false, " + JSON.stringify(step.value) + ")");
        break;
      case "key":
        await press(step.value);
        break;
      case "click": {
        const [x, y] = await js("(() => { const el = " + (step.id === "help" ? "document.querySelector('div.help-div a')" : "document.getElementById(" + id + ")") + "; const r = el.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()");
        const mouse = { x, y, button: "left", clickCount: 1 };
        await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
        await send("Input.dispatchMouseEvent", { type: "mousePressed", ...mouse });
        await send("Input.dispatchMouseEvent", { type: "mouseReleased", ...mouse });
        break;
      }
      case "pick": {
        const v = JSON.stringify(step.value);
        await js("(() => { const s = document.getElementById(" + id + "); s.focus(); if (s.value === " + v + ") return; s.value = " + v + "; if (s.value !== " + v + ") throw new Error('no option ' + " + v + "); s.dispatchEvent(new Event('input', { bubbles: true })); s.dispatchEvent(new Event('change', { bubbles: true })); })()");
        break;
      }
      case "post":
        await js("window.__port.postMessage(" + JSON.stringify(step.value) + ")");
        break;
    }
  };
  for (const flow of request.flows) {
    await win.loadFile(path.join(request.www, flow.page + ".html"));
    win.webContents.focus();
    await js(request.setup);
    await settle();
    for (const post of request.posts[flow.name]) await js("window.__port.postMessage(" + post + ")");
    await settle();
    records.push({ kind: "open", flow: flow.name, state: await js(request.state) });
    let replied = false;
    for (let i = 0; i < flow.steps.length; i++) {
      if (replied) throw new Error(flow.name + ": steps after the window closed");
      await apply(flow.steps[i]);
      await settle();
      const state = await js(request.state);
      replied = state.replies.some((r) => flow.page !== "export" || typeof r !== "string");
      records.push({ kind: "step", flow: flow.name, index: i, state });
    }
    records.push({ kind: "end", flow: flow.name, replied });
  }
  win.destroy();
  fs.writeFileSync(request.output, v8.serialize(records));
  app.exit(0);
}).catch((e) => {
  process.stderr.write("genPrompts main: " + (e && e.stack ? e.stack : e) + "\\n");
  app.exit(1);
});
`;

const jsLiteral = (value: any): string => {
  if (typeof value === "number") return Object.is(value, -0) ? "-0" : String(value);
  if (Array.isArray(value)) return "[" + value.map(jsLiteral).join(",") + "]";
  if (value === null || typeof value !== "object") return value === undefined ? "undefined" : JSON.stringify(value);
  return "{" + Object.keys(value).map((k) => JSON.stringify(k) + ":" + jsLiteral(value[k])).join(",") + "}";
};

const list = preferences ? preferenceFlows() : flows();
const names = new Set(list.map((f) => f.name));
if (names.size !== list.length) {
  process.stderr.write("genPrompts: duplicate flow names\n");
  process.exit(1);
}
const byName = new Map(list.map((f) => [f.name, f]));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-prompts-"));
try {
  const requestPath = path.join(scratch, "request.json");
  const mainPath = path.join(scratch, "main.mjs");
  const output = path.join(scratch, "records.bin");
  fs.writeFileSync(mainPath, ELECTRON_MAIN);
  fs.writeFileSync(
    requestPath,
    JSON.stringify({
      flows: list,
      posts: Object.fromEntries(list.map((f) => [f.name, f.posts.map(jsLiteral)])),
      width: preferences ? 400 : 300,
      height: preferences ? 473 : 189,
      setup: SETUP_SCRIPT,
      state: STATE_SCRIPT,
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
    process.stderr.write(`genPrompts: Electron exited with ${run.status ?? run.signal}\n`);
    process.exit(1);
  }
  const records: any[] = v8.deserialize(fs.readFileSync(output));
  const prefsFile = path.join(scratch, "prefs.json");
  const written = new Map<string, string>();
  for (const r of preferences ? records : []) {
    if (r.kind === "end" || r.state.replies.length === 0) continue;
    for (const reply of r.state.replies) jsonfile.writeFileSync(prefsFile, reply);
    written.set(r.flow, fs.readFileSync(prefsFile, "utf8"));
  }
  for (const r of records) {
    const flow = byName.get(r.flow)!;
    if (r.kind === "open") out({ kind: "open", flow: r.flow, page: flow.page, posts: toTree(flow.posts), state: toTree(r.state) });
    else if (r.kind === "step") out({ kind: "step", flow: r.flow, step: toTree(flow.steps[r.index]), state: toTree(r.state) });
    else out({ kind: "end", flow: r.flow, replied: r.replied, ...(preferences ? { written: written.get(r.flow) ?? null } : {}) });
  }
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
