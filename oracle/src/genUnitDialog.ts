import "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import v8 from "node:v8";
import { toTree } from "./cve1";
import { UPSTREAM_DIR } from "./shims";
import { Units } from "@upstream/shared/units";

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

// Ground truth for asrs_model::unit_conversion_dialog: upstream's real www/unitConversion.html and
// its rolldown bundle in a hidden Electron BrowserWindow (Blink, the DOM oracle of record), driven
// the way main.ts's createUnitConversionWindow drives it. Each flow posts a "port" message carrying
// a MessageChannel port, posts the preset on it, then runs scripted user steps: picking an option
// in one of the three selects (the unit type select also gets the "change" event a real pick
// fires), typing into the extra factor field, clicking confirm or exit, pressing a key, and the
// window's focus messages. After the open and after every step it records each select's options,
// selection and disabled flag, the factor text, the buttons' "blurred" class, and every reply
// posted on the port, alert and uncaught error since the previous record. A flow ends at the first
// reply, as main.ts destroys the window when one arrives.
//
//   genUnitDialog              builds the flows here, runs them in an Electron main process
//                              (ELECTRON_MAIN below) and prints the records

interface Step {
  op: "type" | "from" | "to" | "factor" | "confirm" | "exit" | "key" | "focus";
  value?: any;
}
interface Flow {
  name: string;
  preset: any;
  steps: Step[];
}

const GROUPS = Object.keys(Units.UNIT_GROUPS);
const unitsOf = (group: string) => Object.keys((Units.UNIT_GROUPS as any)[group]);

function flows(): Flow[] {
  const list: Flow[] = [];
  const add = (name: string, preset: any, steps: Step[]) => list.push({ name, preset, steps });

  add("noop confirm", { type: null, factor: 1 }, [{ op: "confirm" }]);
  add("noop exit", { type: null, factor: 1 }, [{ op: "exit" }]);
  add("noop enter", { type: null, factor: 1 }, [{ op: "key", value: "Enter" }]);
  add("noop other keys", { type: null, factor: 1 }, [
    { op: "key", value: "NumpadEnter" },
    { op: "key", value: "Escape" },
    { op: "key", value: "KeyA" },
    { op: "key", value: "Enter" }
  ]);
  add("focus messages", { type: "length", from: "meters", to: "feet", factor: 1 }, [
    { op: "focus", value: false },
    { op: "focus", value: false },
    { op: "focus", value: true },
    { op: "focus", value: false },
    { op: "confirm" }
  ]);

  GROUPS.forEach((group, g) => {
    const units = unitsOf(group);
    const from = units[units.length - 1];
    const to = units[Math.min(1, units.length - 1)];
    add(`preset ${group}`, { type: group, from, to, factor: [1, 2.5, 0.0254, 3][g % 4] }, [{ op: "confirm" }]);
    add(`pick type ${group}`, { type: null, factor: 1 }, [
      { op: "type", value: group },
      { op: "from", value: units[units.length - 1] },
      { op: "to", value: units[0] },
      { op: "confirm" }
    ]);
  });

  const factors = [1, 2, -1, 2.5, 0.0254, 1 / 3, 0.1 + 0.2, 1e21, 1e-7, 123456789.123, -0, NaN, Infinity, -Infinity, 5e-324, 2 ** 53 + 2];
  factors.forEach((factor, i) => {
    add(`factor text ${i}`, { type: i % 2 === 0 ? null : "angle", from: "degrees", to: "radians", factor }, [{ op: "confirm" }]);
  });

  const texts = ["2", "1/12", "1 / 0.0254", "2.54", " 3 ", "", " ", "abc", "x", "2 cm", "[1,2]", "1:3", "{a: 1}", "1;2", "1\n2", "x = 3", "f(x) = x", "true", "false", "null", "undefined", "NaN", "Infinity", "-Infinity", "-0", "0x10", "0xffi8", "1e400", "'5'", "\"5\"", "sqrt(-1)", "sqrt(4)", "i", "pi", "2pi", "180/pi", "1/0", "0/0", "50%", "5!", "2^0.5", "sin", "version", "1 +", "(2", "@", "# comment", "2 # half", "1 == 1", "max(1, 2)", "round(1.005, 2)", "number('5')", "1e3", ".5", "5.", "1,5"];
  texts.forEach((text, i) => {
    const group = GROUPS[i % GROUPS.length];
    const units = unitsOf(group);
    add(`confirm factor ${JSON.stringify(text)}`, { type: i % 3 === 0 ? null : group, from: units[0], to: units[units.length - 1], factor: 1 }, [
      { op: "factor", value: text },
      i % 2 === 0 ? { op: "confirm" } : { op: "key", value: "Enter" }
    ]);
  });

  add("alert then fix", { type: "velocity", from: "feet/second", to: "meters/second", factor: 3 }, [
    { op: "factor", value: "oops" },
    { op: "confirm" },
    { op: "factor", value: "[1]" },
    { op: "key", value: "Enter" },
    { op: "factor", value: "4/3" },
    { op: "confirm" }
  ]);
  add("type round trip", { type: "length", from: "inches", to: "meters", factor: 2 }, [
    { op: "type", value: "none" },
    { op: "type", value: "angle" },
    { op: "to", value: "rotations" },
    { op: "type", value: "length" },
    { op: "confirm" }
  ]);
  add("type change keeps nothing", { type: "time", from: "seconds", to: "milliseconds", factor: 1 }, [
    { op: "type", value: GROUPS[GROUPS.length - 1] },
    { op: "type", value: GROUPS[0] },
    { op: "factor", value: "10" },
    { op: "exit" }
  ]);
  add("none then exit", { type: "angle", from: "degrees", to: "radians", factor: 1 }, [
    { op: "type", value: "none" },
    { op: "factor", value: "2" },
    { op: "exit" }
  ]);
  add("same unit both sides", { type: "length", from: "feet", to: "feet", factor: 1 }, [{ op: "confirm" }]);
  add("unknown from", { type: "length", from: "furlongs", to: "feet", factor: 1 }, [{ op: "confirm" }]);
  add("unknown to", { type: "length", from: "feet", to: "parsecs", factor: 1 }, [{ op: "confirm" }]);
  add("missing from and to", { type: "length", factor: 1 }, [{ op: "confirm" }]);
  add("null type with units", { type: null, from: "feet", to: "meters", factor: 2 }, [{ op: "confirm" }]);
  add("unknown type", { type: "furlongs", from: "a", to: "b", factor: 1 }, [
    { op: "factor", value: "2" },
    { op: "confirm" },
    { op: "key", value: "Enter" },
    { op: "type", value: "angle" },
    { op: "confirm" },
    { op: "exit" }
  ]);
  add("missing factor", { type: null }, [{ op: "confirm" }, { op: "exit" }]);
  add("string factor", { type: null, factor: "2" }, [{ op: "confirm" }]);
  return list;
}

const STATE_SCRIPT = `(() => {
  const select = (id) => {
    const s = document.getElementById(id);
    return {
      options: Array.from(s.options).map((o) => ({ text: o.text, value: o.value })),
      value: s.value,
      selectedIndex: s.selectedIndex,
      disabled: s.disabled
    };
  };
  const rec = window.__rec;
  const record = {
    unitType: select("unitType"),
    fromUnit: select("fromUnit"),
    toUnit: select("toUnit"),
    extraFactor: document.getElementById("extraFactor").value,
    blurred: Array.from(document.getElementsByTagName("button")).map((b) => b.classList.contains("blurred")),
    replies: rec.replies.splice(0),
    alerts: rec.alerts.splice(0),
    errors: rec.errors.splice(0)
  };
  return record;
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

function stepScript(step: Step): string {
  const v = JSON.stringify(step.value);
  switch (step.op) {
    case "type":
      return `{ const s = document.getElementById("unitType"); s.value = ${v}; if (s.value !== ${v}) throw new Error("no option " + ${v}); s.dispatchEvent(new Event("change")); }`;
    case "from":
    case "to": {
      const id = step.op === "from" ? "fromUnit" : "toUnit";
      return `{ const s = document.getElementById("${id}"); s.value = ${v}; if (s.value !== ${v}) throw new Error("no option " + ${v}); }`;
    }
    case "factor":
      return `document.getElementById("extraFactor").value = ${v};`;
    case "confirm":
      return `document.getElementById("confirm").click();`;
    case "exit":
      return `document.getElementById("exit").click();`;
    case "key":
      return `window.dispatchEvent(new KeyboardEvent("keydown", { code: ${v} }));`;
    case "focus":
      return `window.__port.postMessage({ isFocused: ${v} });`;
  }
}

const SETTLE = `new Promise((resolve) => setTimeout(resolve, 0)).then(() => new Promise((resolve) => setTimeout(resolve, 0)))`;

// The Electron main process runs this script from a scratch file. A main process started from an
// ELECTRON_RUN_AS_NODE parent fails plain file:// loads, so file requests under upstream are
// fetched by the main process (as domBlinkMain does). Records come back through V8 serialization
// so NaN and -0 survive to toTree.
const ELECTRON_MAIN = `
import { app, BrowserWindow, net, protocol } from "electron";
import fs from "node:fs";
import path from "node:path";
import v8 from "node:v8";
import { fileURLToPath } from "node:url";
const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
app.setPath("userData", request.userData);
app.on("window-all-closed", () => {});
app.whenReady().then(async () => {
  const root = path.dirname(path.dirname(request.page));
  protocol.handle("file", (req) => {
    const file = fs.realpathSync(fileURLToPath(req.url));
    if (!file.startsWith(root + path.sep)) return new Response("", { status: 404 });
    return net.fetch(req, { bypassCustomProtocolHandlers: true });
  });
  const records = [];
  const win = new BrowserWindow({ show: false, width: 300, height: 162, useContentSize: true, webPreferences: { backgroundThrottling: false } });
  for (const flow of request.flows) {
    await win.loadFile(request.page);
    const js = (code) => win.webContents.executeJavaScript(code, true);
    await js(request.setup);
    await js(request.settle);
    await js("window.__port.postMessage(" + request.presetsJs[flow.name] + ")");
    await js(request.settle);
    records.push({ kind: "open", flow: flow.name, state: await js(request.state) });
    let replied = false;
    for (let i = 0; i < flow.steps.length; i++) {
      if (replied) throw new Error(flow.name + ": steps after a reply");
      await js(request.stepScripts[flow.name][i]);
      await js(request.settle);
      const state = await js(request.state);
      replied = state.replies.length > 0;
      records.push({ kind: "step", flow: flow.name, index: i, state });
    }
    records.push({ kind: "end", flow: flow.name, replied });
  }
  win.destroy();
  fs.writeFileSync(request.output, v8.serialize(records));
  app.exit(0);
}).catch((e) => {
  process.stderr.write("genUnitDialog main: " + (e && e.stack ? e.stack : e) + "\\n");
  app.exit(1);
});
`;

const jsLiteral = (value: any): string => {
  if (typeof value === "number") return Object.is(value, -0) ? "-0" : String(value);
  if (value === null || typeof value !== "object") return value === undefined ? "undefined" : JSON.stringify(value);
  return "{" + Object.keys(value).map((k) => JSON.stringify(k) + ":" + jsLiteral(value[k])).join(",") + "}";
};

const list = flows();
const names = new Set(list.map((f) => f.name));
if (names.size !== list.length) {
  process.stderr.write("genUnitDialog: duplicate flow names\n");
  process.exit(1);
}
const byName = new Map(list.map((f) => [f.name, f]));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-unit-dialog-"));
try {
  const requestPath = path.join(scratch, "request.json");
  const mainPath = path.join(scratch, "main.mjs");
  const output = path.join(scratch, "records.bin");
  fs.writeFileSync(mainPath, ELECTRON_MAIN);
  fs.writeFileSync(
    requestPath,
    JSON.stringify({
      flows: list,
      presetsJs: Object.fromEntries(list.map((f) => [f.name, jsLiteral(f.preset)])),
      stepScripts: Object.fromEntries(list.map((f) => [f.name, f.steps.map(stepScript)])),
      setup: SETUP_SCRIPT,
      settle: SETTLE,
      state: STATE_SCRIPT,
      page: fs.realpathSync(path.join(UPSTREAM_DIR, "www", "unitConversion.html")),
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
    process.stderr.write(`genUnitDialog: Electron exited with ${run.status ?? run.signal}\n`);
    process.exit(1);
  }
  const records: any[] = v8.deserialize(fs.readFileSync(output));
  for (const r of records) {
    const flow = byName.get(r.flow)!;
    if (r.kind === "open") out({ kind: "open", flow: r.flow, preset: toTree(flow.preset), state: toTree(r.state) });
    else if (r.kind === "step") out({ kind: "step", flow: r.flow, step: toTree(flow.steps[r.index]), state: toTree(r.state) });
    else out({ kind: "end", flow: r.flow, replied: r.replied });
  }
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
