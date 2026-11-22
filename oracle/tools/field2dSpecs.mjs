// Field2d DOM parity fixtures for `crates/asrs-tabs/tests/field_2d_dom.rs`.
// Mirrors `lineGraphSpecs.mjs`: shared logs plus a recorder installed as the
// first eval step. SourceList ops ride the existing `sourceLists` recorder
// (fixture key "sourceLists: true"); this file only records the Field2d
// controller and renderer.

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const LOG_DIR = path.join(ROOT, "parity/specs/dom/logs");
const SPEC_DIR = path.join(ROOT, "parity/specs/dom/field-2d");

function write(dir, name, data) {
  fs.writeFileSync(path.join(dir, name), JSON.stringify(data, null, 1) + "\n");
}

const hex = (bytes) => bytes.map((b) => b.toString(16).padStart(2, "0")).join("");
function f64(...values) {
  const buf = new ArrayBuffer(values.length * 8);
  const view = new DataView(buf);
  values.forEach((v, i) => view.setFloat64(i * 8, v, true));
  return hex(Array.from(new Uint8Array(buf)));
}
const pose2d = (x, y, r) => f64(x, y, r);
const pose3d = (x, y, z, qw, qx, qy, qz) => f64(x, y, z, qw, qx, qy, qz);
const trans2d = (x, y) => f64(x, y);
const moduleState = (speed, angle) => f64(speed, angle);

const bytev = (h) => ({ b: h });
const posev = (translation, rotation) => ({ o: [["translation", translation], ["rotation", rotation]] });

const SCHEMA_OPS = [
  ["putRaw", "/.schema/struct:Translation2d", 0, bytev("646f75626c6520783b646f75626c652079")],
  ["putRaw", "/.schema/struct:Rotation2d", 0, bytev("646f75626c652076616c7565")],
  ["putRaw", "/.schema/struct:Pose2d", 0, bytev("5472616e736c6174696f6e3264207472616e736c6174696f6e3b526f746174696f6e326420726f746174696f6e")],
  ["putRaw", "/.schema/struct:Transform2d", 0, bytev("5472616e736c6174696f6e3264207472616e736c6174696f6e3b526f746174696f6e326420726f746174696f6e")],
  ["putRaw", "/.schema/struct:Translation3d", 0, bytev("646f75626c6520783b646f75626c6520793b646f75626c65207a")],
  ["putRaw", "/.schema/struct:Quaternion", 0, bytev("646f75626c6520773b646f75626c6520783b646f75626c6520793b646f75626c65207a")],
  ["putRaw", "/.schema/struct:Rotation3d", 0, bytev("5175617465726e696f6e2071")],
  ["putRaw", "/.schema/struct:Pose3d", 0, bytev("5472616e736c6174696f6e3364207472616e736c6174696f6e3b526f746174696f6e336420726f746174696f6e")],
  ["putRaw", "/.schema/struct:Transform3d", 0, bytev("5472616e736c6174696f6e3364207472616e736c6174696f6e3b526f746174696f6e336420726f746174696f6e")],
  ["putRaw", "/.schema/struct:SwerveModuleState", 0, bytev("646f75626c652073706565643b526f746174696f6e326420616e676c65")],
  ["putRaw", "/.schema/struct:Trajectory", 0, bytev("506f736532645b5d207465737073")]
];

// `advance`-time DOM reads are recorded once per call so the replay can hand the
// port the same values; the port's `Field2dDom` snapshot is rebuilt per record.
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
    if (typeof v === "function") return { fn: v.name };
    if (v instanceof Uint8Array || v instanceof ArrayBuffer) {
      let bytes = v instanceof ArrayBuffer ? new Uint8Array(v) : new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
      return { b: Array.from(bytes).map((x) => x.toString(16).padStart(2, "0")).join("") };
    }
    if (v instanceof Date) return { d: bits(v.getTime()) };
    if (Array.isArray(v)) return v.map(tree);
    return { o: Object.keys(v).map((k) => [k, tree(v[k])]) };
  };
  const errorText = (e) => (e instanceof Error ? e.name + ": " + e.message : String(e));
  const dark = () => w.matchMedia("(prefers-color-scheme: dark)").matches;
  const prefs = () => {
    const p = w.preferences;
    if (p === null || p === undefined) return { u: null };
    return tree({ cs: p.coordinateSystem, ftc: p.prefersFTC, ts: p.timestamps });
  };
  const assetsT = () => (w.assets === undefined ? { u: null } : tree(w.assets));

  const frames = [];
  let selectionDepth = 0;
  const selection = w.selection;
  for (const name of ["getMode", "getSelectedTime", "getRenderTime", "getCurrentLiveTime", "getTimelineRange", "getTimelineIsMaxZoom", "getHoveredTime", "getGrabZoomRange"]) {
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

  const controllers = [];
  const renderers = [];
  w.__f2d = renderers;

  const hook = (c, r) => {
    if (controllers.includes(c)) return;
    const id = controllers.length;
    controllers.push(c);
    renderers.push({ index: id, controller: c, renderer: r });
    write({ f2d: "create", id: tree(id), uuid: tree(c.sourceList.UUID), dark: tree(dark()), prefs: prefs(), assets: assetsT() });
    hookRenderer(r, id);

    const base = () => ({ id: tree(id), dark: tree(dark()), prefs: prefs(), assets: assetsT() });

    const observe = (method, kind) => {
      const original = c[method];
      c[method] = function (...args) {
        frames.push([]);
        const record = Object.assign({ f2d: kind }, base(), { uuid: tree(c.UUID) });
        try {
          const value = original.apply(this, args);
          record.value = tree(value);
          return value;
        } catch (e) {
          record.threw = tree(errorText(e));
          throw e;
        } finally {
          record.reads = frames.pop();
          write(record);
        }
      };
    };
    observe("getCommand", "command");
    observe("saveState", "save");
    observe("getActiveFields", "activeFields");

    for (const method of ["restoreState", "refresh", "newAssets"]) {
      const original = c[method];
      c[method] = function (...args) {
        write(Object.assign({ f2d: method, phase: "begin", args: tree(args) }, base()));
        const end = Object.assign({ f2d: method, phase: "end" }, base());
        try {
          return original.apply(this, args);
        } catch (e) {
          end.threw = tree(errorText(e));
          throw e;
        } finally {
          write(end);
        }
      };
    }

    const controlsSnapshot = () => ({
      value: c.FIELD_SELECT.value,
      frc: Array.from(c.FIELD_SELECT.firstElementChild.children).map((o) => [o.value, o.innerText]),
      ftc: Array.from(c.FIELD_SELECT.lastElementChild.children).map((o) => [o.value, o.innerText]),
      srcHidden: c.FIELD_SOURCE.hidden,
      isFtc: c.SETTINGS.classList.contains("ftc"),
      size: Array.from(c.SIZE_SWITCHER.children).findIndex((el) => el.classList.contains("selected"))
    });
    const queueControls = () => {
      w.setTimeout(() => {
        write(Object.assign({ f2d: "controls", controls: tree(controlsSnapshot()) }, base()));
      }, 0);
    };
    c.FIELD_SELECT.addEventListener("change", () => {
      write(Object.assign({ f2d: "select", value: tree(c.FIELD_SELECT.value) }, base()));
      queueControls();
    }, true);
    c.FIELD_SOURCE.addEventListener("click", () => {
      write(Object.assign({ f2d: "source" }, base()));
      queueControls();
    }, true);
    Array.from(c.ORIENTATION_SWITCHER.children).forEach((el, index) => {
      el.addEventListener("click", () => {
        write(Object.assign({ f2d: "orientation", index: tree(index) }, base()));
        queueControls();
      }, true);
    });
    Array.from(c.SIZE_SWITCHER.children).forEach((el, index) => {
      el.addEventListener("click", () => {
        write(Object.assign({ f2d: "size", index: tree(index) }, base()));
        queueControls();
      }, true);
    });
  };

  const camera = (r) =>
    tree({
      zoom: r.zoom,
      pan: r.pan,
      trackingMode: r.trackingMode,
      lastTrackingMode: r.lastTrackingMode,
      jumpToRobot: r.jumpToRobot,
      lastFieldCenter: r.lastFieldCenter,
      isFTC: r.isFTC,
      aspectRatio: r.aspectRatio,
      lastImageSource: r.lastImageSource
    });
  // The sensor's drag pan calls the callback its constructor captured, which the
  // recorder cannot wrap, so fixtures snapshot the camera after dispatching mouse events.
  w.__f2dCamera = (r) => {
    const entry = renderers.find((e) => e.renderer === r);
    write({ f2dr: "camera", id: tree(entry.index), camera: camera(r) });
  };

  const hookRenderer = (r, id) => {
    write({ f2dr: "create", id: tree(id) });
    const base = () => ({ id: tree(id), assets: assetsT() });
    const domSnapshot = () => {
      const scroll = r.SCROLL_CONTAINER.getBoundingClientRect();
      return {
        cw: r.CONTAINER.clientWidth,
        ch: r.CONTAINER.clientHeight,
        scroll: [scroll.x, scroll.y, scroll.width, scroll.height],
        dpr: w.devicePixelRatio,
        satellite: document.body.classList.contains("satellite")
      };
    };

    // The CANVAS style strings render assigns, as written (Blink re-serializes lengths on read).
    const styleWrites = {};
    // Blink serves these names from a named-property interceptor, so the shadowing accessors
    // forward to the equivalent setProperty / getPropertyValue.
    for (const name of ["width", "height", "transform"]) {
      Object.defineProperty(r.CANVAS.style, name, {
        configurable: true,
        get() {
          return this.getPropertyValue(name);
        },
        set(value) {
          styleWrites[name] = value;
          this.setProperty(name, value);
        }
      });
    }
    // The heatmap canvas is named by the recording canvas's creation counter; the trace
    // calls it "@heatmap", the id the port draws it under.
    const traceCalls = (ctx) => {
      const heatmapCanvas = r.heatmap.getCanvas();
      const heatmapId = heatmapCanvas === null ? null : ctx.argValue(heatmapCanvas);
      return ctx.calls.map((call) => (call[0] === "drawImage" && call[1] === heatmapId ? ["drawImage", "@heatmap", ...call.slice(2)] : call));
    };

    const originalRender = r.render;
    r.render = function (command) {
      const dom = domSnapshot();
      const before = r.CANVAS.__oracleContext?.frames;
      for (const name of Object.keys(styleWrites)) delete styleWrites[name];
      const record = Object.assign({ f2dr: "render", command: tree(command) }, base());
      try {
        const value = originalRender.apply(this, arguments);
        record.value = tree(value);
        return value;
      } catch (e) {
        record.threw = tree(errorText(e));
        throw e;
      } finally {
        // Image src/load state changes inside render; snapshot after the call.
        const src = r.IMAGE.getAttribute("src") ?? "";
        dom.images = { [src]: [r.IMAGE.naturalWidth, r.IMAGE.naturalHeight] };
        dom.loads = r.imageLoadCount;
        record.dom = tree(dom);
        record.camera = camera(r);
        const ctx = r.CANVAS.__oracleContext;
        if (ctx !== undefined && ctx.frames !== before) {
          record.size = tree([r.CANVAS.width, r.CANVAS.height]);
          record.style = tree([styleWrites.width, styleWrites.height, styleWrites.transform]);
          record.calls = tree(traceCalls(ctx));
        }
        write(record);
      }
    };

    const originalSave = r.saveState;
    r.saveState = function (...args) {
      frames.push([]);
      const record = Object.assign({ f2dr: "save" }, base());
      try {
        const value = originalSave.apply(this, args);
        record.value = tree(value);
        return value;
      } catch (e) {
        record.threw = tree(errorText(e));
        throw e;
      } finally {
        frames.pop();
        write(record);
      }
    };

    const aspectDesc = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(r), "aspectRatio");
    if (aspectDesc && aspectDesc.get) {
      Object.defineProperty(r, "aspectRatio", {
        configurable: true,
        get() {
          const value = aspectDesc.get.call(this);
          write(Object.assign({ f2dr: "aspectRatio", value: tree(value) }, base()));
          return value;
        }
      });
    }

    for (const method of ["restoreState", "set2DCamera"]) {
      const original = r[method];
      r[method] = function (...args) {
        write(Object.assign({ f2dr: method, phase: "begin", args: tree(args) }, base()));
        const end = Object.assign({ f2dr: method, phase: "end" }, base());
        try {
          return original.apply(this, args);
        } catch (e) {
          end.threw = tree(errorText(e));
          throw e;
        } finally {
          write(end);
        }
      };
    }

    // ScrollSensor captured the renderer callback in its constructor; wrap the
    // sensor's stored callback so the replay sees each dispatch.
    const sensor = r.scrollSensor;
    if (sensor && sensor.callback) {
      const inner = sensor.callback;
      sensor.callback = (dx, dy, isPan, cursorX, cursorY) => {
        write(Object.assign({ f2dr: "scroll", args: tree([dx, dy, isPan, cursorX, cursorY]) }, base()));
        inner(dx, dy, isPan, cursorX, cursorY);
      };
    }

    for (const type of ["mousedown", "mouseup", "mousemove"]) {
      r.SCROLL_CONTAINER.addEventListener(type, (event) => {
        write(Object.assign({ f2dr: "mouse", type, button: tree(event.button), shiftKey: event.shiftKey, x: tree(event.clientX), y: tree(event.clientY) }, base()));
      }, true);
    }
    r.SCROLL_CONTAINER.addEventListener("scroll", () => {
      write(Object.assign({ f2dr: "scrollEvent" }, base()));
    }, true);
  };

  w.tabs.tabList.forEach((t) => {
    if (t.type === 2) hook(t.controller, t.renderer);
  });
  const addTab = w.tabs.addTab;
  w.tabs.addTab = function (...args) {
    const result = addTab.apply(this, args);
    this.tabList.forEach((t) => {
      if (t.type === 2) hook(t.controller, t.renderer);
    });
    return result;
  };
})()`;

const FIELD_FRC = {
  id: "FRC:FRC 2025 Reefscape",
  name: "FRC 2025 Reefscape",
  path: "fields/frc2025.png",
  sourceUrl: "https://example.com/frc2025",
  topLeft: [56, 23],
  bottomRight: [1440, 700],
  widthInches: 690.875,
  heightInches: 317.6875,
  isFTC: false,
  coordinateSystem: "wall-blue",
  useGrid: true
};
const FIELD_FRC2 = {
  id: "FRC:FRC 2024 Crescendo",
  name: "FRC 2024 Crescendo",
  path: "fields/frc2024.png",
  topLeft: [54, 21],
  bottomRight: [1438, 701],
  widthInches: 651.22,
  heightInches: 323.25,
  isFTC: false,
  coordinateSystem: "wall-alliance",
  useGrid: true
};
const FIELD_FTC = {
  id: "FTC:FTC Decode",
  name: "FTC Decode",
  path: "fields/ftc-decode.png",
  sourceUrl: "https://example.com/ftc",
  topLeft: [30, 30],
  bottomRight: [710, 710],
  widthInches: 141.0,
  heightInches: 141.0,
  isFTC: true,
  coordinateSystem: "center-rotated",
  useGrid: false
};
const assets = (field2ds) => ({ field2ds, field3ds: [], robots: [], joysticks: [], loadFailures: [] });
const ASSETS_ALL = assets([FIELD_FRC, FIELD_FRC2, FIELD_FTC]);
const ASSETS_MISSING = assets([FIELD_FRC2, FIELD_FTC]);
const ASSETS_NONE = assets([]);
const IMAGES_ALL = {
  "fields/frc2025.png": [1440, 720],
  "fields/frc2024.png": [1440, 720],
  "fields/ftc-decode.png": [720, 720]
};
const IMAGES_MISSING = { "fields/frc2024.png": [1440, 720], "fields/ftc-decode.png": [720, 720] };

const logs = {
  "field-2d-struct.json": [
    ...SCHEMA_OPS,
    ["putStruct", "/PoseStruct", 0.0, bytev(pose2d(1.5, 2.5, 0.25)), "Pose2d", false],
    ["putStruct", "/PoseStruct", 1.0, bytev(pose2d(2.5, 3.5, 0.5)), "Pose2d", false],
    ["putStruct", "/PoseArr", 0.0, bytev(pose2d(0.5, 0.5, 0.0) + pose2d(1.5, 1.5, 1.5707963267948966)), "Pose2d", true],
    ["putStruct", "/Pose3dS", 0.0, bytev(pose3d(1.0, 2.0, 0.5, 1.0, 0.0, 0.0, 0.0)), "Pose3d", false],
    ["putStruct", "/Pose3dArr", 0.0, bytev(pose3d(0.25, 0.5, 0.1, 1, 0, 0, 0) + pose3d(1.25, 1.5, 0.2, 1, 0, 0, 0)), "Pose3d", true],
    ["putStruct", "/Swerve", 0.0, bytev(moduleState(1.25, 0.0) + moduleState(-0.5, 1.5707963267948966) + moduleState(0.75, -0.5) + moduleState(0.0, 3.141592653589793)), "SwerveModuleState", true]
  ],
  "field-2d-legacy.json": [
    ...SCHEMA_OPS,
    ["putPose", "/PoseWP", 0.0, posev([1.0, 2.0], 0.1)],
    ["putPose", "/PoseWP", 1.0, posev([1.1, 2.1], 0.2)],
    ["putPose", "/GhostWP", 0.0, posev([3.0, 4.0], -0.3)],
    ["putTranslationArray", "/VisionWP", 0.0, [[0.5, 0.6], [1.5, 1.6], [2.5, 2.6]]],
    ["putNumber", "/RotOverride/value", 0.0, 1.25],
    ["putRaw", "/RawPose", 0.0, bytev(pose2d(2.0, 3.0, 0.75))],
    ["putRaw", "/RawPose", 1.0, bytev(pose2d(2.2, 3.2, 0.85))],
    ["setWpilibType", "/RawPose", "struct:Pose2d"]
  ],
  "field-2d-game.json": [
    ["putRaw", "/.schema/struct:Translation2d", 0, bytev("646f75626c6520783b646f75626c652079")],
    ["putStruct", "/GamePieces", 0.0, bytev(trans2d(0.25, 0.5) + trans2d(1.25, 1.5) + trans2d(2.25, 2.5)), "Translation2d", true],
    ["putNumber", "/DriverStation/AllianceStation", 0.0, 4],
    ["putNumber", "/DriverStation/AllianceStation", 1.0, 1]
  ],
  "field-2d-heatmap.json": [
    ...SCHEMA_OPS,
    ["putStruct", "/Heat", 0.0, bytev(pose2d(0.5, 0.5, 0.0) + pose2d(1.0, 1.0, 0.1)), "Pose2d", true],
    ["putStruct", "/Heat", 0.5, bytev(pose2d(1.5, 1.5, 0.2)), "Pose2d", true],
    ["putStruct", "/Heat", 1.0, bytev(pose2d(2.0, 2.0, 0.3) + pose2d(2.5, 2.5, 0.4) + pose2d(3.0, 3.0, 0.5)), "Pose2d", true],
    ["putStruct", "/Heat", 2.0, bytev(pose2d(4.0, 4.0, 0.6)), "Pose2d", true],
    ["putStruct", "/Heat", 3.0, bytev(pose2d(5.0, 5.0, 0.7) + pose2d(6.0, 6.0, 0.8)), "Pose2d", true],
    ["putNumber", "/DriverStation/Enabled", 0.0, 1],
    ["putNumber", "/DriverStation/Enabled", 4.0, 0],
    ["putNumber", "/DriverStation/Autonomous", 0.0, 0],
    ["putNumber", "/DriverStation/Autonomous", 1.5, 0]
  ],
  "field-2d-multi.json": [
    ...SCHEMA_OPS,
    ["putStruct", "/Multi", 0.0, bytev(pose2d(1.0, 1.0, 0.0) + pose2d(4.0, 2.0, 0.5)), "Pose2d", true],
    ["putStruct", "/Multi", 0.5, bytev(pose2d(1.2, 1.1, 0.1) + pose2d(4.1, 2.2, 0.6) + pose2d(6.0, 3.0, 1.0)), "Pose2d", true],
    ["putStruct", "/Multi", 1.0, bytev(pose2d(1.5, 1.3, 0.2) + pose2d(4.3, 2.5, 0.7)), "Pose2d", true],
    ["putStruct", "/Multi", 2.0, bytev(pose2d(2.0, 1.6, 0.3) + pose2d(4.6, 2.9, 0.8)), "Pose2d", true],
    ["putStruct", "/Multi", 3.0, bytev(pose2d(2.4, 1.9, 0.4) + pose2d(5.0, 3.2, 0.9)), "Pose2d", true],
    ["putStruct", "/Single", 0.0, bytev(pose2d(3.0, 2.0, 0.25)), "Pose2d", false],
    ["putStruct", "/Single", 2.0, bytev(pose2d(5.0, 3.0, 1.25)), "Pose2d", false],
    ["putStruct", "/Empty", 0.0, bytev(""), "Pose2d", true],
    ["putStruct", "/Rot", 0.0, bytev(f64(0.75)), "Rotation2d", false],
    ["putStruct", "/Rot", 1.5, bytev(f64(-1.25)), "Rotation2d", false],
    ["putStruct", "/Vis1", 0.0, bytev(trans2d(0.5, 0.25) + trans2d(1.5, 0.75)), "Translation2d", true],
    ["putStruct", "/Vis2", 0.0, bytev(pose2d(7.0, 3.5, 0.0)), "Pose2d", false],
    ["putStruct", "/SwerveA", 0.0, bytev(moduleState(2.5, 0.0) + moduleState(-1.0, 0.5) + moduleState(0.0005, 1.0) + moduleState(4.0, -0.5)), "SwerveModuleState", true],
    ["putStruct", "/SwerveB", 0.0, bytev(moduleState(0.25, 3.0) + moduleState(0.5, 2.0)), "SwerveModuleState", true],
    ["putNumber", "/DriverStation/AllianceStation", 0.0, 1],
    ["putNumber", "/DriverStation/AllianceStation", 1.75, 4],
    ["putNumber", "/DriverStation/Enabled", 0.0, 1]
  ],
  "field-2d-trail.json": [
    ...SCHEMA_OPS,
    ["putStruct", "/TrailPose", -2.5, bytev(pose2d(0.1, 0.0, 0.0)), "Pose2d", false],
    ["putStruct", "/TrailPose", -1.0, bytev(pose2d(0.5, 0.0, 0.0)), "Pose2d", false],
    ["putStruct", "/TrailPose", 0.0, bytev(pose2d(1.0, 0.0, 0.0)), "Pose2d", false],
    ["putStruct", "/TrailPose", 1.0, bytev(pose2d(1.5, 0.5, 0.1)), "Pose2d", false],
    ["putStruct", "/TrailPose", 2.0, bytev(pose2d(2.0, 1.0, 0.2)), "Pose2d", false],
    ["putStruct", "/TrailPose", 4.0, bytev(pose2d(3.0, 2.0, 0.3)), "Pose2d", false]
  ]
};

const DEFAULT_OPTIONS = {
  robot: { bumpers: "" },
  ghost: { color: "#00ff00" },
  vision: { color: "#00ff00", size: "normal" },
  swerveModuleVelocities: { color: "#ff0000", arrangement: "0,1,2,3" },
  rotationOverride: {},
  rotationOverrideLegacy: { units: "radians" },
  trajectory: { color: "#00ff00", size: "normal" },
  heatmap: { timeRange: "auto" },
  arrow: { position: "center" }
};
const item = (logKey, type, logType, options = {}, visible = true) => ({
  type,
  logKey,
  logType,
  visible,
  options: Object.assign({}, DEFAULT_OPTIONS[type], options)
});
const last = `window.__f2d[window.__f2d.length - 1]`;
const sl = (state) => `(() => { ${last}.controller.sourceList.setState(${JSON.stringify(state)}); })()`;
const click = (sel, index) => `(() => { const c = ${last}.controller; c.${sel}.children[${index}].click(); })()`;
const field = (value) => `(() => { const c = ${last}.controller; c.FIELD_SELECT.value = ${JSON.stringify(value)}; c.FIELD_SELECT.dispatchEvent(new Event("change")); })()`;
const selTime = (t) => `(() => { window.selection.setSelectedTime(${t}); })()`;
const rend = `(() => { const r = ${last}.renderer;`;
const listOp = (expr) => `(() => { const l = ${last}.controller.sourceList; ${expr} })()`;

const fixtures = [
  {
    id: "objects",
    seed: 7201,
    log: "field-2d-struct.json",
    steps: [
      { eval: sl([
        item("/PoseStruct", "robot", "Pose2d", { bumpers: "#ff0000" }),
        item("/PoseArr", "ghost", "Pose2d[]", { color: "#0000ff" }),
        item("/Pose3dS", "ghost", "Pose3d"),
        item("/PoseArr", "vision", "Pose2d[]", { color: "#ffff00", size: "bold" }),
        item("/PoseArr", "trajectory", "Pose2d[]", { color: "#00ffff", size: "normal" }),
        item("/PoseStruct", "arrow", "Pose2d", { position: "front" })
      ]) },
      { eval: selTime(0.0) },
      { frames: 2 },
      { eval: selTime(1.0) },
      { frames: 1 },
      { eval: field("FRC:FRC 2024 Crescendo") },
      { eval: `(() => { const c = ${last}.controller; c.ORIENTATION_SWITCHER.children[0].click(); c.SIZE_SWITCHER.children[1].click(); })()` },
      { frames: 1 },
      { eval: `${rend} r.set2DCamera(1); })()` },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; c.dispatchEvent(new MouseEvent("mousedown", { button: 2, clientX: 10, clientY: 20 })); c.dispatchEvent(new MouseEvent("mouseup", { button: 2, clientX: 10, clientY: 20 })); })()` },
      { frames: 1 },
      { capture: ["sourceLists"] }
    ]
  },
  {
    id: "orientations",
    seed: 7202,
    log: "field-2d-struct.json",
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d")]) },
      { eval: selTime(0.5) },
      { eval: click("ORIENTATION_SWITCHER", 0) },
      { frames: 1 },
      { eval: click("ORIENTATION_SWITCHER", 0) },
      { frames: 1 },
      { eval: click("ORIENTATION_SWITCHER", 0) },
      { frames: 1 },
      { eval: click("ORIENTATION_SWITCHER", 1) },
      { frames: 1 },
      { eval: click("SIZE_SWITCHER", 1) },
      { frames: 1 },
      { eval: click("SIZE_SWITCHER", 2) },
      { eval: click("SIZE_SWITCHER", 0) },
      { frames: 1 },
      { eval: `(() => { const c = window.__f2d[0].controller; c.ORIENTATION_SWITCHER.children[0].click(); c.ORIENTATION_SWITCHER.children[0].click(); })()` },
      { frames: 1 }
    ]
  },
  {
    id: "ftc",
    seed: 7203,
    log: "field-2d-struct.json",
    preferences: { prefersFTC: true },
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d")]) },
      { eval: selTime(0.5) },
      { frames: 2 },
      { eval: field("FRC:FRC 2025 Reefscape") },
      { frames: 1 },
      { eval: field("FTC:FTC Decode") },
      { frames: 1 },
      { eval: `${rend} r.set2DCamera(2); })()` },
      { frames: 1 }
    ]
  },
  {
    id: "coords",
    seed: 7204,
    log: "field-2d-game.json",
    steps: [
      { eval: sl([item("/GamePieces", "trajectory", "Translation2d[]")]) },
      { eval: selTime(0.5) },
      { frames: 1 },
      { eval: `(() => { window.preferences.coordinateSystem = "wall-alliance"; })()` },
      { eval: selTime(0.5) },
      { frames: 1 },
      { eval: selTime(1.5) },
      { frames: 1 },
      { eval: `(() => { window.preferences.coordinateSystem = "wall-blue"; })()` },
      { frames: 1 },
      { eval: `(() => { window.preferences.coordinateSystem = "center-rotated"; })()` },
      { frames: 1 },
      { eval: `(() => { window.preferences.coordinateSystem = "center-red"; })()` },
      { frames: 1 },
      { eval: `(() => { window.preferences.coordinateSystem = "automatic"; })()` },
      { eval: selTime(0.5) },
      { frames: 1 },
      { eval: selTime(1.5) },
      { frames: 1 }
    ]
  },
  {
    id: "restore",
    seed: 7205,
    log: "field-2d-struct.json",
    steps: [
      { eval: `(() => {
        window.tabs.restoreState({ selected: 0, tabs: [{
          type: 2, title: "Restored", controllerUUID: "ctrl-uuid-1",
          controller: {
            sources: [
              { type: "robot", logKey: "/PoseStruct", logType: "Pose2d", visible: true, options: { bumpers: "#ff0000" } },
              { type: "ghost", logKey: "/PoseArr", logType: "Pose2d[]", visible: false, options: { color: "#654321" } },
              { type: "vision", logKey: "/Pose3dS", logType: "Pose3d", visible: true, options: { color: "#ff8c00", size: "bold" } },
              { type: "swerveModuleVelocities", logKey: "/missing", logType: "SwerveModuleState[]", visible: true, options: { color: "#00ffff", arrangement: "1,0,3,2" } }
            ],
            field: "FRC:FRC 2024 Crescendo", orientation: 2, size: "medium"
          },
          renderer: { zoom: 1.75, pan: [10, -20], trackingMode: 1, isFTC: false },
          controlsHeight: 200
        }]});
      })()` },
      { eval: selTime(0.5) },
      { frames: 2 },
      { eval: `(() => {
        window.tabs.restoreState({ selected: 0, tabs: [{
          type: 2, title: "Bad", controllerUUID: "ctrl-uuid-2",
          controller: {
            sources: "not-an-array",
            field: 42, orientation: 99, size: "huge"
          },
          renderer: "bogus",
          controlsHeight: 0
        }]});
      })()` },
      { frames: 1 },
      { eval: `${rend} r.restoreState({ zoom: 0.5, pan: [-5, 15], trackingMode: 2, isFTC: true }); })()` },
      { eval: selTime(0.5) },
      { frames: 1 },
      { capture: ["sourceLists"] }
    ]
  },
  {
    id: "camera",
    seed: 7206,
    log: "field-2d-struct.json",
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d")]) },
      { eval: selTime(0.5) },
      { frames: 1 },
      { eval: `${rend} r.set2DCamera(1); })()` },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; c.scrollLeft = 500100; c.scrollTop = 499950; c.dispatchEvent(new Event("scroll")); })()` },
      { frames: 1 },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; c.dispatchEvent(new MouseEvent("mousedown", { button: 2, clientX: 50, clientY: 60 })); c.dispatchEvent(new MouseEvent("mouseup", { button: 2, clientX: 50, clientY: 60 })); })()` },
      { eval: `${rend} r.set2DCamera(2); })()` },
      { eval: `${rend} r.set2DCamera(0); })()` },
      { frames: 1 },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; c.scrollLeft = 500200; c.scrollTop = 500300; c.dispatchEvent(new Event("scroll")); })()` },
      { frames: 2 },
      { eval: `${rend} r.restoreState(r.saveState()); })()` },
      { frames: 1 }
    ]
  },
  {
    id: "assets",
    seed: 7207,
    log: "field-2d-struct.json",
    assets: ASSETS_ALL,
    images: IMAGES_MISSING,
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d")]) },
      { eval: selTime(0.5) },
      { frames: 1 },
      { main: "set-assets", data: ASSETS_MISSING },
      { eval: field("FRC:FRC 2024 Crescendo") },
      { frames: 1 },
      { main: "set-assets", data: ASSETS_NONE },
      { eval: `(() => { ${last}.controller.FIELD_SOURCE.click(); })()` },
      { frames: 1 },
      { main: "set-assets", data: ASSETS_ALL },
      { eval: `(() => { ${last}.controller.FIELD_SOURCE.click(); })()` },
      { frames: 1 }
    ]
  },
  {
    id: "live",
    seed: 7208,
    log: "field-2d-heatmap.json",
    steps: [
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "auto" }), item("/PoseStruct", "robot", "Pose2d")]) },
      { selection: ["setLiveConnected"] },
      { liveTime: 2.0 },
      { frames: 2 },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "enabled" })]) },
      { frames: 1 },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "teleop" })]) },
      { frames: 1 },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "teleop-no-endgame" })]) },
      { frames: 1 },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "visible" })]) },
      { frames: 1 },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "full" })]) },
      { selection: ["setLiveDisconnected"] },
      { frames: 1 }
    ]
  },
  {
    id: "playback",
    seed: 7209,
    log: "field-2d-struct.json",
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d"), item("/PoseArr", "ghost", "Pose2d[]")]) },
      { selection: ["setSelectedTime", 0.0] },
      { selection: ["setPlaybackSpeed", 2.0] },
      { selection: ["play"] },
      { frames: 3 },
      { selection: ["pause"] },
      { selection: ["setSelectedTime", 0.5] },
      { frames: 4 },
      { selection: ["play"] },
      { frames: 2 }
    ]
  },
  {
    id: "legacy",
    seed: 7210,
    log: "field-2d-legacy.json",
    steps: [
      { eval: sl([
        item("/PoseWP", "robot", "Pose2d"),
        item("/GhostWP", "ghost", "Pose2d"),
        item("/VisionWP", "vision", "Translation2d[]", { color: "#ff8c00" }),
        item("/RawPose", "arrow", "Raw", { position: "back" }),
        item("/RotOverride/value", "rotationOverrideLegacy", "Number", { units: "radians" })
      ]) },
      { eval: selTime(0.5) },
      { frames: 2 },
      { eval: sl([
        item("/PoseWP", "robot", "Pose2d"),
        item("/RotOverride/value", "rotationOverrideLegacy", "Number", { units: "degrees" })
      ]) },
      { frames: 1 },
      { eval: sl([
        item("/PoseWP", "robot", "Pose2d"),
        item("/PoseWP/rotation", "rotationOverride", "Rotation2d")
      ]) },
      { frames: 1 },
      { eval: sl([
        item("/PoseWP", "robot", "Pose2d"),
        item("/VisionWP", "vision", "Translation2d[]", { color: "#00ffff" })
      ]) },
      { frames: 1 }
    ]
  },
  {
    id: "trail",
    seed: 7211,
    log: "field-2d-trail.json",
    steps: [
      { eval: sl([item("/TrailPose", "robot", "Pose2d")]) },
      { eval: selTime(2.0) },
      { frames: 1 },
      { eval: selTime(4.0) },
      { frames: 1 },
      { eval: selTime(0.0) },
      { frames: 1 }
    ]
  },
  {
    id: "aliasing",
    seed: 7217,
    log: "field-2d-multi.json",
    steps: [
      { eval: sl([
        item("/Multi", "robot", "Pose2d[]"),
        item("/Rot", "rotationOverride", "Rotation2d"),
        item("/Vis1", "vision", "Translation2d[]", { color: "#ffff00" }),
        item("/Vis2", "vision", "Pose2d", { size: "bold" }),
        item("/SwerveA", "swerveModuleVelocities", "SwerveModuleState[]"),
        item("/SwerveB", "swerveModuleVelocities", "SwerveModuleState[]", { color: "#0000ff", arrangement: "1,0" }),
        item("/Multi", "ghost", "Pose2d[]", { color: "#ff00ff" }),
        item("/SwerveA", "swerveModuleVelocities", "SwerveModuleState[]", { arrangement: "3,2,1,0" }),
        item("/Vis1", "vision", "Translation2d[]"),
        item("/Single", "robot", "Pose2d", { bumpers: "#00ff00" }),
        item("/Rot", "rotationOverride", "Rotation2d")
      ]) },
      { frames: 1 },
      { eval: selTime(0.25) },
      { frames: 1 },
      { eval: selTime(1.0) },
      { frames: 1 },
      { eval: selTime(2.5) },
      { frames: 1 },
      { eval: `(() => { window.preferences.coordinateSystem = "wall-alliance"; })()` },
      { eval: selTime(1.0) },
      { frames: 1 },
      { eval: selTime(2.0) },
      { frames: 1 },
      { eval: field("FTC:FTC Decode") },
      { eval: `(() => { window.preferences.coordinateSystem = "automatic"; })()` },
      { frames: 1 }
    ]
  },
  {
    id: "pan",
    seed: 7218,
    log: "field-2d-multi.json",
    steps: [
      { eval: sl([item("/Single", "robot", "Pose2d"), item("/Empty", "robot", "Pose2d[]")]) },
      { eval: selTime(1.0) },
      { frames: 1 },
      { eval: `${rend} r.set2DCamera(1); })()` },
      { frames: 1 },
      { eval: sl([item("/Empty", "robot", "Pose2d[]"), item("/Single", "robot", "Pose2d")]) },
      { frames: 1 },
      { eval: `${rend} r.set2DCamera(2); })()` },
      { frames: 1 },
      { eval: `${rend} r.set2DCamera(0); })()` },
      { frames: 1 },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; const d = (e) => { c.dispatchEvent(e); window.__f2dCamera(r); }; d(new MouseEvent("mousedown", { button: 0, clientX: 400, clientY: 300 })); d(new MouseEvent("mousemove", { clientX: 420, clientY: 290 })); d(new MouseEvent("mousemove", { clientX: 470, clientY: 330 })); d(new MouseEvent("mouseup", { button: 0, clientX: 470, clientY: 330 })); })()` },
      { frames: 1 },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; const d = (e) => { c.dispatchEvent(e); window.__f2dCamera(r); }; d(new MouseEvent("mousemove", { clientX: 600, clientY: 250 })); c.scrollTop = c.scrollTop - 120; c.dispatchEvent(new Event("scroll")); })()` },
      { frames: 1 },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; const d = (e) => { c.dispatchEvent(e); window.__f2dCamera(r); }; d(new MouseEvent("mousedown", { button: 0, shiftKey: true, clientX: 300, clientY: 200 })); d(new MouseEvent("mousemove", { clientX: 320, clientY: 210 })); d(new MouseEvent("mousedown", { button: 0, clientX: 320, clientY: 210 })); d(new MouseEvent("mousemove", { clientX: 2000, clientY: 1500 })); d(new MouseEvent("mouseup", { button: 0, clientX: 2000, clientY: 1500 })); })()` },
      { frames: 1 },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; const d = (e) => { c.dispatchEvent(e); window.__f2dCamera(r); }; c.scrollTop = c.scrollTop + 5000; c.dispatchEvent(new Event("scroll")); })()` },
      { frames: 1 },
      { eval: `${rend} r.restoreState({ zoom: 3, pan: ["5", [2]], trackingMode: 0 }); })()` },
      { eval: `${rend} const c = r.SCROLL_CONTAINER; const d = (e) => { c.dispatchEvent(e); window.__f2dCamera(r); }; d(new MouseEvent("mousedown", { button: 0, clientX: 100, clientY: 100 })); d(new MouseEvent("mousemove", { clientX: 90, clientY: 95 })); d(new MouseEvent("mouseup", { button: 0, clientX: 90, clientY: 95 })); r.saveState(); })()` },
      { eval: `${rend} r.saveState(); })()` },
      { frames: 1 },
      { eval: `${rend} r.saveState(); })()` },
      { eval: `${rend} r.restoreState({ zoom: 2, pan: [{}, null], trackingMode: 0 }); r.saveState(); })()` },
      { frames: 1 },
      { eval: `${rend} r.saveState(); })()` },
      { eval: `${rend} r.restoreState({ zoom: 1.5, pan: ["x", -10], trackingMode: 0 }); })()` },
      { frames: 1 },
      { eval: field("FTC:FTC Decode") },
      { frames: 1 },
      { eval: `${rend} r.saveState(); })()` }
    ]
  },
  {
    id: "swerve",
    seed: 7212,
    log: "field-2d-struct.json",
    steps: [
      { eval: sl([
        item("/PoseStruct", "robot", "Pose2d"),
        item("/Swerve", "swerveModuleVelocities", "SwerveModuleState[]", { color: "#ff0000", arrangement: "1,0,3,2" })
      ]) },
      { eval: selTime(0.0) },
      { frames: 2 },
      { eval: sl([item("/Swerve", "swerveModuleVelocities", "SwerveModuleState[]"), item("/PoseStruct", "robot", "Pose2d")]) },
      { frames: 1 }
    ]
  },
  {
    id: "heatmap",
    seed: 7213,
    log: "field-2d-heatmap.json",
    steps: [
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "auto" })]) },
      { eval: `(() => { window.selection.setSelectedTime(2.0); window.selection.setTimelineRange([1.0, 3.0], false); })()` },
      { frames: 1 },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "visible" })]) },
      { frames: 1 },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "full" })]) },
      { frames: 1 },
      { eval: `(() => { window.selection.setTimelineRange([0.0, 0.0], false); })()` },
      { eval: sl([item("/Heat", "heatmap", "Pose2d[]", { timeRange: "visible" })]) },
      { frames: 1 }
    ]
  },
  {
    id: "noassets",
    seed: 7214,
    log: "field-2d-struct.json",
    assets: null,
    images: {},
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d")]) },
      { eval: selTime(0.5) },
      { frames: 1 },
      { eval: `(() => { ${last}.controller.FIELD_SOURCE.click(); })()` },
      { frames: 1 },
      { main: "set-assets", data: ASSETS_ALL },
      { frames: 1 }
    ]
  },
  {
    id: "listops",
    seed: 7215,
    log: "field-2d-struct.json",
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d")]) },
      { eval: selTime(0.5) },
      { frames: 1 },
      { eval: listOp(`l.setTitle("Poses Custom");`) },
      { eval: listOp(`l.clear();`) },
      { eval: sl([item("/PoseStruct", "robot", "Pose2d"), item("/PoseArr", "trajectory", "Pose2d[]", { color: "#00ff00", size: "bold" })]) },
      { eval: `(() => { const c = ${last}.controller; c.getActiveFields(); })()` },
      { eval: `(() => { const c = ${last}.controller; c.saveState(); })()` },
      { eval: listOp(`l.stop();`) },
      { eval: listOp(`l.refresh();`) },
      { frames: 2 },
      { eval: `(() => { const c = ${last}.controller; c.getActiveFields(); })()` },
      { capture: ["sourceLists"] }
    ]
  },
  {
    id: "dark",
    seed: 7216,
    log: "field-2d-struct.json",
    window: { theme: "dark" },
    steps: [
      { eval: sl([item("/PoseStruct", "robot", "Pose2d"), item("/PoseArr", "ghost", "Pose2d[]", { color: "#0000ff" })]) },
      { eval: selTime(0.5) },
      { frames: 2 }
    ]
  }
];

fs.mkdirSync(SPEC_DIR, { recursive: true });
for (const [name, data] of Object.entries(logs)) {
  write(LOG_DIR, name, data);
}
for (const f of fixtures) {
  const fixture = { sourceLists: true, seed: f.seed };
  if (f.window) fixture.window = f.window;
  if (f.preferences) fixture.preferences = f.preferences;
  fixture.assets = f.assets !== undefined ? f.assets : ASSETS_ALL;
  fixture.images = f.images !== undefined ? f.images : IMAGES_ALL;
  fixture.steps = [
    { eval: RECORDER },
    { main: "new-tab", data: 2 },
    { log: `../logs/${f.log}` },
    ...f.steps
  ];
  write(SPEC_DIR, `${f.id}.entry.json`, {
    entry: "dom",
    inputs: [`../logs/${f.log}`],
    fixture
  });
}
console.log(`field-2d: wrote ${fixtures.length} specs, ${Object.keys(logs).length} logs`);
