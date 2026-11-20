import { advance, canvasId, clock, contexts, fixture, framePeriod, setExceptionHandler, settle, windowMessageListeners } from "./env";
import "@upstream/hub/hub";
import { DEFAULT_PREFS, mergePreferences } from "@upstream/shared/Preferences";
import { LITE_VERSION } from "@upstream/shared/buildConstants";
import { fromTree, toTree } from "../cve1";
import { measureFixed, parseFontSize } from "./recordingCanvas";

const w = window as any;
const write = (record: object) => w.__oracleWrite(JSON.stringify(record));
const ignoredMain = new Set(fixture.ignoreMain ?? ["save-state", "save-type-memory", "update-touch-bar-slider"]);

function exception(where: string, error: unknown) {
  const e = error as Error;
  w.__oracleLog(`${where}: ${e?.stack ?? String(error)}`);
  write({ exception: where, error: toTree(e instanceof Error ? `${e.name}: ${e.message}` : String(error)) });
}

const port = {
  onmessage: null as null | ((event: { data: unknown }) => void),
  postMessage(message: { name: string; data?: unknown }) {
    if (!ignoredMain.has(message.name)) write({ main: message.name, data: toTree(message.data) });
  },
  start() {},
  close() {}
};

async function sendMain(name: string, data: unknown) {
  try {
    port.onmessage?.({ data: { name, data } });
  } catch (error) {
    exception("main " + name, error);
  }
  await settle();
}

interface Tab {
  type: number;
  title: string;
  controller: { UUID: string };
  renderer: any;
}

const tabList = (): Tab[] => w.tabs.tabList;

function hookRenderers() {
  for (const tab of tabList()) {
    const renderer = tab.renderer;
    if (renderer.__oracleRender !== undefined) continue;
    const render = renderer.render;
    renderer.__oracleRender = render;
    renderer.render = function (command: unknown) {
      renderer.__oracleCommand = toTree(command);
      return render.call(this, command);
    };
  }
}

async function run(ms: number) {
  await advance(ms, hookRenderers);
}

function domNodes(root: Element, attributes: string[]): unknown[] {
  const out: unknown[] = [];
  const visit = (el: Element, depth: number) => {
    let text = "";
    el.childNodes.forEach((child) => {
      if (child.nodeType === 3 && /\S/.test(child.nodeValue ?? "")) text += child.nodeValue;
    });
    const extra: { [k: string]: unknown } = {};
    for (const name of attributes) {
      const value = el.getAttribute(name);
      if (value !== null) extra[name] = value;
    }
    const tag = el.tagName.toLowerCase();
    if (tag === "input" || tag === "select" || tag === "textarea") extra.value = (el as HTMLInputElement).value;
    if (tag === "input" && ["checkbox", "radio"].includes((el as HTMLInputElement).type)) {
      extra.checked = (el as HTMLInputElement).checked;
    }
    out.push([depth, tag, el.getAttribute("class") ?? "", el.hasAttribute("hidden"), text, extra]);
    for (const child of Array.from(el.children)) visit(child, depth + 1);
  };
  visit(root, 0);
  return out;
}

function selectionState(): object {
  const s = w.selection;
  return {
    mode: s.getMode(),
    hoveredTime: s.getHoveredTime(),
    selectedTime: s.getSelectedTime(),
    renderTime: s.getRenderTime(),
    currentLiveTime: s.getCurrentLiveTime(),
    grabZoomRange: s.getGrabZoomRange(),
    timelineRange: s.getTimelineRange(),
    timelineIsMaxZoom: s.getTimelineIsMaxZoom()
  };
}

function evaluate(source: string): unknown {
  return new Function("window", `return (${source});`)(w);
}

function capture(spec: any) {
  const kind = typeof spec === "string" ? spec : spec.kind;
  switch (kind) {
    case "state":
      write({ state: toTree({ sidebar: w.sidebar.saveState(), tabs: w.tabs.saveState() }) });
      return;
    case "commands":
      tabList().forEach((tab, index) => {
        const command = tab.renderer.__oracleCommand;
        if (command === undefined) return;
        delete tab.renderer.__oracleCommand;
        write({ command: toTree({ index, type: tab.type, title: tab.title, uuid: tab.controller.UUID }), value: command });
      });
      return;
    case "canvas":
      for (const ctx of contexts) {
        const canvas = ctx.canvas;
        if (!ctx.drawnSinceCapture || (spec.selector !== undefined && !canvas.matches(spec.selector))) continue;
        ctx.drawnSinceCapture = false;
        write({
          canvas: canvasId(canvas),
          frame: toTree(ctx.frames),
          width: toTree(canvas.width),
          height: toTree(canvas.height),
          calls: toTree(ctx.calls)
        });
      }
      return;
    case "dom": {
      const roots = Array.from(document.querySelectorAll(spec.selector));
      const chosen = spec.all ? roots : roots.slice(0, 1);
      write({ dom: spec.selector, nodes: toTree(chosen.map((el) => domNodes(el, spec.attributes ?? ["title"]))) });
      return;
    }
    case "selection":
      write({ selection: toTree(selectionState()) });
      return;
    case "eval":
      try {
        write({ eval: spec.expr, value: toTree(evaluate(spec.expr)) });
      } catch (error) {
        write({ eval: spec.expr, throw: toTree(error instanceof Error ? `${error.name}: ${error.message}` : error) });
      }
      return;
    case "measure": {
      const size = parseFontSize(spec.font);
      for (const text of spec.texts as string[]) {
        const metrics = size === null ? null : measureFixed(size, spec.align ?? "start", spec.baseline ?? "alphabetic", text);
        write({ measure: toTree({ font: spec.font, align: spec.align ?? "start", baseline: spec.baseline ?? "alphabetic", text }), metrics: toTree(metrics) });
      }
      return;
    }
    default:
      throw new Error(`unknown capture ${JSON.stringify(kind)}`);
  }
}

async function step(s: any, index: number) {
  if ("main" in s) await sendMain(s.main, "cve1" in s ? fromTree(s.cve1) : s.data);
  else if ("log" in s) {
    for (const [method, ...args] of s.log as [string, ...unknown[]][]) {
      w.log[method](...args.map((a) => fromTree(a)));
    }
    w.sidebar.refresh();
    if (s.newFields !== false) w.tabs.refresh();
    await settle();
  } else if ("advance" in s) await run(s.advance);
  else if ("frames" in s) await run(s.frames * framePeriod);
  else if ("eval" in s) {
    try {
      evaluate(s.eval);
    } catch (error) {
      exception("eval", error);
    }
    await settle();
  } else if ("event" in s) {
    const e = s.event;
    const target = e.target === "window" ? w : e.target === "document" ? document : document.querySelectorAll(e.target)[e.index ?? 0];
    if (target === undefined) throw new Error(`step ${index}: no element ${e.target}`);
    const Kind = w[e.kind ?? "Event"];
    try {
      if (e.props !== undefined) Object.assign(target, e.props);
      target.dispatchEvent(new Kind(e.type, { bubbles: true, cancelable: true, ...(e.init ?? {}) }));
    } catch (error) {
      exception("event " + e.type, error);
    }
    await settle();
  } else if ("selection" in s) {
    const [method, ...rawArgs] = s.selection as [string, ...unknown[]];
    try {
      if (method === "setLiveConnected") {
        w.selection.setLiveConnected(() => (w as any).__liveTime);
      } else {
        (w.selection as any)[method](...rawArgs.map((a) => fromTree(a)));
      }
    } catch (error) {
      exception("selection " + method, error);
    }
    await settle();
  } else if ("liveTime" in s) {
    (w as any).__liveTime = fromTree(s.liveTime);
    await settle();
  } else if ("tuner" in s) {
    if (s.tuner === null) {
      w.tuner = null;
    } else {
      const keys = new Set((s.tuner as unknown[]).map((k) => String(fromTree(k))));
      w.tuner = {
        hasTunableFields: () => keys.size > 0,
        isTunable: (key: string) => keys.has(key),
        publish: (key: string, value: unknown) => write({ tuner: ["publish", toTree(key), toTree(value)] }),
        unpublish: (key: string) => write({ tuner: ["unpublish", toTree(key)] })
      };
    }
    await settle();
  } else if ("capture" in s) {
    write({ capture: toTree(s.label ?? index), time: toTree(clock.now - clock.start) });
    for (const spec of Array.isArray(s.capture) ? s.capture : [s.capture]) capture(spec);
  } else throw new Error(`step ${index}: unknown step ${JSON.stringify(Object.keys(s))}`);
}

w.__oracleRun = async () => {
  setExceptionHandler(exception);
  w.addEventListener("drag-update", (event: Event) => {
    const detail = (event as CustomEvent).detail;
    write({ drag: toTree(detail.data), end: toTree(detail.end), x: toTree(detail.x), y: toTree(detail.y) });
  });
  for (const listener of windowMessageListeners) listener({ data: "port", ports: [port] });
  const prefs = JSON.parse(JSON.stringify(DEFAULT_PREFS));
  if (fixture.preferences !== undefined) mergePreferences(prefs, fixture.preferences);
  const boot: [string, unknown][] = fixture.boot ?? [
    ["set-assets", fixture.assets ?? null],
    ["set-fullscreen", false],
    ["set-battery", false],
    [
      "set-version",
      fixture.version ?? { platform: "darwin", platformRelease: "25.0.0", platformArch: "arm64", appVersion: LITE_VERSION }
    ],
    ["show-update-button", false],
    ["show-feedback-button", false],
    ["show-when-ready", undefined],
    ["set-preferences", prefs],
    ["set-active-satellites", []],
    ...(fixture.typeMemory !== undefined ? [["restore-type-memory", fixture.typeMemory] as [string, unknown]] : []),
    ...(fixture.state !== undefined ? [["restore-state", fixture.state] as [string, unknown]] : [])
  ];
  for (const [name, data] of boot) await sendMain(name, data);
  const steps = fixture.steps ?? [];
  for (let i = 0; i < steps.length; i++) await step(steps[i], i);
};
