// Writes the LineGraph controller fixtures, parity/specs/dom/line-graph/*.entry.json, and the
// logs they share, parity/specs/dom/logs/line-graph-*.json. Run from the repo root:
//   node oracle/tools/lineGraphSpecs.mjs
// The first step of every fixture installs RECORDER, which writes the {"lg": ...} records
// crates/asrs-tabs/tests/line_graph_dom.rs replays (oracle/README.md, "LineGraph controller").

import fs from "node:fs";
import path from "node:path";

const SPECS = "parity/specs/dom/line-graph";
const LOGS = "parity/specs/dom/logs";

const RECORDER = `/*lg:recorder*/ (() => {
  const w = window;
  const write = (record) => w.__oracleWrite(JSON.stringify(record));
  const view = new DataView(new ArrayBuffer(8));
  const tree = (v) => {
    if (v === undefined) return { u: null };
    if (v === null || typeof v === "boolean" || typeof v === "string") return v;
    if (typeof v === "number") {
      view.setFloat64(0, v);
      return { n: view.getUint32(0).toString(16).padStart(8, "0") + view.getUint32(4).toString(16).padStart(8, "0") };
    }
    if (typeof v === "function") return { fn: v.name };
    if (Array.isArray(v)) return v.map(tree);
    return { o: Object.keys(v).map((k) => [k, tree(v[k])]) };
  };
  const errorText = (e) => (e instanceof Error ? e.name + ": " + e.message : String(e));
  const dark = () => w.matchMedia("(prefers-color-scheme: dark)").matches;
  const frames = [];
  let selectionDepth = 0;
  const selection = w.selection;
  for (const name of ["getTimelineRange", "getMode", "getSelectedTime", "getHoveredTime", "getGrabZoomRange"]) {
    const original = selection[name];
    selection[name] = function (...args) {
      if (frames.length === 0 || selectionDepth > 0) return original.apply(this, args);
      selectionDepth++;
      try {
        const value = original.apply(this, args);
        frames[frames.length - 1].push([name, tree(value)]);
        return value;
      } finally {
        selectionDepth--;
      }
    };
  }
  const writes = [];
  let writeDepth = 0;
  for (const name of ["setHoveredTime", "setSelectedTime", "setGrabZoomRange", "finishGrabZoom", "goIdle", "applyTimelineScroll"]) {
    const original = selection[name];
    selection[name] = function (...args) {
      if (writes.length === 0 || writeDepth > 0) return original.apply(this, args);
      writes[writes.length - 1].push([name, tree(args)]);
      writeDepth++;
      try {
        return original.apply(this, args);
      } finally {
        writeDepth--;
      }
    };
  }
  const renderers = [];
  const style = (r) => [r.SCROLL_OVERLAY.style.top, r.SCROLL_OVERLAY.style.left, r.SCROLL_OVERLAY.style.right, r.HELP_OVERLAY.style.display, r.HELP_OVERLAY.classList.contains("fading-out")];
  const pointer = (r, target) => [r.ROOT.hidden, target instanceof w.Node && r.SCROLL_OVERLAY.contains(target), tree(r.ROOT.getBoundingClientRect().x), tree(r.SCROLL_OVERLAY.getBoundingClientRect().x), tree(r.SCROLL_OVERLAY.clientWidth)];
  const hookRenderer = (r, id) => {
    renderers.push([id, r]);
    const render = r.render;
    r.render = function (...args) {
      const canvas = r.CANVAS;
      const before = canvas.__oracleContext?.frames;
      const record = { lg: "render", id: tree(id), width: tree(canvas.clientWidth), height: tree(canvas.clientHeight), dpr: tree(w.devicePixelRatio), dark: dark(), prefs: tree(w.preferences === undefined ? undefined : [w.preferences.timestamps, w.preferences.hasScrolledLineGraph]) };
      writes.push([]);
      try {
        return render.apply(this, args);
      } catch (e) {
        record.threw = errorText(e);
        throw e;
      } finally {
        record.writes = writes.pop();
        const ctx = canvas.__oracleContext;
        if (ctx !== undefined && ctx.frames !== before) {
          record.size = tree([canvas.width, canvas.height]);
          record.calls = tree(ctx.calls);
        }
        record.style = style(r);
        write(record);
      }
    };
    const sensor = r.scrollSensor;
    const callback = sensor.callback;
    sensor.callback = function (...args) {
      const record = { lg: "scroll", id: tree(id), args: tree(args.slice(0, 3)), pointer: pointer(r, null), prefs: tree(w.preferences === undefined ? undefined : [w.preferences.timestamps, w.preferences.hasScrolledLineGraph]) };
      writes.push([]);
      try {
        return callback.apply(this, args);
      } finally {
        record.writes = writes.pop();
        record.style = style(r);
        write(record);
      }
    };
  };
  const dispatch = w.EventTarget.prototype.dispatchEvent;
  w.EventTarget.prototype.dispatchEvent = function (event) {
    if (renderers.length === 0 || !["mousemove", "mousedown", "mouseup", "click", "contextmenu", "mouseleave"].includes(event.type)) return dispatch.call(this, event);
    const record = { lg: "event", type: event.type, x: tree(event.clientX), shift: event.shiftKey, targets: renderers.map(([id, r]) => [tree(id), ...pointer(r, this)]) };
    writes.push([]);
    try {
      return dispatch.call(this, event);
    } finally {
      record.writes = writes.pop();
      write(record);
    }
  };
  const controllers = [];
  const lists = (c) => [["left", c.leftSourceList], ["right", c.rightSourceList], ["discrete", c.discreteSourceList]];
  const hook = (c, r) => {
    if (controllers.includes(c)) return;
    const id = controllers.length;
    controllers.push(c);
    write({ lg: "create", id: tree(id), uuids: lists(c).map(([, sl]) => sl.UUID) });
    hookRenderer(r, id);
    const observe = (name, kind, argCount) => {
      const original = c[name];
      c[name] = function (...args) {
        frames.push([]);
        const record = { lg: kind, id: tree(id), dark: dark(), ts: tree(w.preferences?.timestamps) };
        if (argCount > 0) record.args = tree(args.slice(0, argCount));
        try {
          const value = original.apply(this, args);
          record.value = tree(value);
          return value;
        } catch (e) {
          record.threw = errorText(e);
          throw e;
        } finally {
          record.reads = frames.pop();
          write(record);
        }
      };
    };
    observe("getCommand", "command", 0);
    observe("getPreview", "preview", 2);
    observe("saveState", "save", 0);
    observe("getActiveFields", "activeFields", 0);
    for (const name of ["restoreState", "editAxis", "clearAxis", "setRobotModeVisible", "refresh"]) {
      const original = c[name];
      c[name] = function (...args) {
        write({ lg: name, id: tree(id), phase: "begin", args: tree(args), dark: dark() });
        const end = { lg: name, id: tree(id), phase: "end" };
        try {
          return original.apply(this, args);
        } catch (e) {
          end.threw = errorText(e);
          throw e;
        } finally {
          write(end);
        }
      };
    }
  };
  w.addEventListener("click", (event) => {
    const button = event.target instanceof w.Element ? event.target.closest("button.edit") : null;
    if (button === null) return;
    controllers.forEach((c, id) => {
      for (const [list, sl] of lists(c)) {
        if (!sl.ROOT.contains(button)) continue;
        const r = button.getBoundingClientRect();
        write({ lg: "edit", id: tree(id), list, rect: tree([r.x, r.y, r.width, r.height]) });
      }
    });
  }, true);
  w.tabs.tabList.forEach((t) => t.type === 1 && hook(t.controller, t.renderer));
  const addTab = w.tabs.addTab;
  w.tabs.addTab = function (...args) {
    const result = addTab.apply(this, args);
    this.tabList.forEach((t) => t.type === 1 && hook(t.controller, t.renderer));
    return result;
  };
})()`.replace(/\n\s*/g, " ");

const scratch = new DataView(new ArrayBuffer(8));
const bits = (x) => {
  scratch.setFloat64(0, x);
  return scratch.getUint32(0).toString(16).padStart(8, "0") + scratch.getUint32(4).toString(16).padStart(8, "0");
};
const num = (x) => (Number.isFinite(x) && !Object.is(x, -0) ? x : { n: bits(x) });
const tree = (v) => {
  if (v === undefined) return { u: null };
  if (v === null || typeof v === "boolean" || typeof v === "string") return v;
  if (typeof v === "number") return { n: bits(v) };
  if (Array.isArray(v)) return v.map(tree);
  return { o: Object.keys(v).map((k) => [k, tree(v[k])]) };
};
const r6 = (x) => Math.round(x * 1e6) / 1e6;

// Logs

const logs = {};

logs.numeric = (() => {
  const ops = [];
  for (let i = 0; i <= 20; i++) {
    const t = i * 0.5;
    ops.push(["putNumber", "/Drive/LeftPositionMeters", t, r6(Math.sin(i * 0.7) * 3 + i * 0.1)]);
    ops.push(["putNumber", "/Drive/VelocityMetersPerSec", t + 0.25, r6(Math.cos(i * 0.45) * 2.5)]);
  }
  for (let i = 0; i <= 10; i++) {
    ops.push(["putNumber", "/Drive/RightPositionInches", i + 0.1, r6(i * i * 1.7 - 20)]);
    ops.push(["putNumber", "/Arm/AngleRadians", i * 0.9, r6(((i * 37) % 11) * 0.31 - 1.2)]);
    ops.push(["putNumber", "/Arm/AngleDegrees", i * 0.95 + 0.05, r6(((i * 53) % 17) * 13.5 - 90)]);
    ops.push(["putNumber", "/Motor/Temp", i, 20 + i * 2.5]);
    ops.push(["putNumber", "/Motor/TempF", i + 0.5, 68 + i * 4.5]);
    ops.push(["putNumber", "/Drive/Accel", i * 0.8, r6(Math.sin(i) * 9.8)]);
    ops.push(["putNumber", "/Drive/Duty", i, r6(i / 10)]);
    ops.push(["putNumber", "/Misc/Plain", i * 0.7 + 0.2, i % 3 === 0 ? 1 : r6(i * 0.37 - 1)]);
  }
  ops.push(["setMetadataString", "/Motor/Temp", '{"unit":"celsius"}']);
  ops.push(["setMetadataString", "/Motor/TempF", '{"unit":"fahrenheit"}']);
  ops.push(["setMetadataString", "/Drive/Accel", '{"unit":"mps2"}']);
  ops.push(["setMetadataString", "/Drive/Duty", '{"units":"percent"}']);
  const weird = [0, 1e25, -1e25, NaN, -0, 3, Infinity, -Infinity, 1e-9, 7];
  weird.forEach((v, i) => ops.push(["putNumber", "/Misc/Weird", i, num(v)]));
  ops.push(["putString", "/Misc/Name", 0, "alpha"]);
  ops.push(["putString", "/Misc/Name", 5, "beta"]);
  return ops;
})();

logs.discrete = (() => {
  const ops = [];
  [0.3, 1.1, 2.0, 2.05, 4.4, 6.0, 7.5, 9.9].forEach((t, i) => ops.push(["putBoolean", "/D/Bool", t, i % 3 !== 1]));
  [0.5, 1.5, 3, 4.5, 6, 7.5, 9].forEach((t, i) => ops.push(["putNumber", "/D/Number", t, num([1, 2.5, NaN, -0, 1e21, 1e-7, 123456.789][i])]));
  ["", 'quote "x"', "tab\tnew\nline", "\u00e9\u4e2d\ud83d\ude00", "a\\b", "\u0001ctl", "end"].forEach((s, i) =>
    ops.push(["putString", "/D/String", i * 1.4 + 0.2, s])
  );
  ["", "00ff10", "deadbeef", "01"].forEach((hex, i) => ops.push(["putRaw", "/D/Raw", i * 2.5, { b: hex }]));
  [[true, false], [], [false]].forEach((v, i) => ops.push(["putBooleanArray", "/D/BoolArr", i * 3 + 0.7, v]));
  [[1, 2, 3], [num(NaN), num(Infinity), num(-0)], Array.from({ length: 503 }, (_, k) => k)].forEach((v, i) =>
    ops.push(["putNumberArray", "/D/NumArr", i * 3 + 1, v])
  );
  [["a", "b"], ['q"', ""], Array.from({ length: 501 }, (_, k) => "s" + k)].forEach((v, i) =>
    ops.push(["putStringArray", "/D/StrArr", i * 3 + 1.5, v])
  );
  [0, 1, 2, 3, 4, 5, 6, 7, 8].forEach((t) => ops.push(["putNumber", "/D/Steady", t, t % 2]));
  ops.push(["putNumber", "/Numeric/Value", 0, 1]);
  ops.push(["putNumber", "/Numeric/Value", 10, 2]);
  return ops;
})();

logs.robot = (() => {
  const ops = [];
  const enabled = [[0, false], [2, true], [5, true], [6, false], [8, true], [8, false], [9, true], [11, false]];
  enabled.forEach(([t, v]) => ops.push(["putBoolean", "/DriverStation/Enabled", t, v]));
  [[0, true], [3.5, false], [9, false]].forEach(([t, v]) => ops.push(["putBoolean", "/DriverStation/Autonomous", t, v]));
  [[0, false], [8.5, true], [10, false]].forEach(([t, v]) => ops.push(["putBoolean", "/DriverStation/Test", t, v]));
  for (let i = 0; i <= 12; i++) ops.push(["putNumber", "/Drive/Speed", i, r6(Math.sin(i) * 4)]);
  return ops;
})();

logs.alerts = (() => {
  const ops = [];
  const series = {
    errors: [[0, []], [1, ["Motor fault"]], [2, ["Motor fault", "Motor fault"]], [3, ["Motor fault", "CAN down"]], [5, ["CAN down"]], [7, []], [8, ["Late"]]],
    warnings: [[0.5, ["Battery low"]], [1.5, ["Battery low", "Hot"]], [2.5, ["Hot", "Battery low", "Hot"]], [6, ["Hot"]], [9.5, []]],
    infos: [[0, ["Hello"]], [4, ["Hello", "World"]], [4.5, ["World"]]]
  };
  for (const [kind, samples] of Object.entries(series)) {
    for (const [t, v] of samples) ops.push(["putStringArray", "/Alerts/" + kind, t, v]);
  }
  ops.push(["putStringArray", "/Other/warnings", 3, ["Other group"]]);
  ops.push(["putStringArray", "/Other/warnings", 4, []]);
  ops.push(["putNumber", "/Drive/Speed", 0, 0]);
  ops.push(["putNumber", "/Drive/Speed", 10, 1]);
  return ops;
})();

logs.akit = (() => {
  const ops = [];
  for (let i = 0; i <= 100; i++) ops.push(["putNumber", "/Timestamp", r6(i * 0.02), 1767225600 + r6(i * 0.02)]);
  [0.0, 0.07, 0.33, 0.5, 0.51, 1.2, 1.9].forEach((t, i) => ops.push(["putNumber", "/Drive/Velocity", t, r6(i * 0.8 - (i % 2) * 1.3)]));
  [0.1, 0.9, 1.5].forEach((t, i) => ops.push(["putNumber", "/Drive/Position", t, i * 2]));
  ops.push(["putNumber", "/Drive/Late", 2.5, 4]);
  ops.push(["putNumber", "/Drive/Late", 3.0, 5]);
  return ops;
})();

logs["akit-nt"] = (() => {
  const ops = [];
  ops.push(["putNumber", "/Other/Value", 0, 3]);
  for (let i = 0; i <= 40; i++) ops.push(["putNumber", "NT:/AdvantageKit/Timestamp", r6(i * 0.05), i]);
  [0.03, 0.4, 1.01, 1.6].forEach((t, i) => ops.push(["putNumber", "NT:/AdvantageKit/Drive/X", t, i * i]));
  return ops;
})();

logs.huge = (() => {
  const ops = [];
  for (let i = 0; i < 4000; i++) {
    ops.push(["putNumber", "/Big/Sine", r6(i * 0.005), r6(Math.sin(i * 0.013) * 100)]);
    if (i % 4 === 0) ops.push(["putNumber", "/Big/Saw", r6(i * 0.005 + 0.001), (i % 400) - 200]);
  }
  for (let i = 0; i < 300; i++) ops.push(["putBoolean", "/Big/Toggle", r6(i * 0.066), i % 2 === 0]);
  return ops;
})();

logs.dense = (() => {
  const ops = [];
  for (let i = 0; i <= 40; i++) ops.push(["putBoolean", "/Dense/Toggle", r6(1 + i * 0.008), i % 2 === 0]);
  for (let i = 0; i <= 30; i++) ops.push(["putBoolean", "/Dense/Toggle", r6(2 + i * 0.016), i % 2 === 1]);
  for (let i = 0; i <= 2500; i++) ops.push(["putNumber", "/Dense/Signal", r6(i * 0.004), r6(Math.sin(i * 0.37) * 2 + Math.sin(i * 0.022) * 5)]);
  return ops;
})();
logs.magnitudes = [
  ...[0.1, 0.35, 0.6, 0.85].map((f, i) => ["putNumber", "/Big/Mid", i * 3, 2e8 + f]),
  ...[0.1, 40.4, 80.7].map((f, i) => ["putNumber", "/Big/Over", i * 4, 3e9 + f]),
  ...[0.1, 0.4, 0.7].map((f, i) => ["putNumber", "/Big/Fine", i * 4, 3e9 + f]),
  ["putNumber", "/Big/Flat", 0, 5],
  ["putNumber", "/Big/Flat", 9, 5]
];
logs.vast = [2e13, 2e13 + 4, 2e13 + 8].map((t, i) => ["putNumber", "/Vast/Value", t, [1, 3, 2][i]]);
logs.offset = [["putNumber", "/A", 100, 1], ["putNumber", "/A", 101, 2]];
logs.blank = [["createBlankField", "/Blank", 2], ["putNumber", "/One", 3, 1]];
logs.live = [["putNumber", "/Live/Value", 0, 1], ["putBoolean", "/Live/Flag", 0, true], ["putNumber", "/Timestamp", 0, 0]];

// Steps

const sel = (...call) => ({ selection: call });
const frames = (n = 2) => ({ frames: n });
const lg = (expr) => `window.tabs.tabList[window.tabs.selectedTab].controller.${expr}`;
const add = (list, key) => ({ eval: lg(`${list}SourceList.addField(${JSON.stringify(key)})`) });
const capture = (label, extra = []) => ({
  capture: ["commands", "state", "sourceLists", { kind: "eval", expr: "[...window.tabs.getActiveFields()]" }, ...extra],
  label
});
const edit = (legend, lockedRange, unitConversion, filter) => ({
  main: "edit-axis",
  cve1: tree({ legend, lockedRange, unitConversion, filter })
});
const units = (autoTarget = null, preset = null) => ({ autoTarget, preset });
const item = (type, logKey, logType, options = {}, visible = true) => ({ type, logKey, logType, visible, options });
const colors = ["#2b66a2", "#e5b31b", "#af2437", "#80588e", "#e48b32", "#c0b487", "#858584", "#3b875a", "#d993aa", "#5f4528"];
const num0 = (type, key, color = 0, size = "normal") =>
  item(type, key, "Number", type === "points" ? { color: colors[color] } : { color: colors[color], size });
const controller = (overrides) => ({
  leftSources: [],
  rightSources: [],
  discreteSources: [],
  leftLockedRange: null,
  rightLockedRange: null,
  leftUnitConversion: units(),
  rightUnitConversion: units(),
  leftFilter: 0,
  rightFilter: 0,
  showRobotMode: false,
  ...overrides
});
const restore = (...controllers) => ({
  main: "restore-state",
  cve1: tree({
    sidebar: { width: 300, expanded: [] },
    tabs: {
      selected: 0,
      tabs: controllers.map((c, i) => ({
        type: 1,
        title: "",
        controller: c,
        controllerUUID: "linegraph" + String(i).padStart(23, "0"),
        renderer: null,
        controlsHeight: 200
      }))
    }
  })
});
const click = (target, index = 0) => ({ event: { target, index, kind: "MouseEvent", type: "click" } });
const hoverSweep = (times) => times.flatMap((t) => [sel("setHoveredTime", num(t)), frames(1)]);

const fixtures = {};

fixtures.basic = {
  seed: 11,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    add("left", "/Drive/LeftPositionMeters"),
    add("left", "/Drive/VelocityMetersPerSec"),
    add("right", "/Arm/AngleDegrees"),
    add("right", "/Misc/Plain"),
    frames(),
    capture("added"),
    sel("setSelectedTime", 2.5),
    frames(),
    ...hoverSweep([0, 0.1, 3.3, 9.99, 10, 11, -1]),
    capture("hovered"),
    sel("setTimelineRange", [2.2, 6.9], false),
    frames(),
    ...hoverSweep([2, 2.2, 4, 6.9, 7]),
    capture("zoomed"),
    sel("goIdle"),
    sel("setHoveredTime", null),
    frames(),
    capture("idle")
  ]
};

fixtures["types-and-options"] = {
  seed: 12,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({
        leftSources: [
          num0("stepped", "/Drive/LeftPositionMeters", 0, "bold"),
          num0("smooth", "/Drive/VelocityMetersPerSec", 1, "verybold"),
          num0("points", "/Arm/AngleRadians", 2),
          num0("smooth", "/Drive/RightPositionInches", 3),
          item("smooth", "/Drive/Accel", "Number", { color: colors[4] }),
          num0("stepped", "/Misc/Plain", 5, "normal")
        ],
        rightSources: [
          num0("points", "/Arm/AngleDegrees", 6),
          num0("smooth", "/Motor/Temp", 7),
          num0("stepped", "/Motor/TempF", 8),
          item("weird", "/Drive/Duty", "Number", { color: colors[9] }),
          num0("smooth", "/Misc/Plain", 9)
        ]
      })
    ),
    frames(),
    capture("restored"),
    sel("setTimelineRange", [1.05, 4.6], false),
    sel("setSelectedTime", 3.3),
    frames(),
    ...hoverSweep([1, 1.05, 2.7, 4.6, 4.61]),
    capture("zoomed"),
    click("div.line-graph-left div.item button.hide", 1),
    click("div.line-graph-right div.item button.hide", 0),
    frames(),
    capture("hidden"),
    click("div.line-graph-left div.item button.type", 0),
    {
      eval: lg(
        `leftSourceList.constructor.typePromptCallbacks[${lg("leftSourceList.UUID")}]({ type: "smooth", logKey: "/Drive/LeftPositionMeters", logType: "Number", visible: true, options: { color: "#3b875a", size: "verybold" } })`
      )
    },
    click("div.line-graph-left div.item button.remove", 2),
    frames(),
    capture("retyped")
  ]
};

fixtures["dark-theme"] = {
  seed: 13,
  logs: ["numeric", "robot"],
  window: { theme: "dark" },
  steps: [
    { log: "numeric" },
    { log: "robot" },
    restore(
      controller({
        leftSources: colors.slice(0, 5).map((c, i) => item("stepped", ["/Drive/LeftPositionMeters", "/Drive/VelocityMetersPerSec", "/Arm/AngleRadians", "/Misc/Plain", "/Drive/Speed"][i], "Number", { color: c, size: "normal" })),
        rightSources: colors.slice(5).map((c, i) => item("smooth", ["/Arm/AngleDegrees", "/Motor/Temp", "/Drive/Accel", "/Drive/Duty", "/Drive/RightPositionInches"][i], "Number", { color: c, size: "bold" })),
        discreteSources: [item("stripes", "/DriverStation/Enabled", "Boolean", { color: "#ff0000" }), item("graph", "/Drive/Speed", "Number", { color: "notacolor" })],
        showRobotMode: true
      })
    ),
    frames(),
    capture("dark")
  ]
};

fixtures.discrete = {
  seed: 14,
  logs: ["discrete"],
  steps: [
    { log: "discrete" },
    ...["/D/Bool", "/D/Number", "/D/String", "/D/Raw", "/D/BoolArr", "/D/NumArr", "/D/StrArr", "/D/Steady"].map((k) => add("discrete", k)),
    frames(),
    capture("stripes"),
    sel("setTimelineRange", [1.2, 6.1], false),
    frames(),
    capture("zoomed"),
    sel("setTimelineRange", [2.05, 2.05001], false),
    frames(),
    capture("narrow"),
    sel("setTimelineRange", [0, 12], false),
    restore(
      controller({
        discreteSources: [
          item("graph", "/D/Bool", "Boolean", { color: colors[0] }),
          item("graph", "/D/Number", "Number", { color: colors[1] }),
          item("graph", "/D/String", "String", { color: colors[2] }),
          item("graph", "/D/Raw", "Raw", { color: colors[3] }),
          item("graph", "/D/BoolArr", "BooleanArray", { color: colors[4] }, false),
          item("graph", "/D/NumArr", "NumberArray", { color: colors[5] }),
          item("stripes", "/D/StrArr", "StringArray", { color: colors[6] }),
          item("stripes", "/D/Missing", "Number", { color: colors[7] }),
          item("stripes", "/D/Steady", "Number", { color: colors[8] })
        ],
        leftSources: [num0("stepped", "/D/Number", 0), num0("smooth", "/D/String", 1)]
      })
    ),
    frames(),
    capture("graphs"),
    { log: [["clearBeforeTime", 2.5, true]] },
    frames(),
    capture("cleared")
  ]
};

fixtures.filters = {
  seed: 15,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0), num0("smooth", "/Drive/VelocityMetersPerSec", 1), num0("points", "/Misc/Weird", 2)],
        rightSources: [num0("smooth", "/Drive/Accel", 3), num0("points", "/Arm/AngleRadians", 4), num0("stepped", "/Misc/Plain", 5)]
      })
    ),
    frames(),
    edit("left", null, units(), 1),
    edit("right", null, units(), 2),
    frames(),
    ...hoverSweep([0.4, 3.1, 9.5]),
    capture("full"),
    sel("setTimelineRange", [2.6, 7.4], false),
    frames(),
    ...hoverSweep([2.6, 2.7, 5, 7.4]),
    capture("zoomed"),
    edit("left", null, units(), 2),
    edit("right", null, units(), 1),
    frames(),
    ...hoverSweep([3, 7]),
    capture("swapped"),
    sel("setTimelineRange", [9.6, 9.9], false),
    frames(),
    capture("tail"),
    sel("setTimelineRange", [-3, -1], false),
    frames(),
    capture("before"),
    edit("left", null, units(), 0),
    edit("right", null, units(), 7),
    sel("setTimelineRange", [0, 10], false),
    frames(),
    ...hoverSweep([4]),
    capture("unknown-filter")
  ]
};

fixtures.akit = {
  seed: 16,
  logs: ["akit"],
  steps: [
    { log: "akit" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/Velocity", 0), num0("smooth", "/Drive/Position", 1), num0("points", "/Drive/Late", 2)],
        rightSources: [num0("smooth", "/Drive/Velocity", 3), num0("points", "/Drive/Position", 4), num0("stepped", "/Timestamp", 5)]
      })
    ),
    frames(),
    ...hoverSweep([0.05, 0.5, 1.95]),
    capture("plain"),
    sel("setTimelineRange", [0.31, 1.33], false),
    frames(),
    ...hoverSweep([0.31, 0.32, 1.0]),
    capture("zoomed"),
    edit("left", null, units(), 1),
    edit("right", null, units(), 2),
    frames(),
    ...hoverSweep([0.5, 1.2]),
    capture("filtered"),
    sel("setTimelineRange", [0, 4], false),
    frames(),
    capture("past-akit"),
    edit("left", null, units(), 9),
    frames(),
    capture("unknown-filter")
  ]
};

fixtures["akit-nt"] = {
  seed: 17,
  logs: ["akit-nt"],
  steps: [
    { log: "akit-nt" },
    add("left", "NT:/AdvantageKit/Drive/X"),
    add("right", "/Other/Value"),
    frames(),
    restore(
      controller({
        leftSources: [num0("smooth", "NT:/AdvantageKit/Drive/X", 0), num0("stepped", "NT:/AdvantageKit/Drive/X", 1)],
        rightSources: [num0("points", "/Other/Value", 2)],
        rightFilter: 1
      })
    ),
    frames(),
    ...hoverSweep([0.5, 1.7]),
    capture("nt-akit")
  ]
};

fixtures["units-auto"] = {
  seed: 18,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0), num0("smooth", "/Drive/RightPositionInches", 1)],
        rightSources: [num0("stepped", "/Arm/AngleRadians", 2), num0("smooth", "/Arm/AngleDegrees", 3)]
      })
    ),
    frames(),
    ...hoverSweep([1, 4.4]),
    capture("consistent"),
    edit("left", null, units("inches", null), 0),
    edit("right", null, units("rotations", null), 0),
    frames(),
    ...hoverSweep([1, 4.4, 9.5]),
    capture("targets"),
    edit("left", null, units("feet", null), 1),
    edit("right", null, units("degrees", null), 2),
    frames(),
    ...hoverSweep([3.5, 8.1]),
    capture("filtered-targets"),
    edit("left", null, units("radians", null), 0),
    edit("right", null, units("bogus", null), 0),
    frames(),
    capture("mismatched-targets"),
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0), num0("smooth", "/Arm/AngleRadians", 1)],
        rightSources: [num0("stepped", "/Misc/Plain", 2), num0("smooth", "/Drive/LeftPositionMeters", 3)],
        leftUnitConversion: units("meters", null)
      })
    ),
    frames(),
    ...hoverSweep([2]),
    capture("inconsistent"),
    restore(
      controller({
        leftSources: [num0("stepped", "/Motor/Temp", 0), num0("smooth", "/Motor/TempF", 1)],
        rightSources: [num0("stepped", "/Drive/Accel", 2), num0("smooth", "/Drive/VelocityMetersPerSec", 3)],
        leftUnitConversion: units("kelvin", null)
      })
    ),
    frames(),
    ...hoverSweep([1, 6.5]),
    capture("temperature"),
    edit("right", null, units(null, null), 1),
    edit("left", null, units("fahrenheit", null), 2),
    frames(),
    ...hoverSweep([2, 5]),
    capture("accel-filters"),
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/Duty", 0)],
        rightSources: [num0("smooth", "/Arm/AngleRadians", 2), item("smooth", "/Arm/AngleDegrees", "Number", { color: colors[3] }, false)]
      })
    ),
    frames(),
    ...hoverSweep([0.9, 1.0, 2.7]),
    capture("percent-and-hidden"),
    edit("left", null, units(null, null), 2),
    frames(),
    ...hoverSweep([4]),
    capture("percent-integrated")
  ]
};

const presetOf = (type, factor, from, to) => {
  const p = { type, factor };
  if (from !== undefined) Object.assign(p, { from, to });
  return p;
};

fixtures["units-manual"] = {
  seed: 19,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0), num0("smooth", "/Misc/Plain", 1)],
        rightSources: [num0("stepped", "/Arm/AngleRadians", 2)]
      })
    ),
    frames(),
    edit("left", null, units(null, presetOf(null, 1)), 0),
    edit("right", null, units(null, presetOf(null, 2.5)), 0),
    frames(),
    ...hoverSweep([3]),
    capture("noop-and-factor"),
    edit("left", null, units(null, { type: "length", factor: 1, from: "meters", to: "inches" }), 0),
    edit("right", null, units(null, { type: "angle", from: "radians", to: "degrees", factor: 0.5 }), 1),
    frames(),
    ...hoverSweep([3, 6]),
    capture("from-to"),
    edit("left", null, units(null, { type: "temperature", factor: 1, from: "celsius", to: "fahrenheit" }), 2),
    edit("right", null, units("degrees", { factor: 3 }), 0),
    frames(),
    ...hoverSweep([5]),
    capture("temperature-and-untyped"),
    edit("left", null, units(null, { type: "length", factor: 1, from: "meters", to: "" }), 0),
    frames(),
    ...hoverSweep([5]),
    capture("empty-to"),
    edit("left", null, units(null, { type: "length", factor: 1, from: "bogus", to: "meters" }), 0),
    frames(),
    capture("invalid-unit"),
    edit("left", null, units(null, null), 0),
    edit("right", null, units(null, presetOf(null, 1)), 0),
    frames(),
    capture("reset")
  ]
};

fixtures["unknown-unit"] = {
  seed: 32,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0), num0("smooth", "/Misc/Plain", 1)],
        rightSources: [num0("stepped", "/Arm/AngleRadians", 2)]
      })
    ),
    frames(),
    edit("left", null, units(null, { type: null, factor: 2, to: "bogus" }), 0),
    edit("right", null, units(null, presetOf(null, 2.5)), 0),
    frames(),
    ...hoverSweep([3, 6]),
    capture("left-stopped"),
    edit("left", null, units(null, null), 0),
    frames(),
    ...hoverSweep([4]),
    capture("left-stays-stopped")
  ]
};

fixtures.locked = {
  seed: 20,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0)],
        rightSources: [num0("smooth", "/Arm/AngleRadians", 1)]
      })
    ),
    frames(),
    edit("left", [null, null], units(), 0),
    frames(),
    capture("lock-current"),
    edit("right", [-5, 5], units(), 1),
    sel("setTimelineRange", [3, 4], false),
    frames(),
    capture("locked-both"),
    edit("left", [2, 2], units(), 0),
    edit("right", [1e25, 2e25], units(), 0),
    frames(),
    capture("degenerate"),
    edit("left", [-2e25, -1e25], units(), 0),
    edit("right", [-1e21, 1e21], units(), 0),
    frames(),
    capture("huge"),
    edit("left", [1, 1.000001], units(), 0),
    edit("right", [NaN, Infinity], units(), 0),
    frames(),
    capture("tiny-and-nonfinite"),
    edit("left", [-Infinity, 3], units(), 0),
    edit("right", [5, -5], units(), 0),
    frames(),
    capture("inverted"),
    edit("left", null, units(), 0),
    { main: "clear-axis", data: "right" },
    edit("right", [0, 1], units(), 0),
    frames(),
    capture("right-locked-only"),
    edit("right", null, units(), 0),
    frames(),
    capture("left-only"),
    { main: "clear-axis", data: "left" },
    frames(),
    capture("empty-axes"),
    restore(controller({ rightSources: [num0("stepped", "/Misc/Weird", 0)], leftSources: [num0("stepped", "/Misc/Name", 1)] })),
    sel("setTimelineRange", [0, 12], false),
    frames(),
    capture("weird-values"),
    restore(controller({ leftSources: [num0("stepped", "/Misc/Weird", 0)] })),
    sel("setTimelineRange", [1.5, 2.5], false),
    frames(),
    capture("clamped-only")
  ]
};

fixtures["robot-mode"] = {
  seed: 21,
  logs: ["robot"],
  steps: [
    { log: "robot" },
    add("discrete", "/DriverStation/Enabled"),
    add("left", "/Drive/Speed"),
    { main: "set-robot-mode-visible", data: { showRobotMode: true } },
    frames(),
    capture("shown"),
    ...[[2.5, 7], [5, 5.5], [6, 8], [8, 8.2], [10.5, 14], [-2, 0], [-2, -1], [11, 20]].flatMap(([a, b]) => [
      sel("setTimelineRange", [a, b], false),
      frames(1)
    ]),
    capture("ranges"),
    { main: "set-robot-mode-visible", data: { showRobotMode: false } },
    frames(),
    capture("hidden"),
    { main: "set-robot-mode-visible", data: { showRobotMode: 1 } },
    frames(),
    capture("truthy")
  ]
};

fixtures.alerts = {
  seed: 22,
  logs: ["alerts"],
  steps: [
    { log: "alerts" },
    restore(
      controller({
        discreteSources: [
          item("alerts", "/Alerts", "Alerts", {}),
          item("alerts", "/Other", "Alerts", {}),
          item("alerts", "/Hidden", "Alerts", {}, false),
          item("stripes", "/Alerts/errors", "StringArray", { color: colors[0] })
        ],
        leftSources: [num0("stepped", "/Drive/Speed", 0)]
      })
    ),
    frames(),
    capture("all"),
    sel("setTimelineRange", [2.2, 5.5], false),
    frames(),
    capture("zoomed"),
    sel("setTimelineRange", [8.5, 12], false),
    frames(),
    capture("late"),
    restore(controller({ discreteSources: [item("alerts", "/Other", "Alerts", {}, false), item("alerts", "/Alerts", "Alerts", {})] })),
    sel("setTimelineRange", [0, 10], false),
    frames(),
    capture("reordered")
  ]
};

fixtures["edit-prompt"] = {
  seed: 23,
  logs: ["numeric"],
  layout: [
    { selector: "div.line-graph-left > button.edit", rect: [301.5, 452, 20, 20] },
    { selector: "div.line-graph-right > button.edit", rect: [700, 452.25, 20, 20] },
    { selector: "div.line-graph-discrete > button.edit", rect: [500, 451, 18, 22] }
  ],
  steps: [
    { log: "numeric" },
    click("div.line-graph-left > button.edit"),
    click("div.line-graph-right > button.edit"),
    click("div.line-graph-discrete > button.edit"),
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0)],
        rightSources: [num0("stepped", "/Arm/AngleRadians", 1), num0("stepped", "/Arm/AngleDegrees", 2)],
        showRobotMode: true,
        leftLockedRange: [1, 2]
      })
    ),
    click("div.line-graph-left > button.edit"),
    click("div.line-graph-right > button.edit"),
    click("div.line-graph-discrete > button.edit"),
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0), num0("stepped", "/Arm/AngleRadians", 1)],
        rightSources: [num0("stepped", "/Misc/Plain", 1), num0("stepped", "/Drive/RightPositionInches", 2)],
        leftUnitConversion: units("inches", null),
        rightUnitConversion: units(null, { type: "length", factor: 2, from: "meters", to: "feet" }),
        rightFilter: 2
      })
    ),
    click("div.line-graph-left > button.edit"),
    click("div.line-graph-right > button.edit"),
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/RightPositionInches", 0), num0("stepped", "/Drive/LeftPositionMeters", 1)],
        rightSources: [num0("stepped", "/Drive/RightPositionInches", 2), item("stepped", "/Drive/LeftPositionMeters", "Number", { color: colors[3] }, false), num0("stepped", "/Gone", 4)],
        leftUnitConversion: units("inches", presetOf(null, 1)),
        rightUnitConversion: units("meters", null)
      })
    ),
    click("div.line-graph-left > button.edit"),
    click("div.line-graph-right > button.edit"),
    frames(),
    capture("prompts")
  ]
};

fixtures["clear-and-legends"] = {
  seed: 24,
  logs: ["numeric", "discrete"],
  steps: [
    { log: "numeric" },
    { log: "discrete" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0)],
        rightSources: [num0("stepped", "/Arm/AngleRadians", 1)],
        discreteSources: [item("stripes", "/D/Bool", "Boolean", { color: colors[2] })]
      })
    ),
    frames(),
    { main: "clear-axis", data: "middle" },
    edit("middle", [1, 2], units("inches", null), 1),
    frames(),
    capture("bogus-legend"),
    { main: "clear-axis", data: "discrete" },
    frames(),
    capture("cleared-discrete"),
    { main: "clear-axis", data: "left" },
    { main: "clear-axis", data: "right" },
    frames(),
    capture("cleared-all"),
    add("left", "/Drive/VelocityMetersPerSec"),
    add("right", "/Drive/VelocityMetersPerSec"),
    add("discrete", "/Drive/VelocityMetersPerSec"),
    frames(),
    capture("re-added")
  ]
};

fixtures["restore-values"] = {
  seed: 25,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0)],
        leftLockedRange: [-1, 4],
        rightLockedRange: [0, 0],
        leftUnitConversion: units("inches", null),
        rightUnitConversion: { preset: { factor: 1, type: null }, autoTarget: "feet" },
        leftFilter: 1,
        rightFilter: 2,
        showRobotMode: "yes"
      })
    ),
    frames(),
    capture("full"),
    restore({ leftFilter: 2 }, { rightLockedRange: [5, 6], showRobotMode: 0 }),
    frames(),
    capture("partial"),
    restore(null, [1, 2]),
    frames(),
    capture("non-objects"),
    restore(controller({ leftSources: [item("bogus", "/Drive/LeftPositionMeters", "Number", { color: colors[0] })] })),
    frames(),
    capture("unknown-type"),
    restore(controller({ rightUnitConversion: { autoTarget: null } })),
    frames(),
    capture("missing-preset")
  ]
};

fixtures.empty = {
  seed: 26,
  logs: ["blank"],
  steps: [
    frames(),
    capture("blank"),
    restore(
      controller({
        leftSources: [num0("stepped", "/Nope", 0), num0("smooth", "/Also/Nope", 1)],
        rightSources: [num0("points", "/Nope", 2)],
        discreteSources: [item("stripes", "/Nope", "Number", { color: colors[3] }), item("alerts", "/NoAlerts", "Alerts", {})],
        showRobotMode: true
      })
    ),
    frames(),
    ...hoverSweep([0, 5]),
    capture("missing-fields"),
    { log: "blank" },
    { eval: lg('discreteSourceList.addField("/One")') },
    restore(controller({ leftSources: [num0("smooth", "/One", 0), num0("stepped", "/Blank", 1)], discreteSources: [item("stripes", "/Blank", "Number", { color: colors[0] })] })),
    frames(),
    ...hoverSweep([2, 3, 4]),
    capture("one-sample")
  ]
};

fixtures.huge = {
  seed: 27,
  logs: ["huge"],
  steps: [
    { log: "huge" },
    restore(
      controller({
        leftSources: [num0("smooth", "/Big/Sine", 0), num0("stepped", "/Big/Saw", 1)],
        rightSources: [num0("points", "/Big/Saw", 2)],
        discreteSources: [item("stripes", "/Big/Toggle", "Boolean", { color: colors[3] })],
        rightFilter: 2
      })
    ),
    sel("setTimelineRange", [7.5, 7.75], false),
    frames(1),
    ...hoverSweep([7.6]),
    capture("window"),
    edit("left", null, units(), 1),
    sel("setTimelineRange", [19.9, 21], false),
    frames(1),
    capture("end")
  ]
};

fixtures.live = {
  seed: 28,
  logs: ["live"],
  steps: [
    { log: "live" },
    restore(
      controller({
        leftSources: [num0("smooth", "/Live/Value", 0), num0("stepped", "/Live/Value", 1)],
        rightSources: [num0("points", "/Live/Value", 2)],
        discreteSources: [item("stripes", "/Live/Flag", "Boolean", { color: colors[3] })],
        rightFilter: 1
      })
    ),
    { liveTime: num(0.5) },
    sel("setLiveConnected"),
    frames(),
    capture("connected", ["selection"]),
    ...[1, 2, 3, 4].flatMap((k) => [
      { log: [["putNumber", "/Live/Value", k * 0.4, k * k], ["putBoolean", "/Live/Flag", k * 0.4, k % 2 === 0], ["putNumber", "/Timestamp", k * 0.4, k * 0.4]] },
      { liveTime: num(k * 0.4 + 0.1) },
      frames(3),
      ...hoverSweep([k * 0.4 + 0.05, k * 0.4 + 0.3])
    ]),
    capture("streaming", ["selection"]),
    sel("lock"),
    { liveTime: num(2.5) },
    frames(3),
    ...hoverSweep([2.4, 3]),
    capture("locked", ["selection"]),
    { log: [["clearBeforeTime", 1, true]], newFields: false },
    frames(2),
    capture("trimmed", ["selection"]),
    sel("unlock"),
    sel("setLiveDisconnected"),
    frames(),
    capture("disconnected", ["selection"])
  ]
};

fixtures["timestamps-original"] = {
  seed: 29,
  logs: ["offset"],
  preferences: { timestamps: "original" },
  steps: [
    { log: "offset" },
    add("left", "/A"),
    frames(),
    capture("original")
  ]
};

fixtures["start-zero"] = {
  seed: 30,
  logs: ["offset"],
  steps: [
    { log: "offset" },
    add("right", "/A"),
    frames(),
    capture("zeroed")
  ]
};

fixtures["two-tabs"] = {
  seed: 31,
  logs: ["numeric"],
  steps: [
    { log: "numeric" },
    restore(
      controller({ leftSources: [num0("stepped", "/Drive/LeftPositionMeters", 0)] }),
      controller({ rightSources: [num0("smooth", "/Arm/AngleRadians", 1)], rightLockedRange: [0, 1] })
    ),
    frames(),
    { main: "move-tab", data: 1 },
    frames(),
    ...hoverSweep([3]),
    edit("right", null, units(), 1),
    frames(),
    capture("second"),
    { main: "new-tab", data: 1 },
    add("left", "/Arm/AngleRadians"),
    frames(),
    capture("third")
  ]
};

const mouse = (type, x, target = "div.line-graph-scroll", init = {}) => ({
  event: { target, kind: "MouseEvent", type, init: { clientX: x, ...init } }
});
const wheel = (dx, dy, isPan = false) => ({
  eval: `window.tabs.tabList[window.tabs.selectedTab].renderer.scrollSensor.callback(${dx}, ${dy}, ${isPan}, 0, 0)`
});

fixtures.pointer = {
  seed: 33,
  logs: ["numeric", "discrete"],
  steps: [
    { log: "numeric" },
    { log: "discrete" },
    restore(
      controller({
        leftSources: [num0("smooth", "/Drive/LeftPositionMeters", 0)],
        rightSources: [num0("stepped", "/Motor/Temp", 1, "bold")],
        discreteSources: [item("stripes", "/D/Bool", "Boolean", { color: colors[3] }), item("graph", "/D/Steady", "Number", { color: colors[4] })]
      })
    ),
    frames(),
    mouse("mousemove", 500),
    frames(1),
    capture("hover"),
    ...[541, 543, 640, 642, 1099, 1090, 320].flatMap((x) => [mouse("mousemove", x), frames(1)]),
    mouse("mousemove", 520),
    frames(1),
    mouse("mousedown", 520),
    mouse("mouseup", 520),
    mouse("click", 523),
    frames(1),
    capture("selected"),
    mouse("mousedown", 520),
    mouse("mousemove", 560),
    frames(1),
    mouse("mousemove", 600),
    mouse("mouseup", 600),
    mouse("click", 600),
    frames(1),
    capture("panned"),
    mouse("mousemove", 650),
    frames(1),
    mouse("mousedown", 650, "div.line-graph-scroll", { shiftKey: true }),
    mouse("mousemove", 700, "window"),
    frames(1),
    mouse("mousemove", 740),
    frames(1),
    mouse("mouseleave", 740),
    mouse("mousemove", 800, "window"),
    frames(1),
    mouse("mouseup", 800, "window"),
    frames(1),
    capture("grab-zoom"),
    mouse("mousemove", 700),
    frames(1),
    mouse("mousedown", 700, "div.line-graph-scroll", { shiftKey: true }),
    mouse("mousemove", 760),
    frames(1),
    mouse("mouseup", 760, "window"),
    frames(1),
    mouse("contextmenu", 760),
    frames(1),
    capture("idle"),
    sel("setSelectedTime", 0.6),
    sel("setTimelineRange", [3, 8], false),
    frames(1),
    ...[700, 360, 1080].flatMap((x) => [mouse("mousemove", x), frames(1)]),
    sel("setSelectedTime", 9.5),
    frames(1),
    ...[1080, 400].flatMap((x) => [mouse("mousemove", x), frames(1)]),
    sel("play"),
    frames(1),
    mouse("mouseleave", 400),
    frames(1),
    capture("out-of-range"),
    { main: "new-tab", data: 1 },
    frames(1),
    mouse("mousemove", 500, "window"),
    mouse("mouseup", 500, "window"),
    frames(1),
    capture("hidden")
  ]
};

fixtures["help-fade"] = {
  seed: 34,
  logs: ["numeric"],
  window: { devicePixelRatio: 2 },
  steps: [
    { log: "numeric" },
    add("left", "/Drive/VelocityMetersPerSec"),
    add("right", "/Arm/AngleRadians"),
    frames(),
    wheel(0, -120),
    frames(1),
    wheel(40, 30),
    wheel(-15, -79),
    frames(1),
    wheel(0, -1),
    frames(1),
    capture("fading"),
    { advance: 1100 },
    wheel(0, -500),
    frames(1),
    capture("sent"),
    { eval: "window.preferences.hasScrolledLineGraph = true" },
    frames(1),
    { eval: "window.preferences.hasScrolledLineGraph = false" },
    wheel(0, -250),
    frames(1),
    mouse("mousedown", 600),
    mouse("mousemove", 640),
    mouse("mousemove", 590),
    mouse("mouseup", 590),
    frames(1),
    capture("dragged")
  ]
};

fixtures.scrolled = {
  seed: 35,
  logs: ["numeric", "alerts"],
  window: { devicePixelRatio: 1.5, theme: "dark" },
  preferences: { hasScrolledLineGraph: true, timestamps: "original" },
  steps: [
    { log: "numeric" },
    { log: "alerts" },
    restore(
      controller({
        leftSources: [num0("points", "/Drive/RightPositionInches", 0), num0("stepped", "/Arm/AngleDegrees", 1, "verybold")],
        discreteSources: [item("alerts", "/Alerts", "Alerts", {}), item("stripes", "/Misc/Name", "String", { color: colors[5] })],
        showRobotMode: true
      })
    ),
    frames(),
    wheel(0, -300),
    frames(1),
    mouse("mousemove", 700),
    frames(1),
    sel("setSelectedTime", 4.2),
    frames(1),
    capture("scrolled")
  ]
};

fixtures.dense = {
  seed: 37,
  logs: ["dense"],
  steps: [
    { log: "dense" },
    restore(
      controller({
        leftSources: [num0("stepped", "/Dense/Signal", 0)],
        rightSources: [num0("smooth", "/Dense/Signal", 1)],
        discreteSources: [item("stripes", "/Dense/Toggle", "Boolean", { color: colors[2] }), item("graph", "/Dense/Toggle", "Boolean", { color: colors[3] })]
      })
    ),
    frames(),
    capture("full"),
    sel("setTimelineRange", [0.9, 2.6], false),
    frames(1),
    mouse("mousemove", 600),
    frames(1),
    capture("zoomed")
  ]
};

fixtures.magnitudes = {
  seed: 38,
  logs: ["magnitudes"],
  steps: [
    { log: "magnitudes" },
    restore(
      controller({
        leftSources: [num0("smooth", "/Big/Mid", 0)],
        rightSources: [num0("stepped", "/Big/Over", 1)]
      })
    ),
    frames(),
    capture("big"),
    restore(controller({ rightSources: [num0("smooth", "/Big/Fine", 0)] })),
    frames(),
    capture("fine"),
    restore(controller({ leftSources: [num0("points", "/Big/Flat", 0)] })),
    frames(),
    sel("setTimelineRange", [4, 4], false),
    sel("setSelectedTime", 4),
    frames(1),
    mouse("mousemove", 600),
    frames(1),
    capture("degenerate")
  ]
};

fixtures.vast = {
  seed: 39,
  logs: ["vast"],
  steps: [
    { log: "vast" },
    restore(controller({ leftSources: [num0("smooth", "/Vast/Value", 0)] })),
    frames(),
    sel("setTimelineRange", [2e13 + 4, 2e13 + 4], false),
    sel("setSelectedTime", 2e13 + 4),
    frames(1),
    mouse("mousemove", 600),
    frames(1),
    mouse("mousemove", 300),
    frames(1),
    capture("vast")
  ]
};

fixtures.tiny = {
  seed: 36,
  logs: ["numeric", "alerts", "magnitudes"],
  window: { devicePixelRatio: 3 },
  layout: [{ selector: "div.renderer-content", rect: [300, 92, 61, 37] }],
  steps: [
    { log: "numeric" },
    { log: "alerts" },
    { log: "magnitudes" },
    restore(
      controller({
        leftSources: [num0("smooth", "/Drive/LeftPositionMeters", 0)],
        rightSources: [num0("smooth", "/Motor/Temp", 1), num0("points", "/Big/Flat", 2)],
        discreteSources: [item("alerts", "/Alerts", "Alerts", {})]
      })
    ),
    frames(),
    mouse("mousemove", 340),
    frames(1),
    sel("setSelectedTime", 2),
    frames(1),
    capture("tiny")
  ]
};

// Output

fs.mkdirSync(SPECS, { recursive: true });
for (const [name, ops] of Object.entries(logs)) {
  fs.writeFileSync(path.join(LOGS, `line-graph-${name}.json`), "[\n" + ops.map((op) => "  " + JSON.stringify(op)).join(",\n") + "\n]\n");
}
for (const [name, f] of Object.entries(fixtures)) {
  const logPath = (log) => `../logs/line-graph-${log}.json`;
  const fixture = { sourceLists: true, seed: f.seed };
  if (f.window !== undefined) fixture.window = f.window;
  if (f.preferences !== undefined) fixture.preferences = f.preferences;
  if (f.layout !== undefined) fixture.layout = f.layout;
  fixture.steps = [{ eval: RECORDER }, ...f.steps.map((s) => (typeof s.log === "string" ? { ...s, log: logPath(s.log) } : s))];
  const spec = { entry: "dom", inputs: f.logs.map(logPath).sort(), fixture };
  fs.writeFileSync(path.join(SPECS, `${name}.entry.json`), JSON.stringify(spec, null, 2) + "\n");
}
