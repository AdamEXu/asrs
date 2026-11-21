import "./shims";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import * as THREE from "three";
import { OracleWorker, UPSTREAM_DIR, resolveWorkerScript, terminatedWorkersExited } from "./shims";
import { toTree } from "./cve1";
import { parseField3d, parseRobot } from "@upstream/main/assetUtil";
import optimizeGeometries from "@upstream/shared/renderers/field3d/OptimizeGeometries";
import makeAxesField from "@upstream/shared/renderers/field3d/AxesField";
import makeAxesTemplate from "@upstream/shared/renderers/field3d/AxesTemplate";
import makeEvergreenField from "@upstream/shared/renderers/field3d/EvergreenField";
import { syntheticCases } from "./glb/synthetic";

// Ground truth for asrs-field3d's GLB pipeline. Robot and field cases run upstream's real
// `bundles/loadRobot.js` and `bundles/loadField.js` workers (GLTFLoader + optimizeGeometries +
// toJSON); procedural cases call upstream's optimizeGeometries on upstream's procedural fields.
// The harness only converts the result: typed arrays become {type, count, sha256 of the
// little-endian bytes}, materials and objects keep the toJSON keys that differ from a fresh
// MeshPhongMaterial / Mesh / Group.
//
//   genGlb bundled | synthetic | procedural
//   genGlb release <asset dir> [--jobs N]   (live only, never committed)
//
// Records: {"kind":"file", "sha256", "hex"} and {"kind":"scene", "scene":<id>, "nodes":[...]}
// define inputs that later cases name;
//   {"case", "kind":"robot"|"field", "mode", "config":<config.json text>,
//    "files":[[name, {"bundled":<path under upstream>} | {"sha256":<file record>}]...],
//    "out":<CVE-1 tree> | {"never":true}}
//   {"case", "kind":"optimize", "mode", "scene":<scene id>, "args":<CVE-1>, "out":<CVE-1 tree>}

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const MODES = ["low-power", "standard", "cinematic"] as const;
type Mode = (typeof MODES)[number];

// Field3dRendererImpl's MATERIAL_SPECULAR / MATERIAL_SHININESS per mode.
function materialFor(mode: Mode): { specular: THREE.Color; shininess: number } {
  return mode === "cinematic"
    ? { specular: new THREE.Color(0x666666), shininess: 100 }
    : { specular: new THREE.Color(0x000000), shininess: 0 };
}

const TYPED: { [name: string]: any } = {
  Int8Array,
  Uint8Array,
  Uint8ClampedArray,
  Int16Array,
  Uint16Array,
  Int32Array,
  Uint32Array,
  Float32Array,
  Float64Array
};

function arrayOut(type: string, array: any): object {
  const typed = ArrayBuffer.isView(array) ? (array as any) : new TYPED[type](array);
  const bytes = Buffer.from(typed.buffer, typed.byteOffset, typed.byteLength);
  return { type, count: typed.length, sha256: createHash("sha256").update(bytes).digest("hex") };
}

function changedKeys(json: any, reference: any, skip: string[]): [string, unknown][] {
  return Object.keys(json)
    .filter((key) => !skip.includes(key) && JSON.stringify(json[key]) !== JSON.stringify(reference[key]))
    .map((key) => [key, json[key]]);
}

const DEFAULT_MATERIAL = new THREE.MeshPhongMaterial().toJSON();
const DEFAULT_MESH = new THREE.Mesh().toJSON().object;
const DEFAULT_GROUP = new THREE.Group().toJSON().object;
const DEFAULT_OBJECT = new THREE.Object3D().toJSON().object;

function geometryOut(g: any): object {
  const data = g.data;
  return {
    type: g.type,
    index: data.index === undefined ? null : arrayOut(data.index.type, data.index.array),
    attributes: Object.entries(data.attributes).map(([name, a]: [string, any]) => [
      name,
      a.itemSize,
      a.normalized,
      arrayOut(a.type, a.array),
      changedKeys(a, {}, ["itemSize", "normalized", "type", "array"])
    ]),
    other: [
      ...changedKeys(g, {}, ["metadata", "uuid", "type", "data"]),
      ...changedKeys(data, {}, ["attributes", "index"])
    ]
  };
}

function objectOut(object: any, geometries: Map<string, any>, materials: Map<string, any>): object {
  if (object.type === "Mesh") {
    return {
      type: object.type,
      object: changedKeys(object, DEFAULT_MESH, ["uuid", "geometry", "material"]),
      geometry: geometryOut(geometries.get(object.geometry)),
      material: changedKeys(materials.get(object.material), DEFAULT_MATERIAL, ["uuid", "metadata"])
    };
  }
  const reference = object.type === "Group" ? DEFAULT_GROUP : DEFAULT_OBJECT;
  return {
    type: object.type,
    object: changedKeys(object, reference, ["uuid", "children"]),
    children: (object.children ?? []).map((child: any) => objectOut(child, geometries, materials))
  };
}

function serializedOut(json: any): object {
  const geometries = new Map((json.geometries ?? []).map((g: any) => [g.uuid, g]));
  const materials = new Map((json.materials ?? []).map((m: any) => [m.uuid, m]));
  return objectOut(json.object, geometries as any, materials as any);
}

function fieldOut(result: any): object {
  return {
    field: serializedOut(result.field),
    fieldStagedPieces: serializedOut(result.fieldStagedPieces),
    fieldPieces: Object.entries(result.fieldPieces).map(([name, json]) => [name, serializedOut(json)])
  };
}

function robotOut(result: any[][]): object {
  return result.map((scene) => scene.map(serializedOut));
}

type WorkerResult = { ok: any } | { never: string };

async function runWorker(bundle: string, payload: unknown, timeoutMs = 120000): Promise<WorkerResult> {
  const script = resolveWorkerScript("../bundles/" + bundle);
  // A worker's relative URLs resolve against its own script URL, and Chromium fetches file:
  // URLs; Node's Request and fetch do neither, so the worker realm gets both.
  const loader =
    "data:text/javascript," +
    encodeURIComponent(`
import fs from "node:fs";
import { fileURLToPath } from "node:url";
const BASE = ${JSON.stringify(script)};
globalThis.ProgressEvent ??= class ProgressEvent extends Event {
  constructor(type, init = {}) {
    super(type);
    this.lengthComputable = init.lengthComputable ?? false;
    this.loaded = init.loaded ?? 0;
    this.total = init.total ?? 0;
  }
};
const RealRequest = globalThis.Request;
const realFetch = globalThis.fetch;
globalThis.Request = class extends RealRequest {
  constructor(input, init) { super(typeof input === "string" ? new URL(input, BASE).href : input, init); }
};
globalThis.fetch = async (input) => {
  const url = new URL(typeof input === "string" ? input : input.url, BASE);
  if (url.protocol === "data:") return realFetch(url.href);
  if (url.protocol !== "file:") throw new TypeError("Failed to fetch");
  let bytes;
  try { bytes = fs.readFileSync(fileURLToPath(url)); } catch { throw new TypeError("Failed to fetch"); }
  return new Response(bytes);
};
await import(BASE);
`);
  const worker = new OracleWorker(loader, { type: "module" });
  const result = await new Promise<WorkerResult>((resolve) => {
    const timer = setTimeout(() => resolve({ never: "timeout" }), timeoutMs);
    worker.onmessage = (event) => {
      clearTimeout(timer);
      resolve({ ok: event.data.payload });
    };
    worker.onerror = (event) => {
      clearTimeout(timer);
      resolve({ never: String(event.message) });
    };
    worker.postMessage({ id: 1, payload });
  });
  worker.terminate();
  await terminatedWorkersExited();
  return result;
}

function encodePath(pathStr: string): string {
  return pathStr
    .split(path.sep)
    .map((component, index) => (index === 0 && component.endsWith(":") ? component : encodeURIComponent(component)))
    .join(path.sep);
}

export type FileSource = { bundled: string } | { hex: string } | { abs: string };
export type AssetCase = {
  case: string;
  kind: "robot" | "field";
  config: string;
  files: [string, FileSource][];
  modes?: readonly Mode[];
};

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-glb-"));
process.on("exit", () => fs.rmSync(scratch, { recursive: true, force: true }));
let scratchCount = 0;

function materialize(files: [string, FileSource][]): string {
  const dir = path.join(scratch, String(scratchCount++));
  fs.mkdirSync(dir);
  for (const [name, source] of files) {
    const target = path.join(dir, name);
    if ("bundled" in source) fs.copyFileSync(path.join(UPSTREAM_DIR, source.bundled), target);
    else if ("abs" in source) fs.symlinkSync(source.abs, target);
    else fs.writeFileSync(target, Buffer.from(source.hex, "hex"));
  }
  return dir;
}

async function runAsset(c: AssetCase, mode: Mode): Promise<object> {
  const dir = materialize(c.files);
  const raw = JSON.parse(c.config);
  const parsed: any = c.kind === "robot" ? parseRobot(raw) : parseField3d(raw);
  if (typeof parsed !== "object") return { config: String(parsed) };
  parsed.path = encodePath(path.join(dir, "model.glb"));
  const { specular, shininess } = materialFor(mode);
  const payload =
    c.kind === "robot"
      ? {
          robotConfig: parsed,
          isFTC: parsed.isFTC,
          mode,
          materialSpecular: specular.toArray(),
          materialShininess: shininess
        }
      : { fieldConfig: parsed, mode, materialSpecular: specular.toArray(), materialShininess: shininess };
  const result = await runWorker(c.kind === "robot" ? "loadRobot.js" : "loadField.js", payload);
  fs.rmSync(dir, { recursive: true, force: true });
  if ("never" in result) {
    process.stderr.write(`${c.case} ${mode}: never completes (${result.never.split("\n")[0]})\n`);
    return { never: true };
  }
  return toTree(c.kind === "robot" ? robotOut(result.ok) : fieldOut(result.ok)) as object;
}

async function emitAssets(cases: AssetCase[]) {
  const emitted = new Set<string>();
  for (const c of cases) {
    const files = c.files.map(([name, source]): [string, object] => {
      if (!("hex" in source)) return [name, source];
      const sha256 = createHash("sha256").update(Buffer.from(source.hex, "hex")).digest("hex");
      if (!emitted.has(sha256)) {
        emitted.add(sha256);
        out({ kind: "file", sha256, hex: source.hex });
      }
      return [name, { sha256 }];
    });
    for (const mode of c.modes ?? MODES) {
      const result = await runAsset(c, mode);
      out({ case: c.case, kind: c.kind, mode, config: c.config, files, out: result });
    }
  }
}

function bundledCases(): AssetCase[] {
  const cases: AssetCase[] = [];
  const robots = ["Robot_FRCKitBot", "Robot_FTCSimpleDriveBase"];
  for (const folder of robots) {
    const config = fs.readFileSync(path.join(UPSTREAM_DIR, "bundledAssets", folder, "config.json"), "utf8");
    const files: [string, FileSource][] = [["model.glb", { bundled: `bundledAssets/${folder}/model.glb` }]];
    cases.push({ case: folder, kind: "robot", config, files });
    const variant = JSON.parse(config);
    variant.disableSimplification = true;
    cases.push({ case: `${folder}/no-simplification`, kind: "robot", config: JSON.stringify(variant), files });
    variant.disableSimplification = false;
    variant.isFTC = !variant.isFTC;
    variant.components = [{ zeroedRotations: [], zeroedPosition: [0, 0, 0] }];
    const other = robots.find((r) => r !== folder)!;
    cases.push({
      case: `${folder}/component`,
      kind: "robot",
      config: JSON.stringify(variant),
      files: [...files, ["model_0.glb", { bundled: `bundledAssets/${other}/model.glb` }]]
    });
  }
  // The bundled robot models as field and game-piece models, so loadField's staging and spatial
  // slicing run on real exported geometry.
  const field = {
    name: "Bundled",
    isFTC: false,
    coordinateSystem: "center-red",
    rotations: [{ axis: "x", degrees: 90 }, { axis: "z", degrees: 30 }],
    position: [1.25, -0.5, 0.1],
    widthInches: 100,
    heightInches: 50,
    gamePieces: [
      { name: "Drive Base", rotations: [{ axis: "y", degrees: 45 }], position: [0, 0, 0.2], stagedObjects: ["Part_1_2", "Part_1_4", "Missing"] },
      { name: "Bot", rotations: [], position: [0.5, 0.5, 0], stagedObjects: ["Basic_Square_KoP_Bot_1"] }
    ]
  };
  const fieldFiles: [string, FileSource][] = [
    ["model.glb", { bundled: "bundledAssets/Robot_FRCKitBot/model.glb" }],
    ["model_0.glb", { bundled: "bundledAssets/Robot_FTCSimpleDriveBase/model.glb" }],
    ["model_1.glb", { bundled: "bundledAssets/Robot_FRCKitBot/model.glb" }]
  ];
  cases.push({ case: "field/kitbot", kind: "field", config: JSON.stringify(field), files: fieldFiles });
  const ftcField = {
    ...field,
    name: "Bundled FTC",
    isFTC: true,
    gamePieces: [{ name: "Bot", rotations: [{ axis: "z", degrees: -90 }], position: [0, 0, 0], stagedObjects: ["Part_1", "Part_1_3", "Assembly_1"] }]
  };
  cases.push({
    case: "field/ftc-drive-base",
    kind: "field",
    config: JSON.stringify(ftcField),
    files: [
      ["model.glb", { bundled: "bundledAssets/Robot_FTCSimpleDriveBase/model.glb" }],
      ["model_0.glb", { bundled: "bundledAssets/Robot_FRCKitBot/model.glb" }]
    ]
  });
  return cases;
}

const hex = (ta: ArrayBufferView) => Buffer.from(ta.buffer, ta.byteOffset, ta.byteLength).toString("hex");

function sceneDump(root: THREE.Object3D): object[] {
  const nodes: THREE.Object3D[] = [];
  root.traverse((o) => nodes.push(o));
  return nodes.map((o) => {
    const node: any = {
      parent: o === root ? null : nodes.indexOf(o.parent!),
      type: o.type,
      name: o.name,
      position: toTree(o.position.toArray()),
      quaternion: toTree(o.quaternion.toArray()),
      scale: toTree(o.scale.toArray())
    };
    if ((o as any).isMesh) {
      const mesh = o as THREE.Mesh;
      const g = mesh.geometry;
      node.geometry = {
        index: g.index === null ? null : [g.index.array.constructor.name, hex(g.index.array)],
        attributes: Object.entries(g.attributes).map(([name, a]: [string, any]) => [
          name,
          a.itemSize,
          a.normalized,
          a.array.constructor.name,
          hex(a.array)
        ])
      };
      node.material = Array.isArray(mesh.material)
        ? { array: true }
        : {
            transparent: mesh.material.transparent,
            opacity: toTree(mesh.material.opacity),
            color: "color" in mesh.material ? toTree((mesh.material.color as THREE.Color).toArray()) : null
          };
    }
    return node;
  });
}

function optimizedOut(result: { normal: THREE.Mesh[]; transparent: THREE.Mesh[]; carpet: THREE.Mesh[] }): object {
  const meshes = (list: THREE.Mesh[]) => list.map((mesh) => serializedOut(mesh.toJSON()));
  return { normal: meshes(result.normal), transparent: meshes(result.transparent), carpet: meshes(result.carpet) };
}

function emitProcedural() {
  type Build = (specular: THREE.Color, shininess: number) => THREE.Object3D;
  const scenes: [string, Build, boolean][] = [
    ["evergreen-frc", (s, h) => makeEvergreenField(s, h, false), false],
    ["evergreen-ftc", (s, h) => makeEvergreenField(s, h, true), true]
  ];
  for (const system of ["wall-alliance", "wall-blue", "center-rotated", "center-red"] as const) {
    scenes.push([`axes-frc-${system}`, (s, h) => makeAxesField(s, h, system, false), false]);
    scenes.push([`axes-ftc-${system}`, (s, h) => makeAxesField(s, h, system, true), true]);
  }
  scenes.push([
    "axes-manager",
    (s, h) => {
      const axes = makeAxesTemplate(s, h);
      axes.scale.set(0.25, 0.25, 0.25);
      return axes;
    },
    false
  ]);
  scenes.push(["axes-template", (s, h) => makeAxesTemplate(s, h), false]);
  const scenesEmitted = new Map<string, string>();
  for (const [name, build, isFTC] of scenes) {
    for (const mode of MODES) {
      const { specular, shininess } = materialFor(mode);
      const variants: [string, boolean, boolean, number | undefined][] =
        name === "axes-manager"
          ? [["", false, false, undefined]]
          : [
              ["/sliced", true, isFTC, 1],
              ["/plain", false, isFTC, undefined],
              ["/simplified", true, !isFTC, undefined]
            ];
      for (const [suffix, simplify, ftc, slicing] of variants) {
        const root = build(specular, shininess);
        const scene = sceneDump(root);
        const sceneText = JSON.stringify(scene);
        if (!scenesEmitted.has(sceneText)) {
          scenesEmitted.set(sceneText, `${name}#${scenesEmitted.size}`);
          out({ kind: "scene", scene: scenesEmitted.get(sceneText), nodes: scene });
        }
        const args = { mode, specular: specular.toArray(), shininess, enableSimplification: simplify, isFTC: ftc, slicingSize: slicing };
        const result = optimizeGeometries(root, mode, specular, shininess, simplify, ftc, slicing);
        out({ case: `${name}${suffix}`, kind: "optimize", mode, scene: scenesEmitted.get(sceneText), args: toTree(args), out: toTree(optimizedOut(result)) });
      }
    }
  }
}

async function emitRelease(dir: string, jobs: number) {
  const folders = fs
    .readdirSync(dir)
    .filter((name) => name.startsWith("Field3d_") || name.startsWith("Robot_"))
    .sort();
  const tasks: { folder: string; mode: Mode }[] = [];
  for (const folder of folders) for (const mode of MODES) tasks.push({ folder, mode });
  const results: (object | undefined)[] = new Array(tasks.length);
  let next = 0;
  let written = 0;
  const flush = () => {
    while (written < tasks.length && results[written] !== undefined) {
      out(results[written]!);
      results[written] = undefined as any;
      written++;
    }
  };
  const run = async () => {
    while (next < tasks.length) {
      const i = next++;
      const { folder, mode } = tasks[i];
      const assetDir = path.join(dir, folder);
      const config = fs.readFileSync(path.join(assetDir, "config.json"), "utf8");
      const files: [string, FileSource][] = fs
        .readdirSync(assetDir)
        .filter((name) => name.endsWith(".glb"))
        .map((name) => [name, { abs: path.join(assetDir, name) }]);
      const kind = folder.startsWith("Robot_") ? "robot" : "field";
      const started = Date.now();
      const result = await runAsset({ case: folder, kind, config, files }, mode);
      process.stderr.write(`${folder} ${mode}: ${Date.now() - started} ms\n`);
      results[i] = { case: folder, kind, mode, out: result };
      flush();
    }
  };
  await Promise.all(Array.from({ length: jobs }, run));
}

const [command, ...rest] = process.argv.slice(2);
switch (command) {
  case "bundled":
    await emitAssets(bundledCases());
    break;
  case "synthetic":
    await emitAssets(syntheticCases());
    break;
  case "procedural":
    emitProcedural();
    break;
  case "release": {
    const jobsIndex = rest.indexOf("--jobs");
    const jobs = jobsIndex >= 0 ? Number(rest[jobsIndex + 1]) : 1;
    await emitRelease(path.resolve(rest[0]), jobs);
    break;
  }
  default:
    console.error("usage: genGlb bundled | synthetic | procedural | release <dir> [--jobs N]");
    process.exit(2);
}
