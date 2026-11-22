import "./shims";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import * as THREE from "three";
import { parseRobot } from "@upstream/main/assetUtil";
import { UPSTREAM_DIR } from "./shims";
import { toTree } from "./cve1";
import { encodePath, runWorker } from "./glb/worker";
import { RobotSetup, run } from "./lazyHelpers/field3dManagersRun";

// Ground truth for asrs-field3d's RobotManager: upstream's real manager under a THREE.Group,
// driven through `oracle:field3dManagersRun` like the other managers. Its loadRobot requests are
// answered with what upstream's real `bundles/loadRobot.js` worker returns for each asset (or
// never, when that worker never answers). Records match genField3dManagers:
//
//   {"manager":"robot","case","setup","ops","out":[{children, extra[, throw]}...]}
//
// setup.assets lists the robot assets: {id, config: <config.json text>, files: [[name,
// {"bundled": <path under upstream>}]...], never}. The "assets" op names them by id. The extra
// state is [swerve canvas calls since the last state, swerve texture version, [loadingStart
// count, loadingEnd count], getModel(), dimensions, loadRobot payloads since the last state].

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

let seed = 0x7e3d_0808;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const uniform = (lo: number, hi: number) => lo + (hi - lo) * rng();
const below = (n: number) => Math.floor(rng() * n);
const pick = <T>(items: T[]): T => items[below(items.length)];

const SPECIAL_ROTATIONS = [
  [1, 0, 0, 0],
  [Math.SQRT1_2, 0, 0, Math.SQRT1_2],
  [0.5, -0.5, 0.5, -0.5],
  [2, 0.3, -1, 0.1],
  [0, 0, 0, 0]
];

function rotation(): number[] {
  if (rng() < 0.25) return pick(SPECIAL_ROTATIONS);
  const q = [uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)];
  const n = Math.hypot(...q);
  return q.map((v) => v / n);
}

function pose(annotation: object = {}): object {
  return {
    pose: { translation: [uniform(-8, 8), uniform(-4, 4), uniform(-0.2, 1.5)], rotation: rotation() },
    annotation: { is2DSource: rng() < 0.3, ...annotation }
  };
}

const poses = (n: number) => Array.from({ length: n }, () => pose());

function visionTargets(n: number): object[] {
  return Array.from({ length: n }, () => {
    const annotation: any = {};
    if (rng() < 0.6) annotation.visionColor = pick(["#00ff00", "red", "rgb(10, 200, 255)", "notacolor"]);
    if (rng() < 0.6) annotation.visionSize = pick(["bold", "normal", "thin"]);
    return pose(annotation);
  });
}

const LINE_COLORS = ["#ff0000", "#00ffff", "rgb(200, 100, 0)", "orange", "notacolor"];

function mechanismLine(): object {
  return {
    start: [uniform(0, 2), uniform(0, 1.5)],
    end: [uniform(0, 2), uniform(0, 1.5)],
    color: pick(LINE_COLORS),
    weight: pick([1, 2, 6, 10])
  };
}

function mechanism(n: number, odd: object[] = []): object {
  return {
    backgroundColor: "#000020",
    dimensions: [uniform(1, 3), uniform(1, 2)],
    lines: [...Array.from({ length: n }, mechanismLine), ...odd]
  };
}

// Lines the filter drops or that reach the geometry through JS coercions.
const ODD_LINES = [
  { start: [0.5, 0.5], end: [0.5, 0.5], color: "#ffffff", weight: 4 },
  { start: [0, 0], end: [1, 1], color: "#ffffff", weight: 0.05 },
  { start: [0, 0], end: [1, 1], color: "#ffffff", weight: null },
  { start: [null, 0.25], end: [0.75, null], color: 0, weight: 3 },
  { start: [0.4], end: [1, 1], color: "#ffffff", weight: 3 }
];

function module(): object {
  const r = rng();
  if (r < 0.1) return { velocity: pick([0.0005, -0.0005, 0.2, -0.2]), angle: uniform(-4, 4) };
  if (r < 0.15) return { velocity: null, angle: uniform(-4, 4) };
  if (r < 0.2) return { velocity: uniform(-5, 5), angle: null };
  return { velocity: uniform(-6, 6), angle: uniform(-4, 4) };
}

function swerve(sets: number, modules = 4): object[] {
  return Array.from({ length: sets }, () => ({
    values: Array.from({ length: modules }, module),
    color: pick(["#ff0000", "#0000ff", "rgb(30, 160, 30)", "notacolor"])
  }));
}

type RobotOpts = {
  type?: "robot" | "ghost";
  color?: string;
  poses?: number;
  components?: number;
  xz?: object | null;
  yz?: object | null;
  vision?: number;
  swerve?: object[];
};

function robot(model: string, o: RobotOpts = {}): unknown[] {
  const object: any = {
    type: o.type ?? "robot",
    model,
    poses: poses(o.poses ?? 1),
    components: poses(o.components ?? 0),
    mechanisms: { xz: o.xz ?? null, yz: o.yz ?? null },
    visionTargets: visionTargets(o.vision ?? 0),
    swerveModuleVelocities: o.swerve ?? []
  };
  if (object.type === "ghost") object.color = o.color ?? "#000000";
  return ["set", object];
}

type FileSource = { bundled: string };
type Asset = { id: string; config: string; files: [string, FileSource][] };

const KITBOT = "bundledAssets/Robot_FRCKitBot";
const DRIVE_BASE = "bundledAssets/Robot_FTCSimpleDriveBase";
const kitbotConfig = JSON.parse(fs.readFileSync(path.join(UPSTREAM_DIR, KITBOT, "config.json"), "utf8"));
const driveBaseText = fs.readFileSync(path.join(UPSTREAM_DIR, DRIVE_BASE, "config.json"), "utf8");
const armComponents = [
  { zeroedRotations: [{ axis: "x", degrees: 90 }], zeroedPosition: [0.1, -0.2, 0.3] },
  { zeroedRotations: [{ axis: "z", degrees: -45 }, { axis: "y", degrees: 10 }], zeroedPosition: [0, 0, 0.5] }
];

const ASSETS: Asset[] = [
  { id: "kitbot", config: JSON.stringify(kitbotConfig), files: [["model.glb", { bundled: `${KITBOT}/model.glb` }]] },
  {
    id: "arm",
    config: JSON.stringify({ ...kitbotConfig, name: "KitBot Arm", components: armComponents }),
    files: [
      ["model.glb", { bundled: `${KITBOT}/model.glb` }],
      ["model_0.glb", { bundled: `${DRIVE_BASE}/model.glb` }],
      ["model_1.glb", { bundled: `${DRIVE_BASE}/model.glb` }]
    ]
  },
  {
    id: "arm-short",
    config: JSON.stringify({ ...kitbotConfig, name: "KitBot Arm", components: armComponents.slice(0, 1) }),
    files: [
      ["model.glb", { bundled: `${KITBOT}/model.glb` }],
      ["model_0.glb", { bundled: `${DRIVE_BASE}/model.glb` }]
    ]
  },
  { id: "drive-base", config: driveBaseText, files: [["model.glb", { bundled: `${DRIVE_BASE}/model.glb` }]] },
  { id: "broken", config: JSON.stringify({ name: "Broken", rotations: [], position: [0, 0, 0] }), files: [] }
];

const MODES = [
  { mode: "standard", specular: 0x000000, shininess: 0 },
  { mode: "cinematic", specular: 0x666666, shininess: 100 },
  { mode: "low-power", specular: 0x000000, shininess: 0 }
];

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-robot-"));
process.on("exit", () => fs.rmSync(scratch, { recursive: true, force: true }));
let scratchCount = 0;

async function loadAssets(setup: { mode: string; specular: number; shininess: number }) {
  const configs: any[] = [];
  const results: (unknown[][] | null)[] = [];
  for (const asset of ASSETS) {
    const dir = path.join(scratch, String(scratchCount++));
    fs.mkdirSync(dir);
    for (const [name, source] of asset.files) fs.copyFileSync(path.join(UPSTREAM_DIR, source.bundled), path.join(dir, name));
    const config: any = parseRobot(JSON.parse(asset.config));
    config.path = encodePath(path.join(dir, "model.glb"));
    const result = await runWorker("loadRobot.js", {
      robotConfig: config,
      isFTC: config.isFTC,
      mode: setup.mode,
      materialSpecular: new THREE.Color(setup.specular).toArray(),
      materialShininess: setup.shininess
    });
    configs.push(config);
    results.push("never" in result ? null : result.ok);
  }
  return { configs, results };
}

async function emit(name: string, setup: (typeof MODES)[number], ops: unknown[][]) {
  const { configs, results } = await loadAssets(setup);
  const robots: RobotSetup = { ids: ASSETS.map((a) => a.id), configs, results };
  const states = await run("robot", setup, ops, robots);
  const assets = ASSETS.map((asset, i) => ({ ...asset, never: results[i] === null }));
  out({ manager: "robot", case: name, setup: toTree({ ...setup, assets }), ops: toTree(ops), out: toTree(states) });
}

const fullRun = (): unknown[][] => [
  ["resolution", 800, 600],
  robot("KitBot", { swerve: swerve(1) }),
  robot("KitBot", { poses: 2, vision: 2, xz: mechanism(3) }),
  ["load"],
  robot("KitBot", { vision: 3, xz: mechanism(4), yz: mechanism(2), swerve: swerve(2) }),
  robot("KitBot", { poses: 3, vision: 1, xz: mechanism(2, ODD_LINES), swerve: swerve(1, 6) }),
  robot("KitBot", { poses: 3, vision: 1, xz: mechanism(2) }),
  robot("KitBot", { poses: 2 }),
  robot("KitBot Arm", { poses: 2, components: 1, xz: mechanism(1) }),
  ["load"],
  robot("KitBot Arm", { poses: 2, components: 2, xz: mechanism(1), swerve: swerve(1, 3) }),
  robot("KitBot Arm", { poses: 5, components: 3, vision: 2, swerve: swerve(1) }),
  robot("KitBot Arm", { poses: 0, components: 2, vision: 2, xz: mechanism(2), swerve: swerve(1) }),
  robot("KitBot Arm", { poses: 1, components: 0, vision: 1 }),
  ["newAssets"],
  robot("KitBot Arm", { poses: 1, components: 2 }),
  ["load"],
  robot("KitBot Arm", { poses: 2, components: 2, yz: mechanism(3) }),
  ["hide", true],
  robot("KitBot Arm", { poses: 2, components: 2 }),
  ["hide", false],
  robot("KitBot Arm", { poses: 2, components: 2 }),
  ["load"],
  robot("KitBot Arm", { poses: 2, components: 2, swerve: swerve(2) }),
  robot("Drive Base (Simple)", { poses: 2, swerve: swerve(1) }),
  ["load"],
  robot("Drive Base (Simple)", { poses: 2, swerve: swerve(1) }),
  robot("Missing", { poses: 2, swerve: swerve(1) }),
  robot("Broken", { poses: 1 }),
  ["load"],
  robot("KitBot", { poses: 1, vision: 2, swerve: swerve(1) }),
  ["dispose"]
];

const throwRun = (): unknown[][] => [
  robot("KitBot Arm", { poses: 1, components: 2 }),
  ["load"],
  robot("KitBot Arm", { poses: 2, components: 2 }),
  ["assets", ["kitbot"]],
  robot("KitBot Arm", { poses: 0, components: 2 }),
  robot("KitBot Arm", { poses: 2, components: 2, xz: mechanism(1) }),
  ["assets", ["arm-short", "kitbot"]],
  robot("KitBot Arm", { poses: 2, components: 1 }),
  robot("KitBot Arm", { poses: 2, components: 2 }),
  ["assets", null],
  robot("KitBot Arm", { poses: 1, components: 2 }),
  ["newAssets"],
  robot("KitBot Arm", { poses: 1 }),
  ["assets", ["kitbot", "arm"]],
  robot("KitBot", { poses: 1, swerve: [{ values: [{ velocity: 3, angle: 1 }, undefined, { velocity: 2, angle: 0 }], color: "#ff0000" }] }),
  ["load"],
  robot("KitBot", { poses: 1, swerve: [{ values: [{ velocity: 3, angle: 1 }, { velocity: 2, angle: 0 }], color: "#ff0000" }] })
];

const ghostRun = (): unknown[][] => [
  robot("KitBot", { type: "ghost", color: "#ff0000", xz: mechanism(2) }),
  ["load"],
  robot("KitBot", { type: "ghost", color: "#ff0000", poses: 2, xz: mechanism(2), swerve: swerve(1) }),
  robot("KitBot", { type: "ghost", color: "#00ff00", poses: 2, xz: mechanism(3), yz: mechanism(1) }),
  robot("KitBot", { type: "ghost", color: "notacolor", poses: 1, xz: mechanism(1) }),
  robot("KitBot Arm", { type: "ghost", color: "rgb(10, 20, 30)", poses: 1, components: 2 }),
  ["load"],
  robot("KitBot Arm", { type: "ghost", color: "#0000ff", poses: 3, components: 2, xz: mechanism(2) }),
  ["dispose"]
];

for (const setup of MODES) await emit(`robot/${setup.mode}`, setup, fullRun());
await emit("robot/throws", MODES[0], throwRun());
for (const setup of MODES.slice(0, 2)) await emit(`ghost/${setup.mode}`, setup, ghostRun());
