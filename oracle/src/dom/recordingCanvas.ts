import table from "../canvas/fixedAdvance.json";

const f = Math.fround;

export interface FixedMetrics {
  width: number;
  actualBoundingBoxLeft: number;
  actualBoundingBoxRight: number;
  actualBoundingBoxAscent: number;
  actualBoundingBoxDescent: number;
  fontBoundingBoxAscent: number;
  fontBoundingBoxDescent: number;
  hangingBaseline: number;
  alphabeticBaseline: number;
  ideographicBaseline: number;
}

export function fixedAdvance(unit: number): number {
  for (const [first, last, em] of table.advances) {
    if (unit >= first && unit <= last) return f(em);
  }
  return f(table.defaultAdvance);
}

function isBlank(unit: number): boolean {
  return table.blank.includes(unit);
}

/** Width in f32 of `text` under the shared table, as `FixedAdvance::shape` accumulates it. */
export function fixedWidth(size: number, text: string): number {
  let x = f(0);
  for (let i = 0; i < text.length; i++) x = f(x + f(fixedAdvance(text.charCodeAt(i)) * f(size)));
  return x;
}

function roundHalfAway(v: number): number {
  return v < 0 ? -Math.floor(-v + 0.5) : Math.floor(v + 0.5);
}

function layoutRound(v: number): number {
  return f(roundHalfAway(f(v * 64)) / 64);
}

function normalizedTypo(ascent: number, descent: number, size: number): [number, number] {
  const height = f(ascent + descent);
  if (height <= 0 || ascent < 0 || ascent > height) return [0, 0];
  const a = layoutRound(f(f(ascent * size) / height));
  return [a, f(layoutRound(size) - a)];
}

function baselineOffset(ascent: number, descent: number, size: number, baseline: string): number {
  switch (baseline) {
    case "top":
      return normalizedTypo(ascent, descent, size)[0];
    case "hanging":
      return f(f(80 * ascent) / 100);
    case "ideographic":
      return -descent;
    case "bottom":
      return -normalizedTypo(ascent, descent, size)[1];
    case "middle": {
      const [a, d] = normalizedTypo(ascent, descent, size);
      return f(f(a - d) / 2);
    }
    default:
      return 0;
  }
}

/** `asrs_canvas::text::measure_text` with the `FixedAdvance` backend, in the same f32 steps. */
export function measureFixed(sizeIn: number, align: string, baseline: string, raw: string): FixedMetrics {
  const size = f(sizeIn);
  const text = raw.replace(/[\t\n ]/g, " ");
  let ink: [number, number, number, number] = [0, 0, 0, 0];
  const empty = (r: number[]) => f(r[2] - r[0]) <= 0 || f(r[3] - r[1]) <= 0;
  let x = f(0);
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    const advance = f(fixedAdvance(unit) * size);
    if (!isBlank(unit)) {
      const glyph: [number, number, number, number] = [
        f(0 + x),
        f(f(table.inkTop) * size),
        f(advance + x),
        f(f(table.inkBottom) * size)
      ];
      if (!empty(glyph)) {
        if (empty(ink)) ink = glyph;
        else ink = [Math.min(ink[0], glyph[0]), Math.min(ink[1], glyph[1]), Math.max(ink[2], glyph[2]), Math.max(ink[3], glyph[3])];
      }
    }
    x = f(x + advance);
  }
  const width = x;
  const anchor = align === "center" ? f(width / 2) : align === "right" || align === "end" ? width : 0;
  const ascent = Math.floor(f(f(size * f(table.ascent)) + 0.5));
  const descent = Math.floor(f(f(size * f(table.descent)) + 0.5));
  const by = baselineOffset(ascent, descent, size, baseline);
  const [left, top, right, bottom] = ink;
  return {
    width,
    actualBoundingBoxLeft: f(-left + anchor),
    actualBoundingBoxRight: f(right - anchor),
    actualBoundingBoxAscent: f(-top - by),
    actualBoundingBoxDescent: f(bottom + by),
    fontBoundingBoxAscent: f(ascent - by),
    fontBoundingBoxDescent: f(descent + by),
    hangingBaseline: f(f(f(ascent * 80) / 100) - by),
    alphabeticBaseline: f(-by),
    ideographicBaseline: f(-descent - by)
  };
}

const FONT_KEYWORDS: { [k: string]: number } = {
  "xx-small": 9,
  "x-small": 10,
  small: 13,
  medium: 16,
  large: 18,
  "x-large": 24,
  "xx-large": 32,
  "xxx-large": 48
};
const FONT_UNITS: [string, number][] = [
  ["px", 1],
  ["pt", 4 / 3],
  ["pc", 16],
  ["in", 96],
  ["cm", 96 / 2.54],
  ["mm", 96 / 25.4],
  ["q", 96 / 101.6]
];

function cssNumber(s: string): number | null {
  if (!/^[0-9.e+-]+$/.test(s) || !/^[+-]?([0-9]+\.?[0-9]*|\.[0-9]+)(e[+-]?[0-9]+)?$/.test(s)) return null;
  return Number(s);
}

function fontSize(word: string): number | null {
  if (word in FONT_KEYWORDS) return FONT_KEYWORDS[word];
  for (const [unit, factor] of FONT_UNITS) {
    if (!word.endsWith(unit)) continue;
    const n = cssNumber(word.slice(0, -unit.length));
    if (n !== null) return factor === 1 ? n : n * factor;
  }
  return null;
}

/** The size `asrs_canvas::text::FontSpec::parse` reads from a canvas `font`, or null when the
 * assignment would be ignored. */
export function parseFontSize(css: string): number | null {
  let rest = css.trim();
  const word = (): string | null => {
    const m = /[\t\n\f\r /,]/.exec(rest);
    const end = m === null ? rest.length : m.index;
    if (end === 0) return null;
    const w = rest.slice(0, end);
    rest = rest.slice(end).replace(/^[\t\n\f\r ]+/, "");
    return w;
  };
  let style = false;
  let variant = false;
  let weight = false;
  let stretch = false;
  let prefix = 0;
  let size: number;
  for (;;) {
    const w = word();
    if (w === null) return null;
    const lower = w.toLowerCase();
    const s = fontSize(lower);
    if (s !== null) {
      size = s;
      break;
    }
    if (++prefix > 4) return null;
    if (lower === "normal") continue;
    if ((lower === "italic" || lower === "oblique") && !style) style = true;
    else if (lower === "small-caps" && !variant) variant = true;
    else if ((lower === "bold" || lower === "bolder" || lower === "lighter") && !weight) weight = true;
    else if (/^(ultra-|extra-|semi-)?(condensed|expanded)$/.test(lower) && !stretch) stretch = true;
    else {
      const n = cssNumber(lower);
      if (n === null || weight || !(n >= 1 && n <= 1000)) return null;
      weight = true;
    }
  }
  if (!Number.isFinite(size) || size < 0) return null;
  if (rest.startsWith("/")) {
    rest = rest.slice(1).replace(/^[\t\n\f\r ]+/, "");
    if (word() === null) return null;
  }
  if (rest === "" || rest.split(",").some((item) => item.trim() === "")) return null;
  return f(size);
}

const TEXT_ALIGNS = ["start", "end", "left", "right", "center"];
const TEXT_BASELINES = ["top", "hanging", "middle", "alphabetic", "ideographic", "bottom"];

interface TextState {
  size: number;
  align: string;
  baseline: string;
}

const DEFAULTS: { [name: string]: unknown } = {
  fillStyle: "#000000",
  strokeStyle: "#000000",
  lineWidth: 1,
  lineCap: "butt",
  lineJoin: "miter",
  miterLimit: 10,
  lineDashOffset: 0,
  globalAlpha: 1,
  globalCompositeOperation: "source-over",
  font: "10px sans-serif",
  textAlign: "start",
  textBaseline: "alphabetic",
  direction: "ltr",
  imageSmoothingEnabled: true,
  imageSmoothingQuality: "low",
  shadowBlur: 0,
  shadowColor: "rgba(0, 0, 0, 0)",
  shadowOffsetX: 0,
  shadowOffsetY: 0,
  filter: "none"
};

const QUERIES = new Set(["measureText", "isPointInPath", "isPointInStroke", "getLineDash", "getTransform", "getContextAttributes"]);

class RecordedGradient {
  constructor(
    readonly kind: string,
    readonly params: number[],
    readonly stops: [number, string][] = []
  ) {}
  addColorStop(offset: number, color: string) {
    this.stops.push([offset, color]);
  }
}

class RecordedPattern {
  constructor(
    readonly image: string,
    readonly repetition: unknown
  ) {}
}

export interface CanvasNaming {
  imageId(source: unknown): string;
}

/** A `CanvasRenderingContext2D` stand-in that records every call and assignment in the form
 * `asrs_canvas::trace` decodes. Setting the canvas `width` or `height` starts a new frame. */
export class RecordingContext {
  calls: unknown[][] = [];
  frames = 0;
  drawnSinceCapture = false;
  private props: { [name: string]: unknown } = { ...DEFAULTS };
  private text: TextState = { size: 10, align: "start", baseline: "alphabetic" };
  private stack: { props: { [name: string]: unknown }; text: TextState; dash: number[] }[] = [];
  private dash: number[] = [];
  private registers = new WeakMap<object, string>();
  private nextRegister = 0;
  readonly proxy: any;

  constructor(
    readonly canvas: HTMLCanvasElement,
    private naming: CanvasNaming,
    private makeImageData: (w: number, h: number) => { width: number; height: number; data: Uint8ClampedArray }
  ) {
    const self = this;
    this.proxy = new Proxy(
      {},
      {
        get(_target, name) {
          if (typeof name !== "string") return undefined;
          if (name === "canvas") return self.canvas;
          if (name === "then") return undefined;
          if (Object.hasOwn(self.props, name)) return self.props[name];
          return (...args: unknown[]) => self.method(name, args);
        },
        set(_target, name, value) {
          if (typeof name !== "string") return true;
          self.assign(name, value);
          return true;
        }
      }
    );
  }

  reset() {
    this.calls = [];
    this.frames++;
    this.drawnSinceCapture = true;
    this.props = { ...DEFAULTS };
    this.text = { size: 10, align: "start", baseline: "alphabetic" };
    this.stack = [];
    this.dash = [];
  }

  private record(call: unknown[]) {
    this.calls.push(call);
    this.drawnSinceCapture = true;
  }

  private styleValue(value: unknown): unknown {
    if (value instanceof RecordedGradient) {
      return { [value.kind]: [...value.params], stops: value.stops.map(([o, c]) => [o, c]) };
    }
    if (value instanceof RecordedPattern) return { pattern: value.image, repetition: value.repetition };
    return value;
  }

  private assign(name: string, value: unknown) {
    const recorded = name === "fillStyle" || name === "strokeStyle" ? this.styleValue(value) : value;
    this.record(["=", name, recorded]);
    this.props[name] = value;
    if (name === "font") {
      const size = parseFontSize(String(value));
      if (size !== null) this.text = { ...this.text, size };
    } else if (name === "textAlign" && TEXT_ALIGNS.includes(value as string)) {
      this.text = { ...this.text, align: value as string };
    } else if (name === "textBaseline" && TEXT_BASELINES.includes(value as string)) {
      this.text = { ...this.text, baseline: value as string };
    }
  }

  private argValue(value: unknown): unknown {
    if (value !== null && typeof value === "object") {
      const register = this.registers.get(value);
      if (register !== undefined) return register;
      const tag = (value as any).tagName;
      if (typeof tag === "string" || (value as any).__imageBitmap) return "@" + this.naming.imageId(value);
      if ((value as any).data instanceof Uint8ClampedArray) {
        const image = value as any;
        return { width: image.width, height: image.height, data: new Uint8Array(image.data) };
      }
    }
    return value;
  }

  private method(name: string, passed: unknown[]): unknown {
    const args = [...passed];
    while (args.length > 0 && args[args.length - 1] === undefined) args.pop();
    switch (name) {
      case "createLinearGradient":
        return new RecordedGradient("linearGradient", args as number[]);
      case "createRadialGradient":
        return new RecordedGradient("radialGradient", args as number[]);
      case "createConicGradient":
        return new RecordedGradient("conicGradient", args as number[]);
      case "createPattern":
        return new RecordedPattern("@" + this.naming.imageId(args[0]), args[1]);
      case "createImageData": {
        const first = args[0] as any;
        return typeof first === "object" ? this.makeImageData(first.width, first.height) : this.makeImageData(Number(args[0]), Number(args[1]));
      }
      case "measureText":
        return measureFixed(this.text.size, this.text.align, this.text.baseline, String(args[0]));
      case "isPointInPath":
      case "isPointInStroke":
        return false;
      case "getLineDash":
        return [...this.dash];
      case "getTransform":
        return { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 };
      case "getContextAttributes":
        return { alpha: true, colorSpace: "srgb", desynchronized: false, willReadFrequently: false };
    }
    if (QUERIES.has(name)) return undefined;
    if (name === "getImageData") {
      const register = "r" + this.nextRegister++;
      this.record(["getImageData", ...args.slice(0, 4), register]);
      const image = this.makeImageData(Math.abs(Number(args[2])), Math.abs(Number(args[3])));
      this.registers.set(image, register);
      return image;
    }
    if (name === "save") this.stack.push({ props: { ...this.props }, text: this.text, dash: this.dash });
    if (name === "restore") {
      const saved = this.stack.pop();
      if (saved !== undefined) {
        this.props = saved.props;
        this.text = saved.text;
        this.dash = saved.dash;
      }
    }
    if (name === "reset") {
      this.reset();
      return undefined;
    }
    if (name === "setLineDash") {
      const segments = Array.from(args[0] as ArrayLike<number>);
      this.dash = segments.length % 2 === 1 ? [...segments, ...segments] : segments;
      this.record(["setLineDash", segments]);
      return undefined;
    }
    this.record([name, ...args.map((a) => this.argValue(a))]);
    return undefined;
  }
}
