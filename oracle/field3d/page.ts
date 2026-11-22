// Builds a reference scene with upstream's three 0.168 and renders it the way
// Field3dRendererImpl does (standard mode, or cinematic with `shadows`), then reads the WebGL
// drawing buffer right after render(), before the compositor sees it.
import * as THREE from "three";
import { Line2 } from "three/examples/jsm/lines/Line2.js";
import { LineGeometry } from "three/examples/jsm/lines/LineGeometry.js";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import ObjectManager from "@upstream/shared/renderers/field3d/ObjectManager";
import AprilTagManager from "@upstream/shared/renderers/field3d/objectManagers/AprilTagManager";
import AxesManager from "@upstream/shared/renderers/field3d/objectManagers/AxesManager";
import ConeManager from "@upstream/shared/renderers/field3d/objectManagers/ConeManager";
import HeatmapManager from "@upstream/shared/renderers/field3d/objectManagers/HeatmapManager";
import TrajectoryManager from "@upstream/shared/renderers/field3d/objectManagers/TrajectoryManager";

type Vec3 = [number, number, number];

type MaterialSpec = {
  color?: number;
  specular?: number;
  shininess?: number;
  emissive?: number;
  side?: "front" | "back" | "double";
  transparent?: boolean;
  opacity?: number;
  depthWrite?: boolean;
  depthTest?: boolean;
  vertexColors?: boolean;
  map?: { checker: [number, number, number, number]; filter?: "nearest" | "linear" };
};

// A "manager" node is a group that one of upstream's object managers fills: it is constructed
// with `specular`/`shininess` in standard mode and given each of `commands` in turn. AprilTag
// textures load asynchronously, so the capture sees the white placeholder material.
type NodeSpec = {
  type: "group" | "mesh" | "instanced" | "hemisphere" | "point" | "spot" | "line2" | "line" | "grid" | "manager";
  manager?: "aprilTag" | "axes" | "cone" | "heatmap" | "trajectory";
  commands?: any[];
  specular?: number;
  shininess?: number;
  field?: { widthInches: number; heightInches: number; coordinateSystem: string };
  name?: string;
  parent?: string;
  position?: Vec3;
  rotation?: Vec3;
  scale?: Vec3;
  visible?: boolean;
  renderOrder?: number;
  geometry?: { type: string; args: (number | boolean)[]; palette?: Vec3[] };
  material?: MaterialSpec;
  count?: number;
  instances?: { position: Vec3; rotation?: Vec3 }[];
  color?: number;
  groundColor?: number;
  intensity?: number;
  distance?: number;
  decay?: number;
  angle?: number;
  penumbra?: number;
  target?: Vec3;
  castShadow?: boolean;
  receiveShadow?: boolean;
  frustumCulled?: boolean;
  shadow?: { mapSize?: number; bias?: number };
  points?: number[] | Vec3[];
  linewidth?: number;
  size?: number;
  divisions?: number;
  color1?: number;
  color2?: number;
};

type SceneSpec = {
  size: [number, number];
  antialias: boolean;
  shadows?: boolean;
  fov: number;
  camera: { position: Vec3; target: Vec3 };
  nodes: NodeSpec[];
};

function geometry(spec: NonNullable<NodeSpec["geometry"]>): THREE.BufferGeometry {
  const a = spec.args as any[];
  let g: THREE.BufferGeometry;
  switch (spec.type) {
    case "box":
      g = new THREE.BoxGeometry(a[0], a[1], a[2], a[3], a[4], a[5]);
      break;
    case "plane":
      g = new THREE.PlaneGeometry(a[0], a[1], a[2], a[3]);
      break;
    case "cylinder":
      g = new THREE.CylinderGeometry(a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]);
      break;
    case "cone":
      g = new THREE.ConeGeometry(a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
      break;
    case "sphere":
      g = new THREE.SphereGeometry(a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
      break;
    default:
      throw new Error(`unknown geometry ${spec.type}`);
  }
  if (spec.palette) {
    const count = g.getAttribute("position").count;
    const colors = new Uint8Array(count * 3);
    for (let i = 0; i < count; i++) colors.set(spec.palette[i % spec.palette.length], i * 3);
    g.setAttribute("color", new THREE.BufferAttribute(colors, 3, true));
  }
  return g;
}

function checker([size, cell, a, b]: [number, number, number, number]): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const context = canvas.getContext("2d")!;
  const image = context.createImageData(size, size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const on = (Math.floor(x / cell) + Math.floor(y / cell)) % 2 === 0;
      const hex = on ? a : b;
      const o = (y * size + x) * 4;
      image.data[o] = (hex >> 16) & 255;
      image.data[o + 1] = (hex >> 8) & 255;
      image.data[o + 2] = hex & 255;
      image.data[o + 3] = 255;
    }
  }
  context.putImageData(image, 0, 0);
  return canvas;
}

function material(spec: MaterialSpec): THREE.MeshPhongMaterial {
  const m = new THREE.MeshPhongMaterial();
  if (spec.color !== undefined) m.color = new THREE.Color(spec.color);
  if (spec.specular !== undefined) m.specular = new THREE.Color(spec.specular);
  if (spec.shininess !== undefined) m.shininess = spec.shininess;
  if (spec.emissive !== undefined) m.emissive = new THREE.Color(spec.emissive);
  if (spec.side !== undefined) m.side = { front: THREE.FrontSide, back: THREE.BackSide, double: THREE.DoubleSide }[spec.side];
  if (spec.transparent !== undefined) m.transparent = spec.transparent;
  if (spec.opacity !== undefined) m.opacity = spec.opacity;
  if (spec.depthWrite !== undefined) m.depthWrite = spec.depthWrite;
  if (spec.depthTest !== undefined) m.depthTest = spec.depthTest;
  if (spec.vertexColors !== undefined) m.vertexColors = spec.vertexColors;
  if (spec.map !== undefined) {
    const texture = new THREE.CanvasTexture(checker(spec.map.checker));
    if (spec.map.filter === "nearest") {
      texture.minFilter = THREE.NearestFilter;
      texture.magFilter = THREE.NearestFilter;
    }
    m.map = texture;
  }
  return m;
}

function place(object: THREE.Object3D, spec: { position?: Vec3; rotation?: Vec3; scale?: Vec3 }) {
  if (spec.position) object.position.set(...spec.position);
  if (spec.rotation) object.rotation.set(spec.rotation[0], spec.rotation[1], spec.rotation[2]);
  if (spec.scale) object.scale.set(...spec.scale);
}

function build(spec: SceneSpec, scene: THREE.Scene) {
  const named = new Map<string, THREE.Object3D>();
  for (const node of spec.nodes) {
    const parent = node.parent === undefined ? scene : named.get(node.parent)!;
    let object: THREE.Object3D;
    let spotTarget: THREE.Object3D | null = null;
    switch (node.type) {
      case "group":
        object = new THREE.Group();
        break;
      case "mesh":
        object = new THREE.Mesh(geometry(node.geometry!), material(node.material ?? {}));
        break;
      case "instanced": {
        const mesh = new THREE.InstancedMesh(geometry(node.geometry!), material(node.material ?? {}), node.count!);
        mesh.frustumCulled = false;
        const dummy = new THREE.Object3D();
        for (let i = 0; i < node.count!; i++) {
          const instance = node.instances![i];
          if (instance) {
            dummy.position.set(...instance.position);
            const r = instance.rotation ?? [0, 0, 0];
            dummy.rotation.set(r[0], r[1], r[2]);
          } else {
            dummy.position.set(1e6, 1e6, 1e6);
          }
          dummy.updateMatrix();
          mesh.setMatrixAt(i, dummy.matrix);
        }
        mesh.instanceMatrix.needsUpdate = true;
        mesh.computeBoundingBox();
        object = mesh;
        break;
      }
      case "hemisphere":
        object = new THREE.HemisphereLight(node.color, node.groundColor, node.intensity);
        break;
      case "point":
        object = new THREE.PointLight(node.color, node.intensity, node.distance, node.decay);
        break;
      case "spot": {
        const light = new THREE.SpotLight(node.color, node.intensity, node.distance, node.angle, node.penumbra, node.decay);
        light.target.position.set(...node.target!);
        if (node.castShadow !== undefined) light.castShadow = node.castShadow;
        if (node.shadow?.mapSize !== undefined) {
          light.shadow.mapSize.width = node.shadow.mapSize;
          light.shadow.mapSize.height = node.shadow.mapSize;
        }
        if (node.shadow?.bias !== undefined) light.shadow.bias = node.shadow.bias;
        spotTarget = light.target;
        object = light;
        break;
      }
      case "line2": {
        const geometry = new LineGeometry();
        geometry.setPositions(node.points as number[]);
        object = new Line2(geometry, new LineMaterial({ color: node.color, linewidth: node.linewidth }));
        break;
      }
      case "line":
        object = new THREE.Line(
          new THREE.BufferGeometry().setFromPoints((node.points as Vec3[]).map((p) => new THREE.Vector3(...p))),
          new THREE.LineBasicMaterial({ color: node.color })
        );
        break;
      case "grid":
        object = new THREE.GridHelper(node.size, node.divisions, node.color1, node.color2);
        break;
      case "manager": {
        object = new THREE.Group();
        const args = [object, new THREE.Color(node.specular ?? 0), node.shininess ?? 0, "standard", false, () => {}] as const;
        const field = node.field ?? null;
        const managers: Record<string, () => ObjectManager<any>> = {
          aprilTag: () => new AprilTagManager(...args),
          axes: () => new AxesManager(...args),
          cone: () => new ConeManager(...args),
          heatmap: () => new HeatmapManager(...args, () => field as any),
          trajectory: () => new TrajectoryManager(...args)
        };
        const manager = managers[node.manager!]();
        for (const command of node.commands!) manager.setObjectData(command);
        break;
      }
    }
    place(object, node);
    if (node.visible !== undefined) object.visible = node.visible;
    if (node.renderOrder !== undefined) object.renderOrder = node.renderOrder;
    if (node.frustumCulled !== undefined) object.frustumCulled = node.frustumCulled;
    if (node.castShadow !== undefined && !(object instanceof THREE.Light)) object.castShadow = node.castShadow;
    if (node.receiveShadow !== undefined) object.receiveShadow = node.receiveShadow;
    parent.add(object);
    if (spotTarget !== null) parent.add(spotTarget);
    if (node.name !== undefined) named.set(node.name, object);
  }
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

(window as any).renderScene = (spec: SceneSpec, dark: boolean, scale: number) => {
  const canvas = document.createElement("canvas");
  document.body.appendChild(canvas);
  const renderer = new THREE.WebGLRenderer({ canvas, powerPreference: "default", antialias: spec.antialias });
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.shadowMap.enabled = spec.shadows === true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(spec.fov, 2, 0.15, 1000);
  build(spec, scene);
  camera.position.set(...spec.camera.position);
  camera.lookAt(...spec.camera.target);

  const [width, height] = spec.size;
  renderer.setSize(width, height, false);
  camera.aspect = width / height;
  camera.updateProjectionMatrix();
  scene.background = dark ? new THREE.Color("#222222") : new THREE.Color("#ffffff");
  renderer.setPixelRatio(scale);
  renderer.render(scene, camera);

  const gl = renderer.getContext() as WebGL2RenderingContext;
  const w = gl.drawingBufferWidth;
  const h = gl.drawingBufferHeight;
  const pixels = new Uint8Array(w * h * 4);
  gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
  const flipped = new Uint8Array(pixels.length);
  const row = w * 4;
  for (let y = 0; y < h; y++) flipped.set(pixels.subarray((h - 1 - y) * row, (h - y) * row), y * row);
  const shadowMaps: { width: number; height: number; rgba: string }[] = [];
  if (spec.shadows === true && scale === 1 && dark) {
    scene.traverse((object) => {
      if (object instanceof THREE.SpotLight && object.castShadow && object.shadow.map !== null) {
        const map = object.shadow.map;
        const texels = new Uint8Array(map.width * map.height * 4);
        renderer.readRenderTargetPixels(map, 0, 0, map.width, map.height, texels);
        shadowMaps.push({ width: map.width, height: map.height, rgba: toBase64(texels) });
      }
    });
  }
  const debug = gl.getExtension("WEBGL_debug_renderer_info");
  const glRenderer = debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
  const samples = gl.getParameter(gl.SAMPLES);
  renderer.dispose();
  renderer.forceContextLoss();
  canvas.remove();
  return { width: w, height: h, rgba: toBase64(flipped), glRenderer, samples, shadowMaps };
};
