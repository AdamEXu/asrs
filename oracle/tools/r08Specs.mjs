// Mechanism and Points renderer fixtures for `crates/asrs-tabs/tests/mechanism_points_dom.rs`.
// Writes parity/specs/dom/{mechanism,points,css-style}/*.entry.json and
// parity/specs/dom/logs/r08.json; edit this script, not the specs.

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SPECS = path.join(ROOT, "parity/specs/dom");

function write(dir, name, data) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, name), JSON.stringify(data, null, 1) + "\n");
}

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

// JavaScript source for a command, so NaN, Infinity, -0, undefined and missing keys survive.
function lit(v) {
  if (v === undefined) return "undefined";
  if (v === null) return "null";
  if (typeof v === "number") {
    if (Number.isNaN(v)) return "NaN";
    if (v === Infinity) return "Infinity";
    if (v === -Infinity) return "-Infinity";
    if (Object.is(v, -0)) return "-0";
    return String(v);
  }
  if (typeof v === "string" || typeof v === "boolean") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map(lit).join(",") + "]";
  return "{" + Object.entries(v).map(([k, x]) => JSON.stringify(k) + ":" + lit(x)).join(",") + "}";
}

// Records every render of every Mechanism and Points renderer: the container reads it made,
// each inline style assignment as written, the element tree after it, a throw, and
// getAspectRatio(). Style setters are shadowed per instance because Blink serves the
// camel-cased names from a named-property interceptor; each shadow forwards to setProperty,
// which is what the IDL setter does.
const RECORDER = String.raw`(() => {
  const w = window;
  const write = (record) => w.__oracleWrite(JSON.stringify(record));
  const view = new DataView(new ArrayBuffer(8));
  const bits = (x) => {
    view.setFloat64(0, x);
    return view.getUint32(0).toString(16).padStart(8, "0") + view.getUint32(4).toString(16).padStart(8, "0");
  };
  const tree = (v) => {
    if (v === undefined) return { u: null };
    if (v === null || typeof v === "boolean" || typeof v === "string") return v;
    if (typeof v === "number") return { n: bits(v) };
    if (Array.isArray(v)) {
      const out = [];
      for (let i = 0; i < v.length; i++) out.push(tree(v[i]));
      return out;
    }
    return { o: Object.keys(v).map((k) => [k, tree(v[k])]) };
  };
  const errorText = (e) => (e instanceof Error ? e.name + ": " + e.message : String(e));
  const dark = () => w.matchMedia("(prefers-color-scheme: dark)").matches;
  const node = (el) => [el.localName, el.getAttributeNames().map((n) => [n, el.getAttribute(n)]), Array.from(el.children).map(node)];

  const STYLE = {
    display: "display",
    backgroundColor: "background-color",
    stroke: "stroke",
    strokeWidth: "stroke-width",
    fill: "fill",
    transform: "transform",
    left: "left",
    top: "top",
    width: "width",
    height: "height"
  };
  let active = null;
  const instrument = (el, who) => {
    for (const [camel, css] of Object.entries(STYLE)) {
      Object.defineProperty(el.style, camel, {
        configurable: true,
        get() {
          return this.getPropertyValue(css);
        },
        set(value) {
          if (active !== null) active.assigns.push([who(), camel, tree(value)]);
          this.setProperty(css, value);
        }
      });
    }
  };

  const renderers = [];
  const hook = (kind, r) => {
    if (renderers.includes(r)) return;
    const id = renderers.length;
    renderers.push(r);
    const root = kind === "mechanism" ? r.SVG : r.BACKGROUND;
    instrument(root, () => "root");
    if (kind === "mechanism") {
      for (const name of ["clientWidth", "clientHeight"]) {
        const getter = Object.getOwnPropertyDescriptor(Element.prototype, name).get;
        Object.defineProperty(r.CONTAINER, name, {
          configurable: true,
          get() {
            const value = getter.call(this);
            if (active !== null) active.reads.push([name, tree(value)]);
            return value;
          }
        });
      }
    } else {
      r.CONTAINER.getBoundingClientRect = function () {
        const rect = Element.prototype.getBoundingClientRect.call(this);
        if (active !== null) active.reads.push(["rect", tree([rect.x, rect.y, rect.width, rect.height])]);
        return rect;
      };
    }
    write({ r08: "create", kind, id: tree(id), dom: node(root), aspect: tree(r.getAspectRatio()) });

    const render = r.render;
    r.render = function (command) {
      const record = { r08: "render", kind, id: tree(id), command: tree(command), dark: tree(dark()), reads: [], assigns: [] };
      const created = [];
      const createElementNS = document.createElementNS;
      const cloneNode = Node.prototype.cloneNode;
      const track = (el) => {
        const index = created.length;
        created.push(el);
        instrument(el, () => index);
        return el;
      };
      document.createElementNS = function (...args) {
        return track(createElementNS.apply(this, args));
      };
      Node.prototype.cloneNode = function (...args) {
        const el = cloneNode.apply(this, args);
        return el instanceof Element ? track(el) : el;
      };
      active = record;
      try {
        return render.apply(this, arguments);
      } catch (e) {
        record.threw = tree(errorText(e));
        throw e;
      } finally {
        active = null;
        document.createElementNS = createElementNS;
        Node.prototype.cloneNode = cloneNode;
        record.created = created.length;
        record.dom = node(root);
        record.aspect = tree(r.getAspectRatio());
        write(record);
      }
    };
  };
  const hookAll = () => {
    w.tabs.tabList.forEach((t) => {
      if (t.type === 10) hook("mechanism", t.renderer);
      if (t.type === 11) hook("points", t.renderer);
    });
  };
  hookAll();
  const addTab = w.tabs.addTab;
  w.tabs.addTab = function (...args) {
    const result = addTab.apply(this, args);
    hookAll();
    return result;
  };

  w.__r08Render = (type, index, command) => {
    const tab = w.tabs.tabList.filter((t) => t.type === type)[index];
    try {
      tab.renderer.render(command);
    } catch (e) {}
  };
  w.__r08Size = (width, height) =>
    w.__oracleQuiet(() => {
      const content = document.querySelector("div.renderer-content");
      content.style.width = width;
      content.style.height = height;
      content.style.bottom = "auto";
    });
})()`;

const MECH = 10;
const POINTS = 11;

const render = (type, command, index = 0) => ({ eval: `window.__r08Render(${type}, ${index}, ${lit(command)})` });
const size = (width, height) => ({ eval: `window.__r08Size(${JSON.stringify(width)}, ${JSON.stringify(height)})` });

function spec(area, id, fixture, inputs = []) {
  const steps = fixture.steps;
  const body = { entry: "dom", inputs, fixture: { geometry: "real", ...fixture, steps } };
  if (inputs.length === 0) delete body.inputs;
  write(path.join(SPECS, area), `${id}.entry.json`, body);
}

const START = (type, extra = []) => [
  { main: "new-tab", data: type },
  ...extra,
  { eval: RECORDER },
  { advance: 50 }
];

const line = (start, end, color, weight) => ({ start, end, color, weight });
const mech = (dimensions, lines, backgroundColor = "#000020") => ({ backgroundColor, dimensions, lines });

const ARM = mech(
  [3, 2],
  [
    line([0.5, 0.25], [0.5, 1.25], "#00FF00", 10),
    line([0.5, 1.25], [1.2071067811865475, 1.9571067811865475], "#ff8000", 6),
    line([1.2071067811865475, 1.9571067811865475], [1.7071067811865475, 1.9571067811865475], "#0000ff", 2.5)
  ],
  "#202020"
);

const mechanismFixtures = [];

mechanismFixtures.push({
  id: "basic",
  fixture: {
    steps: [
      ...START(MECH),
      render(MECH, ARM),
      render(MECH, mech([1, 1], [line([0, 0], [1, 1], "#ffffff", 1)])),
      render(MECH, mech([2, 1], [])),
      render(MECH, null),
      render(MECH, null),
      render(MECH, ARM),
      render(MECH, mech([3, 2], [line([1, 1], [1, 1], "#abcdef", 4)], "#ffffff"))
    ]
  }
});

const COLORS = [
  "#ff0000", "#FF0000", "#f00", "#F0a", "#ff000080", "#F0A8", "#00000000", "#2b66a2",
  "red", "Red", "RED", "white", "black", "rebeccapurple", "transparent", "currentcolor", "CurrentColor",
  "rgb(1,2,3)", "rgb(1, 2, 3)", "rgb(1 2 3)", "rgba(1,2,3,0.5)", "rgb(1 2 3 / 50%)", "rgb(10.5, 20, 30)",
  "rgb(100%, 50%, 0%)", "rgba(0, 0, 0, 0)", "rgba(255,255,255,1)", "rgba(10,20,30,0.333)", "rgb(300, -5, 20)",
  "hsl(120, 100%, 50%)", "hsla(240, 100%, 50%, 0.25)", "hsl(0 0% 50%)",
  "", " ", "garbage", "#12345", "#ff000g", "rgb(1,2)", "#", "inherit", "initial", "unset",
  " #00ff00 ", "#ABCDEF", "#abc123", "#8000ff80",
  "000020", "123", "0", "1e3", "abc", "ABCDEF", "fff", "ff00ff", "999999", "00ff00 ", " 0a0", "12345", "1234567",
  "1000000", "-1", "+5", "1.5", "12px", "1ab", "0001ff", "g00", "abcd"
];

mechanismFixtures.push({
  id: "colors",
  fixture: {
    steps: [
      ...START(MECH),
      render(
        MECH,
        mech(
          [4, 4],
          COLORS.map((c, i) => line([0.1 * (i % 20), 0.2 * Math.floor(i / 20)], [0.1 * (i % 20) + 0.5, 0.2 * Math.floor(i / 20) + 1], c, 2))
        )
      ),
      render(MECH, mech([4, 4], [line([1, 1], [2, 2], 0, 3)], "#123456")),
      ...COLORS.map((c) => render(MECH, mech([4, 4], [line([1, 1], [2, 2], "#00ff00", 3)], c))),
      render(MECH, mech([4, 4], [line([1, 1], [2, 2], "#00ff00", 3)], "garbage"))
    ]
  }
});

const WEIGHTS = [0, 0.5, 1, 2.25, 1e-7, 3e-7, 1e21, 2e21, -3, -0, 123456.789, 1 / 3, 7654321.123, 0.1 + 0.2, 1e300, 5e-324];
mechanismFixtures.push({
  id: "weights",
  fixture: {
    steps: [
      ...START(MECH),
      render(MECH, mech([2, 2], WEIGHTS.map((wt, i) => line([0.1 * i, 0.2], [0.1 * i, 1.8], "#ff00ff", wt)))),
      ...WEIGHTS.map((wt) => render(MECH, mech([2, 2], [line([0.25, 0.5], [1.5, 1.75], "#ff00ff", wt)])))
    ]
  }
});

const DIMS = [
  [3, 2], [2, 3], [1, 1], [1100, 640], [1100, 650], [0, 5], [5, 0], [0, 0], [NaN, 1], [1, NaN],
  [Infinity, 2], [2, Infinity], [Infinity, Infinity], [-2, 1], [2, -1], [], [3], [1e-9, 1], [1e9, 1e-9], [1 / 3, 7],
  [80, 33], [800, 330], [8, 3.3], [160, 66], [0.8, 0.33], [24, 9.9], [2.4242424242424243, 1], [56, 23.1]
];
const COORDS = [
  line([0, 0], [1, 1], "#ff0000", 2),
  line([-1, -1], [5, 5], "#00ff00", 2),
  line([0.5, null], [null, 0.5], "#0000ff", 2),
  line([0.5], [], "#ffff00", 2),
  line([NaN, 0.5], [0.25, NaN], "#00ffff", 2),
  line([1e21, 1e-7], [-1e21, 0.1], "#ff00ff", 2),
  line([1 / 3, 2 / 3], [0.1 + 0.2, 0.7], "#808080", 2)
];
mechanismFixtures.push({
  id: "aspect",
  fixture: {
    steps: [...START(MECH), ...DIMS.map((d) => render(MECH, mech(d, COORDS)))]
  }
});

{
  const rand = mulberry32(808);
  const lines = [];
  for (let i = 0; i < 160; i++) {
    const r = () => Math.round(rand() * 4e6) / 1e6;
    const color = "#" + Math.floor(rand() * 0xffffff).toString(16).padStart(6, "0");
    lines.push(line([r(), r() * 0.75], [r(), r() * 0.75], color, Math.round(rand() * 2000) / 100));
  }
  mechanismFixtures.push({
    id: "many",
    fixture: { steps: [...START(MECH), render(MECH, mech([4, 3], lines)), render(MECH, mech([4, 3], lines.slice(0, 80))), render(MECH, mech([4, 3], lines.slice(150)))] }
  });
}

const RESIZES = [
  ["640px", "480px"], ["641px", "480px"], ["300px", "600px"], ["0px", "400px"], ["400px", "0px"],
  ["1px", "1px"], ["333.3px", "222.7px"], ["517.25px", "301.4px"], ["1100px", "200px"], ["99.9px", "1000px"]
];
const resizeSteps = (type, command) => RESIZES.flatMap(([w, h]) => [size(w, h), render(type, command)]);

// A 640x490 container has exactly the aspect ratio of these dimensions, and the two branches of
// the fit comparison give sizes one ulp apart.
const TIE_DIMS = [91.42857142857142, 70];
const tieSteps = (type, command) => [size("640px", "480px"), render(type, command)];

mechanismFixtures.push({
  id: "resize",
  fixture: { steps: [...START(MECH), ...resizeSteps(MECH, ARM), ...tieSteps(MECH, mech(TIE_DIMS, [line([10, 10], [50, 60], "#ff0000", 3)]))] }
});
mechanismFixtures.push({
  id: "dpr15",
  fixture: { window: { width: 1017, height: 689, devicePixelRatio: 1.5 }, steps: [...START(MECH), render(MECH, ARM), ...resizeSteps(MECH, ARM)] }
});
mechanismFixtures.push({
  id: "dpr2",
  fixture: { window: { width: 900, height: 701, devicePixelRatio: 2, theme: "dark" }, steps: [...START(MECH), render(MECH, ARM), ...resizeSteps(MECH, mech([1, 3], COORDS))] }
});

mechanismFixtures.push({
  id: "throws",
  fixture: {
    steps: [
      ...START(MECH),
      render(MECH, ARM),
      render(MECH, mech([2, 2], [line([0, 0], [1, 1], "#ff0000", null)])),
      render(MECH, mech([2, 2], [line([0, 0], [1, 1], "#ff0000", 4), line([1, 1], [2, 0.5], "#00ff00", 2), line([2, 0.5], [0, 2], "#0000ff", null), line([0, 0], [2, 2], "#ffffff", 3)])),
      render(MECH, ARM),
      render(MECH, mech([2, 2], [line([0, 0], [1, 1], "garbage", 4), { start: [1, 1], end: [0.5, 0.5], color: "#ff0000" }])),
      render(MECH, null),
      render(MECH, mech([2, 2], [line([0, 0], [1, 1], 0, null)], "blue")),
      render(MECH, mech([2, 1], [line([0.5, 0.5], [1.5, 0.5], "#ff00ff", 1)]))
    ]
  }
});

mechanismFixtures.push({
  id: "hidden",
  fixture: {
    steps: [
      ...START(MECH),
      render(MECH, ARM),
      { main: "new-tab", data: MECH },
      { advance: 50 },
      render(MECH, ARM, 0),
      render(MECH, mech([2, 1], [line([0, 0], [2, 1], "#ff0000", 3)]), 0),
      render(MECH, ARM, 1),
      { eval: "window.tabs.setSelected(1)" },
      { advance: 50 },
      render(MECH, ARM, 0),
      render(MECH, ARM, 1)
    ]
  }
});

const nan = { n: "7ff8000000000000" };
const inf = { n: "7ff0000000000000" };
const R08_LOG = [
  ["createBlankField", "/R08/NanWeight", 7],
  ["setStructuredType", "/R08/NanWeight", "Mechanism2d"],
  ["setGeneratedParent", "/R08/NanWeight"],
  ["putString", "/R08/NanWeight/backgroundColor", 0.0, "#101010"],
  ["putNumberArray", "/R08/NanWeight/dims", 0.0, [2.0, 1.0]],
  ["putNumber", "/R08/NanWeight/root/x", 0.0, 0.5],
  ["putNumber", "/R08/NanWeight/root/y", 0.0, 0.25],
  ["putNumber", "/R08/NanWeight/root/arm/angle", 0.0, 45.0],
  ["putNumber", "/R08/NanWeight/root/arm/length", 0.0, 0.5],
  ["putString", "/R08/NanWeight/root/arm/color", 0.0, "#ff8800"],
  ["putNumber", "/R08/NanWeight/root/arm/weight", 0.0, 6.0],
  ["putNumber", "/R08/NanWeight/root/arm/weight", 2.0, nan],
  ["putNumber", "/R08/NanWeight/root/arm/weight", 4.0, 3.0],
  ["createBlankField", "/R08/Odd", 7],
  ["setStructuredType", "/R08/Odd", "Mechanism2d"],
  ["setGeneratedParent", "/R08/Odd"],
  ["putString", "/R08/Odd/backgroundColor", 0.0, "not a color"],
  ["putNumberArray", "/R08/Odd/dims", 0.0, [3.0]],
  ["putNumberArray", "/R08/Odd/base/position", 0.0, [0.75]],
  ["putNumber", "/R08/Odd/base/link/angle", 0.0, 90.0],
  ["putNumber", "/R08/Odd/base/link/length", 0.0, inf],
  ["putNumber", "/R08/Odd/base/link/weight", 0.0, 2.5],
  ["putNumber", "/R08/Odd/base/link/tip/angle", 0.0, -30.0],
  ["putNumber", "/R08/Odd/base/link/tip/length", 0.0, 0.3],
  ["putString", "/R08/Odd/base/link/tip/color", 0.0, "rgb(10, 200, 30)"],
  ["putNumber", "/R08/Odd/base/link/tip/weight", 0.0, 1.5],
  ["createBlankField", "/R08/Wide", 7],
  ["setStructuredType", "/R08/Wide", "Mechanism2d"],
  ["setGeneratedParent", "/R08/Wide"],
  ["putString", "/R08/Wide/backgroundColor", 0.0, "#204060"],
  ["putNumberArray", "/R08/Wide/dims", 0.0, [5.0, 0.5]],
  ["putNumber", "/R08/Wide/r/x", 0.0, 1.0],
  ["putNumber", "/R08/Wide/r/y", 0.0, 0.1],
  ["putNumber", "/R08/Wide/r/bar/angle", 0.0, 0.0],
  ["putNumber", "/R08/Wide/r/bar/length", 0.0, 3.0],
  ["putString", "/R08/Wide/r/bar/color", 0.0, "#00ffcc"],
  ["putNumber", "/R08/Wide/r/bar/weight", 0.0, 12.0],
  ["putNumberArray", "/R08/Pts/Xs", 0.0, [0.0, 1.0, 2.5, 3.9999, 4.0, 4.5, -0.25]],
  ["putNumberArray", "/R08/Pts/Ys", 0.0, [0.0, 0.5, 1.5, 2.25, 3.0, 1.0, 1.0]],
  ["putNumberArray", "/R08/Pts/Xs", 3.0, [0.5, nan, 2.0]],
  ["putNumberArray", "/R08/Pts/Ys", 3.0, [0.5, 1.0, inf]]
];
write(path.join(SPECS, "logs"), "r08.json", R08_LOG);

const sel = "window.tabs.tabList[window.tabs.selectedTab].controller";
const LOG_INPUTS = ["../logs/c05.json", "../logs/r08.json"];
const LOG_STEPS = [{ log: "../logs/c05.json" }, { log: "../logs/r08.json" }];

mechanismFixtures.push({
  id: "controller",
  inputs: LOG_INPUTS,
  fixture: {
    steps: [
      ...LOG_STEPS,
      ...START(MECH),
      { eval: `${sel}.sourceList.addField("/Mech/Elevator")` },
      { eval: "window.selection.setSelectedTime(1)" },
      { advance: 50 },
      { eval: `${sel}.sourceList.addField("/R08/Wide")` },
      { advance: 50 },
      { eval: `${sel}.sourceList.addField("/R08/Odd")` },
      { advance: 50 },
      { eval: `${sel}.sourceList.addField("/R08/NanWeight")` },
      { advance: 50 },
      { eval: "window.selection.setSelectedTime(2.5)" },
      { advance: 50 },
      { eval: "window.selection.setSelectedTime(5)" },
      { advance: 50 },
      { eval: `${sel}.sourceList.clear()` },
      { advance: 50 }
    ]
  }
});

// Points

const pset = (points, shape = "plus", sizeName = "medium", groupSize = 0) => ({ points, shape, size: sizeName, groupSize });
const pcmd = (dimensions, sets) => ({ dimensions, sets });
const GRID = [[0.5, 0.5], [1, 2], [2.5, 1.25], [3.75, 2.75], [1 / 3, 2 / 3]];

const pointsFixtures = [];

pointsFixtures.push({
  id: "shapes",
  fixture: {
    steps: [
      ...START(POINTS),
      render(POINTS, pcmd([4, 3], ["plus", "cross", "circle"].flatMap((shape) => ["small", "medium", "large"].map((s) => pset(GRID, shape, s, 0))))),
      render(POINTS, pcmd([4, 3], [])),
      render(POINTS, pcmd([4, 3], [pset([], "circle", "large", 2)])),
      render(POINTS, pcmd([4, 3], [pset([[2, 1.5]], "cross", "small", 0)]))
    ]
  }
});

{
  const pts = [];
  for (let i = 0; i < 25; i++) pts.push([0.15 * i, 0.1 * i]);
  const GROUPS = [0, 1, 2, 3, 0.5, 0.99, 1.5, 10, 2.5, -1, Infinity, 1e-9, 1e9, -0];
  pointsFixtures.push({
    id: "groups",
    fixture: {
      steps: [
        ...START(POINTS),
        ...GROUPS.map((g) => render(POINTS, pcmd([4, 3], [pset(pts, "circle", "small", g)]))),
        render(POINTS, pcmd([4, 3], GROUPS.slice(0, 4).map((g, i) => pset(pts.slice(i * 5, i * 5 + 12), ["plus", "cross", "circle"][i % 3], "large", g))))
      ]
    }
  });
}

{
  const EDGE = [
    [0, 0], [4, 3], [4, 0], [0, 3], [4.000001, 1], [-0.000001, 1], [1, -1e-9], [1, 3.0000001], [-0, -0],
    [NaN, 1], [1, NaN], [NaN, NaN], [Infinity, 1], [1, -Infinity], [1e-7, 1e-7], [3.9999999999, 2.9999999999], [1e-300, 2]
  ];
  const DIMS_P = [
    [4, 3], [0, 0], [0.5, 0.25], [1e9, 1], [10, 1e-3], [1, 1], [0, 3], [4, 0], [-1, 2], [1e-7, 1e-7], [1.5, 1.5],
    [80, 33], [800, 330], [8, 3.3], [160, 66], [24, 9.9], [56, 23.1], [2.4242424242424243, 1]
  ];
  pointsFixtures.push({
    id: "bounds",
    fixture: { steps: [...START(POINTS), ...DIMS_P.map((d) => render(POINTS, pcmd(d, [pset(EDGE, "plus", "medium", 3)])))] }
  });
}

const PT_CMD = pcmd([5, 2], [pset([[0, 0], [1.25, 0.5], [2.5, 1], [5, 2], [4.1, 0.3]], "cross", "large", 2), pset([[3.3, 1.7], [0.7, 1.1]], "circle", "small", 0)]);

pointsFixtures.push({
  id: "dark",
  fixture: {
    window: { theme: "dark" },
    steps: [...START(POINTS), render(POINTS, PT_CMD), render(POINTS, pcmd([2, 2], [pset([[1, 1], [0.5, 1.5]], "plus", "small", 0.5)]))]
  }
});
pointsFixtures.push({
  id: "resize",
  fixture: { steps: [...START(POINTS), ...resizeSteps(POINTS, PT_CMD), ...tieSteps(POINTS, pcmd(TIE_DIMS, [pset([[10, 10], [50, 60], [91.42857142857142, 70]], "plus", "large", 0)]))] }
});
pointsFixtures.push({
  id: "dpr15",
  fixture: { window: { width: 1017, height: 689, devicePixelRatio: 1.5 }, steps: [...START(POINTS), render(POINTS, PT_CMD), ...resizeSteps(POINTS, PT_CMD)] }
});
pointsFixtures.push({
  id: "dpr2",
  fixture: {
    window: { width: 900, height: 701, devicePixelRatio: 2, theme: "dark" },
    steps: [...START(POINTS), render(POINTS, PT_CMD), ...resizeSteps(POINTS, pcmd([1, 3], [pset(GRID.map(([x, y]) => [x / 4, y]), "circle", "medium", 1)]))]
  }
});

pointsFixtures.push({
  id: "throws",
  fixture: {
    steps: [
      ...START(POINTS),
      render(POINTS, PT_CMD),
      render(POINTS, pcmd([4, 3], [pset([[1, 1], [2, 2]], "plus", "large", 0), pset([[0.5, 0.5], [3, 2]], "cross", "small", NaN), pset([[1, 2]], "circle", "medium", 0)])),
      render(POINTS, pcmd([4, 3], [pset([[1, 1], [9, 9], [2, 2]], "circle", "large", NaN)])),
      render(POINTS, pcmd([4, 3], [pset([[9, 9], [-1, 1]], "circle", "large", NaN)])),
      render(POINTS, PT_CMD)
    ]
  }
});

{
  const rand = mulberry32(1108);
  const sets = [];
  for (let s = 0; s < 6; s++) {
    const pts = [];
    for (let i = 0; i < 70; i++) pts.push([Math.round(rand() * 8.4e6) / 1e6 - 0.2, Math.round(rand() * 6.3e6) / 1e6 - 0.15]);
    sets.push(pset(pts, ["plus", "cross", "circle"][s % 3], ["small", "medium", "large"][(s * 2) % 3], s));
  }
  pointsFixtures.push({ id: "many", fixture: { steps: [...START(POINTS), render(POINTS, pcmd([8, 6], sets))] } });
}

pointsFixtures.push({
  id: "hidden",
  fixture: {
    steps: [
      ...START(POINTS),
      render(POINTS, PT_CMD),
      { main: "new-tab", data: MECH },
      { advance: 50 },
      render(POINTS, PT_CMD),
      render(POINTS, pcmd([1, 1], [pset([[0.5, 0.5]], "plus", "large", 0)]))
    ]
  }
});

const pointsSource = (type, logKey, logType, options) => ({ type, logKey, logType, visible: true, options });
pointsFixtures.push({
  id: "controller",
  inputs: LOG_INPUTS,
  fixture: {
    steps: [
      ...LOG_STEPS,
      ...START(POINTS),
      { eval: "window.selection.setSelectedTime(1)" },
      {
        eval: `${sel}.restoreState(${JSON.stringify({
          sources: [
            pointsSource("plus", "/Vision/Corner", "Translation2d", { size: "small", groupSize: "2" }),
            pointsSource("circle", "/Vision/Translations", "Translation2d[]", { size: "large", groupSize: "1" }),
            pointsSource("plusSplit", "/R08/Pts/Xs", "NumberArray", { component: "x", size: "medium", groupSize: "3" }),
            pointsSource("component", "/R08/Pts/Ys", "NumberArray", { component: "y" })
          ],
          width: 4,
          height: 3
        })})`
      },
      { advance: 50 },
      { eval: "window.selection.setSelectedTime(3.5)" },
      { advance: 50 },
      {
        eval: `${sel}.restoreState(${JSON.stringify({
          sources: [
            pointsSource("crossSplit", "/R08/Pts/Xs", "NumberArray", { component: "x", size: "large", groupSize: "0" }),
            pointsSource("component", "/R08/Pts/Ys", "NumberArray", { component: "y" }),
            pointsSource("plus", "/Vision/Translations", "Translation2d[]", { size: "small", groupSize: "abc" })
          ],
          width: 4,
          height: 3
        })})`
      },
      { advance: 50 },
      { eval: "window.selection.setSelectedTime(1)" },
      { advance: 50 },
      { eval: `${sel}.restoreState(${JSON.stringify({ sources: [pointsSource("cross", "/Vision/Translations", "Translation2d[]", { size: "medium", groupSize: "1" })], width: 1, height: 1 })})` },
      { advance: 50 }
    ]
  }
});

// CSS serialization of the values the painters assign, on fresh elements.
const CSS_PROBE = (body) => String.raw`(() => {
  const html = (tag) => document.createElement(tag);
  const svg = (tag) => document.createElementNS("http://www.w3.org/2000/svg", tag);
  const camel = (p) => p.replace(/-([a-z])/g, (m, c) => c.toUpperCase());
  const one = (make, prop, value) => {
    const el = make();
    el.style[camel(prop)] = value;
    return [el.style.getPropertyValue(prop), el.hasAttribute("style") ? el.getAttribute("style") : null];
  };
  ${body}
})()`;

const numberStrings = (() => {
  const rand = mulberry32(2208);
  const fixed = [
    0, -0, 1, -1, 0.5, 12, 1100, 1101, 640.5, 1 / 3, 2 / 3, 0.1 + 0.2, 123456.789, 1234567.5, 999999.5, 9999995, 0.000001,
    0.0000012345678, 1e-7, 1.5e-7, 5e-324, 1e-300, 1e21, 1.5e21, 1e300, 1.7976931348623157e308, 123.456789, 99.99995, 99.999949,
    0.00001234565, 1.000005, 1.0000049999, 2.5e-5, 517.25, 301.4, 333.3333333333333, 222.66666666666666, NaN, Infinity, -Infinity,
    -12.75, -0.0000001, 4.94e-322, 65535.99999, 16777216.5, 1e6, 1e5, 999999, 1e-5, 0.0001, 0.00009999995,
    0.1000005, 0.1234565, 0.5000005, 0.9999995, 0.2500005, 0.3333335, 0.6666665, 0.7000005, 0.4444445, 0.8765435,
    0.12345659, 0.99999951, 0.10000049, 1.2345675, 12.3456785, 123.4567855, 0.0123455, 0.0012345
  ];
  const out = fixed.map((v) => String(v));
  for (let i = 0; i < 120; i++) {
    const exp = Math.floor(rand() * 16) - 8;
    const v = (rand() * 10) * Math.pow(10, exp) * (rand() < 0.2 ? -1 : 1);
    out.push(String(v));
  }
  for (let i = 0; i < 40; i++) out.push(String(Math.ceil(rand() * 2000 + 1)));
  return out;
})();

const group = (ns, tag, property, values) =>
  `[${JSON.stringify(property)}, ${JSON.stringify(ns)}, ${JSON.stringify(tag)}, ${values}.map((v) => [v, ...one(() => ${ns}(${JSON.stringify(tag)}), ${JSON.stringify(property)}, v)])]`;
const COLOR_VALUES = `[...${JSON.stringify([...COLORS, "white", "black", "#e5b31b", "#af2437", "#80588e", "#e48b32", "#c0b487", "#858584", "#3b875a", "#d993aa", "#5f4528", "#203040", "#101010", "not a color", "rgb(10, 200, 30)", "#00ffcc", "#204060", "blue", "Blue", "#FFFFFF"])}, 0, null]`;
const LENGTH_VALUES = JSON.stringify(numberStrings.map((s) => s + "px"));

// Colour strings a Mechanism log can carry, by syntax. Every keyword of Chromium m142's
// css_value_keywords.json5 (r08ColorKeywords.json), the rgb()/hsl() forms Blink's fast path and
// full parser take, hex, and an "open" set the port may report as unmodeled.
const colorSyntaxRows = (() => {
  const rand = mulberry32(4242);
  const pick = (xs) => xs[Math.floor(rand() * xs.length)];
  const rows = [];
  const keywords = JSON.parse(fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), "r08ColorKeywords.json"), "utf8"));
  for (const k of keywords) rows.push(["keyword", k], ["keyword", k.toUpperCase()]);
  const pcts = [];
  for (let i = -20; i <= 1020; i += 3) pcts.push(String(i / 10));
  for (let i = 0; i <= 255; i++) pcts.push(String(i / 2.55), String((i + 0.5) * 100 / 255), ((i + 0.5) * 100 / 255).toFixed(4));
  for (const p of pcts) rows.push(["rgb", `rgb(${p}%, 0%, 0%)`], ["rgb", `rgb(0% ${p}% 0%)`]);
  for (let i = 0; i <= 2560; i += 5) rows.push(["rgb", `rgb(${i / 10}, 0, 0)`]);
  const num = () => { const r = rand(); if (r < 0.3) return String(Math.floor(rand() * 400) - 20); if (r < 0.6) return String(Math.round(rand() * 1e4) / 100); return String(rand() * 400 - 20); };
  const pct = () => { const r = rand(); if (r < 0.3) return String(Math.floor(rand() * 120) - 5) + "%"; if (r < 0.6) return String(Math.round(rand() * 1e4) / 100) + "%"; return String(rand() * 110 - 2) + "%"; };
  const alpha = () => { const r = rand(); if (r < 0.15) return pick(["0", "1", ".5", "0.5", "1.0", "0.0", "-0.5", "1.5", "2", "-1", "0.1", "0.9", ".3", "0.05", "0.95", "00.5", "1.00001"]); if (r < 0.45) return String(Math.round(rand() * 1000) / 1000); if (r < 0.7) return String(rand()); if (r < 0.85) return String(Math.round(rand() * 1000) / 10) + "%"; return String(rand() * 1.2 - 0.1); };
  const numberAlpha = () => { let a; do a = alpha(); while (a.endsWith("%")); return a; };
  for (let i = 0; i < 2500; i++) {
    const comma = rand() < 0.6, withAlpha = rand() < 0.5, p = rand() < 0.5;
    const ch = [0, 1, 2].map(() => (p ? pct() : num()));
    const fn = pick(["rgb", "rgba"]);
    rows.push(["rgb", comma ? `${fn}(${ch.join(", ")}${withAlpha ? ", " + alpha() : ""})` : `${fn}(${ch.join(" ")}${withAlpha ? " / " + alpha() : ""})`]);
  }
  const hue = (units) => { const r = rand(); const n = r < 0.4 ? String(Math.floor(rand() * 800) - 200) : r < 0.7 ? String(Math.round(rand() * 36000) / 100) : String(rand() * 720 - 180); return n + (units ? pick(["", "deg", "rad", "grad", "turn", "DEG", "Turn"]) : ""); };
  for (let i = 0; i < 1500; i++) {
    const comma = rand() < 0.6, withAlpha = rand() < 0.5;
    const ch = [hue(true), pct(), pct()];
    const fn = pick(["hsl", "hsla"]);
    rows.push(["hsl", comma ? `${fn}(${ch.join(", ")}${withAlpha ? ", " + numberAlpha() : ""})` : `${fn}(${ch.join(" ")}${withAlpha ? " / " + numberAlpha() : ""})`]);
  }
  for (let i = 0; i < 300; i++) {
    const ch = [hue(false), pct(), pct()];
    rows.push(["open", `hsla(${ch.join(", ")}, ${Math.round(rand() * 1000) / 10}%)`]);
  }
  const hexDigits = (n) => Array.from({ length: n }, () => pick([..."0123456789abcdefABCDEF"])).join("");
  for (let i = 0; i < 400; i++) {
    const n = pick([1, 2, 3, 4, 5, 6, 7, 8, 9]);
    rows.push(["hex", (rand() < 0.8 ? "#" : "") + hexDigits(n)]);
  }
  for (const c of [
    "rgb(1, 2, 3, 4, 5)", "rgb(1)", "rgb()", "rgb(1, 2, 3", "rgb(1 2 3 4)", "rgb(1,2 3)", "rgb(1 2, 3)", "rgba(1,2,3,)",
    "rgb(1,,2,3)", "rgb(1%,2,3)", "rgb(1,2%,3)", "rgb(1e2%, 0%, 0%)", "rgb(-0, -0.5, 0.49999999)", "rgb(255.4999999, 0.5000000, 254.50000001)",
    "rgb(00000000000000000001, 0, 0)", "rgb(1.12345678901234567890, 0, 0)", "rgb(99999999999999999999, 0, 0)", "rgb(0.0000000001%, 0%, 0%)",
    "rgb(1,2,3);", "rgb(1,2,3) ", " rgb(1,2,3)", "rgb(1,2,3)red", "RGB(1.5, 2.5, 3.5)", "rgb(1e400, 0, 0)", "rgb(-1e400, 0, 0)",
    "rgb(5., 0, 0)", "rgb(.5, 0, 0)", "rgb(+5, 0, 0)", "rgb(1e1, 0, 0)", "rgb(1E1, 0, 0)", "rgb(10,0,0,)", "rgb(10 , 0 , 0)", "rgb(10,0,0) x",
    "rgba(10,0,0)", "rgb(10,0,0,0.5)", "rgb(10 0 0 / 0.5)", "rgba(10 0 0 / 50%)", "rgb(255 50% 0)", "rgb(255, 50%, 0)", "rgb(10 0 0",
    "rgb(1px, 2, 3)", "rgb(1-2 3)", "rgb(1 2 3 / 1e-1)", "rgb(0 0 0 / -50%)", "rgb(0, 0, 0, 150%)", "#ff000080", "red !important",
    "red;", "#fff !important", "red red", "1px red", "#fff;", "#ffff", "#ffffff0", "#ff ff ff", "# fff", "#-ff", "#_ff", "inherit red",
    "none red", "rgb (1,2,3)", "rgb(\n1,2,3)", "", " ", "garbage", "not a color", "12px", "1e3", "-1", "+5", "1.5", "1ab", "0001ff", "-abc", "--abc", "_abc"
  ]) rows.push(["rgb", c]);
  for (const c of [
    "hsl(120deg, 50%, 50%)", "hsl(0.5turn 50% 50%)", "hsl(1rad, 50%, 50%)", "hsl(100grad 50% 50%)", "hsl(120 50 50)", "hwb(120 10% 10%)",
    "lab(50% 40 59)", "lch(50% 40 59)", "oklab(0.5 0.1 0.1)", "oklch(0.5 0.1 120)", "color(srgb 1 0 0)", "color-mix(in srgb, red, blue)",
    "light-dark(red, blue)", "rgb(none 0 0)", "rgb(calc(10), 0, 0)", "rgb(/**/10,0,0)", "rgb(infinity, 0, 0)", "rgb(calc(infinity), 0, 0)",
    "hsl(120 50% 50% / 50%)", "hsl(120, 150%, 50%)", "hsl(1e2, 50%, 50%)", "red /**/", "/**/red", "r\\65 d", "'red'", "url(#a)", "url(#a) red",
    "rgb(from red r g b)", "var(--x)", "rgb(var(--x), 0, 0)", " #ff0000", "réd", "context-fill red"
  ]) rows.push(["open", c]);
  return rows;
})();

// Each group is [property, namespace, tag, rows] on the element type the renderers assign it
// to: Blink parses inline style on SVG elements with quirks behaviour.
const cssSpecs = [
  {
    id: "lengths",
    expr: CSS_PROBE(`return [${["left", "top"].map((p) => group("svg", "svg", p, LENGTH_VALUES)).join(", ")}, ${["width", "height"].map((p) => group("html", "div", p, LENGTH_VALUES)).join(", ")}];`)
  },
  {
    id: "stroke-width",
    expr: CSS_PROBE(`return [${group("svg", "line", "stroke-width", JSON.stringify(numberStrings))}];`)
  },
  {
    id: "colors",
    expr: CSS_PROBE(`return [${group("svg", "line", "stroke", COLOR_VALUES)}, ${group("svg", "circle", "fill", COLOR_VALUES)}, ${group("svg", "svg", "fill", COLOR_VALUES)}, ${group("svg", "svg", "background-color", COLOR_VALUES)}];`)
  },
  {
    id: "color-syntax",
    expr: CSS_PROBE(`const ctx = new OffscreenCanvas(1, 1).getContext("2d", { willReadFrequently: true });
  const canvas = (value) => {
    ctx.fillStyle = "#010203";
    ctx.fillStyle = value;
    ctx.clearRect(0, 0, 1, 1);
    ctx.fillRect(0, 0, 1, 1);
    return [ctx.fillStyle, Array.from(ctx.getImageData(0, 0, 1, 1).data)];
  };
  return ${JSON.stringify(colorSyntaxRows)}.map(([kind, value]) => [kind, value, one(() => svg("line"), "stroke", value)[0], one(() => svg("circle"), "fill", value)[0], one(() => svg("svg"), "background-color", value)[0], ...canvas(value)]);`)
  },
  {
    id: "keywords",
    expr: CSS_PROBE(`return [${group("svg", "svg", "display", JSON.stringify(["none", "initial", "block", "", "garbage"]))}, ${group("svg", "svg", "transform", JSON.stringify(["translate(-50%,-50%) scale(1, 1)", "translate(-50%,-50%) scale(0.5, 0.5)", "translate(-50%,-50%) scale(0.25, 0.25)"]))}];`)
  },
  {
    id: "declarations",
    expr: CSS_PROBE(`const scripts = [
    [["svg", "svg"], ["style", "display", "none"], ["attr", "width", "10"], ["style", "background-color", "#ff0000"], ["attr", "height", "5"]],
    [["svg", "svg"], ["attr", "width", "10"], ["sync"], ["style", "display", "initial"], ["attr", "height", "5"], ["style", "background-color", "red"]],
    [["svg", "line"], ["attr", "x1", "1"], ["style", "stroke", "garbage"], ["style", "stroke-width", "-3"], ["attr", "x2", "2"]],
    [["svg", "line"], ["style", "stroke", "#00ff00"], ["style", "stroke-width", "2"], ["style", "stroke", "#0000ff"]],
    [["svg", "line"], ["style", "stroke", "#00ff00"], ["style", "stroke-width", "2"], ["style", "stroke", ""], ["style", "stroke", "blue"]],
    [["svg", "line"], ["style", "stroke", "#00ff00"], ["sync"], ["style", "stroke", ""], ["attr", "x1", "3"]],
    [["svg", "line"], ["style", "stroke", "#00ff00"], ["style", "stroke", "garbage"], ["style", "stroke-width", "NaN"]],
    [["svg", "svg"], ["style", "display", "none"], ["sync"], ["style", "display", "initial"], ["style", "background-color", "garbage"], ["style", "background-color", "#123456"], ["sync"], ["style", "display", "none"], ["style", "background-color", "nope"]],
    [["html", "div"], ["attr", "class", "points-background"], ["style", "width", "NaNpx"], ["style", "height", "12px"], ["style", "width", "13px"], ["sync"], ["style", "height", "NaNpx"], ["style", "width", "14px"]],
    [["svg", "line"], ["style", "stroke", "#00ff00"], ["style", "stroke", ""], ["attr", "x1", "3"]],
    [["svg", "line"], ["style", "stroke", "garbage"], ["style", "stroke", ""], ["attr", "x1", "3"]],
    [["svg", "line"], ["attr", "x1", "3"], ["sync"], ["style", "stroke", "red"], ["sync"], ["style", "stroke", "red"], ["attr", "x1", "4"]],
    [["svg", "svg"], ["style", "background-color", "000020"], ["style", "display", "none"], ["style", "background-color", "nope"], ["style", "background-color", "123"]],
    [["svg", "svg"], ["attr", "height", "20"], ["attr", "width", "20"], ["style", "transform", "translate(-50%,-50%) scale(1, 1)"], ["style", "fill", "#2b66a2"], ["style", "stroke", "#2b66a2"], ["style", "left", "NaNpx"], ["style", "top", "3px"]]
  ];
  return scripts.map((script) => {
    const el = script[0][0] === "svg" ? svg(script[0][1]) : html(script[0][1]);
    const trace = [];
    for (const step of script.slice(1)) {
      if (step[0] === "attr") el.setAttribute(step[1], step[2]);
      else if (step[0] === "style") el.style[camel(step[1])] = step[2];
      else trace.push(el.getAttribute("style"));
    }
    trace.push(el.getAttributeNames().map((n) => [n, el.getAttribute(n)]));
    return [script, trace];
  });`)
  }
];

// Hub-level #103 cases for crates/asrs-hub/tests/dom_tabs.rs: the port's controllers feed
// the port's painters inside `Tabs`, and a painter throw ends the frame loop as upstream's does.
const hubThrow = (id, type, setup, before, after) =>
  write(path.join(SPECS, "tabs"), `${id}.entry.json`, {
    entry: "dom",
    inputs: LOG_INPUTS,
    fixture: {
      seed: 808,
      steps: [
        ...LOG_STEPS,
        { main: "new-tab", data: type },
        ...setup,
        { eval: before },
        { advance: 100 },
        { capture: ["commands", "state"], label: "drawn" },
        { eval: after },
        { advance: 100 },
        { capture: ["commands", "state"], label: "threw" },
        { advance: 100 },
        { capture: ["commands"], label: "stopped" }
      ]
    }
  });
hubThrow(
  "mechanism-r08",
  MECH,
  [{ eval: `${sel}.sourceList.addField("/R08/NanWeight")` }, { eval: `${sel}.sourceList.addField("/Mech/Elevator")` }],
  "window.selection.setSelectedTime(1)",
  "window.selection.setSelectedTime(2.5)"
);
hubThrow(
  "points-r08",
  POINTS,
  [
    {
      eval: `${sel}.restoreState(${JSON.stringify({
        sources: [
          pointsSource("plus", "/Vision/Corner", "Translation2d", { size: "small", groupSize: "2" }),
          pointsSource("circle", "/Vision/Translations", "Translation2d[]", { size: "large", groupSize: "1" })
        ],
        width: 4,
        height: 3
      })})`
    }
  ],
  "window.selection.setSelectedTime(1)",
  `${sel}.restoreState(${JSON.stringify({
    sources: [pointsSource("cross", "/Vision/Translations", "Translation2d[]", { size: "medium", groupSize: "abc" })],
    width: 4,
    height: 3
  })})`
);

for (const f of mechanismFixtures) spec("mechanism", f.id, f.fixture, f.inputs);
for (const f of pointsFixtures) spec("points", f.id, f.fixture, f.inputs);
for (const c of cssSpecs) {
  write(path.join(SPECS, "css-style"), `${c.id}.entry.json`, {
    entry: "dom",
    fixture: { steps: [{ capture: [{ kind: "eval", expr: c.expr }], label: c.id }] }
  });
}
console.log(`r08: ${mechanismFixtures.length} mechanism, ${pointsFixtures.length} points, ${cssSpecs.length} css-style specs`);
