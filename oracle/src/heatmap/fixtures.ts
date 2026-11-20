import { Rng } from "../corpus/rng";

// Heatmap.update fixtures, shared by the genHeatmap entry (records the setData
// arguments and store contents under a fake DOM) and the canvasMain "heatmap" mode
// (records the same plus the real canvas pixels). The corpus Rng keeps the dense
// cases identical on both sides.

export interface HeatmapUpdate {
  translations: [number, number][];
  pixelDimensions: [number, number];
  fieldDimensions: [number, number];
}

export interface HeatmapFixture {
  name: string;
  updates: HeatmapUpdate[];
  // A negative field height makes the grid loop push empty columns until V8 gives up: a
  // RangeError where the heap allows it (the vector), a renderer OOM crash on smaller hosts.
  // canvasMain leaves these out.
  nodeOnly?: boolean;
}

const FIELD: [number, number] = [16.54, 8.21];

export function heatmapFixtures(): HeatmapFixture[] {
  return [
    {
      name: "basic",
      updates: [
        {
          translations: [
            [0, 0],
            [0.01, 0.02],
            [4, -2],
            [-4, 2],
            [8, 4],
            [8.27, 4.1]
          ],
          pixelDimensions: [800, 400],
          fieldDimensions: FIELD
        }
      ]
    },
    {
      name: "empty",
      updates: [
        { translations: [], pixelDimensions: [640, 320], fieldDimensions: FIELD },
        { translations: [[1, 1]], pixelDimensions: [640, 320], fieldDimensions: FIELD },
        { translations: [], pixelDimensions: [640, 320], fieldDimensions: FIELD }
      ]
    },
    {
      name: "unchanged",
      updates: [
        { translations: [[1, 1], [2, -1]], pixelDimensions: [800, 400], fieldDimensions: FIELD },
        { translations: [[1, 1], [2, -1]], pixelDimensions: [800, 400], fieldDimensions: FIELD },
        { translations: [[1, 1], [2, -1], [3, 0]], pixelDimensions: [800, 400], fieldDimensions: FIELD },
        { translations: [[1, 1], [2, -1]], pixelDimensions: [800, 400], fieldDimensions: FIELD }
      ]
    },
    {
      name: "recreate",
      updates: [
        { translations: [[0, 0], [1, 1]], pixelDimensions: [800, 400], fieldDimensions: FIELD },
        { translations: [[0, 0], [1, 1]], pixelDimensions: [400, 200], fieldDimensions: FIELD },
        { translations: [[0, 0], [1, 1]], pixelDimensions: [400, 200], fieldDimensions: [16.54, 4] },
        { translations: [[0, 0], [1, 1]], pixelDimensions: [400, 200], fieldDimensions: FIELD }
      ]
    },
    {
      name: "fractional-pixels",
      updates: [
        {
          translations: [[0, 0], [3, -2], [-5, 3]],
          pixelDimensions: [817.6, 403.2],
          fieldDimensions: FIELD
        },
        {
          translations: [[0, 0], [3, -2], [-5, 3]],
          pixelDimensions: [817.5, 403.5],
          fieldDimensions: FIELD
        }
      ]
    },
    {
      name: "fractional-field",
      updates: [
        {
          translations: [[0, 0], [1.5, -0.5], [-2.25, 1.75]],
          pixelDimensions: [600, 300],
          fieldDimensions: [16.4892, 8.0693]
        }
      ]
    },
    {
      name: "edges",
      updates: [
        {
          translations: [
            [8.27, 4.105],
            [-8.27, -4.105],
            [8.28, 4.2],
            [-9, 5],
            [0, 4.105],
            [0, 4.105 + 0.0821],
            [-0, -0],
            [1e30, 1e30],
            [-1e30, -1e30],
            [NaN, 0],
            [0, NaN],
            [Infinity, 0],
            [0, -Infinity],
            [0.0409999999, 0],
            [0.0410000001, 0]
          ],
          pixelDimensions: [800, 400],
          fieldDimensions: FIELD
        }
      ]
    },
    {
      name: "dense",
      updates: [
        {
          translations: denseTranslations(),
          pixelDimensions: [800, 400],
          fieldDimensions: FIELD
        }
      ]
    },
    {
      name: "dense-fractional",
      updates: [
        {
          translations: denseTranslations(),
          pixelDimensions: [817.5, 403.5],
          fieldDimensions: FIELD
        }
      ]
    },
    {
      name: "sub-pixel-radius",
      updates: [
        { translations: [[0, 0], [1, 1]], pixelDimensions: [40, 4], fieldDimensions: FIELD },
        { translations: [[0, 0]], pixelDimensions: [40, 4], fieldDimensions: FIELD },
        { translations: [[0, 0]], pixelDimensions: [40, 6], fieldDimensions: FIELD }
      ]
    },
    {
      name: "tiny",
      updates: [
        {
          translations: [[0, 0], [0.5, 0.5], [-1, -1], [1, 1], [1, 1]],
          pixelDimensions: [48, 24],
          fieldDimensions: FIELD
        }
      ]
    },
    {
      name: "large",
      updates: [
        {
          translations: denseTranslations(),
          pixelDimensions: [1600, 800],
          fieldDimensions: FIELD
        }
      ]
    },
    {
      name: "zero-width",
      updates: [
        { translations: [], pixelDimensions: [0, 400], fieldDimensions: FIELD },
        { translations: [[0, 0]], pixelDimensions: [0, 400], fieldDimensions: FIELD },
        { translations: [[0, 0]], pixelDimensions: [400, 0], fieldDimensions: FIELD }
      ]
    },
    {
      name: "nan-dims",
      updates: [
        { translations: [[0, 0], [1, 1]], pixelDimensions: [NaN, 400], fieldDimensions: FIELD },
        { translations: [[0, 0], [1, 1]], pixelDimensions: [NaN, 400], fieldDimensions: FIELD },
        { translations: [[0, 0], [1, 1]], pixelDimensions: [800, NaN], fieldDimensions: FIELD }
      ]
    },
    {
      name: "field-extremes",
      updates: [
        { translations: [[0, 0]], pixelDimensions: [400, 200], fieldDimensions: [0, 0] },
        { translations: [[0, 0]], pixelDimensions: [400, 200], fieldDimensions: [0, 8.21] },
        { translations: [[0, 0]], pixelDimensions: [400, 200], fieldDimensions: [NaN, 8.21] }
      ]
    },
    {
      name: "negative-field-height",
      nodeOnly: true,
      updates: [
        { translations: [[0, 0]], pixelDimensions: [400, 200], fieldDimensions: [16.54, -1] },
        { translations: [[0, 0]], pixelDimensions: [400, 200], fieldDimensions: [NaN, 8.21] }
      ]
    },
    {
      name: "small-field",
      updates: [
        {
          translations: [[0.01, 0.01], [-0.02, -0.01], [0, 0]],
          pixelDimensions: [400, 200],
          fieldDimensions: [0.05, 0.04]
        }
      ]
    },
    {
      name: "single",
      updates: [
        { translations: [[0, 0]], pixelDimensions: [800, 400], fieldDimensions: FIELD }
      ]
    }
  ];
}

function denseTranslations(): [number, number][] {
  const rng = new Rng(0x68ea7a10, "heatmap/dense");
  const points: [number, number][] = [];
  for (let i = 0; i < 2400; i++) {
    const x = (rng.u32() / 0xffffffff) * 17.04 - 8.52;
    const y = (rng.u32() / 0xffffffff) * 8.71 - 4.355;
    points.push([x, y]);
  }
  for (let i = 0; i < 40; i++) {
    points.push([2.0 + (rng.u32() % 1000) / 1e6, 1.5 - (rng.u32() % 1000) / 1e6]);
  }
  return points;
}
