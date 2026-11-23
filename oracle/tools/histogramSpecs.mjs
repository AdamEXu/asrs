// Histogram fixtures for `crates/asrs-tabs/tests/histogram_dom.rs`: upstream's real
// StatisticsRenderer and Chart.js 4.4.0 in Blink, with scripted geometry.
// Writes parity/specs/dom/histogram/*.entry.json; edit this script, not the specs.
// Run from anywhere: `node oracle/tools/histogramSpecs.mjs`.

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const OUT = path.join(ROOT, "parity/specs/dom/histogram");

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function lit(v) {
  if (typeof v === "number") return Object.is(v, -0) ? "-0" : String(v);
  if (typeof v === "string") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map(lit).join(",") + "]";
  return "{" + Object.entries(v).map(([k, x]) => JSON.stringify(k) + ":" + lit(x)).join(",") + "}";
}

// Records, per chart, what reaches it and what it draws:
// - {"hist": "create" | "resize" | "render" | "event" | "draw"}: one outermost entry into the
//   chart, with the canvas calls it made ("reset" when it set the canvas size, which drops the
//   calls before), and after a layout the chart's state.
// - {"hist": "observe"}: the ResizeObserver was notified. {"hist": "dpr"}: the window's scale
//   changed. {"hist": "dom"}: a mouse event reached the canvas.
// - {"hist": "frame"}: an animation frame; the "resize", "event" and "draw" records up to the
//   next record of another kind are the callbacks Chart.js had queued for it.
const RECORDER = String.raw`(() => {
  const w = window;
  const write = (record) => w.__oracleWrite(JSON.stringify(record));
  const tree = w.__oracleTree;
  const charts = [];
  const observers = new Map();
  w.ResizeObserver = class {
    constructor(callback) {
      this.callback = callback;
    }
    observe(el) {
      observers.set(el, this.callback);
    }
    unobserve(el) {
      observers.delete(el);
    }
    disconnect() {
      for (const [el, callback] of observers) if (callback === this.callback) observers.delete(el);
    }
  };
  const containerOf = (chart) => chart.canvas.parentNode;
  let depth = 0;
  const state = (chart) => ({
    size: [chart.width, chart.height, chart.currentDevicePixelRatio],
    area: chart.chartArea === undefined ? undefined : ["left", "top", "right", "bottom", "width", "height"].map((k) => chart.chartArea[k]),
    scales: Object.fromEntries(
      Object.entries(chart.scales).map(([id, s]) => [
        id,
        {
          box: [s.left, s.top, s.right, s.bottom, s.width, s.height],
          range: [s.min, s.max],
          rotation: s.labelRotation,
          ticks: s.ticks.map((t) => [t.value, t.label])
        }
      ])
    ),
    legend: [chart.legend.left, chart.legend.top, chart.legend.right, chart.legend.bottom],
    bars: chart.data.datasets.map((_, i) => chart.getDatasetMeta(i).data.map((b) => [b.x, b.y, b.base, b.width, b.height]))
  });
  const observe = (chart, record, body) => {
    if (depth > 0) return body();
    const ctx = chart.canvas.__oracleContext;
    const [frames, length] = [ctx.frames, ctx.calls.length];
    depth++;
    try {
      return body();
    } catch (e) {
      record.threw = tree(e instanceof Error ? e.name + ": " + e.message : String(e));
      throw e;
    } finally {
      depth--;
      record.reset = ctx.frames !== frames;
      record.calls = tree(record.reset ? ctx.calls : ctx.calls.slice(length));
      record.canvas = tree([chart.canvas.width, chart.canvas.height]);
      if (record.layout) record.state = tree(state(chart));
      delete record.layout;
      write(record);
    }
  };
  const dark = () => w.matchMedia("(prefers-color-scheme: dark)").matches;
  const hook = (r) => {
    const chart = r.histogram;
    const id = charts.length;
    charts.push(chart);
    const ctx = chart.canvas.__oracleContext;
    write({ hist: "create", id: tree(id), dpr: tree(w.devicePixelRatio), calls: tree(ctx.calls), canvas: tree([chart.canvas.width, chart.canvas.height]) });
    let last;
    const render = r.render;
    r.render = function (command) {
      const record = { hist: "render", id: tree(id), t: tree(Date.now()), dark: dark() };
      const theme = r.lastIsLight !== !record.dark;
      record.layout = theme || r.changeCounter !== command.changeCounter || r.firstRender;
      if (command !== last) record.command = tree({ changeCounter: command.changeCounter, bins: command.bins, stepSize: command.stepSize, fields: command.fields.map((f) => ({ title: f.title, color: f.color, histogramCounts: f.histogramCounts })) });
      last = command;
      return observe(chart, record, () => render.apply(this, arguments));
    };
    const resize = chart.resize;
    chart.resize = function (width, height) {
      const rect = containerOf(chart).getBoundingClientRect();
      const record = { hist: "resize", id: tree(id), t: tree(Date.now()), args: tree([width, height]), container: tree([rect.width, rect.height]), layout: true };
      return observe(chart, record, () => resize.apply(this, arguments));
    };
    const handler = chart._eventHandler;
    chart._eventHandler = function (e) {
      const record = { hist: "event", id: tree(id), t: tree(Date.now()), type: e.type, at: tree([e.x, e.y]) };
      return observe(chart, record, () => handler.apply(this, arguments));
    };
    const draw = chart.draw;
    chart.draw = function () {
      return observe(chart, { hist: "draw", id: tree(id), t: tree(Date.now()) }, () => draw.apply(this, arguments));
    };
    for (const type of ["mousemove", "mouseout", "click"]) {
      chart.canvas.addEventListener(type, (e) => write({ hist: "dom", id: tree(id), type, at: tree([e.offsetX, e.offsetY]) }));
    }
  };
  const addTab = w.tabs.addTab;
  w.tabs.addTab = function (...args) {
    const before = this.tabList.slice();
    const result = addTab.apply(this, args);
    this.tabList.forEach((t) => {
      if (t.type === 6 && !before.includes(t)) hook(t.renderer);
    });
    return result;
  };
  const frame = () => {
    write({ hist: "frame", t: tree(Date.now()) });
    w.requestAnimationFrame(frame);
  };
  w.requestAnimationFrame(frame);
  const last = () => charts[charts.length - 1];
  w.__histCommand = (command) => {
    const tab = w.tabs.tabList.filter((t) => t.type === 6).pop();
    let counter = w.__histCounter ?? 0;
    w.__histCounter = ++counter;
    const value = { changeCounter: counter, ...command, fields: command.fields.map((f) => ({ ...f, stats: new Proxy({}, { get: () => 0 }) })) };
    tab.controller.getCommand = () => value;
  };
  w.__histSize = (width, height) => {
    const chart = last();
    const container = containerOf(chart);
    container.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, right: width, bottom: height, width, height });
    write({ hist: "observe", id: tree(charts.length - 1), size: tree([width, height]) });
    const callback = observers.get(container);
    if (callback === undefined) throw new Error("no ResizeObserver on the chart's container");
    callback([{ contentRect: { width, height } }]);
  };
  w.__histDpr = (dpr) => {
    Object.defineProperty(w, "devicePixelRatio", { configurable: true, get: () => dpr });
    write({ hist: "dpr", id: tree(charts.length - 1), dpr: tree(dpr) });
    w.dispatchEvent(new Event("resize"));
  };
  w.__histMouse = (type, x, y) => {
    const rect = last().canvas.getBoundingClientRect();
    const e = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: rect.left + x, clientY: rect.top + y });
    Object.defineProperty(e, "offsetX", { value: x });
    Object.defineProperty(e, "offsetY", { value: y });
    last().canvas.dispatchEvent(e);
  };
  w.__histHover = (type, dataset, index, fx, fy) => {
    const b = last().getDatasetMeta(dataset).data[index];
    w.__histMouse(type, Math.round(b.x + (fx - 0.5) * b.width), Math.round(b.y + (b.base - b.y) * fy));
  };
})()`;

const COLORS = ["#2b66a2", "#e5b31b", "#af2437", "#80588e", "#e48b32", "#c0b487", "#858584", "#3b875a", "#d993aa", "#5f4528"];
const range = (n, f) => Array.from({ length: n }, (_, i) => f(i));
const bins = (min, max, step) => {
  const out = [];
  for (let i = min; i < max && out.length < 1000; i += step) out.push(i);
  return out;
};
const field = (title, color, histogramCounts) => ({ title, color, histogramCounts });
const command = (b, stepSize, fields) => ({ bins: b, stepSize, fields });
const simple = (min, max, step, count, seed, top = 40) => {
  const rand = mulberry32(seed);
  const b = bins(min, max, step);
  return command(b, step, range(count, (k) => field("/Field/Number" + k, COLORS[k % COLORS.length], b.map(() => Math.floor(rand() * rand() * top)))));
};

const draw = (c, ms = 32) => [{ eval: `window.__histCommand(${lit(c)})` }, { advance: ms }];
const size = (width, height, ms = 32) => [{ eval: `window.__histSize(${width}, ${height})` }, { advance: ms }];
const dpr = (value, ms = 32) => [{ eval: `window.__histDpr(${value})` }, { advance: ms }];
const theme = (value, ms = 32) => [{ eval: `window.__oracleTheme = "${value}"` }, { advance: ms }];
const mouse = (type, x, y, ms = 32) => [{ eval: `window.__histMouse("${type}", ${x}, ${y})` }, { advance: ms }];
const hover = (dataset, index, fx = 0.5, fy = 0.5, ms = 32, type = "mousemove") => [
  { eval: `window.__histHover("${type}", ${dataset}, ${index}, ${fx}, ${fy})` },
  { advance: ms }
];
const START = [{ eval: RECORDER }, { main: "new-tab", data: 6 }];

function spec(name, steps, { container = [560, 348], inputs, ...fixture } = {}) {
  const body = {
    entry: "dom",
    inputs,
    fixture: {
      seed: 909,
      ignoreMain: ["save-state", "save-type-memory", "update-touch-bar-slider", "show"],
      layout: [{ selector: "div.stats-histogram-container", rect: [530, 92, ...container] }],
      ...fixture,
      steps
    }
  };
  fs.mkdirSync(OUT, { recursive: true });
  fs.writeFileSync(path.join(OUT, name + ".entry.json"), JSON.stringify(body, null, 1) + "\n");
}

const THREE = simple(0, 10, 1, 3, 11);

spec("basic", [
  ...START,
  ...draw(command(bins(0, 10, 1), 1, [])),
  ...draw(simple(0, 10, 1, 1, 1)),
  ...draw(THREE),
  ...draw(command(bins(0, 10, 1), 1, [field("/Zero", COLORS[0], range(10, () => 0))])),
  ...draw(command([], 1, [field("/NoBins", COLORS[1], [])])),
  ...draw(command([5], 1, [field("/OneBin", COLORS[2], [7])])),
  ...draw(command(bins(-5, 5, 0.5), 0.5, [field("/Short", COLORS[3], range(20, (i) => (i * 7) % 11))])),
  ...draw(simple(0, 10, 1, 2, 4)),
  ...theme("dark"),
  ...draw(THREE),
  ...theme("light"),
  ...draw(command(bins(0, 10, 1), 1, []))
]);

spec("dark", [...START, ...draw(THREE), ...hover(1, 3), ...theme("light"), ...draw(THREE), ...hover(0, 4, 0.5, 0.5, 500)], {
  window: { theme: "dark" }
});
spec("dpr2", [...START, ...draw(THREE), ...draw(simple(-1, 1, 0.05, 2, 5)), ...hover(0, 12), ...size(331, 207)], {
  window: { devicePixelRatio: 2 }
});
spec(
  "dpr15",
  [...START, ...draw(THREE), ...size(333.35, 250.75), ...hover(0, 2, 0.5, 0.5, 480), ...dpr(2), ...dpr(1), ...dpr(1.25), ...dpr(1.25)],
  { window: { devicePixelRatio: 1.5 }, container: [447.3, 301.6] }
);

const SIZES = [
  [640, 480], [640, 480], [640, 500], [641, 500], [640, 480], [300, 600], [100.4, 0.4], [100, 30], [0, 400], [400, 0], [0, 0], [1, 1], [20, 20], [60, 40], [100, 100],
  [120, 348], [333.3, 222.7], [517.25, 301.44], [1100, 60], [99.95, 900], [2000, 1200], [250, 130], [200.04, 200.06]
];
spec("resize", [...START, ...draw(THREE), ...SIZES.flatMap(([w, h]) => size(w, h)), ...draw(simple(0, 1000, 50, 2, 6)), ...SIZES.flatMap(([w, h]) => size(w, h, 16))]);

const RANGES = [
  [0, 10, 1], [0, 10, 0.1], [0, 10, 0.01], [-10, 10, 2], [-1, 1, 0.05], [-0.001, 0.001, 0.0001], [0, 1, 0.3],
  [0, 100, 7], [0, 1e6, 50000], [1e6, 1.001e6, 100], [-5e-7, 5e-7, 1e-7], [0, 1e-4, 1e-5], [0, 9e-5, 1e-5], [3, 4, 5],
  [0, 360, 45], [-180, 180, 15], [0, 12.5, 0.5], [2.5, 7.5, 1.25], [0.1, 0.7, 0.1], [-3.14159, 3.14159, 0.25],
  [1e15, 1.000000000000001e15, 0.125], [1e16, 1.00000000000001e16, 4], [0, 1e18, 1e17], [-1e17, 1e17, 3e16],
  [123456789, 123456799, 1], [0, 1e3, 333], [-1e-12, 1e-12, 1e-13], [0.3333333, 0.9999999, 0.0123], [-7, -2, 0.5],
  [0, 2, 1e-3], [0, 1e-20, 1e-21], [5, 1e5, 4999]
];
spec("ranges", [...START, ...RANGES.flatMap(([a, b, s], i) => draw(simple(a, b, s, 1 + (i % 3), 100 + i), 16))]);
spec("ranges-narrow", [...START, ...RANGES.flatMap(([a, b, s], i) => draw(simple(a, b, s, 1, 200 + i), 16))], { container: [150, 230] });
spec("ranges-wide", [...START, ...RANGES.flatMap(([a, b, s], i) => draw(simple(a, b, s, 1, 300 + i), 16))], { container: [1500, 700] });

// Values the number inputs can produce: an empty or unparsable input is 0 or NaN.
const ODD = [
  command([0], NaN, [field("/NaNStep", COLORS[0], [4])]),
  command([NaN], 1, [field("/NaNBin", COLORS[1], [4])]),
  command([0, 1, 2], 0.5, [field("/Gap", COLORS[2], [1, 2, 3])]),
  command([0, 1, 2], 5, [field("/Overlap", COLORS[3], [1, 2, 3]), field("/Overlap2", COLORS[4], [3, 2, 1])]),
  command([1e17, 1e17, 1e17 + 16], 1, [field("/Dup", COLORS[5], [1, 5, 2]), field("/Dup2", COLORS[6], [2, 1, 4])]),
  command([1e17, 1e17, 1e17 + 16], 1, [field("/Dup", COLORS[5], [3, 1, 2])]),
  command([0, 1, 2], 1, [field("/Dup", COLORS[5], [3, 1, 2])]),
  command([1e17, 1e17, 1e17 + 16], 1, [field("/Dup", COLORS[5], [2, 6, 2]), field("/Dup2", COLORS[6], [1, 1, 1])]),
  command([0, 1, 2], Infinity, [field("/InfStep", COLORS[7], [1, 2, 3])]),
  command([-1e308, 1e308], 1e308, [field("/Huge", COLORS[8], [1, 2])]),
  command([5e-324, 1e-323], 5e-324, [field("/Tiny", COLORS[9], [1, 2])])
];
spec("odd", [...START, ...ODD.flatMap((c) => draw(c, 16)), ...draw(THREE)]);

const COUNTS = [
  [1], [0, 1], [2, 3], [9, 10], [99, 100, 101], [1234, 56], [12345, 67890], [999999, 1], [1234567, 7654321], [1e9, 5e8],
  [0.5, 0.25], [1e15, 2e15], [9e15, 1], [3, 0, 0, 0, 3], [1e-5, 2e-5], [1 / 3, 2 / 3], [950, 20], [19, 1], [20, 1], [49, 50], [190, 3], [200, 3]
];
spec("counts", [
  ...START,
  ...COUNTS.flatMap((c) => draw(command(range(c.length, (i) => i), 1, [field("/Count", COLORS[0], c), field("/Other", COLORS[1], c.map((x) => Math.floor(x / 3)))]), 16))
]);

const TITLES = [
  "", "/A", "/RealOutputs/Drive/LeftVelocity", "12345678901234567890", "123456789012345678901", "Ünïcödé/Ω/測定値/かな",
  "/Emoji/🤖🤖🤖🤖🤖🤖🤖🤖🤖🤖🤖", "/Tab\tand  space", "NT:/SmartDashboard/Very/Long/Key/Name/That/Goes/On", "\ud83e half"
];
const titled = (n) => command(bins(0, 5, 1), 1, range(n, (k) => field(TITLES[k % TITLES.length] + (k >= TITLES.length ? k : ""), COLORS[k % COLORS.length], range(5, (i) => (i * k + k) % 9))));
spec("legend", [
  ...START,
  ...range(11, (n) => n).flatMap((n) => draw(titled(n), 16)),
  ...draw(titled(25)),
  ...size(200, 348),
  ...size(90, 600),
  ...size(1200, 120),
  ...draw(titled(3)),
  ...theme("dark")
]);

spec("hover", [
  ...START,
  ...draw(THREE),
  ...mouse("mousemove", 5, 5),
  ...hover(0, 2, 0.5, 0.5, 240),
  ...hover(0, 2, 0.6, 0.5, 16),
  ...hover(1, 2, 0.5, 0.5, 64),
  ...hover(2, 7, 0.5, 0.5, 600),
  ...hover(2, 7, 0.5, 0.5, 64, "click"),
  ...hover(0, 0, 0.1, 0.1, 100),
  ...hover(0, 9, 0.9, 0.9, 600),
  ...mouse("mousemove", 300, 60, 700),
  ...mouse("mousemove", 3, 340, 100),
  ...hover(1, 4, 0.5, 0.2, 64),
  ...mouse("mouseout", -5, 100, 100),
  ...hover(1, 4, 0.5, 0.2, 500),
  ...mouse("mouseout", -5, 100, 700),
  ...mouse("click", 300, 20, 64),
  ...hover(0, 5, 0, 0),
  ...hover(0, 5, 1, 1),
  ...hover(0, 5, 0.5, -0.3),
  ...hover(1, 5, 0.5, 0.5),
  ...hover(2, 5, 0.5, 0.5, 900)
]);

spec("hover-update", [
  ...START,
  ...draw(THREE),
  ...hover(0, 4, 0.5, 0.5, 64),
  ...draw(simple(0, 10, 1, 3, 21)),
  ...draw(simple(0, 10, 1, 3, 22), 600),
  ...theme("dark", 100),
  ...theme("light", 600),
  ...size(400, 300, 100),
  ...size(560, 348, 600),
  ...hover(2, 6, 0.5, 0.5, 50),
  ...draw(simple(0, 10, 1, 1, 23), 50),
  ...draw(simple(0, 10, 1, 2, 24), 600),
  ...hover(1, 6, 0.5, 0.5, 100),
  ...draw(command(bins(0, 10, 1), 1, []), 100),
  ...draw(simple(0, 4, 2, 2, 25), 600),
  ...hover(1, 1, 0.5, 0.5, 48),
  ...draw(simple(0, 40, 1, 2, 26), 48),
  ...hover(1, 30, 0.5, 0.5, 48),
  ...draw(simple(0, 4, 2, 2, 27), 48),
  ...mouse("mouseout", -1, -1, 48),
  ...draw(simple(0, 10, 1, 3, 28), 48),
  ...dpr(2, 300),
  ...hover(0, 4, 0.5, 0.5, 48),
  ...dpr(1, 500)
]);

const BLACK = command(bins(0, 6, 1), 1, [
  field("/Black", "#000000", [3, 4, 5, 6, 5, 4]),
  field("/White", "#ffffff", [4, 4, 4, 4, 4, 4]),
  field("/Gray", "#808080", [1, 2, 3, 3, 2, 1]),
  field("/Red", "#ff0000", [5, 5, 5, 5, 5, 5]),
  field("/Short", "#abc", [5, 5, 5, 5, 5, 5]),
  field("/Bad", "garbage", [2, 2, 2, 2, 2, 2]),
  field("/Cyan", "#00ffff", [2, 2, 2, 2, 2, 2])
]);
const colorHover = range(7, (k) => hover(k, k % 6, 0.5, 0.5, 80)).flat();
// A chart too short for its axes has a flat chart area near its top edge.
spec("hover-flat", [
  ...START,
  ...draw(simple(0, 20, 1, 1, 51)),
  ...size(300, 40, 48),
  ...range(20, (i) => hover(0, i, 0.5, 0, 48)).flat(),
  ...size(300, 41, 48),
  ...range(20, (i) => hover(0, 19 - i, 0.5, 0, 48)).flat()
]);

// Bars without a height at one pixel of a flat chart area are all active, and stay so when the
// chart grows, with a tooltip tall enough to open upwards.
const zeros = (n) => command(bins(0, 10, 1), 1, range(n, (k) => field("/Zero" + k, COLORS[k], range(10, () => 0))));
spec("hover-bottom", [
  ...START,
  ...draw(zeros(6)),
  ...[0, 1, 3, 5, 8, 9].flatMap((i) => [
    ...size(300, 40, 48),
    ...hover(0, i, 0.5, 0, 48),
    ...size(300, 400, 300),
    ...size(260, 380, 48),
    ...mouse("mouseout", -1, -1, 300)
  ]),
  ...draw(zeros(3)),
  ...[0, 5, 9].flatMap((i) => [
    ...size(120, 40, 48),
    ...hover(0, i, 0.5, 0, 48),
    ...size(220, 300, 48),
    ...mouse("mousemove", 5, 5, 48),
    ...mouse("mouseout", -1, -1, 100)
  ])
]);

spec("hover-colors", [...START, ...draw(BLACK), ...colorHover, ...theme("dark"), ...draw(BLACK), ...colorHover, ...mouse("mouseout", -1, -1, 600)], {
  window: { framePeriod: 20 }
});

const edges = simple(0, 1500, 100, 1, 31, 2e6);
spec("hover-edges", [
  ...START,
  ...draw(edges),
  ...range(15, (i) => hover(0, i, 0.5, 0.5, 48)).flat(),
  ...size(180, 110, 100),
  ...range(15, (i) => hover(0, i, 0.5, 0.3, 32)).flat(),
  ...size(90, 400, 100),
  ...range(15, (i) => hover(0, i, 0.5, 0.9, 32)).flat(),
  ...draw(simple(0, 0.00001, 0.000001, 1, 32), 48),
  ...range(10, (i) => hover(0, i, 0.5, 0.9, 32)).flat()
]);

// The pointer stays where a bar was, outside the next layout's chart area, so the replayed event
// keeps elements the new data no longer has.
const tall = (n, counts) => command(bins(0, 4, 1), 1, range(n, (k) => field("/F" + k, COLORS[k], range(4, () => counts))));
spec("throw-dataset", [
  ...START,
  ...draw(tall(2, 3)),
  ...hover(1, 0, 0.1, 0.5, 64),
  ...draw(tall(1, 1234567), 64),
  ...hover(0, 2, 0.5, 0.5, 64),
  ...draw(tall(2, 3), 64)
]);
spec("throw-resize", [
  ...START,
  ...draw(simple(0, 10, 1, 2, 41)),
  ...hover(1, 0, 0.5, 0.5, 600),
  ...draw(command(bins(0, 10, 1), 1, [field("/A", COLORS[0], range(10, () => 1234567))]), 64),
  ...size(500, 300, 100)
]);

{
  const rand = mulberry32(4040);
  const pick = (a) => a[Math.floor(rand() * a.length)];
  const steps = [...START];
  for (let i = 0; i < 160; i++) {
    const min = pick([0, 0, -10, rand() * 100, -rand() * 1000, Math.round(rand() * 50) / 10]);
    const step = pick([1, 0.5, 0.1, 0.25, 2, 5, 10, 0.02, Math.round(rand() * 1000) / 1000 + 0.001, rand() * 3 + 1e-3]);
    const max = min + step * Math.ceil(rand() * pick([4, 12, 40, 150]));
    steps.push(...draw(simple(min, max, step, Math.floor(rand() * 5), 5000 + i, pick([2, 10, 50, 1000, 40000])), 16));
    if (i % 3 === 0) steps.push(...size(Math.round(rand() * 9000) / 10, Math.round(rand() * 6000) / 10, 16));
    if (i % 11 === 0) steps.push(...theme(pick(["light", "dark"]), 16));
  }
  spec("random", steps);
}

{
  const rand = mulberry32(5050);
  const pick = (a) => a[Math.floor(rand() * a.length)];
  const steps = [...START, ...draw(THREE)];
  let [w, h] = [560, 348];
  for (let i = 0; i < 140; i++) {
    const r = rand();
    const ms = pick([1, 16, 16, 33, 70, 180, 450]);
    if (r < 0.55) steps.push(...mouse("mousemove", Math.round(rand() * w), Math.round(40 + rand() * (h - 70)), ms));
    else if (r < 0.65) steps.push(...mouse(pick(["mouseout", "click"]), Math.round(rand() * w), Math.round(rand() * h), ms));
    else if (r < 0.85) steps.push(...draw(simple(0, pick([3, 10, 20]), 1, Math.floor(rand() * 4), 7000 + i), ms));
    else if (r < 0.93) {
      [w, h] = [Math.round(150 + rand() * 600), Math.round(120 + rand() * 400)];
      steps.push(...size(w, h, ms));
    } else steps.push(...theme(pick(["light", "dark"]), ms));
  }
  spec("random-hover", steps);
}

{
  const rand = mulberry32(90001);
  const pick = (a) => a[Math.floor(rand() * a.length)];
  const steps = [...START, ...draw(THREE)];
  let [w, h] = [560, 348];
  for (let i = 0; i < 160; i++) {
    const r = rand();
    const ms = pick([0, 1, 16, 16, 33, 70, 180, 450]);
    const [x, y] = [Math.round(rand() * w), Math.round(rand() * h)];
    if (r < 0.5) steps.push(...mouse("mousemove", x, y, ms));
    else if (r < 0.58) steps.push(...mouse(pick(["mouseout", "click"]), x, y, ms));
    else if (r < 0.78) {
      const step = pick([1, 1, 0.5, 0.1, 3, 0.037, rand() * 5]);
      const min = pick([0, 0, -5, rand() * 10 - 5, 0.1]);
      const b = range(pick([1, 3, 10, 20]), (k) => min + k * step);
      const top = pick([2, 5, 40, 3000]);
      const count = () => (rand() < 0.3 ? 0 : Math.floor(rand() * top));
      steps.push(...draw(command(b, step, range(Math.floor(rand() * 5), (k) => field("/S" + k, COLORS[(k + i) % 10], b.map(count)))), ms));
    } else if (r < 0.88) {
      [w, h] = pick([[Math.round(150 + rand() * 600), Math.round(120 + rand() * 400)], [300, 40], [rand() * 700, rand() * 90], [0, 0], [560, 348]]);
      steps.push(...size(w, h, ms));
    } else if (r < 0.93) steps.push(...dpr(pick([1, 1.25, 1.5, 1.75, 2, 2.5, 0.8]), ms));
    else steps.push(...theme(pick(["light", "dark"]), ms));
  }
  spec("random-mix", steps);
}

spec(
  "controller",
  [
    ...START,
    { log: "../logs/statistics.json" },
    { event: { target: "select.time-range", props: { value: "full" }, type: "change" } },
    { eval: `window.tabs.tabList[window.tabs.selectedTab].controller.sourceList.addField("/Meas/A")` },
    { advance: 200 },
    { eval: `window.tabs.tabList[window.tabs.selectedTab].controller.sourceList.addField("/Meas/B")` },
    { advance: 200 },
    { event: { target: "div.stats-config input.range-max, input.range-max", props: { value: "3" }, type: "change" } },
    { advance: 200 },
    { event: { target: "input.step-size", props: { value: "0.2" }, type: "change" } },
    { advance: 200 },
    { event: { target: "input.range-min", props: { value: "-2" }, type: "change" } },
    { advance: 200 }
  ],
  { inputs: ["../logs/statistics.json"] }
);

console.log(`histogram: ${fs.readdirSync(OUT).length} specs`);
