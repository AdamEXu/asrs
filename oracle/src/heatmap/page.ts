import Heatmap from "@upstream/shared/renderers/Heatmap";
import { heatmapFixtures } from "./fixtures";
import { installRecorder, runUpdates } from "./record";

// canvasMain's "heatmap" mode drives this in a real BrowserWindow: the same updates
// the genHeatmap entry records under the fake DOM, plus the output canvas pixels.

const recorder = installRecorder();

function base64(bytes: Uint8ClampedArray): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i += 8192) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 8192));
  }
  return btoa(bin);
}

function heatmapRun(index: number) {
  const fixture = heatmapFixtures()[index];
  const container = document.createElement("div");
  document.body.appendChild(container);
  const heatmap = new Heatmap(container);
  const captures: {
    rgba: string | null;
    shadow: string | null;
    readError: { name: string; message: string } | null;
    size: [number, number] | null;
  }[] = [];
  const records = runUpdates(heatmap, recorder, fixture.updates, () => {
    const canvas = heatmap.getCanvas();
    let rgba: string | null = null;
    let shadow: string | null = null;
    let readError: { name: string; message: string } | null = null;
    let size: [number, number] | null = null;
    if (canvas) {
      size = [canvas.width, canvas.height];
      try {
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        const image = ctx!.getImageData(0, 0, canvas.width, canvas.height);
        rgba = base64(image.data);
        const inner = (heatmap as unknown as { heatmap?: { _renderer?: { shadowCanvas?: HTMLCanvasElement } } })
          .heatmap?._renderer?.shadowCanvas;
        if (inner && inner.width > 0 && inner.height > 0) {
          shadow = base64(
            inner.getContext("2d", { willReadFrequently: true })!.getImageData(0, 0, inner.width, inner.height).data
          );
        }
      } catch (e) {
        const error = e as { name?: unknown; message?: unknown };
        readError = { name: String(error.name), message: String(error.message) };
      }
    }
    captures.push({ rgba, shadow, readError, size });
  });
  const results = records.map((record, i) => {
    const capture = captures[i];
    const updateError = record.error as { name?: unknown; message?: unknown } | null;
    return {
      created: record.created,
      setData: record.setData,
      store: record.store,
      error: updateError ? { name: String(updateError.name), message: String(updateError.message) } : null,
      canvas: capture.size,
      readError: capture.readError,
      rgba: capture.rgba,
      shadow: capture.shadow
    };
  });
  container.remove();
  return results;
}

Object.assign(window, { heatmapRun, Heatmap, recorder });
