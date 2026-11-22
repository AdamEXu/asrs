import "./shims";
import { toTree } from "./cve1";
import { run } from "./lazyHelpers/field3dManagersRun";

// Ground truth for asrs-field3d's object managers: each case runs one real upstream manager
// (src/shared/renderers/field3d/objectManagers) through a list of ops with
// `oracle:field3dManagersRun` and records the scene state after every op:
//
//   {"manager","case","setup","ops","out":[{children, extra[, throw]}...]}
//
// Values are CVE-1 trees.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

let seed = 0x7e3d_0707;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const uniform = (lo: number, hi: number) => lo + (hi - lo) * rng();
const below = (n: number) => Math.floor(rng() * n);

const SPECIAL_ROTATIONS = [
  [1, 0, 0, 0],
  [0, 0, 0, 1],
  [Math.SQRT1_2, 0, 0, Math.SQRT1_2],
  [0.5, -0.5, 0.5, -0.5],
  [2, 0.3, -1, 0.1],
  [0, 0, 0, 0]
];

function rotation(): number[] {
  if (rng() < 0.25) return SPECIAL_ROTATIONS[below(SPECIAL_ROTATIONS.length)];
  const q = [uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)];
  const n = Math.hypot(...q);
  return q.map((v) => v / n);
}

function pose(annotation: object = {}): object {
  return {
    pose: {
      translation: [uniform(-9, 9), uniform(-5, 5), uniform(-0.5, 2)],
      rotation: rotation()
    },
    annotation: { is2DSource: rng() < 0.5, ...annotation }
  };
}

const poses = (n: number, annotation: () => object = () => ({})) => Array.from({ length: n }, () => pose(annotation()));

const SPECULAR_STANDARD = 0x000000;
const SPECULAR_CINEMATIC = 0x666666;
const setups = [
  { mode: "standard", specular: SPECULAR_STANDARD, shininess: 0 },
  { mode: "cinematic", specular: SPECULAR_CINEMATIC, shininess: 100 },
  { mode: "low-power", specular: SPECULAR_STANDARD, shininess: 0 }
];

function emit(manager: string, name: string, setup: object, ops: unknown[][]) {
  const states = run(manager, setup, ops);
  out({ manager, case: name, setup: toTree(setup), ops: toTree(ops), out: toTree(states) });
}

const growth = [0, 1, 2, 3, 5, 4, 9, 2, 0, 1, 17];

// Axes: one instanced merged mesh; resizes at powers of two and parks unused instances.
for (const setup of setups) {
  emit(
    "axes",
    `axes/${setup.mode}`,
    setup,
    [...growth.map((n) => ["set", { type: "axes", poses: poses(n) }]), ["dispose"], ["set", { type: "axes", poses: poses(3) }]]
  );
}

// Game pieces: instances per variant; an unknown variant keeps the disposed instances.
const pieces = {
  Note: { geometry: ["TorusGeometry", 0.18, 0.025, 6, 12], color: 0xff8800 },
  Algae: { geometry: ["SphereGeometry", 0.2, 8, 6], color: 0x22aa88 },
  Coral: { geometry: ["CylinderGeometry", 0.05, 0.05, 0.3, 8], color: 0xeeeeee }
};
for (const setup of setups.slice(0, 2)) {
  const gp = (variant: string, n: number) => ["set", { type: "gamePiece", variant, poses: poses(n) }];
  emit("gamePiece", `game-piece/${setup.mode}`, { ...setup, pieces }, [
    gp("Note", 3),
    gp("Note", 5),
    gp("Algae", 1),
    gp("Algae", 0),
    gp("Missing", 2),
    gp("Missing", 9),
    gp("Missing", 1),
    gp("Coral", 4),
    gp("Note", 2),
    ["dispose"],
    gp("Note", 3)
  ]);
}
emit("gamePiece", "game-piece/first-missing", { ...setups[0], pieces }, [
  ["set", { type: "gamePiece", variant: "Missing", poses: poses(2) }],
  ["set", { type: "gamePiece", variant: "Missing", poses: poses(2) }],
  ["set", { type: "gamePiece", variant: "Algae", poses: poses(2) }]
]);

// AprilTags: one mesh per tag id, reused by id, textures arriving later (or never).
const tag = (id: number | undefined) => pose(id === undefined ? {} : { aprilTagId: id });
const tags = (ids: (number | undefined)[]) => ids.map(tag);
for (const setup of setups.slice(0, 2)) {
  const at = (family: string, inches: number, ids: (number | undefined)[], type = "aprilTag") => [
    "set",
    { type, poses: tags(ids), variant: { family, inches } }
  ];
  emit("aprilTag", `april-tag/${setup.mode}`, setup, [
    at("36h11", 6.5, [1, 2, 586]),
    ["loadTextures"],
    at("36h11", 6.5, [2, 1, 3, undefined]),
    at("36h11", 6.5, [2, 2, 2]),
    ["loadTextures"],
    at("36h11", 6.5, []),
    at("16h5", 6, [0, 29, 30, 7], "aprilTagBuiltIn"),
    at("16h5", 6, [0, 29]),
    ["loadTextures"],
    at("36h11", 2, [4, 1.5, -3, 1e21, 600, NaN]),
    ["loadTextures"],
    at("36h11", 3.25, [12, 12]),
    at("36h11", 6.5, [1, 2, 3]),
    ["dispose"]
  ]);
}

// Cones: canvas textures redrawn on colour changes, geometry shifted on position changes.
for (const setup of setups.slice(0, 2)) {
  const cone = (color: string, position: string, n: number) => ["set", { type: "cone", color, position, poses: poses(n) }];
  emit("cone", `cone/${setup.mode}`, setup, [
    cone("#ff0000", "center", 1),
    cone("#ff0000", "front", 3),
    cone("#00ffff", "back", 2),
    cone("notacolor", "back", 5),
    cone("#ff8c00", "center", 4),
    cone("rgb(10, 20, 30)", "front", 0),
    cone("#0f0", "front", 2),
    cone("#0f0", "sideways", 2),
    cone("#ff00ff", "center", 2),
    ["dispose"]
  ]);
}
emit("cone", "cone/blank", setups[0], [
  ["set", { type: "cone", color: "", position: "center", poses: poses(2) }],
  ["set", { type: "cone", color: "", position: "back", poses: poses(2) }],
  ["set", { type: "cone", color: "#0000ff", position: "back", poses: poses(2) }]
]);

// Trajectories: one Line2, hidden below two poses, 2D sources lifted onto the carpet.
{
  const tr = (color: string, size: string, n: number) => ["set", { type: "trajectory", color, size, poses: poses(n) }];
  emit("trajectory", "trajectory", setups[0], [
    ["resolution", 800, 600],
    tr("#ff8c00", "normal", 0),
    tr("#ff8c00", "normal", 1),
    tr("#00ff00", "normal", 2),
    tr("#00ff00", "bold", 5),
    tr("#0000ff", "bold", 5),
    tr("#f0f", "normal", 7),
    tr("rgb(10, 20, 30)", "bold", 3),
    tr("hsl(120, 50%, 50%)", "thin", 3),
    tr("red", "normal", 4),
    tr("notacolor", "normal", 4),
    tr("#ff0000", "bold", 1),
    tr("#ff0000", "bold", 2),
    ["dispose"]
  ]);
}

// Heatmaps: the field-sized plane is rebuilt whenever the heatmap recreates its canvas.
{
  const frc = { widthInches: 651.25, heightInches: 323.25, coordinateSystem: "wall-blue" };
  const ftc = { widthInches: 141.24, heightInches: 141.24, coordinateSystem: "center-rotated" };
  const systemcore = { widthInches: 651.25, heightInches: 323.25, coordinateSystem: "center-red" };
  const spread = (n: number) =>
    Array.from({ length: n }, () => ({
      pose: { translation: [uniform(0, 16), uniform(0, 8), uniform(0, 1)], rotation: rotation() },
      annotation: { is2DSource: true }
    }));
  const hm = (p: object[]) => ["set", { type: "heatmap", poses: p }];
  const first = spread(12);
  emit("heatmap", "heatmap", { ...setups[0], field: null, preferences: undefined }, [
    hm(first),
    ["field", frc],
    hm(first),
    hm(first),
    hm(spread(30)),
    ["redAlliance", true],
    hm(first),
    ["preferences", { coordinateSystem: "automatic" }],
    hm(first),
    ["preferences", { coordinateSystem: "center-rotated" }],
    hm(first),
    ["field", ftc],
    hm(spread(8)),
    ["preferences", { coordinateSystem: "wall-alliance" }],
    ["redAlliance", false],
    hm(spread(8)),
    ["field", systemcore],
    ["preferences", {}],
    hm([]),
    ["dispose"]
  ]);
}
