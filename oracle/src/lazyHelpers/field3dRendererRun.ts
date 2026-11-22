import fs from "node:fs";
import path from "node:path";
import * as THREE from "three";
import { Line2 } from "three/examples/jsm/lines/Line2.js";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import Field3dRenderer from "@upstream/shared/renderers/Field3dRenderer";
import { toTree } from "../cve1";
import { RecordingContext } from "../dom/recordingCanvas";
import { fakeContainer, installFakeDom } from "../heatmap/fakeDom";

// Runs upstream's real Field3dRenderer (and through it Field3dRendererImpl, CameraControls and
// the object managers) under a fake window and DOM, and returns the state after each op.
//
// WebGL: the view canvas hands three's WebGLRenderer a GL context that answers every call
// with a placeholder, so the constructor, setSize and setPixelRatio run unchanged; render() is
// replaced by what it does to the scene graph (scene.updateMatrixWorld(), then
// camera.updateMatrixWorld() for the parentless camera) plus a record of the drawing-buffer
// size, pixel ratio and background it would draw with.
//
// Time: performance.now() and `new Date()` read one fake clock that only "advance" moves;
// window.setTimeout callbacks run when the clock passes their due time.
//
// Geometry is a declared input ("env"): the canvas client size (also its bounding-rect width),
// the canvas parent's client size and the container's client height.
//
//   setup: {preferences, isBattery, platform, assets, env, now}
//   ops: ["env", partial env]; ["advance", ms]; ["render", command];
//     ["key", "down"|"up", code, {meta?, ctrl?, editable?}];
//     ["mouse", "down"|"up", button, x, y]; ["camera", index]; ["fov", fov];
//     ["restore", state]; ["assets", key]; ["preferences", prefs]; ["battery", bool];
//     ["load"] answers every pending worker request in request order (one that never answers
//     stays pending); ["loadTextures"] completes every TextureLoader load whose file exists
//
// Each state: {[throw], scene, camera, controls, dom, renders, messages, requests, textures,
// aspect, saved}.

declare const __UPSTREAM_DIR__: string;
const UPSTREAM_DIR: string = process.env.ORACLE_UPSTREAM_DIR ?? __UPSTREAM_DIR__;

export type Env = {
  width: number;
  height: number;
  parentWidth: number;
  parentHeight: number;
  containerHeight: number;
  dpr: number;
  dark: boolean;
  reducedMotion: boolean;
};

export type WorkerAnswers = { [script: string]: (payload: any) => unknown | null };

const realSetTimeout = globalThis.setTimeout;

let clock = 0;
let timers: { due: number; seq: number; fn: () => void }[] = [];
let timerSeq = 0;

function installClock(start: number) {
  clock = start;
  timers = [];
  Object.defineProperty(globalThis, "performance", {
    value: { now: () => clock, timeOrigin: 0 },
    configurable: true,
    writable: true
  });
  const RealDate = Date;
  class FakeDate extends RealDate {
    constructor(...args: any[]) {
      if (args.length === 0) super(clock);
      else super(...(args as []));
    }
    static now() {
      return clock;
    }
  }
  (globalThis as any).Date = FakeDate;
  (globalThis as any).setTimeout = (fn: () => void, ms = 0) => {
    timers.push({ due: clock + Math.max(0, ms), seq: timerSeq++, fn });
    return timerSeq;
  };
}

function advance(ms: number) {
  const target = clock + ms;
  for (;;) {
    const due = timers.filter((t) => t.due <= target).sort((a, b) => a.due - b.due || a.seq - b.seq);
    if (due.length === 0) break;
    const next = due[0];
    timers = timers.filter((t) => t !== next);
    clock = next.due;
    next.fn();
  }
  clock = target;
}

class FakeDOMRect {
  constructor(public x = 0, public y = 0, public width = 0, public height = 0) {}
  get left() {
    return this.x;
  }
  get top() {
    return this.y;
  }
  get right() {
    return this.x + this.width;
  }
  get bottom() {
    return this.y + this.height;
  }
}

class FakeClassList {
  private names: string[] = [];
  add(name: string) {
    if (!this.names.includes(name)) this.names.push(name);
  }
  remove(name: string) {
    this.names = this.names.filter((n) => n !== name);
  }
  contains(name: string) {
    return this.names.includes(name);
  }
  forEach(fn: (name: string) => void) {
    [...this.names].forEach(fn);
  }
}

class FakeEventTarget {
  listeners: Record<string, Function[]> = {};
  addEventListener(type: string, fn: Function) {
    const list = (this.listeners[type] ??= []);
    if (!list.includes(fn)) list.push(fn);
  }
  removeEventListener(type: string, fn: Function) {
    const list = this.listeners[type];
    if (list !== undefined) this.listeners[type] = list.filter((f) => f !== fn);
  }
  fire(type: string, event: object) {
    for (const fn of [...(this.listeners[type] ?? [])]) fn(event);
  }
}

class FakeHTMLElement extends FakeEventTarget {
  classList = new FakeClassList();
  style: Record<string, string> = {};
  id = "";
  hidden = false;
  innerHTML = "";
  parentElement: FakeHTMLElement | null = null;
  isContentEditable = false;
  ownerDocument: any = null;
  clientWidthValue = 0;
  clientHeightValue = 0;
  get clientWidth() {
    return this.clientWidthValue;
  }
  get clientHeight() {
    return this.clientHeightValue;
  }
  setAttribute() {}
  removeAttribute() {}
  requestPointerLock() {}
  getBoundingClientRect() {
    return new FakeDOMRect(0, 0, this.clientWidth, this.clientHeight);
  }
}
class FakeHTMLInputElement extends FakeHTMLElement {}
class FakeHTMLTextAreaElement extends FakeHTMLElement {}

function fakeGl(canvas: { width: number; height: number }) {
  const constants = new Map<string, number>();
  const names = new Map<number, string>();
  const constant = (name: string) => {
    let value = constants.get(name);
    if (value === undefined) {
      value = 0x10000 + constants.size;
      constants.set(name, value);
      names.set(value, name);
    }
    return value;
  };
  const extension: any = new Proxy({}, { get: (_t, prop) => (typeof prop === "string" ? constant(prop) : undefined) });
  const fns: Record<string, Function> = {
    getParameter(p: number) {
      switch (names.get(p)) {
        case "VERSION":
          return "WebGL 2.0 (oracle)";
        case "SHADING_LANGUAGE_VERSION":
          return "WebGL GLSL ES 3.00 (oracle)";
        case "VIEWPORT":
        case "SCISSOR_BOX":
          return new Int32Array([0, 0, canvas.width, canvas.height]);
        default:
          return 16;
      }
    },
    getExtension: () => extension,
    getSupportedExtensions: () => [],
    getShaderPrecisionFormat: () => ({ precision: 23, rangeMin: 127, rangeMax: 127 }),
    getContextAttributes: () => ({ alpha: true, antialias: true, depth: true, stencil: false, premultipliedAlpha: true, preserveDrawingBuffer: false }),
    isContextLost: () => false,
    checkFramebufferStatus: () => constant("FRAMEBUFFER_COMPLETE")
  };
  return new Proxy(
    {},
    {
      get(_target, prop) {
        if (typeof prop !== "string") return undefined;
        if (prop === "canvas") return canvas;
        if (prop === "drawingBufferWidth") return canvas.width;
        if (prop === "drawingBufferHeight") return canvas.height;
        if (prop === "then") return undefined;
        if (/^[A-Z][A-Z0-9_]*$/.test(prop)) return constant(prop);
        if (prop in fns) return fns[prop];
        if (prop.startsWith("create")) return () => ({});
        return () => undefined;
      }
    }
  );
}

class FakeCanvas extends FakeHTMLElement {
  readonly tagName = "CANVAS";
  private w = 300;
  private h = 150;
  className = "";
  private gl: unknown = null;
  readonly context2d = new RecordingContext(this as unknown as HTMLCanvasElement, { imageId: () => "canvas" }, (w, h) => ({
    width: w,
    height: h,
    data: new Uint8ClampedArray(Math.max(0, w * h * 4))
  }));
  replacedWith: FakeCanvas | null = null;
  get width() {
    return this.w;
  }
  set width(value: number) {
    this.w = value >>> 0;
    this.context2d.reset();
  }
  get height() {
    return this.h;
  }
  set height(value: number) {
    this.h = value >>> 0;
    this.context2d.reset();
  }
  getContext(kind: string) {
    if (kind === "webgl2" || kind === "webgl") return (this.gl ??= fakeGl(this));
    return this.context2d.proxy;
  }
  replaceWith(other: FakeCanvas) {
    this.replacedWith = other;
  }
  appendChild(child: unknown) {
    return child;
  }
}

function fnv1a64(text: string): string {
  let h = 0xcbf29ce484222325n;
  for (let i = 0; i < text.length; i++) {
    h ^= BigInt(text.charCodeAt(i));
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h.toString(16).padStart(16, "0");
}

function geometryDigest(g: THREE.BufferGeometry): string {
  const tree = [
    g.index === null ? null : toTree(g.index.array),
    Object.keys(g.attributes).map((name) => {
      const a = g.attributes[name] as THREE.BufferAttribute;
      return [name, toTree(a.itemSize), a.normalized, toTree(a.array)];
    }),
    g.groups.map((group) => [toTree(group.start), toTree(group.count), toTree(group.materialIndex ?? 0)])
  ];
  return fnv1a64(JSON.stringify(tree));
}

const arrayDigest = (array: ArrayLike<number>) => fnv1a64(JSON.stringify(toTree(array)));

function textureOf(texture: THREE.Texture | null) {
  if (texture === null) return null;
  const source = texture.image instanceof FakeCanvas ? ["canvas"] : ["url", texture.name];
  return [...source, texture.minFilter, texture.magFilter, texture.flipY, texture.generateMipmaps];
}

function materialOf(m: THREE.Material) {
  if (m instanceof LineMaterial) {
    return [m.type, m.color.toArray(), m.linewidth, m.opacity, m.transparent, m.depthTest, m.depthWrite, m.visible];
  }
  if (m instanceof THREE.LineBasicMaterial) {
    return [m.type, m.color.toArray(), m.vertexColors, m.opacity, m.transparent, m.visible];
  }
  const p = m as THREE.MeshPhongMaterial;
  return [
    p.type,
    p.color.toArray(),
    p.specular.toArray(),
    p.shininess,
    p.emissive.toArray(),
    p.opacity,
    p.transparent,
    p.side,
    p.depthTest,
    p.depthWrite,
    p.vertexColors,
    p.visible,
    textureOf(p.map)
  ];
}

function kindOf(o: THREE.Object3D): string {
  if (o instanceof Line2) return "Line2";
  if ((o as any).isLineSegments) return "LineSegments";
  if ((o as any).isLine) return "Line";
  return o.type;
}

function extraOf(o: THREE.Object3D): unknown {
  if (o instanceof Line2) {
    const start = (o.geometry.attributes.instanceStart as THREE.InterleavedBufferAttribute | undefined)?.data.array;
    return [start === undefined ? null : arrayDigest(start), materialOf(o.material)];
  }
  if ((o as any).isLine) {
    const line = o as THREE.Line;
    return [geometryDigest(line.geometry), materialOf(line.material as THREE.Material)];
  }
  if ((o as any).isMesh) {
    const mesh = o as THREE.Mesh;
    const materials = Array.isArray(mesh.material) ? mesh.material.map(materialOf) : [materialOf(mesh.material)];
    const instances = o instanceof THREE.InstancedMesh ? [o.count, arrayDigest(o.instanceMatrix.array)] : null;
    return [geometryDigest(mesh.geometry), Array.isArray(mesh.material), materials, instances];
  }
  if (o instanceof THREE.HemisphereLight) return [o.color.toArray(), o.groundColor.toArray(), o.intensity];
  if (o instanceof THREE.SpotLight) {
    return [o.color.toArray(), o.intensity, o.distance, o.angle, o.penumbra, o.decay, o.castShadow, [o.shadow.mapSize.x, o.shadow.mapSize.y], o.shadow.bias];
  }
  if (o instanceof THREE.PointLight) return [o.color.toArray(), o.intensity, o.distance, o.decay];
  return null;
}

// A subtree already emitted earlier in the case is written as "#" + the FNV-1a 64 digest of its
// CVE-1 text, so unchanged parts of the scene are not repeated in every state.
function treeOf(o: THREE.Object3D, seen: Set<string>): unknown {
  const node = [
    ...[kindOf(o), o.visible, o.position.toArray(), o.quaternion.toArray(), o.scale.toArray(), o.renderOrder].map((v) => toTree(v)),
    ...[o.castShadow, o.receiveShadow, o.frustumCulled, extraOf(o)].map((v) => toTree(v)),
    o.children.map((child) => treeOf(child, seen))
  ];
  const digest = "#" + fnv1a64(JSON.stringify(node));
  if (seen.has(digest)) return digest;
  seen.add(digest);
  return node;
}

type Pending = { url: string; texture: THREE.Texture; onLoad?: (texture: THREE.Texture) => void };

export async function run(setup: any, ops: any[][], assetSets: { [key: string]: unknown }, answers: WorkerAnswers): Promise<unknown[]> {
  installFakeDom();
  installClock(setup.now);
  const g = globalThis as any;
  const doc = g.document;
  const fakeDocument = new FakeEventTarget() as any;
  fakeDocument.pointerLockElement = null;
  fakeDocument.exitPointerLock = () => {};
  const heatmapCreate = doc.createElement;
  doc.createElement = (tag: string) => {
    if (tag !== "canvas") return heatmapCreate(tag);
    const canvas = new FakeCanvas();
    canvas.ownerDocument = fakeDocument;
    return canvas;
  };
  doc.body = fakeContainer();
  doc.addEventListener = fakeDocument.addEventListener.bind(fakeDocument);
  doc.removeEventListener = fakeDocument.removeEventListener.bind(fakeDocument);
  g.DOMRect = FakeDOMRect;
  g.HTMLElement = FakeHTMLElement;
  g.HTMLInputElement = FakeHTMLInputElement;
  g.HTMLTextAreaElement = FakeHTMLTextAreaElement;

  const windowEvents = new FakeEventTarget();
  g.addEventListener = windowEvents.addEventListener.bind(windowEvents);
  g.removeEventListener = windowEvents.removeEventListener.bind(windowEvents);

  let env: Env = { ...setup.env };
  g.matchMedia = (query: string) => {
    if (query === "(prefers-color-scheme: dark)") return { matches: env.dark };
    if (query === "(prefers-reduced-motion: reduce)") return { matches: env.reducedMotion };
    throw new Error(`field3dRendererRun: unexpected media query ${query}`);
  };
  g.platform = setup.platform;
  g.preferences = setup.preferences;
  g.isBattery = setup.isBattery;
  g.assets = assetSets[setup.assets] ?? null;
  let messages: unknown[] = [];
  g.sendMainMessage = (name: string, data: unknown) => messages.push([name, structuredClone(data)]);
  Object.defineProperty(g, "devicePixelRatio", { get: () => env.dpr, configurable: true });

  let pendingTextures: Pending[] = [];
  let textures: string[] = [];
  THREE.TextureLoader.prototype.load = function (url: string, onLoad?: (texture: THREE.Texture) => void) {
    const texture = new THREE.Texture();
    pendingTextures.push({ url, texture, onLoad });
    textures.push(url);
    return texture;
  } as any;

  let requests: unknown[] = [];
  let pendingWork: { worker: FakeWorker; id: number; script: string; payload: any }[] = [];
  class FakeWorker {
    onmessage: ((event: unknown) => void) | null = null;
    constructor(readonly script: string) {
      if (!(script in answers)) throw new Error(`field3dRendererRun: unexpected worker ${script}`);
    }
    postMessage(message: { id: number; payload: any }) {
      const p = message.payload;
      const name = this.script === "../bundles/loadField.js" ? p.fieldConfig.id : p.robotConfig.name;
      requests.push([this.script, name, p.isFTC ?? null, p.mode, p.materialSpecular, p.materialShininess]);
      pendingWork.push({ worker: this, id: message.id, script: this.script, payload: p });
    }
    terminate() {}
  }
  const realWorker = g.Worker;
  g.Worker = FakeWorker;

  const root = new FakeHTMLElement();
  const container = new FakeHTMLElement();
  const canvas = new FakeCanvas();
  canvas.ownerDocument = fakeDocument;
  canvas.classList.add("field-3d-canvas");
  const parent = new FakeHTMLElement();
  const alert = new FakeHTMLElement();
  alert.hidden = true;
  const spinner = new FakeHTMLElement();
  const byClass: Record<string, FakeHTMLElement> = {
    "field-3d-canvas": canvas,
    "field-3d-canvas-container": container,
    "field-3d-alert": alert,
    "spinner-cubes-container": spinner
  };
  (root as any).getElementsByClassName = (name: string) => [byClass[name]];

  let view = canvas;
  const applyEnv = () => {
    view.clientWidthValue = env.width;
    view.clientHeightValue = env.height;
    view.parentElement = parent;
    parent.clientWidthValue = env.parentWidth;
    parent.clientHeightValue = env.parentHeight;
    container.clientHeightValue = env.containerHeight;
  };
  applyEnv();

  let renders: unknown[] = [];
  let hooked: any = null;
  const hookImplementation = (renderer: any) => {
    const impl = renderer.implementation;
    if (impl === null || impl === hooked) return;
    hooked = impl;
    while (view.replacedWith !== null) view = view.replacedWith;
    applyEnv();
    const gl = impl.renderer;
    gl.render = (scene: THREE.Scene, camera: THREE.Camera) => {
      if (scene.matrixWorldAutoUpdate === true) scene.updateMatrixWorld();
      if (camera.parent === null && camera.matrixWorldAutoUpdate === true) camera.updateMatrixWorld();
      const background = scene.background as THREE.Color | null;
      renders.push([view.width, view.height, gl.getPixelRatio(), background === null ? null : background.getHex()]);
    };
  };

  const renderer = new Field3dRenderer(root as unknown as HTMLElement);
  hookImplementation(renderer);

  const answerWork = async () => {
    const work = pendingWork;
    pendingWork = [];
    for (const item of work) {
      const result = answers[item.script](item.payload);
      if (result === null) {
        pendingWork.push(item);
        continue;
      }
      item.worker.onmessage!({ data: { id: item.id, payload: structuredClone(result) } });
    }
    await new Promise((resolve) => realSetTimeout(resolve, 0));
  };

  const loadTextures = () => {
    const loads = pendingTextures;
    pendingTextures = [];
    for (const load of loads) {
      const file = path.resolve(UPSTREAM_DIR, "www", load.url);
      if (!fs.existsSync(file)) continue;
      load.texture.name = load.url;
      load.onLoad?.(load.texture);
    }
  };

  const keyEvent = (code: string, opts: any) => {
    const target = opts.editable === "input" ? new FakeHTMLInputElement() : opts.editable === "textarea" ? new FakeHTMLTextAreaElement() : new FakeHTMLElement();
    if (opts.editable === "content") target.isContentEditable = true;
    return { code, metaKey: opts.meta === true, ctrlKey: opts.ctrl === true, target };
  };

  const seen = new Set<string>();
  const stateOf = (error: unknown) => {
    const impl = (renderer as any).implementation;
    const camera = impl?.camera as THREE.PerspectiveCamera | undefined;
    const controls = impl?.controls;
    const v3 = (v: THREE.Vector3) => [v.x, v.y, v.z];
    const state = {
      scene: impl ? impl.scene.children.map((child: THREE.Object3D) => treeOf(child, seen)) : null,
      camera: camera
        ? [v3(camera.position), camera.quaternion.toArray(), camera.fov, camera.aspect, camera.zoom, camera.projectionMatrix.toArray()]
        : null,
      controls: controls
        ? [
            controls.enabled,
            controls.minDistance,
            v3(controls.getPosition(new THREE.Vector3(), false)),
            v3(controls.getTarget(new THREE.Vector3(), false)),
            v3(controls.getPosition(new THREE.Vector3(), true)),
            v3(controls.getTarget(new THREE.Vector3(), true))
          ]
        : null,
      dom: [
        view.classList.contains("fixed"),
        container.classList.contains("fixed"),
        view.style.width ?? "",
        view.style.height ?? "",
        alert.hidden,
        alert.innerHTML,
        spinner.classList.contains("visible"),
        spinner.classList.contains("animating")
      ],
      renders,
      messages,
      requests,
      textures,
      aspect: renderer.getAspectRatio(),
      saved: renderer.saveState()
    };
    renders = [];
    messages = [];
    requests = [];
    textures = [];
    const pairs: unknown[] = Object.entries(state).map(([key, value]) => [key, key === "scene" && value !== null ? value : toTree(value)]);
    if (error !== undefined) pairs.unshift(["throw", toTree(error)]);
    return { o: pairs };
  };

  const states: unknown[] = [];
  for (const op of ops) {
    let error: unknown = undefined;
    try {
      switch (op[0]) {
        case "env":
          env = { ...env, ...op[1] };
          applyEnv();
          break;
        case "advance":
          advance(op[1]);
          break;
        case "render":
          (renderer as any).updateImplementation();
          hookImplementation(renderer);
          renderer.render(op[1]);
          break;
        case "key":
          windowEvents.fire(op[1] === "down" ? "keydown" : "keyup", keyEvent(op[2], op[3] ?? {}));
          break;
        case "mouse":
          view.fire(op[1] === "down" ? "mousedown" : "mouseup", { button: op[2], clientX: op[3], clientY: op[4] });
          break;
        case "camera":
          renderer.set3DCamera(op[1]);
          break;
        case "fov":
          renderer.setFov(op[1]);
          break;
        case "restore":
          renderer.restoreState(op[1]);
          break;
        case "assets":
          g.assets = op[1] === null ? null : assetSets[op[1]];
          break;
        case "preferences":
          g.preferences = op[1];
          break;
        case "battery":
          g.isBattery = op[1];
          break;
        case "load":
          await answerWork();
          break;
        case "loadTextures":
          loadTextures();
          break;
        default:
          throw new Error(`field3dRendererRun: unknown op ${JSON.stringify(op[0])}`);
      }
    } catch (e) {
      if (e instanceof Error && e.message.startsWith("field3dRendererRun:")) throw e;
      error = e instanceof Error ? `${e.name}: ${e.message}` : e;
    }
    states.push(stateOf(error));
  }
  g.Worker = realWorker;
  return states;
}
