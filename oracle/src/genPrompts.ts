import "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import v8 from "node:v8";
import { toTree } from "./cve1";
import { UPSTREAM_DIR } from "./shims";

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

// Ground truth for the prompt windows (asrs_tabs::prompts and asrs_app::views::prompts):
// upstream's real www/<page>.html and its rolldown bundle in an offscreen Electron window (Blink),
// driven the way main.ts's create*Window drives it: a "port" message carrying a MessageChannel
// port, then the messages main posts on it. The scripted user steps are real input sent over the
// DevTools protocol (typed characters, keys, clicks at an element's centre), so focus changes and
// "change" events come in Chromium's order; "select" is script focus with the whole text selected,
// "pick" is what choosing an option in a select's native popup does (the new value, then "input"
// and "change"), and "post" is another message from main. After the open and after every step it
// records every control, the focused element, the buttons' "blurred" class, and every reply posted
// on the port, alert and uncaught error since the previous record. A flow ends at the reply after
// which main.ts destroys the window, which is the first one except for export.html's help URL.

interface Step {
  op: "select" | "text" | "key" | "click" | "pick" | "post";
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
const key = (value: string): Step => ({ op: "key", value });
const click = (id: string): Step => ({ op: "click", id });
const pick = (id: string, value: string): Step => ({ op: "pick", id, value });
const focused = (isFocused: boolean): Step => ({ op: "post", value: { isFocused } });

function flows(): Flow[] {
  const list: Flow[] = [];
  const add = (page: string, name: string, posts: any[], steps: Step[]) => list.push({ page, name: `${page} ${name}`, posts, steps });

  const numberTexts = ["5", "-5", "0", "-0", "1.5", ".5", "5.", "1e3", "1E-2", "1e", "-", "+5", "e", "", "00012", "1.0000000001", "9007199254740993", "1e400", "1e-400", "1a2", "1 2", "0x10", "-5.", "+", ".", "-.", "+.5", "-.5", "+5.", "+1e3", "1e+3", "5.5e2", "-1e-2", "+-5", "-+5"];

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
  });

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

  const akit = [{ supportsAkit: true }];
  add("export", "defaults", akit, [click("confirm")]);
  add("export", "defaults without akit", [{ supportsAkit: false }], [click("confirm")]);
  add("export", "before any message", [], [click("confirm")]);
  add("export", "enter", akit, [key("Enter")]);
  add("export", "exit", akit, [click("exit")]);
  add("export", "help then exit", akit, [click("help"), click("help"), click("exit")]);
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
  add("unitConversion", "type change", [length], [pick("unitType", "none"), pick("unitType", "length"), click("confirm")]);
  add("unitConversion", "focus messages", [length], [focused(false), focused(true), click("confirm")]);
  return list;
}

const STATE_SCRIPT = `(() => {
  const rec = window.__rec;
  const controls = {};
  for (const el of document.querySelectorAll("input, select")) {
    controls[el.id] = el.tagName === "SELECT"
      ? { options: Array.from(el.options).map((o) => ({ text: o.text, value: o.value, disabled: o.disabled })), value: el.value, selectedIndex: el.selectedIndex, disabled: el.disabled }
      : { value: el.value, disabled: el.disabled };
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
  Tab: { code: "Tab", keyCode: 9 },
  Escape: { code: "Escape", keyCode: 27 },
  Backspace: { code: "Backspace", keyCode: 8 },
  ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
  ArrowRight: { code: "ArrowRight", keyCode: 39 },
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
  const win = new BrowserWindow({ show: false, width: 300, height: 189, useContentSize: true, webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false } });
  win.webContents.debugger.attach("1.3");
  const send = (method, params) => win.webContents.debugger.sendCommand(method, params);
  const js = (code) => win.webContents.executeJavaScript(code, true);
  const settle = () => js("new Promise((resolve) => setTimeout(resolve, 0)).then(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))))");
  const press = async (name) => {
    const parts = name.length > 1 ? name.split("+") : [name];
    const key = parts.pop();
    const modifiers = parts.includes("Shift") ? 8 : 0;
    const k = KEYS[key];
    if (k === undefined) throw new Error("unknown key " + name);
    const down = { key, code: k.code, windowsVirtualKeyCode: k.keyCode, nativeVirtualKeyCode: k.keyCode, modifiers };
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

const list = flows();
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
  for (const r of records) {
    const flow = byName.get(r.flow)!;
    if (r.kind === "open") out({ kind: "open", flow: r.flow, page: flow.page, posts: toTree(flow.posts), state: toTree(r.state) });
    else if (r.kind === "step") out({ kind: "step", flow: r.flow, step: toTree(flow.steps[r.index]), state: toTree(r.state) });
    else out({ kind: "end", flow: r.flow, replied: r.replied });
  }
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
