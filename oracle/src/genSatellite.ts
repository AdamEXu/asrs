import "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import v8 from "node:v8";
import { toTree } from "./cve1";
import { UPSTREAM_DIR } from "./shims";

// Ground truth for asrs_hub::satellite: upstream's real www/satellite.html and its rolldown bundle
// in an offscreen Electron window (Blink), driven as main.ts's createSatellite drives it: a "port"
// message carrying a MessageChannel port, then the messages main posts on it. Keys are real input
// sent over the DevTools protocol, "resize" is the window's content size, "focus" is a click on an
// element's centre and "unload" dispatches beforeunload, whose messages a real navigation would
// lose with the page's own end of the channel. After every step it records document.title, what
// the page posted since the previous record (save-state only as the last one, after one more
// interval) and its uncaught errors.

type Step =
  | { post: [string, unknown] }
  | { key: string }
  | { resize: [number, number] }
  | { focus: string }
  | { unload: true };
interface Flow {
  name: string;
  steps: Step[];
}

const post = (name: string, data?: unknown): Step => ({ post: [name, data] });
const setType = (type: unknown) => post("set-type", type);
const restore = (data: unknown) => post("restore-state", data);
const render = (command: unknown, title: unknown = "Title") => post("render", { command, title });
const key = (value: string): Step => ({ key: value });
const resize = (width: number, height: number): Step => ({ resize: [width, height] });

const points = (width: unknown, height: unknown) => ({ dimensions: [width, height], sets: [] });
const mechanism = (width: number, height: number) => ({ dimensions: [width, height], backgroundColor: "#000000", lines: [] });
const table = (timestamps: number[]) => ({ timestamps, displayOffset: 0, fields: [], selectionMode: 0, selectedTime: null, hoveredTime: null });
const field2d = (field: string, orientation = 0) => ({ field, orientation, size: 0.7, objects: [] });
const config = (id: string, width: number, height: number, isFTC = false) => ({
  name: id,
  path: "missing.png",
  id,
  isFTC,
  coordinateSystem: "wall-blue",
  useGrid: false,
  topLeft: [0, 0],
  bottomRight: [100, 50],
  widthInches: width,
  heightInches: height
});
const assets = {
  field2ds: [config("wide", 690, 320), config("square", 144, 144, true), config("strip", 800, 100)],
  field3ds: [],
  robots: [],
  joysticks: [],
  loadFailures: []
};
const prefs = { hasScrolledLineGraph: true, prefersFTC: false };

function flows(): Flow[] {
  const list: Flow[] = [];
  const add = (name: string, ...steps: Step[]) => list.push({ name, steps });
  const keys = ["Space", "KeyL", "ArrowLeft", "ArrowRight", "ArrowUp", "KeyA", "Enter"].map(key);

  add("no messages", ...keys, resize(700, 400), { unload: true });
  add("render before a type", render(points(2, 1)), render(points(2, 1), "Other"), resize(700, 400), setType(11), render(points(4, 1), "Other"), { unload: true });
  for (let type = 0; type <= 13; type++) {
    if (type === 1 || type === 3 || type === 6 || type === 7 || type === 8) continue;
    const command = [null, null, field2d("wide"), null, table([0, 1, 2]), null, null, null, null, null, mechanism(3, 2), points(3, 2), {}][type] ?? null;
    if (type !== 5 && type !== 9) add(`type ${type}`, post("set-assets", assets), post("set-battery", true), setType(type), post("set-preferences", prefs), render(command), render(command), ...keys, { unload: true });
  }
  for (const type of [null, "2", 2.5, -1, {}, [4]]) {
    add(`type ${JSON.stringify(type)}`, setType(type), render(null), render(points(1, 1), "B"), post("set-2d-camera", 1), post("set-3d-camera", 1), post("edit-fov", 60), { unload: true });
  }

  const ratios: [unknown, unknown][] = [[1, 1], [2, 1], [1, 2], [500, 100], [501, 100], [100, 500], [100, 501], [5000, 1], [1, 5000], [0.5, 3], [0, 0], [-4, 2], [16, 9]];
  add("ratios", setType(11), ...ratios.map(([w, h]) => render(points(w, h))), ...ratios.reverse().map(([w, h]) => render(points(w, h))));
  add("clamped ratios in a row", setType(11), render(points(10, 1)), render(points(20, 1)), render(points(5, 1)), render(points(1, 10)), render(points(1, 20)), resize(700, 400));
  add("not a number", setType(11), render(points(NaN, 1)), render(points(NaN, 1)), resize(640, 360), render(points(1, NaN)));
  add("mechanism", setType(10), render(null), render(null), resize(700, 400), render(mechanism(3, 2)), render(null), render(mechanism(3, 2)), render(mechanism(1, 30)), resize(640, 360), render(mechanism(0, 1)), resize(700, 400), render(mechanism(1, 0)), resize(640, 360));
  add("command throws", setType(11), render(points(2, 1)), render(null), resize(640, 360), render(points(2, 1)), render(points(3, 1), "Later"));

  add("titles", setType(10), ...["A", "A", "", "", " ", "  two  spaces ", "a\tb\nc\r\nd", "<b>&amp;</b>", "a & b", "\u0000x", "café 日本", "\u{1f4c8} up", "\ud83d", " nbsp ", " em", "x".repeat(300), "Documentation", "香0馚"].map((title) => render(null, title)));
  for (const title of [null, 5, {}, ["a"], true]) add(`title ${JSON.stringify(title)}`, setType(11), render(points(2, 1), "First"), render(points(3, 1), title), render(points(4, 1), "First"), resize(640, 360));
  add("title without a type", render(null, "Alone"), setType(4), render(table([0]), "Alone"), render(table([0]), "Typed"));

  add("restore points", restore({ type: 11, visualizer: null }), render(points(2, 1)));
  add("restore field 2d", post("set-assets", assets), restore({ type: 2, visualizer: { zoom: 2.5, pan: [0.1, -0.2], trackingMode: 0, isFTC: false } }), render(field2d("wide")), render(field2d("wide", 1)), render(field2d("square")), resize(700, 400), render(field2d("strip")), render(field2d("missing")), resize(640, 360));
  add("restore field 2d other shapes", restore({ type: 2, visualizer: { zoom: "1", pan: [1], trackingMode: 2, isFTC: 1 } }));
  add("restore without a visualizer", restore({ type: 2 }));
  for (const data of [null, 5, "state", {}, { type: null, visualizer: null }, { type: 99, visualizer: { a: 1 } }, { visualizer: 1 }]) add(`restore ${JSON.stringify(data)}`, restore(data), render(points(2, 1)));

  add("2d cameras", post("set-assets", assets), setType(2), render(field2d("wide")), ...[1, 2, 0, 3, -1, 1].map((i) => post("set-2d-camera", i)), post("set-3d-camera", 0), post("edit-fov", 60), render(field2d("wide")));
  add("assets change", post("set-assets", assets), setType(2), render(field2d("wide")), post("set-assets", { ...assets, field2ds: [config("wide", 100, 300)] }), render(field2d("wide")), post("set-assets", null), render(field2d("wide")), resize(640, 360));
  add("cameras on points", setType(11), post("set-2d-camera", 1), post("set-3d-camera", 1), post("edit-fov", 60));
  add("unknown message", setType(11), post("show"), render(points(2, 1)));

  const rows = Array.from({ length: 200 }, (_, i) => i / 10);
  add("table", post("set-preferences", prefs), setType(4), render(table([])), render(table(rows.slice(0, 5))), render(table(rows)), resize(700, 300), render(table(rows)), { unload: true }, { unload: true });
  add("keys in an input", setType(4), render(table(rows)), { focus: "input.data-table-jump-input" }, ...["Space", "KeyL", "ArrowLeft", "ArrowRight"].map(key));
  add("keys with modifiers", setType(11), ...["Meta+KeyL", "Control+KeyL", "Shift+Space", "Alt+ArrowLeft"].map(key));
  return list;
}

const SETUP_SCRIPT = `(() => {
  const rec = (window.__rec = { messages: [], saved: null, saves: 0, errors: [] });
  window.addEventListener("error", (event) => {
    const e = event.error;
    rec.errors.push(e instanceof Error ? { name: e.name, message: e.message } : { name: null, message: String(event.message) });
  });
  const channel = new MessageChannel();
  channel.port2.onmessage = (event) => {
    if (event.data.name === "save-state") {
      rec.saved = event.data.data;
      rec.saves++;
    } else rec.messages.push(event.data);
  };
  window.__port = channel.port2;
  window.postMessage("port", "*", [channel.port1]);
})()`;

const STATE_SCRIPT = `(() => {
  const rec = window.__rec;
  return { title: document.title, messages: rec.messages.splice(0), saved: rec.saved, errors: rec.errors.splice(0) };
})()`;

const ELECTRON_MAIN = `
import { app, BrowserWindow, net, protocol } from "electron";
import fs from "node:fs";
import path from "node:path";
import v8 from "node:v8";
import { fileURLToPath } from "node:url";
const request = v8.deserialize(fs.readFileSync(process.argv[process.argv.length - 1]));
const KEYS = {
  Space: [" ", 32, " "], KeyL: ["l", 76, "l"], KeyA: ["a", 65, "a"], Enter: ["Enter", 13, "\\r"],
  ArrowLeft: ["ArrowLeft", 37], ArrowRight: ["ArrowRight", 39], ArrowUp: ["ArrowUp", 38]
};
app.setPath("userData", request.userData);
app.dock?.hide();
app.on("window-all-closed", () => {});
app.whenReady().then(async () => {
  const root = path.dirname(path.dirname(request.www));
  protocol.handle("file", (req) => {
    const url = fileURLToPath(req.url);
    const file = fs.existsSync(url) ? fs.realpathSync(url) : "";
    if (!file.startsWith(root + path.sep)) return new Response("", { status: 404 });
    return net.fetch(req, { bypassCustomProtocolHandlers: true });
  });
  const records = [];
  for (const flow of request.flows) {
    const win = new BrowserWindow({ show: false, width: request.width, height: request.height, useContentSize: true, webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false } });
    win.webContents.debugger.attach("1.3");
    const send = (method, params) => win.webContents.debugger.sendCommand(method, params);
    const js = (code) => win.webContents.executeJavaScript(code, true);
    const settle = () => js("new Promise((resolve) => setTimeout(resolve, 0)).then(() => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0))))");
    const saved = () => js("(async () => { const n = window.__rec.saves; while (window.__rec.saves < n + 2) await new Promise((r) => setTimeout(r, 10)); })()");
    const press = async (name) => {
      const parts = name.split("+");
      const code = parts.pop();
      const modifiers = parts.reduce((bits, part) => bits | { Alt: 1, Control: 2, Meta: 4, Shift: 8 }[part], 0);
      const [key, keyCode, text] = KEYS[code];
      const down = { key, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode, modifiers };
      await send("Input.dispatchKeyEvent", text === undefined || modifiers !== 0 ? { type: "rawKeyDown", ...down } : { type: "keyDown", text, ...down });
      await send("Input.dispatchKeyEvent", { type: "keyUp", ...down });
    };
    await win.loadFile(path.join(request.www, "satellite.html"));
    win.webContents.focus();
    await js(request.setup);
    await settle();
    for (const [i, step] of flow.steps.entries()) {
      if (step.post) await js("window.__port.postMessage({ name: " + JSON.stringify(step.post[0]) + ", data: " + flow.data[i] + " })");
      else if (step.key) await press(step.key);
      else if (step.resize) {
        win.setContentSize(step.resize[0], step.resize[1]);
        await js("(async () => { while (window.innerWidth !== " + step.resize[0] + " || window.innerHeight !== " + step.resize[1] + ") await new Promise((r) => requestAnimationFrame(r)); })()");
      } else if (step.focus) {
        const [x, y] = await js("(() => { const r = document.querySelector(" + JSON.stringify(step.focus) + ").getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()");
        await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
        await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
      } else if (step.unload) await js("window.dispatchEvent(new Event('beforeunload'))");
      await settle();
      await saved();
      records.push({ flow: flow.name, index: i, state: await js(request.state) });
    }
    win.destroy();
  }
  fs.writeFileSync(request.output, v8.serialize(records));
  app.exit(0);
}).catch((e) => {
  process.stderr.write("genSatellite main: " + (e && e.stack ? e.stack : e) + "\\n");
  app.exit(1);
});
`;

const jsLiteral = (value: any): string => {
  if (typeof value === "number") return Object.is(value, -0) ? "-0" : String(value);
  if (Array.isArray(value)) return "[" + value.map(jsLiteral).join(",") + "]";
  if (value === null || typeof value !== "object") return value === undefined ? "undefined" : JSON.stringify(value);
  return "{" + Object.keys(value).map((k) => JSON.stringify(k) + ":" + jsLiteral(value[k])).join(",") + "}";
};

const WIDTH = 900;
const HEIGHT = 472;
const list = flows();
if (new Set(list.map((f) => f.name)).size !== list.length) {
  process.stderr.write("genSatellite: duplicate flow names\n");
  process.exit(1);
}
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-satellite-"));
try {
  const requestPath = path.join(scratch, "request.bin");
  const mainPath = path.join(scratch, "main.mjs");
  const output = path.join(scratch, "records.bin");
  fs.writeFileSync(mainPath, ELECTRON_MAIN);
  fs.writeFileSync(
    requestPath,
    v8.serialize({
      flows: list.map((f) => ({ ...f, data: f.steps.map((s) => ("post" in s ? jsLiteral(s.post[1]) : "")) })),
      width: WIDTH,
      height: HEIGHT,
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
    process.stderr.write(`genSatellite: Electron exited with ${run.status ?? run.signal}\n`);
    process.exit(1);
  }
  const records: any[] = v8.deserialize(fs.readFileSync(output));
  const byName = new Map(list.map((f) => [f.name, f]));
  let last = "";
  // The table renderer's UUID is random; records name it by the order it was first seen in.
  const uuids = new Map<string, string>();
  const named = (message: any) => {
    const uuid = message.name === "add-table-range" ? message.data.uuid : undefined;
    if (typeof uuid !== "string") return message;
    if (!uuids.has(uuid)) uuids.set(uuid, `uuid-${uuids.size}`);
    return { ...message, data: { ...message.data, uuid: uuids.get(uuid) } };
  };
  for (const r of records) {
    if (r.flow !== last) {
      last = r.flow;
      uuids.clear();
      process.stdout.write(JSON.stringify({ kind: "open", flow: r.flow, width: toTree(WIDTH), height: toTree(HEIGHT) }) + "\n");
    }
    const state = { ...r.state, messages: r.state.messages.map(named) };
    process.stdout.write(JSON.stringify({ kind: "step", step: toTree(byName.get(r.flow)!.steps[r.index]), state: toTree(state) }) + "\n");
  }
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
