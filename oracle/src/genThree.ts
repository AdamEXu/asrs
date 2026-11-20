import "./shims";
import * as THREE from "three";
import * as BufferGeometryUtils from "three/examples/jsm/utils/BufferGeometryUtils.js";
import { toTree, type Cve1 } from "./cve1";
import makeAxesField from "@upstream/shared/renderers/field3d/AxesField";
import makeAxesTemplate from "@upstream/shared/renderers/field3d/AxesTemplate";
import makeEvergreenField from "@upstream/shared/renderers/field3d/EvergreenField";
import { FRC_STANDARD_FIELD_LENGTH, FRC_STANDARD_FIELD_WIDTH, FTC_STANDARD_FIELD_LENGTH, FTC_STANDARD_FIELD_WIDTH } from "@upstream/shared/AdvantageScopeAssets";
import { Units } from "@upstream/shared/units";

// Ground truth for asrs-field3d's three.js port (three 0.168 from upstream's node_modules).
// One JSON line per case; values are CVE-1 trees.
//
//   {"kind":"geometry","case","spec":<geometry spec>,"out":<geometry>}
//   {"kind":"merge","case","specs":[<geometry spec>...],"out":<geometry> | null}
//   {"kind":"object","case","ops":[[op, ...args]...],"out":<object state>}
//   {"kind":"graph","case","nodes":[{"parent":<index>|null,"ops":[...]}],"out":[<world state>...]}
//   {"kind":"math","fn","args":[...],"out":<value>}
//   {"kind":"color","arg":<number | string>,"out":[r, g, b, getHex()]}
//
// A geometry spec is {"ctor","args","ops"}: `new THREE[ctor](...args)` (or
// `new BufferGeometry().setFromPoints(args)` for "Points"), then each op in order. A geometry
// is [index, [[name, itemSize, normalized, array]...], [[start, count, materialIndex]...],
// boundingBox, boundingSphere] with null for what is absent. A geometry whose JSON text is longer
// than DIGEST_OVER is recorded as {"digest": FNV-1a 64 of that text, "length": its length}.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

let seed = 0x7e3d_0168;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const uniform = (lo: number, hi: number) => lo + (hi - lo) * rng();
const below = (n: number) => Math.floor(rng() * n);
const pick = <T>(items: readonly T[]): T => items[below(items.length)];
const angle = () => pick([0, Math.PI / 2, -Math.PI / 2, Math.PI, Math.PI / 4, uniform(-7, 7), uniform(-Math.PI, Math.PI)]);

type Op = [string, ...unknown[]];
type Spec = { ctor: string; args: unknown[]; ops: Op[] };

const ORDERS = ["XYZ", "YXZ", "ZXY", "ZYX", "YZX", "XZY"] as const;

const DIGEST_OVER = 200000;

function fnv1a64(text: string): string {
  let h = 0xcbf29ce484222325n;
  for (let i = 0; i < text.length; i++) {
    h ^= BigInt(text.charCodeAt(i));
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h.toString(16).padStart(16, "0");
}

function geometryOut(g: THREE.BufferGeometry): Cve1 {
  const tree = geometryTree(g);
  const text = JSON.stringify(tree);
  if (text.length <= DIGEST_OVER) return tree;
  return { digest: fnv1a64(text), length: text.length };
}

function geometryTree(g: THREE.BufferGeometry): Cve1 {
  const attribute = (a: THREE.BufferAttribute) => toTree(a.array);
  return [
    g.index === null ? null : attribute(g.index),
    Object.keys(g.attributes).map((name) => {
      const a = g.attributes[name] as THREE.BufferAttribute;
      return [name, toTree(a.itemSize), a.normalized, attribute(a)];
    }),
    g.groups.map((group) => [toTree(group.start), toTree(group.count), toTree(group.materialIndex ?? 0)]),
    g.boundingBox === null ? null : [toTree(g.boundingBox.min.toArray()), toTree(g.boundingBox.max.toArray())],
    g.boundingSphere === null ? null : [toTree(g.boundingSphere.center.toArray()), toTree(g.boundingSphere.radius)]
  ];
}

function build(spec: Spec): THREE.BufferGeometry {
  const args = spec.args as any[];
  let g: THREE.BufferGeometry;
  if (spec.ctor === "Points") {
    g = new THREE.BufferGeometry().setFromPoints(args.map((p) => (p.length === 2 ? new THREE.Vector2(p[0], p[1]) : new THREE.Vector3(p[0], p[1], p[2]))));
  } else {
    g = new (THREE as any)[spec.ctor](...args);
  }
  for (const [op, ...a] of spec.ops as [string, ...any[]][]) {
    switch (op) {
      case "translate":
      case "scale":
        (g as any)[op](a[0], a[1], a[2]);
        break;
      case "rotateX":
      case "rotateY":
      case "rotateZ":
        (g as any)[op](a[0]);
        break;
      case "applyMatrix4":
        g.applyMatrix4(new THREE.Matrix4().fromArray(a[0]));
        break;
      case "applyQuaternion":
        g.applyQuaternion(new THREE.Quaternion(a[0], a[1], a[2], a[3]));
        break;
      case "deleteAttribute":
        g.deleteAttribute(a[0]);
        break;
      case "clone":
        g = g.clone();
        break;
      case "toNonIndexed":
        g = g.toNonIndexed();
        break;
      case "computeBoundingBox":
        g.computeBoundingBox();
        break;
      case "computeBoundingSphere":
        g.computeBoundingSphere();
        break;
      case "color": {
        // OptimizeGeometries: material color times 255 into a normalized Uint8Array.
        const rgb = a as number[];
        const colors = new Uint8Array(3 * g.getAttribute("position").count);
        colors.forEach((_, ndx) => {
          colors[ndx] = rgb[ndx % 3];
        });
        g.setAttribute("color", new THREE.BufferAttribute(colors, 3, true));
        break;
      }
      case "attribute": {
        // [name, typed array name, itemSize, normalized, values]
        const ctor = (globalThis as any)[a[1]];
        g.setAttribute(a[0], new THREE.BufferAttribute(new ctor(a[4]), a[2], a[3]));
        break;
      }
      case "setIndex":
        g.setIndex(a[0]);
        break;
      default:
        throw new Error(`unknown geometry op ${op}`);
    }
  }
  return g;
}

function specTree(spec: Spec): Cve1 {
  return { o: [["ctor", spec.ctor], ["args", toTree(spec.args)], ["ops", toTree(spec.ops)]] };
}

function emitGeometry(name: string, spec: Spec) {
  out({ kind: "geometry", case: name, spec: specTree(spec), out: geometryOut(build(spec)) });
}

function emitMerge(name: string, specs: Spec[], useGroups = false) {
  const merged = BufferGeometryUtils.mergeGeometries(specs.map(build), useGroups);
  out({
    kind: "merge",
    case: name,
    useGroups,
    specs: specs.map(specTree),
    out: merged === null ? null : geometryOut(merged)
  });
}

// Upstream geometry: every mesh of the procedural fields, by its recorded parameters.

const PARAMETER_ORDER: { [ctor: string]: string[] } = {
  BoxGeometry: ["width", "height", "depth", "widthSegments", "heightSegments", "depthSegments"],
  PlaneGeometry: ["width", "height", "widthSegments", "heightSegments"],
  CylinderGeometry: ["radiusTop", "radiusBottom", "height", "radialSegments", "heightSegments", "openEnded", "thetaStart", "thetaLength"],
  ConeGeometry: ["radius", "height", "radialSegments", "heightSegments", "openEnded", "thetaStart", "thetaLength"],
  SphereGeometry: ["radius", "widthSegments", "heightSegments", "phiStart", "phiLength", "thetaStart", "thetaLength"]
};

function meshSpecs(root: THREE.Object3D): { spec: Spec; mesh: THREE.Mesh }[] {
  const specs: { spec: Spec; mesh: THREE.Mesh }[] = [];
  root.updateMatrixWorld(true);
  root.traverse((object) => {
    if (!(object as any).isMesh) return;
    const mesh = object as THREE.Mesh;
    const g = mesh.geometry as any;
    const order = PARAMETER_ORDER[g.type];
    if (order === undefined) throw new Error(`unexpected geometry ${g.type}`);
    specs.push({ spec: { ctor: g.type, args: order.map((key) => g.parameters[key]), ops: [] }, mesh });
  });
  return specs;
}

const specular = new THREE.Color(0x666666);
const fields: [string, THREE.Object3D][] = [
  ["evergreen-frc", makeEvergreenField(specular, 30, false)],
  ["evergreen-ftc", makeEvergreenField(specular, 30, true)],
  ["axes-template", makeAxesTemplate(specular, 30)],
  ["axes-frc-wall-alliance", makeAxesField(specular, 30, "wall-alliance", false)],
  ["axes-ftc-center-rotated", makeAxesField(specular, 30, "center-rotated", true)]
];

for (const [fieldName, root] of fields) {
  const entries = meshSpecs(root);
  const seen = new Set<string>();
  entries.forEach(({ spec }, i) => {
    const key = JSON.stringify(spec);
    if (seen.has(key)) return;
    seen.add(key);
    emitGeometry(`${fieldName}/mesh-${i}`, spec);
  });

  // The OptimizeGeometries treatment: bake the world matrix, keep position and normal, add the
  // material color, then merge per bucket.
  const baked = entries.map(({ spec, mesh }) => {
    const rgb = ((mesh.material as THREE.MeshPhongMaterial).color.toArray() as number[]).map((v) => v * 255);
    const ops: Op[] = [["clone"], ["applyMatrix4", mesh.matrixWorld.toArray()], ["deleteAttribute", "uv"], ["color", ...rgb]];
    return { ctor: spec.ctor, args: spec.args, ops } as Spec;
  });
  if (baked.length > 0) {
    emitMerge(`${fieldName}/optimized`, baked);
    emitMerge(`${fieldName}/groups`, baked, true);
  }

  root.traverse((object) => {
    if ((object as any).isLine) {
      const position = (object as THREE.Line).geometry.getAttribute("position");
      const points: number[][] = [];
      for (let i = 0; i < position.count; i++) points.push([position.getX(i), position.getY(i), position.getZ(i)]);
      emitGeometry(`${fieldName}/outline`, { ctor: "Points", args: points, ops: [] });
    }
  });
}

// Upstream geometry built directly in the object managers.

const aprilTagThickness = 0.01;
const aprilTagOffset = -0.005 + 1e-6;
for (const [family, inches] of [
  ["36h11", 6.5],
  ["36h11", 2],
  ["36h11", 3],
  ["36h11", 4],
  ["36h11", 6],
  ["36h11", 8],
  ["16h5", 6]
] as [string, number][]) {
  let size = Units.convert(inches, "inches", "meters");
  if (family === "36h11") {
    size *= 10 / 8;
  } else {
    size *= 8 / 6;
  }
  emitGeometry(`apriltag/${family}-${inches}`, {
    ctor: "BoxGeometry",
    args: [aprilTagThickness, size, size],
    ops: [
      ["translate", aprilTagOffset, 0, 0],
      ["rotateX", Math.PI / 2]
    ]
  });
}
emitGeometry("cone-manager", { ctor: "ConeGeometry", args: [0.06, 0.25, 16, 32], ops: [] });
emitGeometry("robot/mechanism-box", { ctor: "BoxGeometry", args: [1, 1, 1], ops: [] });
emitGeometry("robot/swerve-plane", { ctor: "PlaneGeometry", args: [3, 3], ops: [["translate", 0, 0, 0.1]] });
for (const [name, w, h] of [
  ["frc", FRC_STANDARD_FIELD_LENGTH, FRC_STANDARD_FIELD_WIDTH],
  ["ftc", FTC_STANDARD_FIELD_LENGTH, FTC_STANDARD_FIELD_WIDTH],
  ["2024", 16.541, 8.211],
  ["2025", 17.548, 8.052]
] as [string, number, number][]) {
  emitGeometry(`heatmap/${name}`, { ctor: "PlaneGeometry", args: [w, h], ops: [] });
}

// Parameter sweep: defaults, odd segment counts, partial sweeps, degenerate sizes.

const edgeSegments = [undefined, 0, 1, 2, 3, 2.5, 7.9, -1, NaN];
for (const s of edgeSegments) {
  emitGeometry(`sweep/plane-seg-${s}`, { ctor: "PlaneGeometry", args: [2, 3, s, s === undefined ? undefined : 2], ops: [] });
  emitGeometry(`sweep/box-seg-${s}`, { ctor: "BoxGeometry", args: [1.5, 0.5, 2, s, 2, s], ops: [] });
  emitGeometry(`sweep/cylinder-seg-${s}`, { ctor: "CylinderGeometry", args: [0.5, 1, 2, s, s], ops: [] });
  emitGeometry(`sweep/sphere-seg-${s}`, { ctor: "SphereGeometry", args: [1, s, s], ops: [] });
}
emitGeometry("sweep/plane-default", { ctor: "PlaneGeometry", args: [], ops: [] });
emitGeometry("sweep/box-default", { ctor: "BoxGeometry", args: [], ops: [] });
emitGeometry("sweep/cylinder-default", { ctor: "CylinderGeometry", args: [], ops: [] });
emitGeometry("sweep/cone-default", { ctor: "ConeGeometry", args: [], ops: [] });
emitGeometry("sweep/sphere-default", { ctor: "SphereGeometry", args: [], ops: [] });
emitGeometry("sweep/box-negative", { ctor: "BoxGeometry", args: [-1, 2, -0.5, 2, 1, 3], ops: [] });
emitGeometry("sweep/box-zero", { ctor: "BoxGeometry", args: [0, 0, 0], ops: [] });
emitGeometry("sweep/plane-zero", { ctor: "PlaneGeometry", args: [0, -1, 3, 1], ops: [] });
emitGeometry("sweep/cylinder-open", { ctor: "CylinderGeometry", args: [1, 1, 1, 6, 2, true], ops: [] });
emitGeometry("sweep/cylinder-partial", { ctor: "CylinderGeometry", args: [0.3, 0.6, 1.2, 5, 3, false, 0.4, 2.1], ops: [] });
emitGeometry("sweep/cylinder-no-top", { ctor: "CylinderGeometry", args: [0, 0.4, 1, 9], ops: [] });
emitGeometry("sweep/cylinder-no-bottom", { ctor: "CylinderGeometry", args: [0.4, -0.1, 1, 9], ops: [] });
emitGeometry("sweep/cylinder-flat", { ctor: "CylinderGeometry", args: [1, 2, 0, 4], ops: [] });
emitGeometry("sweep/cone-partial", { ctor: "ConeGeometry", args: [0.5, 1, 7, 3, false, -1, 4], ops: [] });
emitGeometry("sweep/cone-open", { ctor: "ConeGeometry", args: [0.5, 1, 7, 1, true], ops: [] });
emitGeometry("sweep/sphere-cap", { ctor: "SphereGeometry", args: [2, 10, 6, 0, Math.PI * 2, 0, 1], ops: [] });
emitGeometry("sweep/sphere-band", { ctor: "SphereGeometry", args: [2, 10, 6, 0.3, 2, 0.5, 2], ops: [] });
emitGeometry("sweep/sphere-overshoot", { ctor: "SphereGeometry", args: [1, 5, 4, 0, 6, 0.2, 5], ops: [] });
emitGeometry("sweep/sphere-big", { ctor: "SphereGeometry", args: [3, 64, 32], ops: [] });
emitGeometry("sweep/plane-uint32-index", { ctor: "PlaneGeometry", args: [1, 1, 300, 220], ops: [] });

for (let i = 0; i < 60; i++) {
  const ctor = pick(["BoxGeometry", "PlaneGeometry", "CylinderGeometry", "ConeGeometry", "SphereGeometry"]);
  const size = () => pick([uniform(0.01, 5), uniform(-2, 2), 1]);
  const seg = () => pick([1 + below(12), uniform(0, 10)]);
  let args: unknown[];
  switch (ctor) {
    case "BoxGeometry":
      args = [size(), size(), size(), seg(), seg(), seg()];
      break;
    case "PlaneGeometry":
      args = [size(), size(), seg(), seg()];
      break;
    case "CylinderGeometry":
      args = [size(), size(), size(), seg(), seg(), rng() < 0.2, uniform(-3, 3), uniform(0.1, 7)];
      break;
    case "ConeGeometry":
      args = [size(), size(), seg(), seg(), rng() < 0.2, uniform(-3, 3), uniform(0.1, 7)];
      break;
    default:
      args = [size(), seg(), seg(), uniform(-1, 1), uniform(0.1, 7), pick([0, uniform(0, 1)]), uniform(0.1, 4)];
  }
  const ops: Op[] = [];
  const nOps = below(4);
  for (let j = 0; j < nOps; j++) {
    switch (below(6)) {
      case 0:
        ops.push(["translate", uniform(-3, 3), uniform(-3, 3), uniform(-3, 3)]);
        break;
      case 1:
        ops.push([pick(["rotateX", "rotateY", "rotateZ"]), angle()]);
        break;
      case 2:
        ops.push(["scale", uniform(-2, 2), uniform(0.1, 2), pick([1, -1, 0.5, 0])]);
        break;
      case 3:
        ops.push(["applyMatrix4", randomMatrix().toArray()]);
        break;
      case 4: {
        const q = new THREE.Quaternion(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
        ops.push(["applyQuaternion", q.x, q.y, q.z, q.w]);
        break;
      }
      default:
        ops.push([pick(["computeBoundingBox", "computeBoundingSphere"])]);
    }
  }
  emitGeometry(`random/${i}`, { ctor, args, ops });
}

emitGeometry("points/vector2", { ctor: "Points", args: [[1, 2], [3, -4], [0.1, 0.2]], ops: [] });
emitGeometry("points/vector3", { ctor: "Points", args: [[1, 2, -0], [3, -4, 5], [0.1, 0.2, NaN]], ops: [["computeBoundingSphere"]] });

function randomMatrix(): THREE.Matrix4 {
  const kind = below(4);
  if (kind === 0) {
    return new THREE.Matrix4().compose(
      new THREE.Vector3(uniform(-5, 5), uniform(-5, 5), uniform(-5, 5)),
      new THREE.Quaternion().setFromEuler(new THREE.Euler(angle(), angle(), angle(), pick(ORDERS))),
      new THREE.Vector3(uniform(-2, 2), uniform(0.1, 2), uniform(0.1, 3))
    );
  }
  const m = new THREE.Matrix4();
  for (let i = 0; i < 16; i++) m.elements[i] = kind === 1 && (i === 3 || i === 7 || i === 11) ? 0 : uniform(-2, 2);
  if (kind === 3) m.elements[15] = 1;
  return m;
}

// mergeGeometries edge cases.

const plane: Spec = { ctor: "PlaneGeometry", args: [1, 2], ops: [] };
const cyl: Spec = { ctor: "CylinderGeometry", args: [0.2, 0.2, 1, 6], ops: [["rotateZ", Math.PI / 2]] };
emitMerge("merge/plane-cylinder", [plane, cyl]);
emitMerge("merge/plane-cylinder-groups", [plane, cyl, plane], true);
emitMerge("merge/single", [cyl]);
emitMerge("merge/indexed-mismatch", [plane, { ctor: "PlaneGeometry", args: [1, 2], ops: [["toNonIndexed"]] }]);
emitMerge("merge/non-indexed", [
  { ctor: "PlaneGeometry", args: [1, 2], ops: [["toNonIndexed"]] },
  { ctor: "BoxGeometry", args: [1, 2, 3], ops: [["toNonIndexed"]] }
]);
emitMerge("merge/non-indexed-groups", [
  { ctor: "PlaneGeometry", args: [1, 2], ops: [["toNonIndexed"]] },
  { ctor: "BoxGeometry", args: [1, 2, 3], ops: [["toNonIndexed"]] }
], true);
emitMerge("merge/extra-attribute", [plane, { ...cyl, ops: [["color", 255, 0, 10]] }]);
emitMerge("merge/missing-attribute", [plane, { ...cyl, ops: [["deleteAttribute", "uv"]] }]);
emitMerge("merge/attribute-order", [
  { ctor: "PlaneGeometry", args: [1, 2], ops: [["deleteAttribute", "position"], ["attribute", "position", "Float32Array", 3, false, [0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0]]] },
  plane
]);
emitMerge("merge/item-size", [
  { ctor: "PlaneGeometry", args: [1, 2], ops: [["attribute", "uv", "Float32Array", 4, false, [0, 0, 0, 0, 1, 1, 1, 1]]] },
  plane
]);
emitMerge("merge/array-type", [
  { ctor: "PlaneGeometry", args: [1, 2], ops: [["attribute", "uv", "Float64Array", 2, false, [0, 0, 0, 0, 1, 1, 1, 1]]] },
  plane
]);
emitMerge("merge/normalized", [
  { ctor: "PlaneGeometry", args: [1, 2], ops: [["attribute", "uv", "Float32Array", 2, true, [0, 0, 0, 0, 1, 1, 1, 1]]] },
  plane
]);
emitMerge("merge/int-attributes", [
  { ctor: "PlaneGeometry", args: [1, 2], ops: [["attribute", "skin", "Int16Array", 2, true, [-32768, 32767, 5, -5, 0, 1, 2, 3]]] },
  { ctor: "PlaneGeometry", args: [3, 2], ops: [["attribute", "skin", "Int16Array", 2, true, [1, 2, 3, 4, 5, 6, 7, 8]]] }
]);
emitMerge("merge/uint32-promotion", [
  { ctor: "PlaneGeometry", args: [1, 1, 200, 163], ops: [] },
  { ctor: "PlaneGeometry", args: [1, 1, 200, 163], ops: [["translate", 1, 0, 0]] }
]);
emitMerge("merge/uint16-uint32-index", [
  { ctor: "PlaneGeometry", args: [1, 1, 300, 220], ops: [] },
  { ctor: "PlaneGeometry", args: [1, 1], ops: [] }
]);
emitMerge("merge/colors-truncate", [
  { ctor: "BoxGeometry", args: [1, 1, 1], ops: [["deleteAttribute", "uv"], ["color", 255.9, 256, -1.5]] },
  { ctor: "BoxGeometry", args: [1, 1, 1], ops: [["deleteAttribute", "uv"], ["color", 0.2 * 255, 136.00000000000003, 1e10]] }
]);

// Object3D transform sequences.

const AXES: [number, number, number][] = [
  [1, 0, 0],
  [0, 1, 0],
  [0, 0, 1]
];

function randomOp(): Op {
  switch (below(14)) {
    case 0:
      return ["position.set", uniform(-9, 9), uniform(-9, 9), uniform(-9, 9)];
    case 1:
      return ["scale.set", uniform(-2, 2), uniform(0.1, 2), pick([1, 2, -1])];
    case 2:
      return ["rotation.set", angle(), angle(), angle()];
    case 3: {
      const q = new THREE.Quaternion().setFromEuler(new THREE.Euler(angle(), angle(), angle(), pick(ORDERS)));
      return ["rotation.setFromQuaternion", q.x, q.y, q.z, q.w];
    }
    case 4: {
      const q = new THREE.Quaternion(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
      return ["quaternion.copy", q.x, q.y, q.z, q.w];
    }
    case 5:
      return ["setRotationFromAxisAngle", ...pick(AXES), angle()];
    case 6:
      return [pick(["rotateX", "rotateY", "rotateZ"]), angle()];
    case 7: {
      const axis = new THREE.Vector3(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
      return [pick(["rotateOnAxis", "rotateOnWorldAxis"]), axis.x, axis.y, axis.z, angle()];
    }
    case 8: {
      const q = new THREE.Quaternion(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
      return ["applyQuaternion", q.x, q.y, q.z, q.w];
    }
    case 9:
      return ["applyMatrix4", randomMatrix().toArray()];
    case 10:
      return [pick(["translateX", "translateY", "translateZ"]), uniform(-3, 3)];
    case 11:
      return ["lookAt", uniform(-9, 9), uniform(-9, 9), uniform(-9, 9)];
    case 12:
      return ["rotation.order", pick(ORDERS)];
    default:
      return ["updateMatrix"];
  }
}

function applyOp(o: THREE.Object3D, [op, ...a]: [string, ...any[]]) {
  switch (op) {
    case "position.set":
      o.position.set(a[0], a[1], a[2]);
      break;
    case "scale.set":
      o.scale.set(a[0], a[1], a[2]);
      break;
    case "rotation.set":
      o.rotation.set(a[0], a[1], a[2]);
      break;
    case "rotation.order":
      o.rotation.order = a[0];
      break;
    case "rotation.setFromQuaternion":
      o.rotation.setFromQuaternion(new THREE.Quaternion(a[0], a[1], a[2], a[3]));
      break;
    case "quaternion.copy":
      o.quaternion.copy(new THREE.Quaternion(a[0], a[1], a[2], a[3]));
      break;
    case "setRotationFromAxisAngle":
      o.setRotationFromAxisAngle(new THREE.Vector3(a[0], a[1], a[2]), a[3]);
      break;
    case "rotateX":
    case "rotateY":
    case "rotateZ":
    case "translateX":
    case "translateY":
    case "translateZ":
      (o as any)[op](a[0]);
      break;
    case "rotateOnAxis":
    case "rotateOnWorldAxis":
      (o as any)[op](new THREE.Vector3(a[0], a[1], a[2]), a[3]);
      break;
    case "applyQuaternion":
      o.applyQuaternion(new THREE.Quaternion(a[0], a[1], a[2], a[3]));
      break;
    case "applyMatrix4":
      o.applyMatrix4(new THREE.Matrix4().fromArray(a[0]));
      break;
    case "lookAt":
      o.lookAt(a[0], a[1], a[2]);
      break;
    case "updateMatrix":
      o.updateMatrix();
      break;
    default:
      throw new Error(`unknown object op ${op}`);
  }
}

function objectState(o: THREE.Object3D): Cve1 {
  return toTree([
    o.position.toArray(),
    [o.rotation.x, o.rotation.y, o.rotation.z, o.rotation.order],
    o.quaternion.toArray(),
    o.scale.toArray(),
    o.matrix.toArray(),
    o.matrixWorld.toArray()
  ]);
}

// The sequences upstream runs: EvergreenField and AxesTemplate place meshes with
// rotate*/position.set, Field3dRendererImpl sets coordinate groups from quaternions.
const fixedSequences: [string, Op[]][] = [
  ["guardrail", [["rotateZ", Math.PI / 2], ["position.set", 0, -4.1148, 0.508], ["updateMatrix"]]],
  ["panel", [["rotateX", Math.PI / 2], ["position.set", 0, 4.1148, 0.254], ["updateMatrix"]]],
  ["station", [["rotateY", Math.PI / 2], ["position.set", -8.2296, 1.7526, 0.9906], ["updateMatrix"]]],
  ["axes-x", [["position.set", 0.5, 0, 0], ["rotateZ", Math.PI / 2], ["updateMatrix"]]],
  ["wpilib-group", [["rotation.setFromQuaternion", -0.5, 0.5, 0.5, 0.5], ["updateMatrix"]]],
  ["identity-quaternion", [["rotation.setFromQuaternion", 0, 0, 0, 1], ["updateMatrix"]]],
  ["gimbal-lock", [["rotation.setFromQuaternion", 0, Math.SQRT1_2, 0, Math.SQRT1_2], ["updateMatrix"]]],
  ["axis-angle-pi", [["setRotationFromAxisAngle", 0, 0, 1, Math.PI], ["position.set", 8.2296, 4.1148, 0], ["updateMatrix"]]],
  ["camera-look", [["position.set", 0, 6, -12], ["lookAt", 0, 0.5, 0], ["updateMatrix"]]],
  ["look-up", [["position.set", 0, 0, 0], ["lookAt", 0, 5, 0], ["updateMatrix"]]]
];
for (const [name, ops] of fixedSequences) {
  const o = new THREE.Object3D();
  ops.forEach((op) => applyOp(o, op as any));
  out({ kind: "object", case: `fixed/${name}`, camera: false, ops: toTree(ops), out: objectState(o) });
}
for (let i = 0; i < 300; i++) {
  const camera = rng() < 0.2;
  const o = camera ? new THREE.PerspectiveCamera() : new THREE.Object3D();
  const ops: Op[] = [];
  const n = 1 + below(8);
  for (let j = 0; j < n; j++) ops.push(randomOp());
  ops.push(["updateMatrix"]);
  ops.forEach((op) => applyOp(o, op as any));
  out({ kind: "object", case: `random/${i}`, camera, ops: toTree(ops), out: objectState(o) });
}

// Scene graphs: world matrices, world position and quaternion, attach.

for (let i = 0; i < 60; i++) {
  const count = 2 + below(4);
  const nodes: { parent: number | null; ops: Op[] }[] = [];
  const objects: THREE.Object3D[] = [];
  for (let j = 0; j < count; j++) {
    const parent = j === 0 ? null : below(j);
    const ops: Op[] = [];
    const n = below(5);
    for (let k = 0; k < n; k++) ops.push(randomOp());
    const o = new THREE.Object3D();
    if (parent !== null) objects[parent].add(o);
    ops.forEach((op) => applyOp(o, op as any));
    objects.push(o);
    nodes.push({ parent, ops });
  }
  const attach = rng() < 0.3 && count > 2 ? [count - 1, below(count - 1)] : null;
  if (attach !== null) objects[attach[1]].attach(objects[attach[0]]);
  objects[0].updateMatrixWorld();
  const leaf = objects[count - 1];
  const worldPosition = leaf.getWorldPosition(new THREE.Vector3());
  const worldQuaternion = leaf.getWorldQuaternion(new THREE.Quaternion());
  out({
    kind: "graph",
    case: `graph/${i}`,
    nodes: toTree(nodes.map((n) => [n.parent, n.ops])),
    attach: toTree(attach),
    out: toTree([objects.map((o) => [o.matrixWorld.toArray(), o.position.toArray(), o.quaternion.toArray()]), worldPosition.toArray(), worldQuaternion.toArray()])
  });
}

// Pure math functions.

function emitMath(fn: string, args: unknown[], value: unknown) {
  out({ kind: "math", fn, args: toTree(args), out: toTree(value) });
}

const specialAngles = [0, -0, Math.PI, -Math.PI, Math.PI / 2, 1e-9, 3 * Math.PI, 100, NaN, Infinity];
for (let i = 0; i < 400; i++) {
  const order = pick(ORDERS);
  const e = [rng() < 0.1 ? pick(specialAngles) : angle(), rng() < 0.1 ? pick(specialAngles) : angle(), angle()];
  const q = new THREE.Quaternion().setFromEuler(new THREE.Euler(e[0], e[1], e[2], order));
  emitMath("Quaternion.setFromEuler", [...e, order], q.toArray());
  emitMath("Matrix4.makeRotationFromEuler", [...e, order], new THREE.Matrix4().makeRotationFromEuler(new THREE.Euler(e[0], e[1], e[2], order)).toArray());
  const order2 = pick(ORDERS);
  const raw = rng() < 0.3 ? q : new THREE.Quaternion(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1));
  const back = new THREE.Euler().setFromQuaternion(raw, order2);
  emitMath("Euler.setFromQuaternion", [...raw.toArray(), order2], [back.x, back.y, back.z]);
  const m = randomMatrix();
  emitMath("Euler.setFromRotationMatrix", [m.toArray(), order2], (([x, y, z]) => [x, y, z])(new THREE.Euler().setFromRotationMatrix(m, order2).toArray()));
  emitMath("Quaternion.setFromRotationMatrix", [m.toArray()], new THREE.Quaternion().setFromRotationMatrix(m).toArray());
  emitMath("Matrix4.invert", [m.toArray()], m.clone().invert().toArray());
  emitMath("Matrix4.determinant", [m.toArray()], m.determinant());
  const p = new THREE.Vector3(), dq = new THREE.Quaternion(), s = new THREE.Vector3();
  m.decompose(p, dq, s);
  emitMath("Matrix4.decompose", [m.toArray()], [p.toArray(), dq.toArray(), s.toArray()]);
  emitMath("Matrix3.getNormalMatrix", [m.toArray()], new THREE.Matrix3().getNormalMatrix(m).toArray());
  const m2 = randomMatrix();
  emitMath("Matrix4.multiplyMatrices", [m.toArray(), m2.toArray()], new THREE.Matrix4().multiplyMatrices(m, m2).toArray());
  const v = new THREE.Vector3(uniform(-9, 9), uniform(-9, 9), uniform(-9, 9));
  emitMath("Vector3.applyMatrix4", [v.toArray(), m.toArray()], v.clone().applyMatrix4(m).toArray());
  emitMath("Vector3.applyQuaternion", [v.toArray(), raw.toArray()], v.clone().applyQuaternion(raw).toArray());
  emitMath("Vector3.transformDirection", [v.toArray(), m.toArray()], v.clone().transformDirection(m).toArray());
  const axis = new THREE.Vector3(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
  const a = angle();
  emitMath("Quaternion.setFromAxisAngle", [axis.toArray(), a], new THREE.Quaternion().setFromAxisAngle(axis, a).toArray());
  emitMath("Matrix4.makeRotationAxis", [axis.toArray(), a], new THREE.Matrix4().makeRotationAxis(axis, a).toArray());
  const q2 = new THREE.Quaternion(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
  const t = pick([0, 1, 0.5, uniform(0, 1), uniform(-1, 2)]);
  const qa = raw.clone().normalize();
  const near = rng() < 0.2 ? qa.clone() : rng() < 0.2 ? new THREE.Quaternion(-qa.x, -qa.y, -qa.z, -qa.w) : q2;
  emitMath("Quaternion.slerp", [qa.toArray(), near.toArray(), t], qa.clone().slerp(near, t).toArray());
  emitMath("Quaternion.multiply", [raw.toArray(), q2.toArray()], raw.clone().multiply(q2).toArray());
  emitMath("Quaternion.angleTo", [qa.toArray(), q2.toArray()], qa.angleTo(q2));
  const step = pick([0, uniform(0, 2), 10]);
  emitMath("Quaternion.rotateTowards", [qa.toArray(), q2.toArray(), step], qa.clone().rotateTowards(q2, step).toArray());
  const to = rng() < 0.1 ? axis.clone().negate() : new THREE.Vector3(uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)).normalize();
  emitMath("Quaternion.setFromUnitVectors", [axis.toArray(), to.toArray()], new THREE.Quaternion().setFromUnitVectors(axis, to).toArray());
  const eye = new THREE.Vector3(uniform(-9, 9), uniform(-9, 9), uniform(-9, 9));
  const target = rng() < 0.1 ? eye.clone() : new THREE.Vector3(uniform(-9, 9), uniform(-9, 9), uniform(-9, 9));
  const up = pick([new THREE.Vector3(0, 1, 0), new THREE.Vector3(0, 0, 1), axis]);
  emitMath("Matrix4.lookAt", [eye.toArray(), target.toArray(), up.toArray()], new THREE.Matrix4().lookAt(eye, target, up).toArray());
  emitMath("Vector3.angleTo", [v.toArray(), to.toArray()], v.angleTo(to));
}
emitMath("Matrix4.makePerspective", [-0.4, 0.4, 0.3, -0.3, 0.1, 1000], new THREE.Matrix4().makePerspective(-0.4, 0.4, 0.3, -0.3, 0.1, 1000).toArray());
emitMath("Matrix4.makeOrthographic", [-4, 4, 3, -3, 0.1, 100], new THREE.Matrix4().makeOrthographic(-4, 4, 3, -3, 0.1, 100).toArray());
for (const [fov, aspect, near, far, zoom] of [
  [50, 1100 / 650, 0.15, 1000, 1],
  [75, 16 / 9, 0.1, 100, 1],
  [120, 0.5, 0.01, 50, 2]
]) {
  const camera = new THREE.PerspectiveCamera(fov, aspect, near, far);
  camera.zoom = zoom;
  camera.updateProjectionMatrix();
  emitMath("PerspectiveCamera.projectionMatrix", [fov, aspect, near, far, zoom], [camera.projectionMatrix.toArray(), camera.projectionMatrixInverse.toArray()]);
}

// Colors.

const colorArgs: (number | string)[] = [
  0x666666, 0x000000, 0xffffff, 0x888888, 0xdddddd, 0x6379a6, 0xa66363, 0x2020ff, 0x444444, 1193046.7, -1, 2 ** 32 + 0x10, NaN,
  "#222222", "#ffffff", "#9370db", "#888888", "#000000", "#abc", "#ABCDEF", "#12345", "#1234567", "#xyz", "",
  "rgb(255, 0, 128)", "rgba(10,20,30,0.5)", "rgb(300, 1, 2)", "rgb(10%, 50%, 100%)", "rgb(150%,0%,0%)", "rgb(1,2)",
  "hsl(120, 50%, 25%)", "hsla(0.5, 100%, 50%, 1)", "hsl(-30, 20%, 80%)", "hsl(400,10%,10%)", "foo(1,2,3)",
  "red", "Red", "REBECCAPURPLE", "notacolor", "constructor", "__proto__", "toString", " red", "#222222 "
];
for (let i = 0; i < 60; i++) colorArgs.push(Math.floor(rng() * 0x1000000));
for (const arg of colorArgs) {
  const c = new THREE.Color(arg as any);
  out({ kind: "color", arg: toTree(arg), out: toTree([c.r, c.g, c.b, c.getHex()]) });
}
