import h337 from "heatmap.js";

// Records what Heatmap.update observably does through the real h337: whether a new
// instance was created, the argument each setData call received, and the store's
// getData() output afterwards. Both oracle harnesses share it; only conversion lives
// at the call sites.

export interface Recorder {
  created: unknown[];
  setCalls: unknown[];
}

export function installRecorder(): Recorder {
  const created: unknown[] = [];
  const setCalls: unknown[] = [];
  const original = (h337 as any).create;
  (h337 as any).create = function (this: unknown, config: unknown) {
    const instance = original.call(this, config);
    const setData = instance.setData;
    instance.setData = function (this: unknown, ...args: unknown[]) {
      setCalls.push(args[0]);
      return setData.apply(this, args);
    };
    created.push(instance);
    return instance;
  };
  return { created, setCalls };
}

export interface UpdateRecord {
  created: boolean;
  setData: unknown;
  store: unknown;
  error: unknown;
}

export function runUpdates(
  heatmap: { update(t: [number, number][], p: [number, number], f: [number, number]): void },
  recorder: Recorder,
  updates: { translations: [number, number][]; pixelDimensions: [number, number]; fieldDimensions: [number, number] }[],
  afterEach?: () => void
): UpdateRecord[] {
  const results: UpdateRecord[] = [];
  for (const update of updates) {
    const createdBefore = recorder.created.length;
    const setBefore = recorder.setCalls.length;
    let error: unknown = null;
    try {
      heatmap.update(update.translations, update.pixelDimensions, update.fieldDimensions);
    } catch (e) {
      error = e;
    }
    afterEach?.();
    const instance = recorder.created[recorder.created.length - 1] as
      | { getData(): unknown }
      | undefined;
    results.push({
      created: recorder.created.length > createdBefore,
      setData: recorder.setCalls.length > setBefore ? recorder.setCalls[recorder.setCalls.length - 1] : null,
      store: instance ? instance.getData() : null,
      error
    });
  }
  return results;
}
