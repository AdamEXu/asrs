type Cmd = unknown[];
type Source = HTMLCanvasElement;
type Context = CanvasRenderingContext2D;

interface ReplayScene {
  width: number;
  height: number;
  dpr: number;
  cmds: Cmd[];
}

interface Replay {
  sources: Map<string, Source>;
  registers: Map<string, ImageData>;
  cpu: boolean;
}

function newContext(canvas: HTMLCanvasElement, cpu: boolean): Context {
  const ctx = canvas.getContext("2d", cpu ? { willReadFrequently: true } : undefined);
  if (ctx === null) throw new Error("no 2d context");
  return ctx;
}

function imageDataFrom(width: number, height: number, rgba: string): ImageData {
  const bytes = Uint8ClampedArray.from(atob(rgba), (c) => c.charCodeAt(0));
  return new ImageData(bytes, width, height);
}

function style(ctx: Context, value: unknown): unknown {
  if (value === null || typeof value !== "object") return value;
  const spec = value as { linearGradient?: number[]; radialGradient?: number[]; stops: [number, string][] };
  let gradient: CanvasGradient;
  if (spec.linearGradient) {
    const [x0, y0, x1, y1] = spec.linearGradient;
    gradient = ctx.createLinearGradient(x0, y0, x1, y1);
  } else if (spec.radialGradient) {
    const [x0, y0, r0, x1, y1, r1] = spec.radialGradient;
    gradient = ctx.createRadialGradient(x0, y0, r0, x1, y1, r1);
  } else {
    throw new Error(`unknown style ${JSON.stringify(value)}`);
  }
  for (const [offset, color] of spec.stops) gradient.addColorStop(offset, color);
  return gradient;
}

function arg(replay: Replay, value: unknown): unknown {
  if (typeof value === "string" && value.startsWith("@")) {
    const source = replay.sources.get(value.slice(1));
    if (source === undefined) throw new Error(`unknown image ${value}`);
    return source;
  }
  return value;
}

function run(replay: Replay, ctx: Context, cmds: Cmd[]) {
  for (const cmd of cmds) {
    const [name, ...args] = cmd as [string, ...unknown[]];
    switch (name) {
      case "=":
        (ctx as any)[args[0] as string] = style(ctx, args[1]);
        break;
      case "image": {
        const [id, width, height, rgba] = args as [string, number, number, string];
        const canvas = document.createElement("canvas");
        canvas.width = width;
        canvas.height = height;
        newContext(canvas, true).putImageData(imageDataFrom(width, height, rgba), 0, 0);
        replay.sources.set(id, canvas);
        break;
      }
      case "canvas": {
        const [id, width, height, inner] = args as [string, number, number, Cmd[]];
        const canvas = document.createElement("canvas");
        canvas.width = width;
        canvas.height = height;
        run(replay, newContext(canvas, replay.cpu), inner);
        replay.sources.set(id, canvas);
        break;
      }
      case "frame":
        ctx.canvas.width = ctx.canvas.width;
        break;
      case "resize": {
        const [width, height] = args as [number, number];
        ctx.canvas.width = width;
        ctx.canvas.height = height;
        break;
      }
      case "getImageData": {
        const [sx, sy, sw, sh, register] = args as [number, number, number, number, string];
        replay.registers.set(register, ctx.getImageData(sx, sy, sw, sh));
        break;
      }
      case "putImageData": {
        const [data, dx, dy] = args as [unknown, number, number];
        let image: ImageData | undefined;
        if (typeof data === "string") {
          image = replay.registers.get(data);
        } else {
          const spec = data as { width: number; height: number; rgba: string };
          image = imageDataFrom(spec.width, spec.height, spec.rgba);
        }
        if (image === undefined) throw new Error(`unknown image data ${String(data)}`);
        ctx.putImageData(image, dx, dy);
        break;
      }
      default:
        (ctx as any)[name](...args.map((a) => arg(replay, a)));
    }
  }
}

function toBase64(data: Uint8ClampedArray): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < data.length; i += chunk) {
    binary += String.fromCharCode.apply(null, Array.from(data.subarray(i, i + chunk)));
  }
  return btoa(binary);
}

function replay(scene: ReplayScene, cpu: boolean) {
  const canvas = document.createElement("canvas");
  canvas.width = scene.width * scene.dpr;
  canvas.height = scene.height * scene.dpr;
  document.body.appendChild(canvas);
  const ctx = newContext(canvas, cpu);
  run({ sources: new Map(), registers: new Map(), cpu }, ctx, scene.cmds);
  const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
  canvas.remove();
  return { width: canvas.width, height: canvas.height, rgba: toBase64(data) };
}

const METRIC_KEYS = [
  "width",
  "actualBoundingBoxLeft",
  "actualBoundingBoxRight",
  "actualBoundingBoxAscent",
  "actualBoundingBoxDescent",
  "fontBoundingBoxAscent",
  "fontBoundingBoxDescent",
  "emHeightAscent",
  "emHeightDescent",
  "hangingBaseline",
  "alphabeticBaseline",
  "ideographicBaseline"
] as const;

function measure(fonts: string[], strings: string[], aligns: string[], baselines: string[]) {
  const ctx = newContext(document.createElement("canvas"), false);
  const out: unknown[] = [];
  for (const font of fonts) {
    ctx.font = font;
    const resolvedFont = ctx.font;
    for (const text of strings) {
      for (const align of aligns) {
        for (const baseline of baselines) {
          ctx.textAlign = align as CanvasTextAlign;
          ctx.textBaseline = baseline as CanvasTextBaseline;
          const m = ctx.measureText(text);
          const metrics: [string, number][] = METRIC_KEYS.map((k) => [k, m[k]]);
          out.push({ font, resolvedFont, text, align, baseline, metrics });
        }
      }
    }
  }
  return out;
}

function domBoxes() {
  const rows: unknown[] = [];
  for (const el of Array.from(document.querySelectorAll<HTMLElement>("[data-row]"))) {
    const text = el.firstChild;
    const r = el.getBoundingClientRect();
    const chars: number[][] = [];
    if (text !== null && text.nodeType === Node.TEXT_NODE) {
      const value = text.nodeValue ?? "";
      const range = document.createRange();
      for (let i = 0; i < value.length; ) {
        const end = i + (value.codePointAt(i)! > 0xffff ? 2 : 1);
        range.setStart(text, i);
        range.setEnd(text, end);
        const b = range.getBoundingClientRect();
        chars.push([i, b.x, b.y, b.width, b.height]);
        i = end;
      }
    }
    rows.push({ id: el.dataset.row, rect: [r.x, r.y, r.width, r.height], chars });
  }
  const patches = Array.from(document.querySelectorAll<HTMLElement>("[data-patch]")).map((el) => {
    const r = el.getBoundingClientRect();
    return { color: el.dataset.patch, rect: [r.x, r.y, r.width, r.height] };
  });
  return { devicePixelRatio: window.devicePixelRatio, rows, patches };
}

Object.assign(window, { replay, measure, domBoxes });
