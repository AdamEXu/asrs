import defaultLayout from "./layout.json";
import { installRealGeometry } from "./realGeometry";
import { fixedWidth, RecordingContext } from "./recordingCanvas";

export type Rect = [number, number, number, number];
export interface LayoutRule {
  selector: string;
  rect?: Rect;
  parent?: boolean;
  text?: number;
}

export interface Fixture {
  window?: { width?: number; height?: number; devicePixelRatio?: number; theme?: "light" | "dark"; framePeriod?: number };
  geometry?: "scripted" | "real";
  seed?: number;
  startTime?: number;
  version?: object;
  preferences?: object;
  assets?: unknown;
  typeMemory?: unknown;
  state?: unknown;
  boot?: [string, unknown][];
  images?: { [src: string]: [number, number] };
  layout?: LayoutRule[];
  ignoreMain?: string[];
  sourceLists?: boolean;
  steps?: unknown[];
}

const w = window as any;
export const fixture: Fixture = w.__oracleFixture;
const view = fixture.window ?? {};
export const framePeriod = view.framePeriod ?? 16;
const theme = view.theme ?? "light";

const realGeometry = fixture.geometry === "real";

// jsdom has no text codecs. Encoded bytes are copied into this realm so `instanceof Uint8Array`
// holds for upstream code.

const outer = w.__oracleOuter;
if (w.TextEncoder === undefined) w.TextEncoder = class TextEncoder {
  readonly encoding = "utf-8";
  encode(input = ""): Uint8Array {
    return new Uint8Array(new outer.TextEncoder().encode(input));
  }
};
if (w.TextDecoder === undefined) w.TextDecoder = class TextDecoder {
  private decoder: any;
  constructor(label?: string, options?: object) {
    this.decoder = new outer.TextDecoder(label, options);
  }
  get encoding() {
    return this.decoder.encoding;
  }
  get fatal() {
    return this.decoder.fatal;
  }
  get ignoreBOM() {
    return this.decoder.ignoreBOM;
  }
  decode(input?: ArrayBufferView | ArrayBuffer, options?: object): string {
    return this.decoder.decode(input, options);
  }
};

export const clock = { start: fixture.startTime ?? 1767225600000, now: fixture.startTime ?? 1767225600000 };
const RealDate = Date;
class OracleDate extends RealDate {
  constructor(...args: any[]) {
    if (args.length === 0) super(clock.now);
    else super(...(args as [any]));
  }
  static now() {
    return clock.now;
  }
}
w.Date = OracleDate;
Object.defineProperty(w.performance, "now", { value: () => clock.now - clock.start, configurable: true });

let randomState = (fixture.seed ?? 1) >>> 0;
function nextU32(): number {
  randomState = (randomState + 0x6d2b79f5) >>> 0;
  let t = randomState;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return (t ^ (t >>> 14)) >>> 0;
}
Math.random = () => nextU32() / 4294967296;
const randomBytes = <T extends ArrayBufferView>(array: T): T => {
  const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
  for (let i = 0; i < bytes.length; i++) bytes[i] = nextU32() & 0xff;
  return array;
};
Object.defineProperty(w, "crypto", {
  configurable: true,
  value: {
    getRandomValues: randomBytes,
    randomUUID() {
      const b = randomBytes(new Uint8Array(16));
      b[6] = (b[6] & 0x0f) | 0x40;
      b[8] = (b[8] & 0x3f) | 0x80;
      const hex = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    }
  }
});

interface Timer {
  id: number;
  due: number;
  seq: number;
  fn: Function;
  args: unknown[];
  interval: number | null;
}
let timers: Timer[] = [];
let nextTimerId = 1;
let seq = 0;
let frameCallbacks: [number, FrameRequestCallback][] = [];
let nextFrameId = 1;
export let lastFrame = clock.now;

function addTimer(fn: unknown, ms: unknown, args: unknown[], repeat: boolean): number {
  const delay = Math.max(0, Number(ms) || 0);
  const id = nextTimerId++;
  if (typeof fn !== "function") return id;
  timers.push({ id, due: clock.now + delay, seq: seq++, fn, args, interval: repeat ? Math.max(1, delay) : null });
  return id;
}
w.setTimeout = (fn: unknown, ms?: unknown, ...args: unknown[]) => addTimer(fn, ms, args, false);
w.setInterval = (fn: unknown, ms?: unknown, ...args: unknown[]) => addTimer(fn, ms, args, true);
w.clearTimeout = w.clearInterval = (id: number) => {
  timers = timers.filter((t) => t.id !== id);
};
w.requestAnimationFrame = (cb: FrameRequestCallback) => {
  const id = nextFrameId++;
  frameCallbacks.push([id, cb]);
  return id;
};
w.cancelAnimationFrame = (id: number) => {
  frameCallbacks = frameCallbacks.filter(([i]) => i !== id);
};

const settle: () => Promise<void> = w.__oracleYield;
export { settle };

let onException: (where: string, error: unknown) => void = (_where, error) => {
  throw error;
};
export function setExceptionHandler(handler: (where: string, error: unknown) => void) {
  onException = handler;
}

function guarded(where: string, fn: () => void) {
  try {
    fn();
  } catch (error) {
    onException(where, error);
  }
}

/** Runs timers and animation frames in time order until `ms` have passed. Timers due at the
 * same instant as a frame run first. */
export async function advance(ms: number, beforeFrame: () => void) {
  const end = clock.now + ms;
  for (;;) {
    const timer = timers.reduce<Timer | null>((a, t) => (a === null || t.due < a.due || (t.due === a.due && t.seq < a.seq) ? t : a), null);
    const frameAt = lastFrame + framePeriod;
    if (timer !== null && timer.due <= end && timer.due <= frameAt) {
      clock.now = Math.max(clock.now, timer.due);
      if (timer.interval === null) timers = timers.filter((t) => t !== timer);
      else {
        timer.due += timer.interval;
        timer.seq = seq++;
      }
      guarded(timer.interval === null ? "setTimeout" : "setInterval", () => timer.fn(...timer.args));
      await settle();
    } else if (frameAt <= end) {
      clock.now = frameAt;
      lastFrame = frameAt;
      const callbacks = frameCallbacks;
      frameCallbacks = [];
      beforeFrame();
      for (const [, cb] of callbacks) guarded("requestAnimationFrame", () => cb(clock.now - clock.start));
      await settle();
    } else {
      clock.now = end;
      return;
    }
  }
}

const define = (target: object, name: string, get: () => unknown) =>
  Object.defineProperty(target, name, { get, configurable: true });
if (!realGeometry) {
  define(w, "innerWidth", () => view.width ?? 1100);
  define(w, "innerHeight", () => view.height ?? 650);
  define(w, "devicePixelRatio", () => view.devicePixelRatio ?? 1);
}
define(w, "outerWidth", () => view.width ?? 1100);
define(w, "outerHeight", () => view.height ?? 650);
w.matchMedia = (query: string) => ({
  matches: query.replace(/\s+/g, "").includes("prefers-color-scheme:" + (w.__oracleTheme ?? theme)),
  media: query,
  onchange: null,
  addListener() {},
  removeListener() {},
  addEventListener() {},
  removeEventListener() {},
  dispatchEvent: () => false
});
w.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

if (!("innerText" in w.HTMLElement.prototype)) Object.defineProperty(w.HTMLElement.prototype, "innerText", {
  configurable: true,
  get(this: HTMLElement) {
    let out = "";
    const visit = (node: Node) => {
      node.childNodes.forEach((child) => {
        if (child.nodeType === 3) out += child.nodeValue;
        else if ((child as Element).tagName === "BR") out += "\n";
        else visit(child);
      });
    };
    visit(this);
    return out;
  },
  set(this: HTMLElement, value: unknown) {
    while (this.firstChild) this.removeChild(this.firstChild);
    String(value)
      .split(/\r\n|\r|\n/)
      .forEach((line, i) => {
        if (i > 0) this.appendChild(document.createElement("br"));
        if (line !== "") this.appendChild(document.createTextNode(line));
      });
  }
});

const rules: LayoutRule[] = [...(fixture.layout ?? []), ...(defaultLayout as LayoutRule[])];

function isHidden(el: Element | null): boolean {
  for (let e = el; e !== null; e = e.parentElement) {
    if (e.hasAttribute("hidden") || (e as HTMLElement).style?.display === "none") return true;
  }
  return false;
}

export function rectOf(el: Element): Rect {
  if (!el.isConnected || isHidden(el)) return [0, 0, 0, 0];
  for (const rule of rules) {
    if (!el.matches(rule.selector)) continue;
    if (rule.rect !== undefined) return rule.rect;
    const parent = el.parentElement === null ? ([0, 0, 0, 0] as Rect) : rectOf(el.parentElement);
    if (rule.text !== undefined) return [parent[0], parent[1], fixedWidth(rule.text, el.textContent ?? ""), Math.round(rule.text * 1.25)];
    return parent;
  }
  return [0, 0, 0, 0];
}

function domRect([x, y, width, height]: Rect) {
  return { x, y, width, height, left: x, top: y, right: x + width, bottom: y + height, toJSON() {} };
}

function installScriptedGeometry() {
  const proto = w.Element.prototype;
  proto.getBoundingClientRect = function (this: Element) {
    return domRect(rectOf(this));
  };
  proto.getClientRects = function (this: Element) {
    const r = rectOf(this);
    return r[2] === 0 && r[3] === 0 ? [] : [domRect(r)];
  };
  for (const [name, index] of [
    ["clientWidth", 2],
    ["clientHeight", 3],
    ["scrollWidth", 2],
    ["scrollHeight", 3]
  ] as [string, number][]) {
    define(proto, name, function (this: Element) {
      return Math.round(rectOf(this)[index]);
    });
  }
  for (const [name, index] of [
    ["offsetWidth", 2],
    ["offsetHeight", 3],
    ["offsetLeft", 0],
    ["offsetTop", 1]
  ] as [string, number][]) {
    define(w.HTMLElement.prototype, name, function (this: Element) {
      return Math.round(rectOf(this)[index]);
    });
  }
  proto.scrollIntoView = function () {};
  proto.scrollTo = function () {};
}
if (realGeometry) installRealGeometry(w, (record) => w.__oracleWrite(JSON.stringify(record)));
else installScriptedGeometry();

export const contexts: RecordingContext[] = [];
const canvasIds = new WeakMap<object, string>();
let nextCanvas = 0;

export function canvasId(canvas: Element): string {
  let id = canvasIds.get(canvas);
  if (id === undefined) {
    id = (canvas.getAttribute("class") || "canvas") + "#" + nextCanvas++;
    canvasIds.set(canvas, id);
  }
  return id;
}

const naming = {
  imageId(source: unknown): string {
    const el = source as Element;
    if (el?.tagName === "CANVAS") return "canvas:" + canvasId(el);
    if (el?.tagName === "IMG") return el.getAttribute("src") ?? "";
    return String((source as any)?.constructor?.name ?? typeof source);
  }
};

const makeImageData = (width: number, height: number) => ({
  width,
  height,
  data: new Uint8ClampedArray(Math.max(0, width * height * 4) || 0),
  colorSpace: "srgb"
});
w.ImageData = function (this: any, a: any, b: any, c: any) {
  if (a instanceof Uint8ClampedArray) return { width: b, height: c ?? a.length / 4 / b, data: a, colorSpace: "srgb" };
  return makeImageData(a, b);
};

const canvasProto = w.HTMLCanvasElement.prototype;
canvasProto.getContext = function (this: HTMLCanvasElement, type: string) {
  if (type !== "2d") return null;
  let ctx = (this as any).__oracleContext as RecordingContext | undefined;
  if (ctx === undefined) {
    canvasId(this);
    ctx = new RecordingContext(this, naming, makeImageData);
    (this as any).__oracleContext = ctx;
    contexts.push(ctx);
  }
  return ctx.proxy;
};
canvasProto.toDataURL = () => "data:,";
for (const name of ["width", "height"]) {
  const original = Object.getOwnPropertyDescriptor(canvasProto, name)!;
  Object.defineProperty(canvasProto, name, {
    configurable: true,
    get: original.get,
    set(this: HTMLCanvasElement, value: unknown) {
      original.set!.call(this, value);
      (this as any).__oracleContext?.reset();
    }
  });
}

const images = fixture.images ?? {};
const imageProto = w.HTMLImageElement.prototype;
const imageSize = (img: HTMLImageElement): [number, number] => images[img.getAttribute("src") ?? ""] ?? [0, 0];
define(imageProto, "naturalWidth", function (this: HTMLImageElement) {
  return imageSize(this)[0];
});
define(imageProto, "naturalHeight", function (this: HTMLImageElement) {
  return imageSize(this)[1];
});
define(imageProto, "complete", function (this: HTMLImageElement) {
  return (this.getAttribute("src") ?? "") in images;
});
for (const [name, index] of [
  ["width", 0],
  ["height", 1]
] as [string, number][]) {
  Object.defineProperty(imageProto, name, {
    configurable: true,
    get(this: HTMLImageElement) {
      const attr = this.getAttribute(name);
      return attr !== null ? Number(attr) : imageSize(this)[index];
    },
    set(this: HTMLImageElement, value: unknown) {
      this.setAttribute(name, String(value));
    }
  });
}
const srcDescriptor = Object.getOwnPropertyDescriptor(imageProto, "src")!;
Object.defineProperty(imageProto, "src", {
  configurable: true,
  get: srcDescriptor.get,
  set(this: HTMLImageElement, value: unknown) {
    srcDescriptor.set!.call(this, value);
    if ((this.getAttribute("src") ?? "") in images) w.setTimeout(() => this.dispatchEvent(new w.Event("load")), 0);
  }
});
const mediaProto = w.HTMLMediaElement.prototype;
mediaProto.play = () => Promise.resolve();
mediaProto.pause = () => {};
mediaProto.load = () => {};

export const windowMessageListeners: Function[] = [];
const addEventListener = w.addEventListener;
w.addEventListener = function (this: unknown, type: string, listener: Function, ...rest: unknown[]) {
  if (type === "message") windowMessageListeners.push(listener);
  return addEventListener.call(this, type, listener, ...rest);
};
