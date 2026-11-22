import { advance, framePeriod, fixture, settle, setExceptionHandler, windowMessageListeners } from "../dom/env";
import "@upstream/hub/hub";
import { DEFAULT_PREFS, mergePreferences } from "@upstream/shared/Preferences";
import { LITE_VERSION } from "@upstream/shared/buildConstants";
import { fromTree, toTree } from "../cve1";

const w = window as any;
const hub = w.__oracleHub;
const write = (record: object) => w.__oracleWrite(JSON.stringify(record));
const ignoredMain = new Set((fixture as any).ignoreMain ?? ["save-state", "save-type-memory", "update-touch-bar-slider"]);

function exception(where: string, error: unknown) {
  const e = error as Error;
  w.__oracleLog(`${where}: ${e?.stack ?? String(error)}`);
  write({ exception: where });
}

/** A copy built from this realm's constructors, as a structured clone into the page gives. */
function adopt(value: any): any {
  if (value === null || typeof value !== "object") return value;
  const tag = Object.prototype.toString.call(value);
  if (ArrayBuffer.isView(value)) return new w[value.constructor.name](value as any);
  if (tag === "[object ArrayBuffer]") return new w.Uint8Array(new Uint8Array(value)).buffer;
  if (Array.isArray(value)) return w.Array.from(value, adopt);
  if (tag === "[object Map]") return new w.Map(Array.from(value as Map<unknown, unknown>, ([k, v]) => [adopt(k), adopt(v)]));
  if (tag === "[object Set]") return new w.Set(Array.from(value as Set<unknown>, adopt));
  if (tag === "[object Date]") return new w.Date((value as Date).getTime());
  const out = new w.Object();
  for (const key of Object.keys(value)) out[key] = adopt(value[key]);
  return out;
}

const scripts: string[] = [];

/** `Worker` on a real thread, reporting what the page does with it. */
class PageWorker {
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  private index: number;

  constructor(script: string) {
    this.index = hub.spawn(script, (data: unknown) => this.onmessage?.({ data: adopt(data) }));
    scripts.push(script);
    write({ worker: toTree(this.index), script: toTree(script.split("/").pop()) });
  }

  postMessage(data: any) {
    if (data?.type === "start") {
      write({ post: toTree(this.index), type: "start", files: toTree(data.data.map((f: Uint8Array | null) => (f === null ? null : f.length))) });
    } else if (data?.type === "parseField") {
      write({ post: toTree(this.index), type: "parseField", key: toTree(data.key) });
    } else {
      write({ post: toTree(this.index), type: "request" });
    }
    hub.post(this.index, data);
  }

  terminate() {
    write({ terminate: toTree(this.index) });
    hub.terminate(this.index);
  }
}
w.Worker = PageWorker;

const port = {
  onmessage: null as null | ((event: { data: unknown }) => void),
  postMessage(message: { name: string; data?: any }) {
    const { name, data } = message;
    if (name === "historical-start") {
      write({ main: name, data: toTree({ uuid: data.uuid, path: data.path }) });
      const reply = hub.historicalStart(data.uuid, data.path);
      w.setTimeout(() => deliverMain("historical-data", adopt(reply)), 0);
    } else if (name === "historical-start-raw") {
      write({ main: name, data: toTree({ uuid: data.uuid, extension: data.extension, bytes: data.data.length }) });
    } else if (!ignoredMain.has(name)) {
      write({ main: name, data: toTree(data) });
    }
  },
  start() {},
  close() {}
};

function deliverMain(name: string, data: unknown) {
  try {
    port.onmessage?.({ data: { name, data } });
  } catch (error) {
    exception("main " + name, error);
  }
}

function selectionState(): object {
  const s = w.selection;
  return {
    mode: s.getMode(),
    selectedTime: s.getSelectedTime(),
    hoveredTime: s.getHoveredTime(),
    timelineRange: s.getTimelineRange(),
    timelineIsMaxZoom: s.getTimelineIsMaxZoom()
  };
}

function capture(kind: string) {
  const root = document.documentElement.style;
  switch (kind) {
    case "title":
      write({ title: toTree(document.getElementsByTagName("title")[0].textContent) });
      return;
    case "loading":
      write({
        loading: toTree({
          show: root.getPropertyValue("--show-loading-glow"),
          progress: root.getPropertyValue("--loading-glow-progress")
        })
      });
      return;
    case "selection":
      write({ selection: toTree(selectionState()) });
      return;
    case "log":
      write({
        log: toTree({
          keys: w.log.getFieldKeys(),
          range: w.log.getTimestampRange(),
          loading: Array.from(w.getLoadingFields() as Set<string>).sort()
        })
      });
      return;
    case "state":
      write({ state: toTree({ sidebar: w.sidebar.saveState(), tabs: w.tabs.saveState() }) });
      return;
    case "window":
      write({
        window: toTree({
          classes: Array.from(document.body.classList).filter((c) => c.startsWith("fancy-")),
          updateButton: root.getPropertyValue("--show-update-button"),
          feedbackButton: root.getPropertyValue("--show-feedback-button")
        })
      });
      return;
    default:
      throw new Error(`unknown capture ${JSON.stringify(kind)}`);
  }
}

async function step(s: any, index: number) {
  if ("main" in s) {
    deliverMain(s.main, "cve1" in s ? fromTree(s.cve1) : s.data);
    await settle();
  } else if ("advance" in s) await advance(s.advance, () => {});
  else if ("frames" in s) await advance(s.frames * framePeriod, () => {});
  else if ("capture" in s) {
    write({ capture: toTree(s.label ?? index) });
    for (const kind of s.capture) capture(kind);
  } else throw new Error(`step ${index}: unknown step ${JSON.stringify(Object.keys(s))}`);
}

w.__oracleRun = async () => {
  setExceptionHandler(exception);
  for (const listener of windowMessageListeners) listener({ data: "port", ports: [port] });
  const prefs = JSON.parse(JSON.stringify(DEFAULT_PREFS));
  if ((fixture as any).preferences !== undefined) mergePreferences(prefs, (fixture as any).preferences);
  const boot: [string, unknown][] = (fixture as any).boot ?? [
    ["set-assets", fixture.assets ?? null],
    ["set-fullscreen", false],
    ["set-battery", false],
    ["set-version", fixture.version ?? { platform: "darwin", platformRelease: "25.0.0", platformArch: "arm64", appVersion: LITE_VERSION }],
    ["show-update-button", false],
    ["show-feedback-button", false],
    ["show-when-ready", undefined],
    ["set-preferences", prefs],
    ["set-active-satellites", []]
  ];
  for (const [name, data] of boot) {
    deliverMain(name, data);
    await settle();
  }
  const steps = (fixture as any).steps ?? [];
  for (let i = 0; i < steps.length; i++) await step(steps[i], i);
};
