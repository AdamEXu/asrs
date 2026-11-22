import fs from "node:fs";
import path from "node:path";
import * as THREE from "three";
import { Line2 } from "three/examples/jsm/lines/Line2.js";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import ObjectManager from "@upstream/shared/renderers/field3d/ObjectManager";
import AprilTagManager from "@upstream/shared/renderers/field3d/objectManagers/AprilTagManager";
import AxesManager from "@upstream/shared/renderers/field3d/objectManagers/AxesManager";
import ConeManager from "@upstream/shared/renderers/field3d/objectManagers/ConeManager";
import GamePieceManager from "@upstream/shared/renderers/field3d/objectManagers/GamePieceManager";
import HeatmapManager from "@upstream/shared/renderers/field3d/objectManagers/HeatmapManager";
import RobotManager from "@upstream/shared/renderers/field3d/objectManagers/RobotManager";
import TrajectoryManager from "@upstream/shared/renderers/field3d/objectManagers/TrajectoryManager";
import { toTree } from "../cve1";
import { RecordingContext } from "../dom/recordingCanvas";
import { fakeContainer, installFakeDom } from "../heatmap/fakeDom";

// Runs one upstream object manager under a THREE.Group root through a list of ops and returns the
// state after each op (see stateOf). The setup and op objects are the managers' real inputs:
//
//   setup: {mode, specular, shininess, pieces?, field?, preferences?}
//     pieces       gamePiece: {name: {geometry: [ctor, ...args], color}} built as THREE.Meshes
//     field        heatmap: the Config3dField subset getFieldConfig returns, or null
//     preferences  heatmap: window.preferences, or undefined
//   ops: ["set", object] setObjectData; ["loadTextures"] completes every pending TextureLoader
//     load whose file exists under www (the others never call back, as a 404 does);
//     ["redAlliance", bool]; ["field", config | null]; ["preferences", prefs | undefined];
//     ["resolution", w, h]; ["dispose"]
//   robot only (`robots` gives each asset's id, parsed config and loadRobot result, null for a
//   worker that never answers; window.assets starts with every config): ["load"] answers every
//     pending loadRobot request; ["assets", ids | null] replaces window.assets; ["newAssets"];
//     ["hide", bool]
//
// Each result is {children, extra} or {throw, children, extra} when the op threw, as plain values
// (numbers and typed arrays unencoded); geometries are FNV-1a 64 digests of their CVE-1 text.

declare const __UPSTREAM_DIR__: string;
const UPSTREAM_DIR: string = process.env.ORACLE_UPSTREAM_DIR ?? __UPSTREAM_DIR__;

let recordCanvases = false;

class RecordingCanvas {
  readonly tagName = "CANVAS";
  private w = 300;
  private h = 150;
  readonly context = new RecordingContext(this as unknown as HTMLCanvasElement, { imageId: () => "canvas" }, (w, h) => ({
    width: w,
    height: h,
    data: new Uint8ClampedArray(Math.max(0, w * h * 4))
  }));
  get width() {
    return this.w;
  }
  set width(value: number) {
    this.w = value >>> 0;
    this.context.reset();
  }
  get height() {
    return this.h;
  }
  set height(value: number) {
    this.h = value >>> 0;
    this.context.reset();
  }
  getContext() {
    return this.context.proxy;
  }
}

function installDom() {
  installFakeDom();
  const doc = (globalThis as any).document;
  const heatmapCreate = doc.createElement;
  doc.createElement = (tag: string) =>
    recordCanvases && tag === "canvas" ? new RecordingCanvas() : heatmapCreate(tag);
  doc.body = fakeContainer();
}

type Pending = { url: string; texture: THREE.Texture; onLoad?: (texture: THREE.Texture) => void };
let pending: Pending[] = [];
let requested: string[] = [];

export type RobotSetup = { ids: string[]; configs: any[]; payloads: any[]; results: (unknown[][] | null)[] };
let robotLoads: { worker: LoadRobotWorker; id: number; payload: any }[] = [];
let robotRequests: unknown[] = [];

// Stands in for the Worker that WorkerManager.request starts, so the request's real promise
// resolves when the "load" op answers it.
class LoadRobotWorker {
  onmessage: ((event: unknown) => void) | null = null;
  constructor(script: string) {
    if (script !== "../bundles/loadRobot.js") throw new Error(`field3dManagersRun: unexpected worker ${script}`);
  }
  postMessage(message: { id: number; payload: any }) {
    const payload = message.payload;
    robotRequests.push([payload.robotConfig.name, payload.isFTC, payload.mode, payload.materialSpecular, payload.materialShininess]);
    robotLoads.push({ worker: this, id: message.id, payload });
  }
  terminate() {}
}

async function answerRobotLoads(robots: RobotSetup) {
  const loads = robotLoads;
  robotLoads = [];
  for (const load of loads) {
    const index = robots.configs.indexOf(load.payload.robotConfig);
    const result = robots.results[index];
    if (result === undefined) throw new Error("field3dManagersRun: a loadRobot request for an unknown config");
    const { robotConfig: _sent, ...sent } = load.payload;
    const { robotConfig: _ran, ...ran } = robots.payloads[index];
    if (JSON.stringify(sent) !== JSON.stringify(ran)) {
      throw new Error("field3dManagersRun: a loadRobot payload the worker was not run on");
    }
    if (result !== null) load.worker.onmessage!({ data: { id: load.id, payload: structuredClone(result) } });
  }
  await new Promise((resolve) => setTimeout(resolve, 0));
}

function installTextureLoader() {
  THREE.TextureLoader.prototype.load = function (url: string, onLoad?: (texture: THREE.Texture) => void) {
    const texture = new THREE.Texture();
    pending.push({ url, texture, onLoad });
    requested.push(url);
    return texture;
  } as any;
}

function loadTextures() {
  const loads = pending;
  pending = [];
  for (const load of loads) {
    const file = path.resolve(UPSTREAM_DIR, "www", load.url);
    if (!fs.existsSync(file)) continue;
    load.texture.name = load.url;
    load.onLoad?.(load.texture);
  }
}

function fnv1a64(text: string): string {
  let h = 0xcbf29ce484222325n;
  for (let i = 0; i < text.length; i++) {
    h ^= BigInt(text.charCodeAt(i));
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h.toString(16).padStart(16, "0");
}

function geometryDigest(g: THREE.BufferGeometry): string {
  const tree = [
    g.index === null ? null : toTree(g.index.array),
    Object.keys(g.attributes).map((name) => {
      const a = g.attributes[name] as THREE.BufferAttribute;
      return [name, toTree(a.itemSize), a.normalized, toTree(a.array)];
    }),
    g.groups.map((group) => [toTree(group.start), toTree(group.count), toTree(group.materialIndex ?? 0)])
  ];
  return fnv1a64(JSON.stringify(tree));
}

function makePiece(spec: { geometry: [string, ...number[]]; color: number }): THREE.Mesh {
  const [ctor, ...args] = spec.geometry;
  const geometry = new (THREE as any)[ctor](...args) as THREE.BufferGeometry;
  return new THREE.Mesh(geometry, new THREE.MeshPhongMaterial({ color: spec.color }));
}

export async function run(kind: string, setup: any, ops: any[][], robots?: RobotSetup): Promise<unknown[]> {
  installDom();
  installTextureLoader();
  const realWorker = (globalThis as any).Worker;
  (globalThis as any).Worker = LoadRobotWorker;
  pending = [];
  requested = [];
  robotLoads = [];
  robotRequests = [];
  recordCanvases = kind === "cone" || kind === "robot";
  const robotConfigs = (ids: string[]) => ids.map((id) => robots!.configs[robots!.ids.indexOf(id)]);
  (globalThis as any).assets = robots === undefined ? undefined : { robots: robots.configs };
  let loading = [0, 0];
  (globalThis as any).preferences = setup.preferences;
  let field = setup.field ?? null;

  const root = new THREE.Group();
  const specular = new THREE.Color(setup.specular);
  const args = [root, specular, setup.shininess, setup.mode, false, () => {}] as const;
  let manager: ObjectManager<any>;
  switch (kind) {
    case "gamePiece": {
      const pieces: { [key: string]: THREE.Mesh } = {};
      for (const [name, spec] of Object.entries(setup.pieces)) pieces[name] = makePiece(spec as any);
      manager = new GamePieceManager(...args, pieces);
      break;
    }
    case "aprilTag":
      manager = new AprilTagManager(...args);
      break;
    case "axes":
      manager = new AxesManager(...args);
      break;
    case "cone":
      manager = new ConeManager(...args);
      break;
    case "trajectory":
      manager = new TrajectoryManager(...args);
      break;
    case "heatmap":
      manager = new HeatmapManager(...args, () => field);
      break;
    case "robot":
      manager = new RobotManager(
        ...args,
        () => loading[0]++,
        () => loading[1]++
      );
      break;
    default:
      throw new Error(`field3dManagersRun: unknown manager ${JSON.stringify(kind)}`);
  }

  const labels = new Map<unknown, number>();
  const label = (key: unknown) => {
    if (!labels.has(key)) labels.set(key, labels.size);
    return labels.get(key)!;
  };

  const textureOf = (texture: THREE.Texture | null) => {
    if (texture === null) return null;
    const image = texture.image as unknown;
    const source =
      image instanceof RecordingCanvas || (image !== null && typeof image === "object" && (image as any).tag === "canvas")
        ? ["canvas", label(image)]
        : ["url", texture.name];
    return [...source, texture.minFilter, texture.magFilter, texture.flipY, texture.generateMipmaps];
  };

  const materialOf = (m: THREE.Material) => {
    if (m instanceof LineMaterial) {
      return [m.type, m.color.toArray(), m.linewidth, m.opacity, m.transparent, m.depthTest, m.depthWrite, m.visible];
    }
    const p = m as THREE.MeshPhongMaterial;
    return [
      p.type,
      p.color.toArray(),
      p.specular.toArray(),
      p.shininess,
      p.emissive.toArray(),
      p.opacity,
      p.transparent,
      p.side,
      p.depthTest,
      p.depthWrite,
      p.vertexColors,
      p.visible,
      textureOf(p.map)
    ];
  };

  const childOf = (o: THREE.Object3D) => {
    const mesh = o as THREE.Mesh;
    const materials = Array.isArray(mesh.material) ? ["array", mesh.material.map(materialOf)] : ["single", [materialOf(mesh.material)]];
    let geometry: unknown;
    if (o instanceof Line2) {
      const start = (o.geometry.attributes.instanceStart as THREE.InterleavedBufferAttribute | undefined)?.data.array;
      geometry = start === undefined ? null : start.slice();
    } else {
      geometry = geometryDigest(mesh.geometry);
    }
    const instances = o instanceof THREE.InstancedMesh ? [o.count, o.instanceMatrix.array.slice()] : null;
    return [
      o.type,
      o.visible,
      o.castShadow,
      o.receiveShadow,
      o.frustumCulled,
      o.position.toArray(),
      o.quaternion.toArray(),
      o.scale.toArray(),
      geometry,
      materials,
      instances
    ];
  };

  let swerveCallsSeen = 0;
  const extraOf = () => {
    if (kind === "aprilTag") {
      const urls = requested;
      requested = [];
      return urls;
    }
    if (kind === "cone") {
      const canvases = [...labels.keys()].filter((k) => k instanceof RecordingCanvas) as RecordingCanvas[];
      return canvases.map((c) => [label(c), structuredClone(c.context.calls)]);
    }
    if (kind === "robot") {
      const robot = manager as any;
      const calls = robot.swerveCanvas.context.calls;
      const drawn = calls.slice(swerveCallsSeen);
      swerveCallsSeen = calls.length;
      const requests = robotRequests;
      robotRequests = [];
      return [
        structuredClone(drawn),
        robot.swerveTexture.version,
        [...loading],
        robot.getModel(),
        [...robot.dimensions],
        requests
      ];
    }
    if (kind === "heatmap") {
      const heatmap = (manager as any).heatmap;
      const canvas = heatmap.getCanvas();
      const store = heatmap.heatmap === null ? null : heatmap.heatmap.getData();
      return [canvas === null ? null : [canvas.width, canvas.height], structuredClone(store)];
    }
    return null;
  };

  const stateOf = () => {
    const children = root.children.map(childOf);
    return { children, extra: extraOf() };
  };

  const states: unknown[] = [];
  for (const op of ops) {
    let error: unknown = undefined;
    try {
      switch (op[0]) {
        case "set":
          manager.setObjectData(op[1]);
          break;
        case "loadTextures":
          loadTextures();
          break;
        case "redAlliance":
          (manager as HeatmapManager).setIsRedAlliance(op[1]);
          break;
        case "field":
          field = op[1];
          break;
        case "preferences":
          (globalThis as any).preferences = op[1];
          break;
        case "resolution":
          manager.setResolution(new THREE.Vector2(op[1], op[2]));
          break;
        case "dispose":
          manager.dispose();
          break;
        case "load":
          await answerRobotLoads(robots!);
          break;
        case "assets":
          (globalThis as any).assets = op[1] === null ? null : { robots: robotConfigs(op[1]) };
          break;
        case "newAssets":
          (manager as RobotManager).newAssets();
          break;
        case "hide":
          (manager as RobotManager).setHideRobotModels(op[1]);
          break;
        default:
          throw new Error(`field3dManagersRun: unknown op ${JSON.stringify(op[0])}`);
      }
    } catch (e) {
      if (e instanceof Error && e.message.startsWith("field3dManagersRun:")) throw e;
      error = e;
    }
    const state = stateOf();
    states.push(error === undefined ? state : { throw: error, ...state });
  }
  (globalThis as any).Worker = realWorker;
  return states;
}
