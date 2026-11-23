// Swerve and Joysticks renderer fixtures for `crates/asrs-tabs/tests/swerve_joysticks_dom.rs`.
// Writes parity/specs/dom/{swerve,joysticks}/*.entry.json, parity/specs/dom/assets/r07.json and
// parity/specs/dom/tabs/{swerve,joysticks}-r07.entry.json; edit this script, not the specs.

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

// The recording canvas never throws, so `arc` and `ellipse` also run on a context of an
// untouched realm, whose IndexSizeError reaches the renderer before the call is recorded.
const MIRROR = String.raw`const frame = document.body.appendChild(document.createElement("iframe"));
  frame.style.display = "none";
  const real = frame.contentDocument.createElement("canvas").getContext("2d");
  if (!(real instanceof frame.contentWindow.CanvasRenderingContext2D)) throw new Error("no real 2D context");
  const mirror = (canvas) => {
    canvas.getContext("2d");
    const ctx = canvas.__oracleContext;
    const method = ctx.method;
    ctx.method = function (name, args) {
      if (name === "arc" || name === "ellipse") real[name](...args);
      return method.call(this, name, args);
    };
  };`;

// Records every render of every Swerve and Joysticks renderer: the size reads it made, the
// theme and device pixel ratio it ran under, the canvas style strings as assigned, the canvas
// size and calls when it reset the canvas, and a throw. Style accessors are shadowed per
// instance because Blink serves the names from a named-property interceptor.
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
  const errorText = (e) => (e instanceof Error || e instanceof frame.contentWindow.DOMException ? e.name + ": " + e.message : String(e));
  const dark = () => w.matchMedia("(prefers-color-scheme: dark)").matches;
  ${MIRROR}

  let lastAssets;
  const assets = () => {
    if (w.assets === lastAssets) return;
    lastAssets = w.assets;
    write({ r07: "assets", assets: tree(w.assets) });
  };

  const renderers = [];
  const hook = (kind, r) => {
    if (renderers.includes(r)) return;
    const id = renderers.length;
    renderers.push(r);
    const canvas = r.CANVAS;
    mirror(canvas);
    const ctx = canvas.__oracleContext;
    let active = null;
    const sized = kind === "swerve" ? r.CONTAINER : canvas;
    for (const name of ["clientWidth", "clientHeight"]) {
      const getter = Object.getOwnPropertyDescriptor(Element.prototype, name).get;
      Object.defineProperty(sized, name, {
        configurable: true,
        get() {
          const value = getter.call(this);
          if (active !== null) active.reads.push([name, tree(value)]);
          return value;
        }
      });
    }
    for (const name of ["width", "height"]) {
      Object.defineProperty(canvas.style, name, {
        configurable: true,
        get() {
          return this.getPropertyValue(name);
        },
        set(value) {
          if (active !== null) active.style.push([name, tree(value)]);
          this.setProperty(name, value);
        }
      });
    }
    if (kind === "joysticks") {
      r.IMAGES.forEach((image, index) => {
        image.addEventListener("load", () => write({ r07: "load", id: tree(id), index: tree(index) }));
      });
    }
    write({ r07: "create", kind, id: tree(id), aspect: tree(r.getAspectRatio()), state: tree(r.saveState()) });

    const render = r.render;
    r.render = function (command) {
      if (kind === "joysticks") assets();
      const record = { r07: "render", kind, id: tree(id), command: tree(command), dark: tree(dark()), dpr: tree(w.devicePixelRatio), reads: [], style: [] };
      const before = ctx.frames;
      const loads = r.imageLoadCount;
      active = record;
      try {
        return render.apply(this, arguments);
      } catch (e) {
        record.threw = tree(errorText(e));
        throw e;
      } finally {
        active = null;
        if (kind === "joysticks") record.loads = tree(loads);
        record.aspect = tree(r.getAspectRatio());
        if (ctx.frames !== before) {
          record.size = tree([canvas.width, canvas.height]);
          record.calls = tree(ctx.calls);
        } else if (record.style.length > 0) throw new Error("style without a frame");
        write(record);
      }
    };
  };
  const hookAll = () => {
    w.tabs.tabList.forEach((t) => {
      if (t.type === 8) hook("joysticks", t.renderer);
      if (t.type === 9) hook("swerve", t.renderer);
    });
  };
  hookAll();
  const addTab = w.tabs.addTab;
  w.tabs.addTab = function (...args) {
    const result = addTab.apply(this, args);
    hookAll();
    return result;
  };

  w.__r07Render = (type, index, command) => {
    const tab = w.tabs.tabList.filter((t) => t.type === type)[index];
    try {
      tab.renderer.render(command);
    } catch (e) {}
  };
  w.__r07Size = (width, height) =>
    w.__oracleQuiet(() => {
      const content = document.querySelector("div.renderer-content");
      content.style.width = width;
      content.style.height = height;
      content.style.bottom = "auto";
    });
})()`;

const JOY = 8;
const SWERVE = 9;

const render = (type, command, index = 0) => ({ eval: `window.__r07Render(${type}, ${index}, ${lit(command)})` });
const size = (width, height) => ({ eval: `window.__r07Size(${JSON.stringify(width)}, ${JSON.stringify(height)})` });
const theme = (name) => ({ eval: `window.__oracleTheme = ${JSON.stringify(name)}` });
const dpr = (value) => ({ eval: `window.devicePixelRatio = ${value}` });

function spec(area, id, fixture, inputs = []) {
  const body = { entry: "dom", inputs, fixture: { geometry: "real", ...fixture } };
  if (inputs.length === 0) delete body.inputs;
  write(path.join(SPECS, area), `${id}.entry.json`, body);
}

const START = (type, extra = []) => [{ main: "new-tab", data: type }, ...extra, { eval: RECORDER }, { advance: 50 }];

const RESIZES = [
  ["640px", "480px"], ["641px", "480px"], ["300px", "600px"], ["0px", "400px"], ["400px", "0px"],
  ["1px", "1px"], ["333.3px", "222.7px"], ["517.25px", "301.4px"], ["1100px", "200px"], ["99.9px", "1000px"]
];
const resizeSteps = (type, command) => RESIZES.flatMap(([w, h]) => [size(w, h), render(type, command)]);

// Swerve

const modules = (color, ...values) => ({ values: values.map(([velocity, angle]) => ({ velocity, angle })), color });
const speeds = (color, vx, vy, omega) => ({ value: { vx, vy, omega }, color });
const swerve = (rotation, frameAspectRatio, moduleVelocities = [], robotVelocities = []) => ({ rotation, frameAspectRatio, moduleVelocities, robotVelocities });

const FOUR = modules("#ff0000", [0.5, 0.3], [-0.25, 1.2], [1, -2.8], [0, 0.7]);
const FOUR_B = modules("#0000ff", [0.75, -0.3], [0.1, 3.1], [-1, 2], [0.04, 0]);
const SPEED = speeds("#00ff00", 0.5, -0.25, 0.75);
const TYPICAL = swerve(Math.PI / 2, 1, [FOUR, FOUR_B], [SPEED, speeds("#ffff00", -0.2, 0.7, -1.5)]);

const swerveFixtures = [];

swerveFixtures.push({
  id: "basic",
  fixture: {
    steps: [
      ...START(SWERVE),
      render(SWERVE, swerve(0, 1)),
      render(SWERVE, swerve(0, 1, [FOUR])),
      render(SWERVE, swerve(0, 1, [FOUR, FOUR_B])),
      render(SWERVE, swerve(0, 1, [], [SPEED])),
      render(SWERVE, TYPICAL),
      render(SWERVE, swerve(1.1, 0.5, [FOUR], [SPEED])),
      render(SWERVE, swerve(1.1, 1.7, [FOUR], [SPEED])),
      render(SWERVE, swerve(1.1, 0.8461538461538461, [FOUR, FOUR_B, FOUR], [SPEED])),
      ...[0, 1, 2, 3, 5, 6].map((n) =>
        render(SWERVE, swerve(0.4, 1, [modules("#112233", ...Array.from({ length: n }, (_, i) => [0.2 * (i + 1), 0.5 * i]))]))
      ),
      render(SWERVE, swerve(0.4, 1, [modules("#112233", [0.5, 1]), modules("#445566", [0.5, 1], [0.25, 2])]))
    ]
  }
});

const PI = Math.PI;
const ANGLES = [
  0, -0, PI / 6, -PI / 6, PI / 4, PI / 2, -PI / 2, PI, -PI, (5 * PI) / 6, (-5 * PI) / 6, (11 * PI) / 6, 2 * PI, -2 * PI, 3 * PI,
  -3 * PI, 7.5, -7.5, 100, -100, 1e6, 1e-9, 12345.678, 1e300, PI - (5 * PI) / 6, -PI + (5 * PI) / 6, 1e-320, 2.6179938779914944
];
const VELOCITIES = [
  0, -0, 0.001, -0.001, 0.0010000000000000002, -0.0010000000000000002, 0.0009999999999999999, 0.05, -0.05, 0.049999999999999996,
  -0.049999999999999996, 0.050000000000000003, 1, -1, 0.3333333333333333, -0.6666666666666666, 2.5, -40, 1e-300, 1e300
];
swerveFixtures.push({
  id: "angles",
  fixture: {
    steps: [
      ...START(SWERVE),
      ...ANGLES.map((a, i) => render(SWERVE, swerve(ANGLES[(i * 5 + 3) % ANGLES.length], 1, [modules("#ff00ff", [0.5, a], [-0.5, a], [1, -a], [-1, a + PI])]))),
      ...ANGLES.map((a) => render(SWERVE, swerve(a, 0.75, [FOUR, FOUR_B], [speeds("#00ffff", Math.cos(a), Math.sin(a), a)]))),
      ...VELOCITIES.map((v) => render(SWERVE, swerve(0.3, 1.2, [modules("#ff8800", [v, 0.1], [-v, 2], [v, -2], [v / 2, 4])])))
    ]
  }
});

{
  const rand = mulberry32(707);
  const r = (scale) => (rand() * 2 - 1) * scale;
  const color = () => "#" + Math.floor(rand() * 0xffffff).toString(16).padStart(6, "0");
  const commands = [];
  for (let i = 0; i < 60; i++) {
    const sets = Array.from({ length: Math.floor(rand() * 4) }, () =>
      modules(color(), ...Array.from({ length: Math.floor(rand() * 6) }, () => [r(1), r(8)]))
    );
    const robots = Array.from({ length: Math.floor(rand() * 3) }, () => speeds(color(), r(1), r(1), r(6)));
    commands.push(swerve(r(10), 0.2 + rand() * 3, sets, robots));
  }
  swerveFixtures.push({ id: "many", fixture: { steps: [...START(SWERVE), ...commands.map((c) => render(SWERVE, c))] } });
}

const ROBOTS = [
  [0, 0, 0], [0.05, 0, 0], [0.049999999999999996, 0, 0], [0.03, 0.04, 0], [0.03, -0.03999, 0], [0, 0, 0.1], [0, 0, -0.1],
  [0, 0, 0.10000000000000002], [0, 0, -0.10000000000000002], [1, 1, 7], [-1, 0, -7], [0, -1, PI], [-0, -0, -PI], [0.5, 0.5, 2 * PI],
  [NaN, 0, 0], [0, NaN, 0.5], [0.5, 0.5, NaN], [Infinity, 0, 0], [0, -Infinity, 1], [1, 1, Infinity], [1, 1, -Infinity], [1e-300, 1e-300, 1e-300],
  [1e200, 1e200, 1e200], [null, 1, 1], [1, null, 1], [1, 1, null]
];
swerveFixtures.push({
  id: "robot",
  fixture: {
    steps: [
      ...START(SWERVE),
      ...ROBOTS.map(([vx, vy, omega]) => render(SWERVE, swerve(0.2, 0.9, [], [speeds("#00aa00", vx, vy, omega)]))),
      render(SWERVE, swerve(0.2, 0.9, [], ROBOTS.map(([vx, vy, omega], i) => speeds(i % 2 === 0 ? "#00aa00" : "#aa0000", vx, vy, omega))))
    ]
  }
});

const RATIOS = [0, -0, 0.1, 0.999999999, 1, 1.000000001, 3, 1e9, 1e-9, NaN, Infinity, null, undefined, -0.5, -1, -2, -Infinity];
swerveFixtures.push({
  id: "edges",
  fixture: {
    steps: [
      ...START(SWERVE),
      ...RATIOS.map((ratio) => render(SWERVE, swerve(0.6, ratio, [FOUR]))),
      ...RATIOS.map((ratio) => render(SWERVE, swerve(0.6, ratio, [], [speeds("#123456", 0.5, 0.5, 0.05)]))),
      ...[NaN, Infinity, -Infinity, null, undefined].flatMap((x) => [
        render(SWERVE, swerve(x, 1, [FOUR], [SPEED])),
        render(SWERVE, swerve(0.6, 1, [modules("#ff0000", [x, 0.5], [0.5, x], [x, x], [0.5, 0.5])]))
      ])
    ]
  }
});

const COLORS = [
  "#ff0000", "#F0a", "#ff000080", "red", "White", "transparent", "rgb(1, 2, 3)", "rgba(1,2,3,0.5)", "rgb(1 2 3 / 50%)", "rgb(100%, 50%, 0%)",
  "hsl(120, 100%, 50%)", "", " ", "garbage", "#12345", "inherit", "initial", " #00ff00 ", "123456", undefined
];
swerveFixtures.push({
  id: "colors",
  fixture: {
    steps: [
      ...START(SWERVE),
      ...COLORS.map((c) => render(SWERVE, swerve(0, 1, [{ ...FOUR, color: c }], [{ ...SPEED, color: c }])))
    ]
  }
});

// The exact render state decides whether a frame is painted, so -0, NaN and undefined compare
// as JSON writes them.
swerveFixtures.push({
  id: "gate",
  fixture: {
    steps: [
      ...START(SWERVE),
      render(SWERVE, TYPICAL),
      render(SWERVE, TYPICAL),
      render(SWERVE, swerve(0, 1)),
      render(SWERVE, swerve(-0, 1)),
      render(SWERVE, swerve(NaN, 1)),
      render(SWERVE, swerve(null, 1)),
      render(SWERVE, swerve(Infinity, 1)),
      render(SWERVE, swerve(0, 1, [modules(undefined, [0.5, 1])])),
      render(SWERVE, swerve(0, 1, [{ values: [{ velocity: 0.5, angle: 1 }] }])),
      render(SWERVE, TYPICAL),
      theme("dark"),
      render(SWERVE, TYPICAL),
      render(SWERVE, TYPICAL),
      theme("light"),
      render(SWERVE, TYPICAL),
      dpr(2),
      render(SWERVE, TYPICAL),
      dpr(1.1),
      render(SWERVE, TYPICAL),
      dpr(1),
      render(SWERVE, TYPICAL),
      size("500px", "400px"),
      render(SWERVE, TYPICAL),
      render(SWERVE, TYPICAL),
      { advance: 100 }
    ]
  }
});

swerveFixtures.push({ id: "resize", fixture: { steps: [...START(SWERVE), ...resizeSteps(SWERVE, TYPICAL)] } });
swerveFixtures.push({
  id: "dpr15",
  fixture: { window: { width: 1017, height: 689, devicePixelRatio: 1.5 }, steps: [...START(SWERVE), render(SWERVE, TYPICAL), ...resizeSteps(SWERVE, TYPICAL)] }
});
swerveFixtures.push({
  id: "dpr2-dark",
  fixture: {
    window: { width: 900, height: 701, devicePixelRatio: 2, theme: "dark" },
    steps: [...START(SWERVE), render(SWERVE, TYPICAL), ...resizeSteps(SWERVE, swerve(2, 0.6, [FOUR], [SPEED]))]
  }
});

// The arc radius is 24.75 times the ratio in the default 660x330 container.
const NEGATIVE_RATIOS = [
  -0.5, -1, -1 / 3, -1e-7, -1e7, -1e300, -3e-8, -4e-8, -4.1e-8, -5e-9, -1e-30, -1e-46, -1e-47, -5e-324, -4000.123, -40400, -40404.04, -40404.1, -2e9, -4e20,
  -4.0404040404040403e18, -1.25e19, -1.37e37, -1.3748e37, -1.375e37, -1.4e37, -4.1e37, -7.123456789, -0.0505050505050505, -123.4565656565
];
swerveFixtures.push({
  id: "throws",
  fixture: {
    steps: [
      ...START(SWERVE),
      render(SWERVE, TYPICAL),
      ...NEGATIVE_RATIOS.flatMap((ratio) => {
        const c = swerve(0.5, ratio, [FOUR], [speeds("#00ff00", 0.5, 0.5, 0.5), SPEED]);
        return [render(SWERVE, c), render(SWERVE, c)];
      }),
      size("333.3px", "222.7px"),
      render(SWERVE, swerve(0.5, -0.7, [], [speeds("#00ff00", 0, 0, -3)])),
      render(SWERVE, swerve(NaN, -0.7, [], [speeds("#00ff00", 0, 0, -3)])),
      render(SWERVE, swerve(0.5, -Infinity, [], [speeds("#00ff00", 0, 0, -3)])),
      render(SWERVE, null),
      render(SWERVE, null),
      render(SWERVE, TYPICAL),
      render(SWERVE, { rotation: 0, frameAspectRatio: 1, moduleVelocities: [] }),
      render(SWERVE, { rotation: 0, frameAspectRatio: 1, robotVelocities: [] }),
      render(SWERVE, swerve(0, 1, [{ color: "#ff0000" }])),
      render(SWERVE, swerve(0, 1, [{ values: [undefined, null], color: "#ff0000" }])),
      render(SWERVE, swerve(0, 1, [null])),
      render(SWERVE, swerve(0, 1, [], [{ color: "#ff0000" }])),
      render(SWERVE, swerve(0, 1, [], [null])),
      render(SWERVE, TYPICAL)
    ]
  }
});

swerveFixtures.push({
  id: "hidden",
  fixture: {
    steps: [
      ...START(SWERVE),
      render(SWERVE, TYPICAL),
      { main: "new-tab", data: SWERVE },
      { advance: 50 },
      render(SWERVE, TYPICAL, 0),
      render(SWERVE, TYPICAL, 1),
      { eval: "window.tabs.setSelected(1)" },
      { advance: 50 },
      render(SWERVE, TYPICAL, 0),
      render(SWERVE, TYPICAL, 1)
    ]
  }
});

const sel = "window.tabs.tabList[window.tabs.selectedTab].controller";
const source = (type, logKey, logType, options) => ({ type, logKey, logType, visible: true, options });
const MODULES = source("moduleVelocities", "/Drive/Velocities", "SwerveModuleVelocity[]", { color: "#ff0000", arrangement: "0,1,2,3" });
const ROBOT = source("robotVelocities", "/Drive/ChassisSpeeds", "ChassisSpeeds", { color: "#00ff00" });
const ROTATION = source("rotation", "/Drive/Rotation", "Rotation2d", {});
const restore = (sources, extra) => ({ eval: `${sel}.restoreState(${JSON.stringify({ sources, maxSpeed: 5, sizeX: 0.65, sizeY: 0.65, orientation: 1, ...extra })})` });

swerveFixtures.push({
  id: "controller",
  inputs: ["../logs/c05.json"],
  fixture: {
    steps: [
      { log: "../logs/c05.json" },
      ...START(SWERVE),
      { eval: `${sel}.sourceList.addField("/Drive/Velocities")` },
      { eval: `${sel}.sourceList.addField("/Drive/ChassisSpeeds")` },
      { eval: `${sel}.sourceList.addField("/Drive/Rotation")` },
      { eval: "window.selection.setSelectedTime(1)" },
      { advance: 50 },
      restore([MODULES, ROBOT, ROTATION], { maxSpeed: 2, sizeX: 0.5, sizeY: 0.9, orientation: 3 }),
      { advance: 50 },
      restore([MODULES, { ...MODULES, options: { color: "#0000ff", arrangement: "3,2,1,0" } }, ROBOT], { sizeX: 0, orientation: 0 }),
      { advance: 50 },
      restore([ROTATION], { orientation: 2 }),
      { advance: 50 }
    ]
  }
});

// Joysticks

const button = (id, state) => ({ id, state });
const axis = (id, value) => ({ id, value });
const pov = (id, angle) => ({ id, angle });
const finger = (x, y, down = true) => ({ x, y, down });
const state = ({ mapping = "sdl", buttons = [], axes = [], povs = [], touchpads = [] } = {}) => ({ mapping, buttons, axes, povs, touchpads });
const stick = (layout, s) => ({ layout, state: state(s) });
const generic = (s) => stick("Generic Joystick", s);

const PATHS = {
  wide: "/Users/pilot/Assets/Joystick_Wide/image.png",
  tall: "bundledAssets/Joystick_Tall/image.png",
  small: "C:\\Users\\pilot\\Assets\\Joystick_Small%20Pad\\image.png",
  broken: "bundledAssets/Joystick_Broken/image.png"
};
const IMAGES = { [PATHS.wide]: [1250, 800], [PATHS.tall]: [333, 1000], [PATHS.small]: [101, 67] };
const cButton = (centerPx, sizePx, sources, isEllipse = false, isYellow = false) => ({ type: "button", isYellow, isEllipse, centerPx, sizePx, ...sources });
const cStick = (centerPx, radiusPx, sources, isYellow = false) => ({ type: "joystick", isYellow, centerPx, radiusPx, ...sources });
const cAxis = (centerPx, sizePx, sources, isYellow = false) => ({ type: "axis", isYellow, centerPx, sizePx, ...sources });
const cPad = (centerPx, sizePx, sdlSourceIndex, isYellow = false) => ({ type: "touchpad", isYellow, centerPx, sizePx, sdlSourceIndex });

const WIDE = {
  name: "Wide",
  path: PATHS.wide,
  components: [
    cButton([100, 120], [40, 40], { sdlSourceIndex: 0, niSourceIndex: 1 }, true, true),
    cButton([160, 120], [44, 30], { sdlSourceIndex: 1, niSourceIndex: 2 }, false, false),
    cButton([160, 180], [44.5, 30.25], { niSourceIndex: 3 }, true, false),
    cButton([220, 180], [44, 30], { sdlSourceIndex: 2 }, false, true),
    cButton([300, 100], [30, 30], { sdlSourceIndex: 0, sdlSourcePov: "up", niSourceIndex: 0, niSourcePov: "up" }),
    cButton([330, 130], [30, 30], { sdlSourceIndex: 0, sdlSourcePov: "right", niSourceIndex: 0, niSourcePov: "right" }),
    cButton([300, 160], [30, 30], { sdlSourceIndex: 0, sdlSourcePov: "down", niSourceIndex: 0, niSourcePov: "down" }),
    cButton([270, 130], [30, 30], { sdlSourceIndex: 0, sdlSourcePov: "left", niSourceIndex: 1, niSourcePov: "left" }),
    cButton([400, 130], [30, 30], { sdlSourcePov: "left", niSourceIndex: 1 }),
    cStick([500, 400], 90, { sdlXSourceIndex: 0, sdlYSourceIndex: 1, sdlButtonSourceIndex: 1, niXSourceIndex: 1, niYSourceIndex: 0 }),
    cStick([760, 400], 90.5, {
      sdlXSourceIndex: 2, sdlXSourceInverted: true, sdlYSourceIndex: 3, sdlYSourceInverted: true, sdlButtonSourceIndex: 2,
      niXSourceIndex: 0, niXSourceInverted: true, niYSourceIndex: 1, niYSourceInverted: true, niButtonSourceIndex: 1
    }, true),
    cStick([900, 600], 33, { niXSourceIndex: 0, niXSourceInverted: false, niYSourceIndex: 1, niButtonSourceIndex: 2 }),
    cAxis([1000, 200], [40, 160], { sdlSourceIndex: 0, sdlSourceRange: [0, 1], niSourceIndex: 1 }),
    cAxis([1060, 200], [40, 160], { sdlSourceIndex: 1, niSourceIndex: 0, niSourceRange: [1, -1] }, true),
    cAxis([1120, 200], [40, 160], { sdlSourceIndex: 2, sdlSourceRange: [1, 1] }),
    cAxis([1180, 200], [40, 160], { niSourceIndex: 2 }),
    cPad([600, 650], [200, 100], 0),
    cPad([850, 650], [150.5, 100.25], 1, true),
    cPad([1000, 650], [10, 100], 0.5)
  ]
};
const TALL = {
  name: "Tall",
  path: PATHS.tall,
  components: [cButton([100, 100], [80, 80], { sdlSourceIndex: 0, niSourceIndex: 0 }), cAxis([200, 500], [30, 300], { sdlSourceIndex: 0, niSourceIndex: 0 }), cStick([160, 850], 70, { sdlXSourceIndex: 0, sdlYSourceIndex: 1 })]
};
const SMALL = { name: "Small \u00e9", path: PATHS.small, components: [cPad([50, 30], [40, 20], 0), cButton([10.5, 10.25], [3, 4], { sdlSourceIndex: 3 }, true)] };
const BROKEN = { name: "Broken", path: PATHS.broken, components: [cButton([100, 100], [80, 80], { sdlSourceIndex: 0 })] };
const EMPTY = { name: "Empty", path: PATHS.small, components: [] };
const DUPLICATE = { name: "Tall", path: PATHS.wide, components: [] };
const ASSETS = { field2ds: [], field3ds: [], robots: [], joysticks: [WIDE, TALL, SMALL, BROKEN, EMPTY, DUPLICATE], loadFailures: [] };
write(path.join(SPECS, "assets"), "r07.json", ASSETS);
const ASSET_FIXTURE = { assets: "../assets/r07.json", images: IMAGES };
const ASSET_INPUTS = ["../assets/r07.json"];

const FULL = {
  buttons: [button(0, true), button(1, false), button(2, true), button(3, false)],
  axes: [axis(0, 0.25), axis(1, -0.75), axis(2, 1), axis(3, -0.1)],
  povs: [pov(0, 45), pov(1, 270)],
  touchpads: [[finger(0.25, 0.75), finger(0.5, 0.5, false), finger(1, 0), finger(1.01, 0.5), finger(0.5, -0.01)], [finger(0, 1)]]
};
const BIG = {
  buttons: Array.from({ length: 17 }, (_, i) => button(i + 1, i % 3 === 0)),
  axes: Array.from({ length: 8 }, (_, i) => axis(i, -1 + i * 0.3)),
  povs: Array.from({ length: 7 }, (_, i) => pov(i, i * 45)),
  touchpads: Array.from({ length: 7 }, (_, i) => [finger(i / 6, 1 - i / 6)])
};

const joystickFixtures = [];

joystickFixtures.push({
  id: "generic",
  fixture: {
    steps: [
      ...START(JOY),
      render(JOY, []),
      render(JOY, [generic()]),
      render(JOY, [generic(FULL)]),
      render(JOY, [generic(BIG)]),
      render(JOY, [generic({ ...FULL, mapping: "ni" })]),
      render(JOY, [generic({ buttons: BIG.buttons })]),
      render(JOY, [generic({ buttons: [button(1, true)] })]),
      render(JOY, [generic({ axes: BIG.axes })]),
      render(JOY, [generic({ povs: BIG.povs })]),
      render(JOY, [generic({ touchpads: BIG.touchpads })]),
      render(JOY, [generic({ touchpads: [[]] })]),
      render(JOY, [generic({ touchpads: [[finger(0.5, 0.5)], [finger(0.1, 0.2), finger(0.9, 0.8)]] })]),
      render(JOY, [generic({ povs: [pov(0, 0)] })]),
      ...[-1, 0, 1, 45, 90, 135, 180, 225, 270, 315, 360, NaN, -0].map((a) => render(JOY, [generic({ povs: [pov(3, a)] })])),
      ...[-2, -1, 0, -0, 0.5, 1, 1.5, NaN, Infinity, -Infinity, 1e-300, null, undefined].map((v) => render(JOY, [generic({ axes: [axis(0, v)] })])),
      ...[[NaN, 0.5], [0.5, NaN], [0, 0], [-0, -0], [1, 1], [1.0000000000000002, 0.5], [0.5, 1.0000000000000002], [-5e-324, 0.5], [0.5, -5e-324]].map(([x, y]) =>
        render(JOY, [generic({ touchpads: [[finger(x, y)]] })])
      ),
      ...[0, 10, 123, 4294967295].map((id) => render(JOY, [generic({ buttons: [button(id, true)], axes: [axis(id, 0)], povs: [pov(id, 90)] })])),
      render(JOY, [stick("Missing layout", FULL)])
    ]
  }
});

const COUNTS = [1, 2, 3, 4, 5, 6, 7];
joystickFixtures.push({
  id: "regions",
  fixture: {
    ...ASSET_FIXTURE,
    steps: [
      ...START(JOY),
      ...COUNTS.map((n) => render(JOY, Array.from({ length: n }, (_, i) => generic(i % 2 === 0 ? FULL : BIG)))),
      ...COUNTS.slice(0, 6).map((n) => render(JOY, Array.from({ length: n }, (_, i) => stick(["Wide", "Tall", "Generic Joystick", "Small \u00e9"][i % 4], FULL)))),
      ...COUNTS.slice(0, 6).map((n) => render(JOY, Array.from({ length: n }, (_, i) => stick(["Tall", "Wide"][i % 2], FULL)))),
      size("200px", "640px"),
      ...COUNTS.slice(0, 6).map((n) => render(JOY, Array.from({ length: n }, (_, i) => stick(["Wide", "Tall", "Generic Joystick"][i % 3], FULL))))
    ]
  },
  inputs: ASSET_INPUTS
});

joystickFixtures.push({
  id: "layouts",
  fixture: {
    ...ASSET_FIXTURE,
    steps: [
      ...START(JOY),
      ...["sdl", "ni"].flatMap((mapping) => [
        render(JOY, [stick("Wide", { ...FULL, mapping })]),
        render(JOY, [stick("Wide", { mapping })]),
        ...[-1, 0, 45, 90, 135, 180, 225, 270, 315, 360, 44.5].map((a) => render(JOY, [stick("Wide", { mapping, povs: [pov(0, a), pov(1, (a + 180) % 360)] })])),
        ...[-2, -1, -0, 0.3333333333333333, 1, 2, NaN, Infinity, null, undefined].map((v) =>
          render(JOY, [stick("Wide", { mapping, axes: [axis(0, v), axis(1, v), axis(2, v), axis(3, v)], buttons: [button(1, true), button(2, v === 1)] })])
        ),
        render(JOY, [stick("Wide", { mapping, buttons: [button(1, 1), button(1, false), button(2, ""), button(3, "x")] })]),
        render(JOY, [stick("Wide", { mapping, touchpads: BIG.touchpads })]),
        render(JOY, [stick("Wide", { mapping, touchpads: [null, undefined] })])
      ]),
      render(JOY, [stick("Tall", FULL)]),
      render(JOY, [stick("Small \u00e9", FULL)]),
      render(JOY, [stick("Empty", FULL)]),
      render(JOY, [stick("Broken", FULL)]),
      render(JOY, [stick("Broken", FULL), stick("Tall", FULL), generic(FULL)]),
      render(JOY, [stick("wide", FULL)])
    ]
  },
  inputs: ASSET_INPUTS
});

// A frame follows every image `load`: assigning `src` again when it already resolves to the same
// image fires one too, and `src` reads back as a URL that no asset path equals.
joystickFixtures.push({
  id: "gate",
  fixture: {
    ...ASSET_FIXTURE,
    steps: [
      ...START(JOY),
      render(JOY, [generic(FULL)]),
      render(JOY, [generic(FULL)]),
      render(JOY, [stick("Wide", FULL)]),
      render(JOY, [stick("Wide", FULL)]),
      { advance: 1 },
      render(JOY, [stick("Wide", FULL)]),
      render(JOY, [stick("Wide", FULL)]),
      { advance: 1 },
      render(JOY, [stick("Wide", FULL), stick("Wide", FULL), stick("Broken", FULL)]),
      render(JOY, [stick("Broken", FULL)]),
      { advance: 1 },
      render(JOY, [stick("Broken", FULL)]),
      render(JOY, [generic(FULL)]),
      render(JOY, [generic({ ...FULL, axes: [axis(0, -0)] })]),
      render(JOY, [generic({ ...FULL, axes: [axis(0, 0)] })]),
      render(JOY, [generic({ ...FULL, axes: [axis(0, NaN)] })]),
      render(JOY, [generic({ ...FULL, axes: [axis(0, null)] })]),
      theme("dark"),
      render(JOY, [generic(FULL)]),
      render(JOY, [generic(FULL)]),
      theme("light"),
      render(JOY, [generic(FULL)]),
      dpr(2),
      render(JOY, [generic(FULL)]),
      dpr(1.1),
      render(JOY, [generic(FULL)]),
      size("500px", "400px"),
      render(JOY, [generic(FULL)]),
      render(JOY, [stick("Tall", FULL)]),
      { advance: 1 },
      { main: "set-assets", data: { ...ASSETS, joysticks: [{ ...TALL, path: PATHS.wide }] } },
      render(JOY, [stick("Tall", FULL)]),
      render(JOY, [stick("Tall", FULL)]),
      { advance: 1 },
      { main: "set-assets", data: { ...ASSETS, joysticks: [] } },
      render(JOY, [stick("Tall", FULL)]),
      render(JOY, [stick("Tall", { buttons: [button(4, true)] })]),
      { advance: 100 }
    ]
  },
  inputs: ASSET_INPUTS
});

{
  const rand = mulberry32(1307);
  const r = () => rand() * 2.4 - 1.2;
  const n = (max) => Math.floor(rand() * (max + 1));
  const commands = [];
  for (let i = 0; i < 36; i++) {
    commands.push(
      Array.from({ length: n(6) }, () =>
        stick(["Wide", "Tall", "Small \u00e9", "Generic Joystick", "Generic Joystick"][n(4)], {
          mapping: rand() < 0.5 ? "sdl" : "ni",
          buttons: Array.from({ length: n(14) }, (_, k) => button(k, rand() < 0.5)),
          axes: Array.from({ length: n(8) }, (_, k) => axis(k, r())),
          povs: Array.from({ length: n(3) }, (_, k) => pov(k, rand() < 0.2 ? -1 : n(7) * 45)),
          touchpads: Array.from({ length: n(3) }, () => Array.from({ length: n(3) }, () => finger(rand() * 1.2 - 0.1, rand() * 1.2 - 0.1, rand() < 0.8)))
        })
      )
    );
  }
  joystickFixtures.push({
    id: "many",
    fixture: { ...ASSET_FIXTURE, steps: [...START(JOY), ...commands.map((c) => render(JOY, c)), theme("dark"), ...commands.slice(0, 8).map((c) => render(JOY, c))] },
    inputs: ASSET_INPUTS
  });
}

const PAIR = [stick("Wide", FULL), generic(FULL)];
joystickFixtures.push({ id: "resize", fixture: { ...ASSET_FIXTURE, steps: [...START(JOY), ...resizeSteps(JOY, PAIR), size("1250px", "790px"), render(JOY, [stick("Wide", FULL)])] }, inputs: ASSET_INPUTS });
joystickFixtures.push({
  id: "dpr15",
  fixture: { ...ASSET_FIXTURE, window: { width: 1017, height: 689, devicePixelRatio: 1.5 }, steps: [...START(JOY), render(JOY, PAIR), ...resizeSteps(JOY, PAIR)] },
  inputs: ASSET_INPUTS
});
joystickFixtures.push({
  id: "dpr2-dark",
  fixture: { ...ASSET_FIXTURE, window: { width: 900, height: 701, devicePixelRatio: 2, theme: "dark" }, steps: [...START(JOY), render(JOY, PAIR), ...resizeSteps(JOY, [generic(BIG), stick("Tall", FULL), stick("Wide", FULL)])] },
  inputs: ASSET_INPUTS
});

joystickFixtures.push({
  id: "throws",
  fixture: {
    ...ASSET_FIXTURE,
    steps: [
      ...START(JOY),
      render(JOY, [generic(FULL)]),
      render(JOY, Array.from({ length: 7 }, () => stick("Wide", FULL))),
      render(JOY, null),
      render(JOY, null),
      render(JOY, [null]),
      render(JOY, [{ layout: "Generic Joystick" }]),
      render(JOY, [{ layout: "Wide" }]),
      ...["buttons", "axes", "povs", "touchpads"].flatMap((key) => [
        render(JOY, [generic({ ...FULL, [key]: null })]),
        render(JOY, [stick("Wide", { ...FULL, [key]: null })]),
        render(JOY, [stick("Wide", { ...FULL, mapping: "ni", [key]: null })])
      ]),
      render(JOY, [generic({ buttons: [button(1, true), { state: true }, button(3, false)] })]),
      render(JOY, [generic({ axes: [{ value: 1 }] })]),
      render(JOY, [generic({ povs: [{ angle: 90 }] })]),
      render(JOY, [generic({ buttons: [null] })]),
      render(JOY, [generic({ touchpads: [null] })]),
      render(JOY, [generic({ touchpads: [[null]] })]),
      render(JOY, [stick("Wide", { ...FULL, buttons: [null] })]),
      render(JOY, [stick("Wide", { ...FULL, axes: [null] })]),
      render(JOY, [stick("Wide", { ...FULL, povs: [null] })]),
      render(JOY, [stick("Wide", { ...FULL, touchpads: [[null]] })]),
      render(JOY, [generic(FULL), stick("Wide", { ...FULL, povs: [null] }), generic(FULL)]),
      render(JOY, [generic(FULL)])
    ]
  },
  inputs: ASSET_INPUTS
});

joystickFixtures.push({
  id: "hidden",
  fixture: {
    ...ASSET_FIXTURE,
    steps: [
      ...START(JOY),
      render(JOY, PAIR),
      { main: "new-tab", data: JOY },
      { advance: 50 },
      render(JOY, PAIR, 0),
      render(JOY, PAIR, 1),
      { eval: "window.tabs.setSelected(1)" },
      { advance: 50 },
      render(JOY, PAIR, 0),
      render(JOY, PAIR, 1)
    ]
  },
  inputs: ASSET_INPUTS
});

const choose = (layouts) => ({ eval: `${sel}.restoreState(${JSON.stringify(layouts)})` });
joystickFixtures.push({
  id: "controller",
  fixture: {
    ...ASSET_FIXTURE,
    steps: [
      { log: "../logs/c05.json" },
      ...START(JOY),
      choose(["Generic Joystick", "Generic Joystick", "Generic Joystick", "Generic Joystick", "None", "Generic Joystick"]),
      { advance: 50 },
      { eval: "window.selection.setSelectedTime(1)" },
      { advance: 50 },
      choose(["Wide", "Tall", "None", "Wide", "None", "None"]),
      { advance: 100 },
      choose(["None", "None", "Broken", "None", "None", "None"]),
      { advance: 50 },
      { main: "set-assets", data: null },
      { advance: 50 },
      choose(["None", "None", "None", "None", "None", "None"]),
      { advance: 50 }
    ]
  },
  inputs: [...ASSET_INPUTS, "../logs/c05.json"]
});

// Hub-level cases for crates/asrs-hub/tests/dom_tabs.rs: the port's controllers feed its painters
// inside `Tabs`, and a painter throw ends the frame loop as upstream's does.
const hub = (id, type, body) =>
  write(path.join(SPECS, "tabs"), `${id}.entry.json`, {
    entry: "dom",
    inputs: body.inputs,
    fixture: { seed: 707, ...body.fixture, steps: [{ log: "../logs/c05.json" }, { main: "new-tab", data: type }, ...body.steps] }
  });
const drawn = (label, steps) => [...steps, { advance: 100 }, { capture: ["commands", "state"], label }];
hub("swerve-r07", SWERVE, {
  inputs: ["../logs/c05.json"],
  fixture: {},
  steps: [
    { eval: `/*r07:mirror*/(() => {\n  ${MIRROR}\n  mirror(window.tabs.tabList[window.tabs.selectedTab].renderer.CANVAS);\n})()` },
    ...drawn("drawn", [restore([MODULES, ROBOT], { sizeX: 0.5 }), { eval: "window.selection.setSelectedTime(1)" }]),
    ...drawn("threw", [restore([MODULES, ROBOT], { sizeX: -0.5 })]),
    ...drawn("stopped", [restore([ROBOT], { sizeX: 0.5 })])
  ]
});
hub("joysticks-r07", JOY, {
  inputs: ["../assets/r07.json", "../logs/c05.json"],
  fixture: ASSET_FIXTURE,
  steps: [
    ...drawn("drawn", [choose(["Wide", "Generic Joystick", "Tall", "Broken", "None", "None"]), { eval: "window.selection.setSelectedTime(1)" }]),
    ...drawn("generic", [choose(["Generic Joystick", "None", "None", "None", "None", "Generic Joystick"])])
  ]
});

for (const f of swerveFixtures) spec("swerve", f.id, f.fixture, f.inputs);
for (const f of joystickFixtures) spec("joysticks", f.id, f.fixture, f.inputs);
console.log(`r07: ${swerveFixtures.length} swerve, ${joystickFixtures.length} joysticks specs`);
