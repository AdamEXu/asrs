import "./shims";
import * as THREE from "three";
import { toTree, type Cve1 } from "./cve1";

// Ground truth for asrs-field3d's camera-controls port (camera-controls 2.9.0 and three 0.168
// from upstream's node_modules), driven through a fake DOM element exactly as upstream's
// Field3dRendererImpl drives it. One JSON line per scripted case:
//
//   {"kind":"controls","case","init":{camera,rect,config},"steps":[[op, result, events, state]...],"final":<camera matrices>}
//
// argv[2] is "mac" or "other": camera-controls reads `navigator.platform` once at module load to
// pick its wheel scale, so each platform is a separate process.

const platform = process.argv[2];
if (platform !== "mac" && platform !== "other") throw new Error("usage: genCameraControls mac|other");
Object.defineProperty(globalThis, "navigator", {
  value: { platform: platform === "mac" ? "MacIntel" : "Win32" },
  configurable: true,
  writable: true
});

class FakeDOMRect {
  constructor(public x = 0, public y = 0, public width = 0, public height = 0) {}
  get left() { return this.x; }
  get top() { return this.y; }
  get right() { return this.x + this.width; }
  get bottom() { return this.y + this.height; }
}
(globalThis as any).DOMRect = FakeDOMRect;

const ccModule: any = await import("camera-controls");
const CameraControls = ccModule.default?.default ?? ccModule.default;
CameraControls.install({ THREE });

// V8 gives the same NaN different bits depending on which tier ran the code, so NaNs are
// written as one canonical pattern to keep the vector bytes reproducible.
const NAN_BITS = /^[7f]ff[0-9a-f]{13}$/;
const canonicalNaN = (tree: unknown): unknown => {
  if (Array.isArray(tree)) return tree.map(canonicalNaN);
  if (tree !== null && typeof tree === "object") {
    const n = (tree as { n?: unknown }).n;
    if (typeof n === "string" && NAN_BITS.test(n) && !/^[7f]ff0{13}$/.test(n)) return { n: "7ff8000000000000" };
    return Object.fromEntries(Object.entries(tree).map(([k, v]) => [k, canonicalNaN(v)]));
  }
  return tree;
};
const out = (value: object) => process.stdout.write(JSON.stringify(canonicalNaN(value)) + "\n");

let seed = platform === "mac" ? 0x0c0c_0290 : 0x0c0c_0291;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const uniform = (lo: number, hi: number) => lo + (hi - lo) * rng();
const below = (n: number) => Math.floor(rng() * n);
const pick = <T>(items: readonly T[]): T => items[below(items.length)];
const chance = (p: number) => rng() < p;

class FakeTarget {
  listeners: Record<string, Function[]> = {};
  addEventListener(type: string, fn: Function) {
    const list = (this.listeners[type] ??= []);
    if (!list.includes(fn)) list.push(fn);
  }
  removeEventListener(type: string, fn: Function) {
    const list = this.listeners[type];
    if (list === undefined) return;
    const i = list.indexOf(fn);
    if (i !== -1) list.splice(i, 1);
  }
  fire(type: string, event: object) {
    for (const fn of [...(this.listeners[type] ?? [])]) fn(event);
  }
}

class FakeDocument extends FakeTarget {
  pointerLockElement = null;
  exitPointerLock() {}
}

class FakeElement extends FakeTarget {
  style: Record<string, string> = {};
  rect = [0, 0, 300, 150];
  constructor(public ownerDocument: FakeDocument) {
    super();
  }
  setAttribute() {}
  removeAttribute() {}
  requestPointerLock() {}
  getBoundingClientRect() {
    const [x, y, width, height] = this.rect;
    return new FakeDOMRect(x, y, width, height);
  }
}

const EVENT_TYPES = ["controlstart", "control", "controlend", "transitionstart", "update", "wake", "rest", "sleep"];

type Op = [string, ...unknown[]];

type Init = {
  camera: number[];
  rect: number[];
  config: [string, number | boolean][];
};

function vec(v: THREE.Vector3): number[] {
  return [v.x, v.y, v.z];
}

function sph(s: THREE.Spherical): number[] {
  return [s.radius, s.phi, s.theta];
}

function runCase(name: string, init: Init, ops: Op[]) {
  const doc = new FakeDocument();
  const element = new FakeElement(doc);
  element.rect = init.rect;
  const [fov, aspect, near, far, px, py, pz] = init.camera;
  const camera = new THREE.PerspectiveCamera(fov, aspect, near, far);
  camera.position.set(px, py, pz);
  const controls = new CameraControls(camera, element);
  const events: string[] = [];
  for (const type of EVENT_TYPES) controls.addEventListener(type, () => events.push(type));
  for (const [key, value] of init.config) (controls as any)[key] = value;

  const c = controls as any;
  const state = (): Cve1 =>
    toTree([
      sph(c._spherical),
      sph(c._sphericalEnd),
      vec(c._target),
      vec(c._targetEnd),
      [c._thetaVelocity.value, c._phiVelocity.value, c._radiusVelocity.value, c._zoomVelocity.value],
      vec(c._targetVelocity),
      c._state,
      c._enabled,
      c._isDragging,
      c._dragNeedsUpdate,
      c._needsUpdate,
      c._updatedLastTime,
      c._hasRested,
      [c._isUserControllingRotate, c._isUserControllingDolly, c._isUserControllingTruck],
      c._lastDollyDirection,
      c._changedDolly,
      c._lastDistance,
      [c._zoom, c._zoomEnd, c._lastZoom],
      [c._elementRect.x, c._elementRect.y, c._elementRect.width, c._elementRect.height],
      c._activePointers.map((p: any) => [p.pointerId, p.clientX, p.clientY, p.mouseButton]),
      (doc.listeners["pointermove"] ?? []).length > 0,
      [controls.distance, controls.azimuthAngle, controls.polarAngle, controls.active],
      vec(controls.getPosition(new THREE.Vector3())),
      vec(controls.getPosition(new THREE.Vector3(), false)),
      vec(controls.getTarget(new THREE.Vector3(), false)),
      vec(camera.position),
      camera.quaternion.toArray(),
      [camera.rotation.x, camera.rotation.y, camera.rotation.z],
      camera.matrix.toArray(),
      camera.matrixWorldNeedsUpdate,
      camera.zoom
    ]);

  const pointerEvent = (pointerId: number, buttons: number, clientX: number, clientY: number) => ({
    pointerType: "mouse",
    pointerId,
    buttons,
    clientX,
    clientY,
    movementX: 0,
    movementY: 0,
    cancelable: true,
    preventDefault() {}
  });

  const apply = (op: Op): unknown => {
    const [kind, ...args] = op as [string, ...any[]];
    switch (kind) {
      case "update":
        return controls.update(args[0]);
      case "rotate":
        return void controls.rotate(args[0], args[1], args[2]);
      case "rotateTo":
        return void controls.rotateTo(args[0], args[1], args[2]);
      case "dolly":
        return void controls.dolly(args[0], args[1]);
      case "dollyTo":
        return void controls.dollyTo(args[0], args[1]);
      case "dollyInFixed":
        return void controls.dollyInFixed(args[0], args[1]);
      case "truck":
        return void controls.truck(args[0], args[1], args[2]);
      case "elevate":
        return void controls.elevate(args[0], args[1]);
      case "moveTo":
        return void controls.moveTo(args[0], args[1], args[2], args[3]);
      case "setLookAt":
        return void controls.setLookAt(args[0], args[1], args[2], args[3], args[4], args[5], args[6]);
      case "setPosition":
        return void controls.setPosition(args[0], args[1], args[2], args[3]);
      case "setTarget":
        return void controls.setTarget(args[0], args[1], args[2], args[3]);
      case "normalizeRotations":
        return void controls.normalizeRotations();
      case "stop":
        return void controls.stop();
      case "cancel":
        return void controls.cancel();
      case "set":
        (controls as any)[args[0]] = args[1];
        return undefined;
      case "cameraFov":
        camera.fov = args[0];
        camera.updateProjectionMatrix();
        return undefined;
      case "cameraAspect":
        camera.aspect = args[0];
        camera.updateProjectionMatrix();
        return undefined;
      case "cameraPose":
        camera.position.set(args[0], args[1], args[2]);
        camera.quaternion.set(args[3], args[4], args[5], args[6]);
        return undefined;
      case "rect":
        element.rect = args;
        return undefined;
      case "keyRotate": {
        // Field3dRendererImpl.render's keyboard `rotate` helper.
        const [x, y] = args;
        const reference = new THREE.Object3D();
        reference.position.copy(camera.position);
        reference.rotation.copy(camera.rotation);
        reference.rotateY(x);
        if (
          controls.polarAngle + y > controls.minPolarAngle + 0.2 &&
          controls.polarAngle + y < controls.maxPolarAngle - 0.2
        ) {
          reference.rotateX(y);
        }
        reference.translateZ(-controls.distance);
        controls.setTarget(reference.position.x, reference.position.y, reference.position.z, false);
        controls.update(0);
        return undefined;
      }
      case "pointerdown":
        element.fire("pointerdown", pointerEvent(args[0], args[1], args[2], args[3]));
        return undefined;
      case "pointermove":
        doc.fire("pointermove", pointerEvent(args[0], args[1], args[2], args[3]));
        return undefined;
      case "pointerup":
        doc.fire("pointerup", pointerEvent(args[0], args[1], args[2], args[3]));
        return undefined;
      case "pointercancel":
        element.fire("pointercancel", pointerEvent(args[0], args[1], args[2], args[3]));
        return undefined;
      case "wheel":
        element.fire("wheel", {
          deltaY: args[0],
          deltaX: 0,
          deltaMode: args[1],
          clientX: 0,
          clientY: 0,
          preventDefault() {}
        });
        return undefined;
      default:
        throw new Error(`unknown op ${kind}`);
    }
  };

  const steps: Cve1[] = [[null, toTree(events.splice(0)), state()]];
  for (const op of ops) {
    const result = apply(op);
    steps.push([toTree(op), toTree(result === undefined ? null : result), toTree(events.splice(0)), state()]);
  }
  camera.updateMatrixWorld();
  out({
    kind: "controls",
    case: name,
    init: toTree(init),
    steps,
    final: toTree([camera.matrixWorld.toArray(), camera.matrixWorldInverse.toArray(), camera.projectionMatrix.toArray(), camera.projectionMatrixInverse.toArray()])
  });
}

// Upstream's configuration (Field3dRendererImpl constructor).
const UPSTREAM_CONFIG: [string, number | boolean][] = [
  ["minDistance", 1],
  ["maxDistance", Infinity],
  ["smoothTime", 0.1],
  ["draggingSmoothTime", 0.1],
  ["infinityDolly", true],
  ["dollySpeed", 0.25]
];
const UPSTREAM_CAMERA = [50, 2, 0.15, 1000, 0, 0, 0];
const FRAME_DTS = [1 / 60, 1 / 120, 1 / 144, 0.016, 0.0166, 0.033, 0];

const point = (spread: number) => [uniform(-spread, spread), uniform(-spread, spread), uniform(-spread, spread)];
const frameDt = () => (chance(0.8) ? pick(FRAME_DTS) : chance(0.5) ? uniform(0, 0.1) : pick([0.25, 0.5, 1, 1e-7]));

function frames(ops: Op[], count: number, dt?: number) {
  for (let i = 0; i < count; i++) ops.push(["update", dt ?? frameDt()]);
}

// Upstream's per-frame keyboard handling for one held key at frame time `dt`.
function keyFrame(ops: Op[], key: string, dt: number, distanceAtMin: boolean) {
  switch (key) {
    case "W":
      ops.push(distanceAtMin ? ["dollyInFixed", 5 * dt, false] : ["dolly", 5 * dt, false]);
      break;
    case "S":
      ops.push(["dolly", -5 * dt, false]);
      break;
    case "A":
      ops.push(["truck", -5 * dt, 0, false]);
      break;
    case "D":
      ops.push(["truck", 5 * dt, 0, false]);
      break;
    case "Q":
      ops.push(["elevate", -5 * dt, false]);
      break;
    case "E":
      ops.push(["elevate", 5 * dt, false]);
      break;
    case "I":
      ops.push(["keyRotate", 0, 2.5 * dt]);
      return;
    case "K":
      ops.push(["keyRotate", 0, -2.5 * dt]);
      return;
    case "J":
      ops.push(["keyRotate", 2.5 * dt, 0]);
      return;
    case "L":
      ops.push(["keyRotate", -2.5 * dt, 0]);
      return;
  }
  ops.push(["update", 0]);
}

function drag(ops: Op[], rect: number[]) {
  const id = pick([1, 1, 1, 2, 7]);
  const buttons = pick([1, 1, 1, 2, 2, 4, 3, 5, 6, 7]);
  let x = rect[0] + uniform(0, rect[2]);
  let y = rect[1] + uniform(0, rect[3]);
  if (chance(0.5)) {
    x = Math.round(x);
    y = Math.round(y);
  }
  ops.push(["pointerdown", id, buttons, x, y]);
  const moves = 1 + below(12);
  let held = buttons;
  for (let i = 0; i < moves; i++) {
    const step = chance(0.3) ? 1 : chance(0.5) ? 5 : 40;
    x += chance(0.7) ? Math.round(uniform(-step, step)) : uniform(-step, step);
    y += chance(0.7) ? Math.round(uniform(-step, step)) : uniform(-step, step);
    if (chance(0.08)) held = pick([1, 2, 4, 3, 0]);
    ops.push(["pointermove", chance(0.05) ? id + 1 : id, held, x, y]);
    if (chance(0.25)) ops.push(["pointermove", id, held, x + 1, y - 1]);
    if (chance(0.75)) ops.push(["update", frameDt()]);
  }
  const end = rng();
  if (end < 0.8) ops.push(["pointerup", chance(0.05) ? id + 1 : id, 0, x, y]);
  else if (end < 0.9) ops.push(["pointercancel", id, 0, x, y]);
  else if (end < 0.95) ops.push(["set", "enabled", false], ["set", "enabled", true]);
  frames(ops, 1 + below(6));
}

function wheel(ops: Op[]) {
  const count = 1 + below(8);
  for (let i = 0; i < count; i++) {
    const mode = chance(0.8) ? 0 : 1;
    const deltaY =
      mode === 1
        ? pick([1, -1, 3, -3, 0.5])
        : chance(0.5)
          ? pick([4, -4, 40, -40, 100, -100, 120, -120, 1, -1, 0, -0.5])
          : Math.round(uniform(-300, 300) * 4) / 4;
    ops.push(["wheel", deltaY, mode]);
    if (chance(0.7)) ops.push(["update", frameDt()]);
  }
  frames(ops, below(10));
}

function randomOp(ops: Op[], rect: number[]) {
  const r = rng();
  const tr = chance(0.5);
  if (r < 0.14) drag(ops, rect);
  else if (r < 0.26) wheel(ops);
  else if (r < 0.36) {
    const key = pick(["W", "S", "A", "D", "Q", "E", "I", "K", "J", "L"]);
    const count = 1 + below(10);
    for (let i = 0; i < count; i++) {
      const dt = pick(FRAME_DTS.slice(0, 6));
      keyFrame(ops, key, dt, chance(0.2));
      ops.push(["update", dt]);
    }
  } else if (r < 0.48) frames(ops, 1 + below(20));
  else if (r < 0.54) {
    const [px, py, pz] = point(chance(0.5) ? 20 : 3);
    const [tx, ty, tz] = chance(0.3) ? [0, 0.5, 0] : point(5);
    ops.push(["setLookAt", px, py, pz, tx, ty, tz, tr]);
  } else if (r < 0.58) ops.push(["rotate", uniform(-2, 2), uniform(-1, 1), tr]);
  else if (r < 0.6) ops.push(["rotateTo", uniform(-9, 9), uniform(-1, 4), tr]);
  else if (r < 0.63) ops.push(["dolly", uniform(-5, 5), tr]);
  else if (r < 0.65) ops.push(["dollyTo", pick([0, 0.5, 1, uniform(0, 30)]), tr]);
  else if (r < 0.68) ops.push(["dollyInFixed", uniform(-3, 3), tr]);
  else if (r < 0.71) ops.push(["truck", uniform(-3, 3), uniform(-3, 3), tr]);
  else if (r < 0.73) ops.push(["elevate", uniform(-3, 3), tr]);
  else if (r < 0.75) ops.push(["moveTo", ...point(10), tr]);
  else if (r < 0.77) ops.push(["setPosition", ...point(15), tr]);
  else if (r < 0.79) ops.push(["setTarget", ...point(5), tr]);
  else if (r < 0.8) ops.push(["normalizeRotations"]);
  else if (r < 0.81) ops.push(["stop"]);
  else if (r < 0.82) ops.push(["cancel"]);
  else if (r < 0.84) ops.push(["set", "enabled", chance(0.5)]);
  else if (r < 0.86) ops.push(["cameraFov", pick([10, 50, 75, 160, uniform(10, 160)])]);
  else if (r < 0.88) {
    const a = pick([2, 1100 / 650, uniform(0.2, 4)]);
    ops.push(["cameraAspect", a]);
  } else if (r < 0.9) {
    const q = new THREE.Quaternion(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
    ops.push(["cameraPose", ...point(8), ...q.toArray()]);
  } else if (r < 0.92) {
    const rectNew = [Math.round(uniform(0, 400)), Math.round(uniform(0, 300)), Math.round(uniform(100, 2000)), chance(0.8) ? Math.round(uniform(80, 1400)) : uniform(80, 1400)];
    ops.push(["rect", ...rectNew]);
    rect.splice(0, 4, ...rectNew);
  } else if (r < 0.95) {
    const key = pick(["minDistance", "maxDistance", "smoothTime", "draggingSmoothTime", "dollySpeed", "truckSpeed", "azimuthRotateSpeed", "polarRotateSpeed", "minPolarAngle", "maxPolarAngle", "minAzimuthAngle", "maxAzimuthAngle", "maxSpeed", "restThreshold", "boundaryFriction"]);
    const values: Record<string, () => number> = {
      minDistance: () => pick([1, 0.5, Number.EPSILON, uniform(0, 5)]),
      maxDistance: () => pick([Infinity, uniform(5, 40)]),
      smoothTime: () => pick([0.1, 0.25, 0, uniform(0, 1)]),
      draggingSmoothTime: () => pick([0.1, 0.125, uniform(0, 1)]),
      dollySpeed: () => pick([0.25, 1, uniform(0, 3)]),
      truckSpeed: () => pick([2, uniform(0, 4)]),
      azimuthRotateSpeed: () => pick([1, uniform(-2, 2)]),
      polarRotateSpeed: () => pick([1, uniform(-2, 2)]),
      minPolarAngle: () => pick([0, uniform(0, 1.5)]),
      maxPolarAngle: () => pick([Math.PI, uniform(1.5, Math.PI)]),
      minAzimuthAngle: () => pick([-Infinity, uniform(-4, 0)]),
      maxAzimuthAngle: () => pick([Infinity, uniform(0, 4)]),
      maxSpeed: () => pick([Infinity, uniform(0.5, 20)]),
      restThreshold: () => pick([0.01, uniform(0, 0.1)]),
      boundaryFriction: () => pick([0, uniform(0, 1)])
    };
    ops.push(["set", key, values[key]()]);
  } else if (r < 0.97) ops.push(["set", "infinityDolly", chance(0.5)]);
  else if (r < 0.98) ops.push(["set", "dollyDragInverted", chance(0.5)]);
  else {
    // Pointer traffic outside a drag: the document listeners are not attached.
    ops.push(["pointermove", 1, pick([0, 1]), uniform(0, 500), uniform(0, 500)]);
    ops.push(["pointerup", 1, 0, uniform(0, 500), uniform(0, 500)]);
  }
}

// Hand-written cases: upstream's startup and reset transitions, then each input in isolation.
{
  const rect = [0, 0, 1100, 650];
  const startup: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, true]];
  frames(startup, 90, 1 / 60);
  runCase("upstream startup orbit field", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, startup);

  const robot: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0], ["setLookAt", 2, 1, 1, 0, 0.5, 0, true]];
  frames(robot, 60, 1 / 120);
  runCase("upstream orbit robot transition", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, robot);

  const restored: Op[] = [["setLookAt", 3.25, 4.5, -7.125, 0.5, 0.25, 0.75, false], ["update", 1 / 60]];
  frames(restored, 5, 1 / 60);
  runCase("upstream restoreState", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, restored);

  const rotateDrag: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0], ["pointerdown", 1, 1, 500, 300]];
  for (let i = 1; i <= 20; i++) rotateDrag.push(["pointermove", 1, 1, 500 + 7 * i, 300 - 3 * i], ["update", 1 / 60]);
  rotateDrag.push(["pointerup", 1, 0, 640, 240]);
  frames(rotateDrag, 40, 1 / 60);
  runCase("left drag rotate", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, rotateDrag);

  const truckDrag: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0], ["pointerdown", 1, 2, 400, 400]];
  for (let i = 1; i <= 15; i++) truckDrag.push(["pointermove", 1, 2, 400 - 5 * i, 400 + 2 * i], ["update", 1 / 60]);
  truckDrag.push(["pointerup", 1, 0, 325, 430]);
  frames(truckDrag, 40, 1 / 60);
  runCase("right drag truck", { camera: UPSTREAM_CAMERA, rect: [220, 38, 880, 612], config: UPSTREAM_CONFIG }, truckDrag);

  const dollyDrag: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0], ["pointerdown", 1, 4, 400, 400]];
  for (let i = 1; i <= 30; i++) dollyDrag.push(["pointermove", 1, 4, 400, 400 + 9 * i], ["update", 1 / 60]);
  dollyDrag.push(["pointerup", 1, 0, 400, 670]);
  frames(dollyDrag, 40, 1 / 60);
  runCase("middle drag dolly through minDistance", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, dollyDrag);

  const wheelIn: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0]];
  for (let i = 0; i < 60; i++) wheelIn.push(["wheel", -40, 0], ["update", 1 / 60]);
  frames(wheelIn, 30, 1 / 60);
  runCase("wheel dolly in past minDistance", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, wheelIn);

  const wheelOut: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0]];
  for (let i = 0; i < 20; i++) wheelOut.push(["wheel", i % 3 === 0 ? 3 : 2.5, i % 5 === 0 ? 1 : 0]);
  frames(wheelOut, 40, 1 / 60);
  runCase("wheel dolly out bursts", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, wheelOut);

  const keys: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0]];
  for (const key of ["W", "S", "A", "D", "Q", "E", "I", "K", "J", "L"]) {
    for (let i = 0; i < 6; i++) {
      keyFrame(keys, key, 1 / 60, false);
      keys.push(["update", 1 / 60]);
    }
  }
  for (let i = 0; i < 40; i++) {
    keyFrame(keys, "W", 1 / 60, true);
    keys.push(["update", 1 / 60]);
  }
  runCase("keyboard navigation", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, keys);

  const polar: Op[] = [["setLookAt", 0, 20, 0.0001, 0, 0, 0, false], ["update", 0]];
  for (let i = 0; i < 20; i++) polar.push(["keyRotate", 0, 2.5 / 60], ["update", 1 / 60]);
  for (let i = 0; i < 20; i++) polar.push(["keyRotate", 0.3, -2.5 / 60], ["update", 1 / 60]);
  runCase("keyboard rotate near the poles", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, polar);

  const degenerate: Op[] = [
    ["setLookAt", 1, 2, 3, 1, 2, 3, false],
    ["update", 0],
    ["setLookAt", 0, 5, 0, 0, 0, 0, false],
    ["update", 0],
    ["setLookAt", 0, -5, 0, 0, 0, 0, true],
    ["update", 1 / 60],
    ["update", 0],
    ["setLookAt", 1e300, 1e300, -1e300, 0, 0, 0, false],
    ["update", 1 / 60],
    ["setLookAt", 0, 6, -12, 0, 0.5, 0, true],
    ["update", 1 / 60],
    ["update", 0],
    ["update", 0],
    ["setLookAt", -3, 1, 9, 2, 0, 0, true],
    ["stop"],
    ["update", 1 / 60],
    ["rotate", 50, 0, true],
    ["normalizeRotations"],
    ["update", 1 / 60],
    ["rotate", -50, 0, false],
    ["normalizeRotations"],
    ["update", 1 / 60]
  ];
  runCase("degenerate look-at and wrapping", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, degenerate);

  const disabled: Op[] = [
    ["setLookAt", 0, 6, -12, 0, 0.5, 0, false],
    ["update", 0],
    ["pointerdown", 1, 1, 100, 100],
    ["pointermove", 1, 1, 120, 90],
    ["set", "enabled", false],
    ["pointermove", 1, 1, 140, 80],
    ["wheel", 100, 0],
    ["pointerdown", 1, 1, 100, 100],
    ["update", 1 / 60],
    ["cameraPose", 1, 2, 3, 0, 0.7071067811865476, 0, 0.7071067811865476],
    ["update", 1 / 60],
    ["set", "enabled", true],
    ["pointerdown", 1, 3, 100, 100],
    ["pointerdown", 1, 1, 105, 100],
    ["pointermove", 1, 1, 110, 100],
    ["pointerup", 1, 0, 110, 100],
    ["pointerup", 1, 0, 110, 100],
    ["update", 1 / 60]
  ];
  runCase("disable during drag and fixed camera pose", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, disabled);

  // Reverse a transition mid-flight so the damped value carries past its new end value.
  const overshoot: Op[] = [["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0]];
  for (const dt of [1 / 60, 0.05, 0.1, 0.25]) {
    for (const f of [0.1, 0.2, 0.35, 0.5]) {
      overshoot.push(["setLookAt", 0, 6, -12, 0, 0.5, 0, false], ["update", 0]);
      overshoot.push(["moveTo", 8, 0.5, 3, true], ["update", 1 / 60], ["update", 1 / 60], ["update", 1 / 60]);
      overshoot.push(["moveTo", 8 * f, 0.5, 3 * f, true], ["update", dt], ["update", dt], ["update", dt]);
      overshoot.push(["rotate", 2, 0.5, true], ["update", 1 / 60], ["update", 1 / 60], ["update", 1 / 60]);
      overshoot.push(["rotate", -2 * (1 - f), -0.5 * (1 - f), true], ["update", dt], ["update", dt], ["update", dt]);
      overshoot.push(["dolly", -10, true], ["update", 1 / 60], ["update", 1 / 60]);
      overshoot.push(["dolly", 10 * (1 - f), true], ["update", dt], ["update", dt]);
      frames(overshoot, 10, 1 / 60);
    }
  }
  runCase("reversed transitions", { camera: UPSTREAM_CAMERA, rect, config: UPSTREAM_CONFIG }, overshoot);
}

// Library defaults with the camera away from the origin, as the constructor sees it.
{
  const ops: Op[] = [];
  frames(ops, 3, 1 / 60);
  ops.push(["wheel", 120, 0]);
  frames(ops, 30, 1 / 60);
  runCase("library defaults", { camera: [75, 16 / 9, 0.1, 100, 3, 4, 5], rect: [0, 0, 800, 450], config: [] }, ops);
}

// Seeded random scripts.
const RANDOM_CASES = platform === "mac" ? 110 : 30;
for (let n = 0; n < RANDOM_CASES; n++) {
  const rect = [Math.round(uniform(0, 300)), Math.round(uniform(0, 100)), Math.round(uniform(200, 2200)), chance(0.9) ? Math.round(uniform(150, 1300)) : uniform(150, 1300)];
  const camera = chance(0.7) ? UPSTREAM_CAMERA : [pick([50, 75, uniform(10, 160)]), uniform(0.3, 3), 0.15, 1000, ...(chance(0.5) ? [0, 0, 0] : point(10))];
  const config = chance(0.8) ? UPSTREAM_CONFIG : [];
  const ops: Op[] = [];
  if (chance(0.85)) ops.push(["setLookAt", 0, 6, -12, 0, 0.5, 0, chance(0.5)]);
  const target = platform === "mac" ? 20 + below(30) : 10 + below(15);
  const liveRect = [...rect];
  while (ops.length < target) randomOp(ops, liveRect);
  runCase(`random ${n}`, { camera, rect, config }, ops);
}
