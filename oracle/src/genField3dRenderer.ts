import "./shims";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import * as THREE from "three";
import { parseField3d, parseRobot } from "@upstream/main/assetUtil";
import { UPSTREAM_DIR } from "./shims";
import { toTree } from "./cve1";
import { encodePath, runWorker } from "./glb/worker";
import type { Env, WorkerAnswers } from "./lazyHelpers/field3dRendererRun";

// camera-controls reads navigator.platform once at module load to pick its wheel scale.
Object.defineProperty(globalThis, "navigator", { value: { platform: "MacIntel" }, configurable: true, writable: true });
const { run } = await import("./lazyHelpers/field3dRendererRun");

// Ground truth for asrs-field3d's renderer orchestration (Field3dRenderer.ts and
// Field3dRendererImpl.ts), run through `oracle:field3dRendererRun`. One record per case:
//
//   {"case","setup","assets","ops","out":[state...]}
//
// assets lists the field and robot assets: {id, kind: "field"|"robot", config: <config.json
// text>, files: [[name, {"bundled": <path under upstream>}]...], never}; setup.assetSets maps
// each window.assets key to {field3ds: [ids], robots: [ids]}. Worker requests are answered
// with what upstream's real bundles/loadField.js and bundles/loadRobot.js return for the asset
// and mode, or never when that worker never answers.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

type FileSource = { bundled: string };
type Asset = { id: string; kind: "field" | "robot"; config: string; files: [string, FileSource][] };

const KITBOT = "bundledAssets/Robot_FRCKitBot";
const DRIVE_BASE = "bundledAssets/Robot_FTCSimpleDriveBase";
const kitbotText = fs.readFileSync(path.join(UPSTREAM_DIR, KITBOT, "config.json"), "utf8");
const driveBaseText = fs.readFileSync(path.join(UPSTREAM_DIR, DRIVE_BASE, "config.json"), "utf8");
const kitbot = JSON.parse(kitbotText);

const glbField = {
  name: "Test Field",
  isFTC: false,
  coordinateSystem: "wall-alliance",
  rotations: [{ axis: "z", degrees: 90 }],
  position: [0.5, -0.25, 0],
  widthInches: 600,
  heightInches: 300,
  driverStations: [
    [-7.5, 2],
    [-7.5, 0],
    [-7.5, -2],
    [7.5, 2],
    [7.5, 0],
    [7.5, -2]
  ],
  gamePieces: [{ name: "Piece", rotations: [{ axis: "x", degrees: 90 }], position: [0, 0, 0.1], stagedObjects: ["Missing"] }],
  aprilTags: []
};

const ASSETS: Asset[] = [
  { id: "kitbot", kind: "robot", config: kitbotText, files: [["model.glb", { bundled: `${KITBOT}/model.glb` }]] },
  {
    id: "kitbot-2cam",
    kind: "robot",
    config: JSON.stringify({ ...kitbot, cameras: kitbot.cameras.slice(0, 2) }),
    files: [["model.glb", { bundled: `${KITBOT}/model.glb` }]]
  },
  { id: "drive-base", kind: "robot", config: driveBaseText, files: [["model.glb", { bundled: `${DRIVE_BASE}/model.glb` }]] },
  {
    id: "glb-field",
    kind: "field",
    config: JSON.stringify(glbField),
    files: [
      ["model.glb", { bundled: `${KITBOT}/model.glb` }],
      ["model_0.glb", { bundled: `${DRIVE_BASE}/model.glb` }]
    ]
  },
  {
    id: "glb-field-moved",
    kind: "field",
    config: JSON.stringify({ ...glbField, position: [1, 0, 0] }),
    files: [
      ["model.glb", { bundled: `${KITBOT}/model.glb` }],
      ["model_0.glb", { bundled: `${DRIVE_BASE}/model.glb` }]
    ]
  },
  { id: "missing-field", kind: "field", config: JSON.stringify({ ...glbField, name: "Missing Field", gamePieces: [] }), files: [] }
];

const FIELD_IDS: { [id: string]: string } = { "glb-field": "glb-field", "glb-field-moved": "glb-field", "missing-field": "missing-field" };

const ASSET_SETS: { [key: string]: { field3ds: string[]; robots: string[] } } = {
  all: { field3ds: ["glb-field", "missing-field"], robots: ["kitbot", "drive-base"] },
  moved: { field3ds: ["glb-field-moved", "missing-field"], robots: ["kitbot", "drive-base"] },
  cams: { field3ds: ["glb-field", "missing-field"], robots: ["kitbot-2cam", "drive-base"] }
};

const MODES = {
  standard: [0x000000, 0],
  cinematic: [0x666666, 100],
  "low-power": [0x000000, 0]
} as const;

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-renderer-"));
process.on("exit", () => fs.rmSync(scratch, { recursive: true, force: true }));

const configs = new Map<string, any>();
for (const asset of ASSETS) {
  const dir = path.join(scratch, asset.id);
  fs.mkdirSync(dir);
  for (const [name, source] of asset.files) fs.copyFileSync(path.join(UPSTREAM_DIR, source.bundled), path.join(dir, name));
  const raw = JSON.parse(asset.config);
  const config: any = asset.kind === "robot" ? parseRobot(raw) : parseField3d(raw);
  if (typeof config !== "object") throw new Error(`asset ${asset.id} does not parse`);
  config.path = encodePath(path.join(dir, "model.glb"));
  if (asset.kind === "field") config.id = FIELD_IDS[asset.id];
  configs.set(asset.id, config);
}

const assetSets: { [key: string]: unknown } = {};
for (const [key, set] of Object.entries(ASSET_SETS)) {
  assetSets[key] = {
    field2ds: [],
    field3ds: set.field3ds.map((id) => configs.get(id)),
    robots: set.robots.map((id) => configs.get(id)),
    joysticks: [],
    loadFailures: []
  };
}

const results = new Map<string, unknown | null>();
const never = new Map<string, boolean>();

async function workerResult(script: string, payload: any): Promise<unknown | null> {
  const config = script === "loadField.js" ? payload.fieldConfig : payload.robotConfig;
  const key = JSON.stringify([script, config.path, payload.mode]);
  if (!results.has(key)) {
    const result = await runWorker(script, payload);
    results.set(key, "never" in result ? null : result.ok);
  }
  return results.get(key)!;
}

async function prepareAnswers(modes: (keyof typeof MODES)[]): Promise<WorkerAnswers> {
  const table = new Map<string, unknown | null>();
  for (const mode of modes) {
    const [specular, shininess] = MODES[mode];
    const common = { mode, materialSpecular: new THREE.Color(specular).toArray(), materialShininess: shininess };
    for (const asset of ASSETS) {
      const config = configs.get(asset.id);
      const payload =
        asset.kind === "field"
          ? { fieldConfig: config, ...common }
          : { robotConfig: config, isFTC: config.isFTC, ...common };
      const result = await workerResult(asset.kind === "field" ? "loadField.js" : "loadRobot.js", payload);
      never.set(asset.id, result === null);
      table.set(JSON.stringify([asset.kind === "field" ? config.id : config.name, mode, path.dirname(config.path)]), result);
    }
  }
  const answer = (kind: "field" | "robot") => (payload: any) => {
    const config = kind === "field" ? payload.fieldConfig : payload.robotConfig;
    const key = JSON.stringify([kind === "field" ? config.id : config.name, payload.mode, path.dirname(config.path)]);
    if (!table.has(key)) throw new Error(`genField3dRenderer: no worker result for ${key}`);
    return table.get(key)!;
  };
  return { "../bundles/loadField.js": answer("field"), "../bundles/loadRobot.js": answer("robot") };
}

let seed = 0x7e3d_0909;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const uniform = (lo: number, hi: number) => lo + (hi - lo) * rng();

function rotation(): number[] {
  const q = [uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)];
  const n = Math.hypot(...q);
  return q.map((v) => v / n);
}

const yaw = (radians: number) => [Math.cos(radians / 2), 0, 0, Math.sin(radians / 2)];

function pose(translation?: number[], rot?: number[]): any {
  return {
    pose: { translation: translation ?? [uniform(-7, 7), uniform(-3.5, 3.5), 0], rotation: rot ?? yaw(uniform(-3, 3)) },
    annotation: { is2DSource: false }
  };
}

const poses = (n: number) => Array.from({ length: n }, () => pose());

function robot(model: string, posesList: any[], extra: object = {}): any {
  return {
    type: "robot",
    model,
    poses: posesList,
    components: [],
    mechanisms: { xz: null, yz: null },
    visionTargets: [],
    swerveModuleVelocities: [],
    ...extra
  };
}

function ghost(model: string, posesList: any[], color: string): any {
  return { ...robot(model, posesList), type: "ghost", color };
}

const gamePiece = (variant: string, n: number) => ({ type: "gamePiece", variant, poses: poses(n) });
const trajectory = (n: number) => ({ type: "trajectory", color: "#ff8800", size: "normal", poses: poses(n) });
const heatmap = (n: number) => ({ type: "heatmap", poses: poses(n) });
const wallHeatmap = (n: number) => ({
  type: "heatmap",
  poses: Array.from({ length: n }, () => pose([uniform(1, 14), uniform(1, 6.5), 0]))
});
const aprilTags = (type: "aprilTag" | "aprilTagBuiltIn", ids: number[]) => ({
  type,
  poses: ids.map((id) => ({ ...pose([uniform(-7, 7), uniform(-3.5, 3.5), uniform(0.2, 1.5)], rotation()), annotation: { aprilTagId: id, is2DSource: false } })),
  variant: { family: "36h11", inches: 6.5 }
});
const axes = (n: number) => ({ type: "axes", poses: poses(n) });
const cone = (n: number, position = "center") => ({ type: "cone", color: "#00ff00", position, poses: poses(n) });

type Command = {
  field: string;
  isRedAlliance: boolean;
  coordinateSystem: string;
  objects: any[];
  cameraOverride: any;
  autoDriverStation: number;
  allRobotModels: string[];
};

function command(field: string, objects: any[] = [], extra: Partial<Command> = {}): Command {
  return {
    field,
    isRedAlliance: false,
    coordinateSystem: "center-red",
    objects,
    cameraOverride: null,
    autoDriverStation: -1,
    allRobotModels: objects.filter((o) => o.type === "robot" || o.type === "ghost").map((o) => o.model),
    ...extra
  };
}

const ENV: Env = {
  width: 800,
  height: 450,
  parentWidth: 800,
  parentHeight: 450,
  containerHeight: 450,
  dpr: 2,
  dark: true,
  reducedMotion: false
};

const prefs = (mode: string, extra: object = {}) => ({
  field3dModeAc: mode,
  field3dModeBattery: "",
  field3dAntialiasing: true,
  coordinateSystem: "automatic",
  ...extra
});

function frames(ops: unknown[][], cmd: Command, count: number, ms = 16) {
  for (let i = 0; i < count; i++) {
    ops.push(["advance", ms], ["render", cmd]);
  }
}

function hold(ops: unknown[][], cmd: Command, code: string, count: number, opts: object = {}) {
  ops.push(["key", "down", code, opts]);
  frames(ops, cmd, count);
  ops.push(["key", "up", code, {}]);
  frames(ops, cmd, 1);
}

async function emit(name: string, setup: any, ops: unknown[][], modes: (keyof typeof MODES)[]) {
  const answers = await prepareAnswers(modes);
  const states = await run(setup, ops, assetSets, answers);
  const assets = ASSETS.map((asset) => ({ ...asset, fieldId: FIELD_IDS[asset.id] ?? null, never: never.get(asset.id) }));
  out({ case: name, setup: toTree({ ...setup, assetSets: ASSET_SETS }), assets: toTree(assets), ops: toTree(ops), out: states });
}

const baseSetup = (mode: string, extra: object = {}) => ({
  preferences: prefs(mode),
  isBattery: false,
  platform: "darwin",
  assets: "all",
  env: ENV,
  now: 1000,
  ...extra
});

// Orbit cameras, keyboard navigation, FOV, the camera menu and manager reconciliation.
function orbitOps(): unknown[][] {
  const ops: unknown[][] = [];
  const empty = command("FRC:Evergreen");
  ops.push(["render", empty]);
  frames(ops, empty, 6, 50);
  hold(ops, empty, "KeyW", 3);
  hold(ops, empty, "KeyS", 2);
  hold(ops, empty, "KeyA", 2);
  hold(ops, empty, "KeyD", 2);
  hold(ops, empty, "KeyQ", 2);
  hold(ops, empty, "KeyE", 2);
  hold(ops, empty, "KeyI", 2);
  hold(ops, empty, "KeyK", 2);
  hold(ops, empty, "KeyJ", 2);
  hold(ops, empty, "KeyL", 2);
  hold(ops, empty, "KeyW", 2, { meta: true });
  hold(ops, empty, "KeyW", 2, { editable: "input" });
  hold(ops, empty, "KeyA", 2, { editable: "content" });
  ops.push(["key", "down", "KeyD", {}], ["key", "up", "KeyD", { editable: "textarea" }]);
  frames(ops, empty, 2);
  ops.push(["key", "up", "KeyD", {}]);
  ops.push(["env", { containerHeight: 0 }]);
  hold(ops, empty, "KeyE", 2);
  ops.push(["env", { containerHeight: 450 }]);
  for (let i = 0; i < 30; i++) {
    ops.push(["key", "down", "KeyW", {}], ["advance", 100], ["render", empty]);
  }
  ops.push(["key", "up", "KeyW", {}]);
  frames(ops, empty, 2);
  ops.push(["wheel", 100, 0]);
  frames(ops, empty, 3);
  ops.push(["wheel", -3, 1]);
  frames(ops, empty, 3);
  ops.push(["pointer", "down", 1, 1, 400, 200]);
  frames(ops, empty, 1);
  ops.push(["pointer", "move", 1, 1, 430, 215]);
  frames(ops, empty, 2);
  ops.push(["pointer", "move", 1, 1, 470, 190], ["pointer", "up", 1, 0, 470, 190]);
  frames(ops, empty, 4);
  ops.push(["pointer", "down", 2, 2, 300, 300], ["pointer", "move", 2, 2, 320, 280]);
  frames(ops, empty, 2);
  ops.push(["pointer", "up", 2, 0, 320, 280]);
  frames(ops, empty, 3);
  ops.push(["fov", 75], ["render", empty], ["fov", 500], ["render", empty], ["fov", -20], ["render", empty], ["fov", 50]);
  ops.push(["mouse", "down", 2, 100, 120], ["mouse", "up", 2, 100, 120]);
  ops.push(["mouse", "down", 2, 100, 120], ["mouse", "up", 2, 101, 120]);
  ops.push(["mouse", "down", 2, 100, 120], ["mouse", "up", 2, 100, 121]);
  ops.push(["mouse", "down", 0, 10, 10], ["mouse", "up", 0, 10, 10]);

  const kit = pose([2, 1, 0], yaw(0.7));
  const objects1 = [
    robot("KitBot", [kit]),
    trajectory(5),
    aprilTags("aprilTag", [1, 2, 3]),
    aprilTags("aprilTagBuiltIn", [7]),
    axes(2),
    cone(2, "front"),
    heatmap(20),
    gamePiece("Note", 2)
  ];
  const c1 = command("FRC:Evergreen", objects1, { isRedAlliance: true });
  ops.push(["render", c1]);
  ops.push(["mouse", "down", 2, 5, 6], ["mouse", "up", 2, 5, 6]);
  ops.push(["loadTextures"]);
  frames(ops, c1, 1);
  ops.push(["load"]);
  frames(ops, c1, 1);
  frames(ops, c1, 1);
  const c2 = command("FRC:Evergreen", [robot("KitBot", [pose([3, 1, 0], yaw(0.9))]), ghost("KitBot", [pose()], "#ff0000"), axes(3), trajectory(4), axes(1)]);
  ops.push(["render", c2]);
  ops.push(["load"]);
  frames(ops, c2, 1);
  const c3 = command(
    "FRC:Evergreen",
    [robot("Drive Base (Simple)", [pose()]), robot("KitBot", [pose([1, -1, 0], yaw(-0.4))])],
    { allRobotModels: ["Drive Base (Simple)", "KitBot"] }
  );
  ops.push(["render", c3]);
  ops.push(["load"]);
  frames(ops, c3, 1);
  const c4 = command("FRC:Evergreen", [robot("KitBot", [pose([1, -1, 0], yaw(-0.4))])], { allRobotModels: ["Drive Base (Simple)", "KitBot"] });
  frames(ops, c4, 1);
  const c5 = command("FRC:Evergreen", [robot("KitBot", [pose([1, -1, 0], yaw(-0.4))])], { allRobotModels: ["KitBot"] });
  frames(ops, c5, 1);

  // Orbit robot follows the primary robot.
  ops.push(["camera", -2]);
  frames(ops, c5, 4, 40);
  const moved = command("FRC:Evergreen", [robot("KitBot", [pose([4, 2, 0.1], yaw(1.2))])]);
  frames(ops, moved, 2, 40);
  const noPose = command("FRC:Evergreen", [robot("KitBot", [])]);
  frames(ops, noPose, 2);
  frames(ops, moved, 2);

  // Driver Station cameras.
  ops.push(["camera", -3]);
  frames(ops, moved, 2);
  const auto2 = command("FRC:Evergreen", moved.objects, { autoDriverStation: 2 });
  frames(ops, auto2, 3, 40);
  const auto4 = command("FRC:Evergreen", moved.objects, { autoDriverStation: 4 });
  frames(ops, auto4, 3, 40);
  for (const index of [-4, -5, -6, -7, -8, -9]) {
    ops.push(["camera", index]);
    frames(ops, auto4, 2, 40);
  }
  ops.push(["camera", -10]);
  frames(ops, auto4, 2);

  // Fixed robot cameras.
  ops.push(["camera", 0]);
  frames(ops, auto4, 2);
  ops.push(["env", { parentWidth: 500, parentHeight: 500 }]);
  frames(ops, auto4, 1);
  ops.push(["env", { width: 500, height: 375, parentWidth: 500, parentHeight: 500 }]);
  frames(ops, auto4, 1);
  ops.push(["env", { parentWidth: 777, parentHeight: 501 }]);
  frames(ops, auto4, 1);
  ops.push(["env", { parentWidth: 333, parentHeight: 701 }]);
  frames(ops, auto4, 1);
  ops.push(["wheel", 100, 0], ["pointer", "down", 3, 1, 100, 100], ["pointer", "move", 3, 1, 150, 120], ["pointer", "up", 3, 0, 150, 120]);
  frames(ops, auto4, 2);
  ops.push(["camera", 2]);
  frames(ops, auto4, 1);
  const override = command("FRC:Evergreen", auto4.objects, { cameraOverride: pose([1, 2, 1.5], rotation()) });
  frames(ops, override, 1);
  frames(ops, noPose, 1);
  ops.push(["mouse", "down", 2, 50, 60], ["mouse", "up", 2, 50, 60]);
  ops.push(["assets", "cams"]);
  frames(ops, auto4, 1);
  ops.push(["load"]);
  frames(ops, auto4, 1);
  ops.push(["camera", 1]);
  frames(ops, auto4, 1);
  ops.push(["env", { width: 800, height: 450, parentWidth: 800, parentHeight: 450 }]);
  ops.push(["camera", -1]);
  frames(ops, auto4, 4, 40);
  ops.push(["camera", 0]);
  frames(ops, command("FRC:Evergreen"), 1);
  ops.push(["camera", -1]);
  ops.push(["assets", "all"]);
  frames(ops, auto4, 1);
  ops.push(["load"]);
  frames(ops, auto4, 1);

  // Coordinate systems, theme, size and pixel ratio.
  for (const [system, red] of [
    ["wall-alliance", false],
    ["wall-alliance", true],
    ["wall-blue", true],
    ["center-rotated", false],
    ["center-red", true]
  ] as const) {
    const c = command("FRC:Evergreen", [robot("KitBot", [pose([2, 1, 0], yaw(0.3))]), heatmap(10)], {
      coordinateSystem: system,
      isRedAlliance: red
    });
    frames(ops, c, 1);
  }
  const plain = command("FRC:Evergreen", [robot("KitBot", [pose([2, 1, 0], yaw(0.3))])]);
  frames(ops, plain, 1);
  ops.push(["env", { dark: false }]);
  frames(ops, plain, 1);
  ops.push(["env", { width: 640, height: 400, parentWidth: 640, parentHeight: 400 }]);
  frames(ops, plain, 1);
  ops.push(["env", { dpr: 1 }]);
  frames(ops, plain, 1);
  ops.push(["env", { dpr: 1.25 }]);
  frames(ops, plain, 2);
  frames(ops, plain, 1);
  ops.push(["env", { width: 0, height: 0 }]);
  frames(ops, plain, 2);
  ops.push(["env", { width: 640, height: 400 }]);
  frames(ops, plain, 1);
  frames(ops, command("FRC:Evergreen"), 1);
  return ops;
}

// FTC fields, the grid, FRC/FTC switches, axes fields with coordinate system changes and the
// FTC Driver Station camera.
function ftcOps(): unknown[][] {
  const ops: unknown[][] = [];
  const ftc = command("FTC:Evergreen", [robot("Drive Base (Simple)", [pose([0.5, 0.4, 0], yaw(0.2))])]);
  ops.push(["render", ftc]);
  ops.push(["load"]);
  frames(ops, ftc, 4, 40);
  ops.push(["camera", -3]);
  const ds = command("FTC:Evergreen", ftc.objects, { autoDriverStation: 1 });
  frames(ops, ds, 3, 40);
  ops.push(["camera", -5]);
  frames(ops, ds, 2, 40);
  ops.push(["camera", -2]);
  frames(ops, ds, 3, 40);
  ops.push(["camera", -1]);
  frames(ops, ds, 2, 40);
  frames(ops, command("FRC:Evergreen", ftc.objects), 3, 40);
  frames(ops, command("FTC:Axes", ftc.objects, { coordinateSystem: "center-red" }), 2, 40);
  frames(ops, command("FTC:Axes", ftc.objects, { coordinateSystem: "wall-blue" }), 2);
  frames(ops, command("FTC:Axes", ftc.objects, { coordinateSystem: "wall-alliance", isRedAlliance: true }), 2);
  frames(ops, command("FRC:Axes", ftc.objects, { coordinateSystem: "center-rotated" }), 2);
  frames(ops, command("FRC:Axes", ftc.objects, { coordinateSystem: "center-red" }), 2);
  ops.push(["camera", -4]);
  frames(ops, command("FTC:Evergreen", ftc.objects), 2);
  frames(ops, command("Unknown:Field", ftc.objects), 2);
  ops.push(["assets", null]);
  frames(ops, command("FTC:Evergreen", ftc.objects), 2);
  ops.push(["mouse", "down", 2, 1, 1], ["mouse", "up", 2, 1, 1]);
  ops.push(["assets", "all"]);
  frames(ops, command("FTC:Evergreen", ftc.objects), 2);
  ops.push(["mouse", "down", 2, 1, 1], ["mouse", "up", 2, 1, 1]);
  return ops;
}

// A field model from a worker, its game pieces and staged pieces, and asset changes.
function glbFieldOps(): unknown[][] {
  const ops: unknown[][] = [];
  const pieces = (n: number) =>
    command("glb-field", [robot("KitBot", [pose()]), gamePiece("Piece", n), wallHeatmap(6)], { coordinateSystem: "wall-alliance" });
  ops.push(["render", pieces(2)]);
  frames(ops, pieces(3), 2);
  ops.push(["load"]);
  frames(ops, pieces(3), 2);
  frames(ops, command("glb-field", [robot("KitBot", [pose()]), wallHeatmap(6)], { coordinateSystem: "wall-alliance", isRedAlliance: true }), 1);
  frames(ops, pieces(5), 1);
  ops.push(["camera", -6]);
  frames(ops, pieces(5), 2, 40);
  ops.push(["assets", "moved"]);
  frames(ops, pieces(1), 2);
  frames(ops, command("FRC:Evergreen", [gamePiece("Piece", 2)]), 2);
  ops.push(["load"]);
  frames(ops, command("FRC:Evergreen", [gamePiece("Piece", 2)]), 2);
  frames(ops, command("FRC:Evergreen", [gamePiece("Piece", 2)]), 1);
  frames(ops, command("missing-field", [robot("KitBot", [pose()])]), 3);
  ops.push(["load"]);
  frames(ops, command("missing-field", [robot("KitBot", [pose()])]), 2);
  frames(ops, command("FRC:Evergreen", [robot("KitBot", [pose()])]), 2);
  ops.push(["advance", 300]);
  frames(ops, command("FRC:Evergreen", [robot("KitBot", [pose()])]), 1);
  return ops;
}

// Saved state, restore edge cases, reduced motion and implementation swaps.
function restoreOps(): unknown[][] {
  const ops: unknown[][] = [];
  const c = command("FRC:Evergreen", [robot("KitBot", [pose([1, 1, 0], yaw(0.5))])]);
  ops.push(["restore", { cameraIndex: -2, orbitFov: 70, cameraPosition: [3, 2, 1], cameraTarget: [0, 0.5, 0] }]);
  frames(ops, c, 2);
  ops.push(["restore", { cameraIndex: -1, orbitFov: 40, cameraPosition: [0, 0, 0], cameraTarget: [1, 1, 1] }]);
  frames(ops, c, 3, 40);
  ops.push(["restore", { cameraIndex: "x", orbitFov: null, cameraPosition: [1, 2], cameraTarget: [1, 1, 1] }]);
  frames(ops, c, 2, 40);
  ops.push(["restore", null], ["restore", 5], ["restore", { cameraIndex: 1, cameraPosition: [5, 5, 5], cameraTarget: [0, 0, 1] }]);
  frames(ops, c, 2);
  ops.push(["load"]);
  frames(ops, c, 2);
  ops.push(["preferences", prefs("standard", { field3dAntialiasing: false })]);
  frames(ops, c, 2);
  ops.push(["load"]);
  frames(ops, c, 2);
  ops.push(["preferences", prefs("standard", { field3dModeBattery: "low-power" })]);
  frames(ops, c, 1);
  ops.push(["battery", true]);
  frames(ops, c, 3, 40);
  ops.push(["load"]);
  frames(ops, c, 3, 40);
  ops.push(["battery", false]);
  frames(ops, c, 2, 40);
  ops.push(["env", { reducedMotion: true }]);
  ops.push(["camera", -3]);
  frames(ops, command("FRC:Evergreen", c.objects, { autoDriverStation: 3 }), 2);
  frames(ops, command("FTC:Evergreen", c.objects, { autoDriverStation: 3 }), 2);
  ops.push(["camera", -4.5]);
  frames(ops, command("FTC:Evergreen", c.objects, { autoDriverStation: 3 }), 2);
  ops.push(["camera", -1]);
  frames(ops, command("FTC:Evergreen", c.objects, { autoDriverStation: 3 }), 1);
  return ops;
}

function deferredOps(): unknown[][] {
  const ops: unknown[][] = [];
  const c = command("FRC:Evergreen", [robot("KitBot", [pose([1, 1, 0], yaw(0.5))])]);
  ops.push(["render", c], ["restore", { cameraIndex: -2, orbitFov: 60, cameraPosition: [4, 3, 2], cameraTarget: [0, 0, 0] }]);
  ops.push(["restore", { cameraIndex: -1, orbitFov: 30, cameraPosition: [2, 3, 4], cameraTarget: [0, 1, 0] }]);
  ops.push(["preferences", prefs("cinematic")]);
  frames(ops, c, 3, 40);
  ops.push(["load"]);
  frames(ops, c, 2);
  return ops;
}

// Cinematic lights follow FRC/FTC switches.
function cinematicOps(): unknown[][] {
  const ops: unknown[][] = [];
  const frc = command("FRC:Evergreen", [robot("KitBot", [pose([1, 0.5, 0], yaw(0.25))])]);
  frames(ops, frc, 2);
  ops.push(["load"]);
  frames(ops, frc, 1);
  frames(ops, command("FTC:Evergreen", [robot("Drive Base (Simple)", [pose([0.3, 0.2, 0], yaw(1))])]), 2, 40);
  ops.push(["load"]);
  frames(ops, command("FTC:Axes", []), 2, 40);
  frames(ops, frc, 2, 40);
  ops.push(["env", { dark: false, dpr: 1 }]);
  frames(ops, frc, 1);
  return ops;
}

// Low power mode: the 30 fps cap and the 0.75 pixel ratio.
function lowPowerOps(): unknown[][] {
  const ops: unknown[][] = [];
  const c = command("FRC:Evergreen", [robot("KitBot", [pose([1, 0.5, 0], yaw(0.25))])]);
  ops.push(["render", c]);
  frames(ops, c, 6, 10);
  ops.push(["load"]);
  frames(ops, c, 4, 20);
  ops.push(["env", { dpr: 1 }]);
  frames(ops, c, 3, 40);
  ops.push(["env", { width: 801, height: 451, parentWidth: 801, parentHeight: 451 }]);
  frames(ops, c, 3, 40);
  ops.push(["key", "down", "KeyA", {}]);
  frames(ops, c, 4, 12);
  ops.push(["key", "up", "KeyA", {}]);
  frames(ops, c, 2, 40);
  return ops;
}

// A manager that throws leaves the rest of render() undone; a robot manager disposed while its
// model loads still ends its load.
function throwOps(): unknown[][] {
  const ops: unknown[][] = [];
  const ok = command("FRC:Evergreen", [robot("KitBot", [pose([1, 0.5, 0], yaw(0.25))]), axes(1)]);
  frames(ops, ok, 1);
  ops.push(["load"]);
  frames(ops, ok, 1);
  const bad = command("FRC:Evergreen", [
    robot("KitBot", [pose([1, 0.5, 0], yaw(0.25))], {
      swerveModuleVelocities: [{ values: [{ velocity: 1, angle: 0 }, undefined], color: "#ff0000" }]
    }),
    axes(2)
  ]);
  ops.push(["camera", -2]);
  frames(ops, bad, 2);
  frames(ops, ok, 2);
  frames(ops, command("FRC:Evergreen", [robot("Drive Base (Simple)", [pose()])]), 1);
  frames(ops, command("FRC:Evergreen", []), 1);
  ops.push(["load"]);
  frames(ops, command("FRC:Evergreen", []), 2);
  ops.push(["advance", 300]);
  frames(ops, command("FRC:Evergreen", []), 1);
  return ops;
}

await emit("orbit/standard", baseSetup("standard"), orbitOps(), ["standard"]);
await emit("ftc/standard", baseSetup("standard", { env: { ...ENV, dark: false, dpr: 1 } }), ftcOps(), ["standard"]);
await emit("glb-field/standard", baseSetup("standard"), glbFieldOps(), ["standard"]);
await emit("restore/standard", baseSetup("standard"), restoreOps(), ["standard", "low-power"]);
await emit("restore/deferred", baseSetup("standard", { preferences: undefined }), deferredOps(), ["cinematic"]);
await emit("cinematic", baseSetup("cinematic", { platform: "win32" }), cinematicOps(), ["cinematic"]);
await emit("low-power", baseSetup("low-power"), lowPowerOps(), ["low-power"]);
await emit("throws/standard", baseSetup("standard"), throwOps(), ["standard"]);
