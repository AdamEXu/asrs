import type { AssetCase, FileSource } from "../genGlb";

// Hand-built GLB and glTF files for loader paths the bundled models never reach: name
// sanitizing and deduplication, materials and alpha modes, accessor layouts, primitive modes,
// node transforms, merge failures, and files GLTFLoader rejects.

let seed = 0x7e3d_0002;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const uniform = (lo: number, hi: number) => lo + (hi - lo) * rng();

const FLOAT = 5126;
const U8 = 5121;
const U16 = 5123;
const U32 = 5125;
const SIZES: { [type: string]: number } = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT4: 16 };

function encodeValues(componentType: number, values: number[]): Buffer {
  const width = componentType === FLOAT || componentType === U32 ? 4 : componentType === U16 || componentType === 5122 ? 2 : 1;
  const buffer = Buffer.alloc(values.length * width);
  values.forEach((v, i) => {
    switch (componentType) {
      case FLOAT:
        buffer.writeFloatLE(v, i * 4);
        break;
      case U32:
        buffer.writeUInt32LE(v, i * 4);
        break;
      case U16:
        buffer.writeUInt16LE(v, i * 2);
        break;
      case 5122:
        buffer.writeInt16LE(v, i * 2);
        break;
      case U8:
        buffer.writeUInt8(v, i);
        break;
      default:
        buffer.writeInt8(v, i);
    }
  });
  return buffer;
}

class Builder {
  json: any = { asset: { version: "2.0", generator: "asrs oracle" }, scene: 0, scenes: [], nodes: [], meshes: [], materials: [], accessors: [], bufferViews: [], buffers: [] };
  private chunks: Buffer[] = [];
  private length = 0;

  view(bytes: Buffer, extra: object = {}): number {
    const pad = (4 - (this.length % 4)) % 4;
    if (pad) this.push(Buffer.alloc(pad));
    const byteOffset = this.length;
    this.push(bytes);
    this.json.bufferViews.push({ buffer: 0, byteOffset, byteLength: bytes.length, ...extra });
    return this.json.bufferViews.length - 1;
  }

  private push(bytes: Buffer) {
    this.chunks.push(bytes);
    this.length += bytes.length;
  }

  accessor(componentType: number, type: string, values: number[], extra: object = {}): number {
    const def: any = { bufferView: this.view(encodeValues(componentType, values)), componentType, count: values.length / SIZES[type], type, ...extra };
    if (type === "VEC3" && componentType === FLOAT) {
      def.min = [0, 1, 2].map((c) => Math.min(...values.filter((_, i) => i % 3 === c)));
      def.max = [0, 1, 2].map((c) => Math.max(...values.filter((_, i) => i % 3 === c)));
    }
    this.json.accessors.push(def);
    return this.json.accessors.length - 1;
  }

  raw(def: object): number {
    this.json.accessors.push(def);
    return this.json.accessors.length - 1;
  }

  material(def: object): number {
    this.json.materials.push(def);
    return this.json.materials.length - 1;
  }

  mesh(primitives: object[], name?: string): number {
    this.json.meshes.push(name === undefined ? { primitives } : { name, primitives });
    return this.json.meshes.length - 1;
  }

  node(def: object): number {
    this.json.nodes.push(def);
    return this.json.nodes.length - 1;
  }

  body(): Buffer {
    const bytes = Buffer.concat(this.chunks);
    return Buffer.concat([bytes, Buffer.alloc((4 - (bytes.length % 4)) % 4)]);
  }

  finish() {
    for (const key of ["materials", "meshes", "nodes", "bufferViews", "accessors"]) {
      if (this.json[key].length === 0) delete this.json[key];
    }
  }

  glb(): Buffer {
    this.finish();
    const body = this.body();
    this.json.buffers = body.length > 0 ? [{ byteLength: body.length }] : [];
    if (this.json.buffers.length === 0) delete this.json.buffers;
    return glbContainer(JSON.stringify(this.json), body);
  }

  gltf(bufferUri: (body: Buffer) => string): Buffer {
    this.finish();
    const body = this.body();
    this.json.buffers = [{ byteLength: body.length, uri: bufferUri(body) }];
    return Buffer.from(JSON.stringify(this.json));
  }
}

function glbContainer(jsonText: string, body: Buffer): Buffer {
  let json = Buffer.from(jsonText);
  json = Buffer.concat([json, Buffer.alloc((4 - (json.length % 4)) % 4, 0x20)]);
  const chunks = [chunk(json, 0x4e4f534a)];
  if (body.length > 0) chunks.push(chunk(body, 0x004e4942));
  const header = Buffer.alloc(12);
  const total = 12 + chunks.reduce((n, c) => n + c.length, 0);
  header.writeUInt32LE(0x46546c67, 0);
  header.writeUInt32LE(2, 4);
  header.writeUInt32LE(total, 8);
  return Buffer.concat([header, ...chunks]);
}

function chunk(data: Buffer, type: number): Buffer {
  const head = Buffer.alloc(8);
  head.writeUInt32LE(data.length, 0);
  head.writeUInt32LE(type, 4);
  return Buffer.concat([head, data]);
}

type Blob = { positions: number[]; normals: number[]; indices: number[] };

function blob(center: number[], radius: number, verts = 6): Blob {
  const positions: number[] = [];
  const normals: number[] = [];
  for (let i = 0; i < verts; i++) {
    for (let c = 0; c < 3; c++) positions.push(center[c] + uniform(-radius, radius));
    const n = [uniform(-1, 1), uniform(-1, 1), uniform(-1, 1)];
    const l = Math.hypot(...n) || 1;
    normals.push(...n.map((v) => v / l));
  }
  // The vertex farthest from the centroid sits at exactly `radius` along x, so the
  // simplification threshold compares against a known distance.
  positions[0] = center[0] + radius;
  positions[1] = center[1];
  positions[2] = center[2];
  const indices: number[] = [];
  for (let i = 0; i + 2 < verts; i++) indices.push(i, i + 1, i + 2);
  return { positions, normals, indices };
}

function prim(b: Builder, g: Blob, extra: { material?: number; indexType?: number; mode?: number; indexed?: boolean } = {}): any {
  const p: any = {
    attributes: {
      POSITION: b.accessor(FLOAT, "VEC3", g.positions),
      NORMAL: b.accessor(FLOAT, "VEC3", g.normals)
    }
  };
  if (extra.indexed !== false) p.indices = b.accessor(extra.indexType ?? U16, "SCALAR", g.indices);
  if (extra.material !== undefined) p.material = extra.material;
  if (extra.mode !== undefined) p.mode = extra.mode;
  return p;
}

function at(i: number): number[] {
  return [uniform(-3, 3) + (i % 5) * 0.7, uniform(-2, 2) - (i % 3) * 1.3, uniform(-0.5, 1)];
}

function namesGlb(): Buffer {
  const b = new Builder();
  const grey = b.material({ pbrMetallicRoughness: { baseColorFactor: [0.5, 0.5, 0.5, 1] } });
  const simple = (name: string | undefined, radius: number, i: number) => b.mesh([prim(b, blob(at(i), radius), { material: grey })], name);
  const roots: number[] = [];
  const add = (def: object) => {
    const n = b.node(def);
    return n;
  };
  roots.push(add({ name: "Note 1", mesh: simple("Note 1", 0.3, 0) }));
  roots.push(add({ name: "Note 1", mesh: b.mesh([prim(b, blob(at(1), 0.2), { material: grey }), prim(b, blob(at(2), 0.03)), prim(b, blob(at(3), 0.01))], "Note 1") }));
  roots.push(add({ mesh: simple(undefined, 0.05, 4) }));
  roots.push(add({ mesh: b.mesh([prim(b, blob(at(5), 0.2)), prim(b, blob(at(6), 0.2))]) }));
  roots.push(add({ name: "a.b:c/d[e]", mesh: simple("x.y", 0.1, 7) }));
  roots.push(add({ name: "constructor", mesh: simple("constructor", 0.1, 8) }));
  roots.push(add({ name: "__proto__", mesh: simple("__proto__", 0.1, 9) }));
  roots.push(add({ name: "toString", mesh: b.mesh([prim(b, blob(at(10), 0.1)), prim(b, blob(at(11), 0.1))], "toString") }));
  roots.push(add({ name: "Tab\tName\u00a0x\u2028y\ufeffz", mesh: simple(" lead", 0.1, 12) }));
  const carpetChild = add({ name: "CARPET", mesh: simple("floor", 1.5, 13) });
  const noSimplify = add({ name: "Wall NOSIMPLIFY", mesh: simple("tiny", 0.001, 14), translation: [0.25, 0, 0] });
  roots.push(add({ name: "carpet_Area", children: [carpetChild, noSimplify], translation: [0.5, -0.25, 0] }));
  roots.push(add({ mesh: simple("Carpet thing", 0.004, 15) }));
  roots.push(add({ name: "Note_1", mesh: simple("Note_1", 0.3, 16) }));
  const shared = b.mesh([prim(b, blob(at(17), 0.4), { material: grey })], "shared mesh");
  roots.push(add({ name: "shared", mesh: shared, translation: [1, 2, 3] }));
  roots.push(add({ mesh: shared, translation: [-1, -2, 0.5] }));
  const sharedMulti = b.mesh([prim(b, blob(at(18), 0.4)), prim(b, blob(at(19), 0.4))], "multi shared");
  roots.push(add({ mesh: sharedMulti, rotation: [0, 0, Math.SQRT1_2, Math.SQRT1_2] }));
  roots.push(add({ name: "multi", mesh: sharedMulti }));
  b.json.cameras = [{ name: "cam", type: "perspective", perspective: { yfov: 0.8, znear: 0.1, aspectRatio: 1.5 } }];
  roots.push(add({ name: "camnode", mesh: simple("cam mesh", 0.2, 20), camera: 0 }));
  roots.push(add({ name: "cam only", camera: 0 }));
  roots.push(add({ name: "", mesh: simple("", 0.2, 21) }));
  roots.push(add({ name: "Note 1", children: [add({ name: "Note 1", mesh: simple("Note 1_1", 0.3, 22) })] }));
  b.json.scenes = [{ name: "Scene", nodes: roots }];
  return b.glb();
}

function materialsGlb(): Buffer {
  const b = new Builder();
  const materials: object[] = [
    { pbrMetallicRoughness: { baseColorFactor: [0.2, 0.4, 0.6, 1] } },
    { pbrMetallicRoughness: { baseColorFactor: [1 / 255, 51 / 255, 254 / 255, 1] } },
    { pbrMetallicRoughness: { baseColorFactor: [1.5, -0.25, 0.9999999999, 1] } },
    { pbrMetallicRoughness: { baseColorFactor: [0.800000011920929, 0.0029999999, 0.10000000149011612, 1] } },
    { alphaMode: "BLEND", pbrMetallicRoughness: { baseColorFactor: [0, 0.5, 1, 0.5] } },
    { alphaMode: "BLEND", pbrMetallicRoughness: { baseColorFactor: [1, 1, 1, 0.75] } },
    { alphaMode: "BLEND", pbrMetallicRoughness: { baseColorFactor: [1, 0.25, 0, 0.7499] } },
    { alphaMode: "BLEND" },
    { alphaMode: "MASK", alphaCutoff: 0.3, pbrMetallicRoughness: { baseColorFactor: [0.3, 0.3, 0.9, 0.1] } },
    { alphaMode: "OPAQUE", pbrMetallicRoughness: { baseColorFactor: [0.9, 0.1, 0.1, 0.1] } },
    {},
    { pbrMetallicRoughness: { baseColorFactor: [0.25, 0.5] } },
    { pbrMetallicRoughness: { baseColorFactor: ["0.5", null, true, "0.5"] }, alphaMode: "BLEND" },
    { extensions: { KHR_materials_unlit: {} }, pbrMetallicRoughness: { baseColorFactor: [0.1, 0.7, 0.3, 0.2] }, alphaMode: "BLEND" },
    { extensions: { KHR_materials_emissive_strength: { emissiveStrength: 2 } }, emissiveFactor: [1, 0, 0], doubleSided: true, pbrMetallicRoughness: { baseColorFactor: [0.6, 0.6, 0.6, 1], metallicFactor: 0.2 } },
    { pbrMetallicRoughness: { baseColorFactor: [0.4, 0.3, 0.2, 1], baseColorTexture: { index: 0 } } },
    { alphaMode: 7, pbrMetallicRoughness: { baseColorFactor: [0.4, 0.3, 0.2, 0.1] } }
  ];
  b.json.extensionsUsed = ["KHR_materials_unlit", "KHR_materials_emissive_strength"];
  b.json.images = [{ bufferView: b.view(Buffer.from("not an image")), mimeType: "image/png" }];
  b.json.textures = [{ source: 0 }];
  const roots: number[] = [];
  materials.forEach((m, i) => {
    const material = b.material({ name: `m${i}`, ...m });
    roots.push(b.node({ name: `mat ${i}`, mesh: b.mesh([prim(b, blob(at(i), 0.3), { material })]) }));
  });
  roots.push(b.node({ name: "default", mesh: b.mesh([prim(b, blob(at(40), 0.3))]) }));
  roots.push(b.node({ name: "shared material", mesh: b.mesh([prim(b, blob(at(41), 0.3), { material: 4 }), prim(b, blob(at(42), 0.3), { material: 0 })]) }));
  b.json.scenes = [{ nodes: roots }];
  return b.glb();
}

function geometryGlb(): Buffer {
  const b = new Builder();
  const roots: number[] = [];
  const g = () => blob(at(roots.length), 0.25, 7);
  roots.push(b.node({ name: "u8", mesh: b.mesh([prim(b, g(), { indexType: U8 })]) }));
  roots.push(b.node({ name: "u32", mesh: b.mesh([prim(b, g(), { indexType: U32 })]) }));
  roots.push(b.node({ name: "strip", mesh: b.mesh([prim(b, g(), { mode: 5 })]) }));
  roots.push(b.node({ name: "fan", mesh: b.mesh([prim(b, g(), { mode: 6 })]) }));
  roots.push(b.node({ name: "strip unindexed", mesh: b.mesh([prim(b, g(), { mode: 5, indexed: false })]) }));
  roots.push(b.node({ name: "points", mesh: b.mesh([prim(b, g(), { mode: 0 })]) }));
  roots.push(b.node({ name: "lines", mesh: b.mesh([prim(b, g(), { mode: 1 }), prim(b, g(), { mode: 3 }), prim(b, g(), { mode: 2 })]) }));
  const noNormal = g();
  roots.push(b.node({ name: "no normal", mesh: b.mesh([{ attributes: { POSITION: b.accessor(FLOAT, "VEC3", noNormal.positions) }, indices: b.accessor(U16, "SCALAR", noNormal.indices) }]) }));
  {
    const e = g();
    const p = prim(b, e);
    p.attributes.TEXCOORD_0 = b.accessor(FLOAT, "VEC2", e.positions.slice(0, 14));
    p.attributes.COLOR_0 = b.accessor(FLOAT, "VEC4", [...e.normals, ...e.normals.slice(0, 7)]);
    p.attributes.TANGENT = b.accessor(FLOAT, "VEC4", [...e.normals, ...e.normals.slice(0, 7)]);
    p.attributes._CUSTOM = b.accessor(FLOAT, "SCALAR", e.positions.slice(0, 7));
    roots.push(b.node({ name: "extra attributes", mesh: b.mesh([p]) }));
  }
  const glass = b.material({ alphaMode: "BLEND", pbrMetallicRoughness: { baseColorFactor: [0.5, 0.9, 0.5, 0.4] } });
  for (const name of ["interleaved", "interleaved again"]) roots.push(interleavedNode(b, g(), name, glass));
  {
    const e = g();
    const sparse = {
      count: 2,
      indices: { bufferView: b.view(encodeValues(U16, [1, 5])), componentType: U16 },
      values: { bufferView: b.view(encodeValues(FLOAT, [0.7, -0.1, 0.2, 0.3, 0.9, -0.4])) }
    };
    const position = b.accessor(FLOAT, "VEC3", e.positions, { sparse });
    const normal = b.raw({ componentType: FLOAT, count: 7, type: "VEC3", sparse });
    roots.push(b.node({ name: "sparse", mesh: b.mesh([{ attributes: { POSITION: position, NORMAL: normal }, indices: b.accessor(U16, "SCALAR", e.indices) }]) }));
  }
  {
    const e = g();
    const p = prim(b, e);
    p.attributes.position = b.accessor(FLOAT, "VEC3", e.normals);
    roots.push(b.node({ name: "duplicate attribute", mesh: b.mesh([p]) }));
  }
  {
    const e = g();
    roots.push(
      b.node({
        name: "normal first",
        mesh: b.mesh([{ attributes: { NORMAL: b.accessor(FLOAT, "VEC3", e.normals), POSITION: b.accessor(FLOAT, "VEC3", e.positions) }, indices: b.accessor(U16, "SCALAR", e.indices) }])
      })
    );
  }
  {
    const e = g();
    const p = prim(b, e);
    const q = prim(b, e);
    q.attributes = p.attributes;
    q.indices = p.indices;
    roots.push(b.node({ name: "same primitive", mesh: b.mesh([p, q]) }));
  }
  roots.push(b.node({ name: "empty mesh", mesh: b.mesh([{ attributes: { POSITION: b.raw({ componentType: FLOAT, count: 0, type: "VEC3" }), NORMAL: b.raw({ componentType: FLOAT, count: 0, type: "VEC3" }) }, indices: b.raw({ componentType: U16, count: 0, type: "SCALAR" }) }]) }));
  b.json.scenes = [{ nodes: roots }];
  return b.glb();
}

function interleavedNode(b: Builder, e: Blob, name: string, material?: number): number {
  const interleaved: number[] = [];
  for (let i = 0; i < 7; i++) interleaved.push(...e.positions.slice(i * 3, i * 3 + 3), ...e.normals.slice(i * 3, i * 3 + 3));
  const view = b.view(encodeValues(FLOAT, interleaved), { byteStride: 24 });
  const position = b.raw({ bufferView: view, byteOffset: 0, componentType: FLOAT, count: 7, type: "VEC3" });
  const normal = b.raw({ bufferView: view, byteOffset: 12, componentType: FLOAT, count: 7, type: "VEC3" });
  const primitive: any = { attributes: { POSITION: position, NORMAL: normal }, indices: b.accessor(U16, "SCALAR", e.indices) };
  if (material !== undefined) primitive.material = material;
  return b.node({ name, mesh: b.mesh([primitive]) });
}

function interleavedMixGlb(): Buffer {
  const b = new Builder();
  b.json.scenes = [{ nodes: [b.node({ name: "plain", mesh: b.mesh([prim(b, blob([0, 0, 0], 0.3, 7))]) }), interleavedNode(b, blob([1, 0, 0], 0.3, 7), "interleaved")] }];
  return b.glb();
}

function hierarchyGlb(): Buffer {
  const b = new Builder();
  const mesh = (i: number) => b.mesh([prim(b, blob(at(i), 0.2))]);
  const leaf = b.node({ name: "leaf", mesh: mesh(0), translation: [0.1, 0.2, 0.3], rotation: [0.1, 0.2, 0.3, 0.927361849549570], scale: [2, 0.5, 1.5] });
  const flipped = b.node({ name: "flipped", mesh: mesh(1), scale: [-1, 1, 1] });
  const matrixNode = b.node({
    name: "matrix",
    mesh: mesh(2),
    matrix: [0.36, 0.48, -0.8, 0, -0.8, 0.6, 0, 0, 0.48, 0.64, 0.6, 0, 1.5, -2.25, 0.125, 1],
    children: [leaf]
  });
  const skewed = b.node({ name: "skewed", mesh: mesh(3), matrix: [1, 0.2, 0, 0, 0, 1, 0.3, 0, 0.1, 0, 2, 0, 0, 0, 0, 1], translation: [9, 9, 9] });
  const middle = b.node({ name: "middle", translation: [-1, 0.5, 0.25], rotation: [0, 0.38268343236509, 0, 0.923879532511287], children: [matrixNode, flipped] });
  const top = b.node({ name: "top", mesh: mesh(4), scale: [0.5, 0.5, 0.5], children: [middle, skewed] });
  const other = b.node({ name: "other scene", mesh: mesh(5) });
  const unused = b.node({ name: "unused", mesh: mesh(6) });
  void unused;
  b.json.scenes = [{ name: "first", nodes: [other] }, { name: "first", nodes: [top, other] }];
  b.json.scene = 1;
  return b.glb();
}

function mixedIndexGlb(): Buffer {
  const b = new Builder();
  const blend = b.material({ alphaMode: "BLEND", pbrMetallicRoughness: { baseColorFactor: [0.2, 0.2, 1, 0.3] } });
  const roots = [
    b.node({ name: "indexed", mesh: b.mesh([prim(b, blob(at(0), 0.4))]) }),
    b.node({ name: "unindexed", mesh: b.mesh([prim(b, blob(at(1), 0.4), { indexed: false })]) }),
    b.node({ name: "glass", mesh: b.mesh([prim(b, blob(at(2), 0.4), { material: blend })]) })
  ];
  b.json.scenes = [{ nodes: roots }];
  return b.glb();
}

function bigIndexGlb(): Buffer {
  const b = new Builder();
  const zeros = (count: number) => ({
    attributes: { POSITION: b.raw({ componentType: FLOAT, count, type: "VEC3" }), NORMAL: b.raw({ componentType: FLOAT, count, type: "VEC3" }) },
    indices: b.accessor(U16, "SCALAR", [0, 1, 2])
  });
  b.json.scenes = [
    {
      nodes: [
        b.node({ name: "big NOSIMPLIFY", mesh: b.mesh([zeros(70000)]) }),
        b.node({ name: "after NOSIMPLIFY", mesh: b.mesh([zeros(3)]) }),
        b.node({ name: "visible", mesh: b.mesh([prim(b, blob([1, 1, 1], 0.5))]) })
      ]
    }
  ];
  return b.glb();
}

function fieldGlb(): Buffer {
  const b = new Builder();
  const blend = b.material({ alphaMode: "BLEND", pbrMetallicRoughness: { baseColorFactor: [0.6, 0.6, 0.7, 0.25] } });
  const carpet = b.material({ pbrMetallicRoughness: { baseColorFactor: [0.3, 0.3, 0.3, 1] } });
  const roots: number[] = [];
  roots.push(b.node({ name: "Carpet", mesh: b.mesh([prim(b, blob([0, 0, 0], 4), { material: carpet })]) }));
  roots.push(b.node({ name: "carpet tile", mesh: b.mesh([prim(b, blob([2.5, -1.5, 0], 0.3), { material: carpet })]) }));
  for (let i = 0; i < 40; i++) {
    const center = [uniform(-9, 9), uniform(-5, 5), uniform(0, 1)];
    const radius = [0.01, 0.03, 0.05, 0.2, 0.45, 0.6, 1.2][i % 7];
    const material = i % 4 === 0 ? blend : undefined;
    roots.push(b.node({ name: `part ${i % 9}`, mesh: b.mesh([prim(b, blob(center, radius), { material })]) }));
  }
  roots.push(b.node({ name: "far", mesh: b.mesh([prim(b, blob([40.5, -33.25, 0], 0.2))]) }));
  roots.push(b.node({ name: "negative edge", mesh: b.mesh([prim(b, blob([-1e-9, -0.5, 0], 0.1))]) }));
  const pieceA = b.node({ name: "Note", mesh: b.mesh([prim(b, blob([1, 1, 0.1], 0.15))]), translation: [0.5, 0, 0] });
  const pieceB = b.node({ name: "Note", mesh: b.mesh([prim(b, blob([2, 1, 0.1], 0.15), { material: blend })]) });
  const pieceC = b.node({ name: "Cone Group", rotation: [0, 0, 0.6, 0.8], scale: [2, 2, 2], children: [b.node({ name: "Cone", mesh: b.mesh([prim(b, blob([-2, 0.5, 0.2], 0.005))]) })] });
  const carpetPiece = b.node({ name: "loose carpet", mesh: b.mesh([prim(b, blob([3, 3, 0], 0.1), { material: carpet })]) });
  roots.push(b.node({ name: "pieces", translation: [0.25, -0.5, 0.05], rotation: [0.2, 0, 0, 0.9797958971132712], children: [pieceA, pieceB, pieceC, carpetPiece] }));
  b.json.scenes = [{ nodes: roots }];
  return b.glb();
}

function pieceGlb(transparentOnly: boolean): Buffer {
  const b = new Builder();
  const material = transparentOnly ? b.material({ alphaMode: "BLEND", pbrMetallicRoughness: { baseColorFactor: [1, 0.5, 0, 0.5] } }) : b.material({ pbrMetallicRoughness: { baseColorFactor: [1, 0.5, 0, 1] } });
  b.json.scenes = [
    {
      nodes: [
        b.node({ name: "piece", mesh: b.mesh([prim(b, blob([0.1, 0, 0.05], 0.12), { material })]) }),
        b.node({ name: "piece detail", mesh: b.mesh([prim(b, blob([0, 0.05, 0], 0.003))]) })
      ]
    }
  ];
  return b.glb();
}

function nanFieldGlb(): Buffer {
  const b = new Builder();
  const g = blob([0, 0, 0], 0.2);
  g.positions[4] = NaN;
  b.json.scenes = [{ nodes: [b.node({ name: "bad NOSIMPLIFY", mesh: b.mesh([prim(b, g)]) }), b.node({ name: "good", mesh: b.mesh([prim(b, blob([1, 0, 0], 0.5))]) })] }];
  return b.glb();
}

// Meshes whose largest vertex distance is the nearest float32 below and above each
// simplification limit, so the comparison and the limit arithmetic are pinned.
function thresholdsGlb(): Buffer {
  const b = new Builder();
  const roots: number[] = [];
  const f32 = new Float32Array(1);
  const nearest = (v: number, direction: number) => {
    f32[0] = v;
    const bits = new Uint32Array(f32.buffer);
    if (direction < 0 ? f32[0] >= v : f32[0] <= v) bits[0] += direction;
    return f32[0];
  };
  [0.08, 0.04, 0.02, 0.08 * 0.25, 0.04 * 0.25, 0.02 * 0.25].forEach((limit, i) => {
    for (const direction of [-1, 1]) {
      const r = nearest(limit, direction);
      const g = { positions: [-r, 0, 0, r, 0, 0, 0, 0, 0], normals: [0, 0, 1, 0, 0, 1, 0, 0, 1], indices: [0, 1, 2] };
      roots.push(b.node({ name: `limit ${i} ${direction}`, mesh: b.mesh([prim(b, g)]) }));
    }
  });
  b.json.scenes = [{ nodes: roots }];
  return b.glb();
}

function simpleBuilder(): Builder {
  const b = new Builder();
  b.json.scenes = [{ nodes: [b.node({ name: "one", mesh: b.mesh([prim(b, blob([0, 0, 0], 0.5))]) })] }];
  return b;
}

const hexOf = (bytes: Buffer): FileSource => ({ hex: bytes.toString("hex") });

const ROBOT = {
  name: "Synthetic",
  isFTC: false,
  rotations: [
    { axis: "x", degrees: 90 },
    { axis: "z", degrees: -37.5 }
  ],
  position: [0.1, -0.2, 0.05],
  cameras: [],
  components: []
};

function robotCases(name: string, model: Buffer, components: Buffer[] = []): AssetCase[] {
  const files: [string, FileSource][] = [["model.glb", hexOf(model)], ...components.map((c, i): [string, FileSource] => [`model_${i}.glb`, hexOf(c)])];
  const base = { ...ROBOT, components: components.map(() => ({ zeroedRotations: [{ axis: "y", degrees: 10 }], zeroedPosition: [0, 0, 0.1] })) };
  return [
    { case: `${name}/frc`, kind: "robot", config: JSON.stringify(base), files },
    { case: `${name}/ftc`, kind: "robot", config: JSON.stringify({ ...base, isFTC: true }), files },
    { case: `${name}/no-simplification`, kind: "robot", config: JSON.stringify({ ...base, disableSimplification: true }), files }
  ];
}

const once = ["standard"] as const;

export function syntheticCases(): AssetCase[] {
  const names = namesGlb();
  const materials = materialsGlb();
  const geometry = geometryGlb();
  const hierarchy = hierarchyGlb();
  const cases: AssetCase[] = [
    ...robotCases("names", names),
    ...robotCases("materials", materials),
    ...robotCases("geometry", geometry, [hierarchy]),
    ...robotCases("hierarchy", hierarchy),
    ...robotCases("mixed-index", mixedIndexGlb()),
    ...robotCases("interleaved-mix", interleavedMixGlb()),
    ...robotCases("thresholds", simpleBuilder().glb(), [thresholdsGlb()]),
    ...robotCases("big-index", bigIndexGlb())
  ];

  const field = {
    name: "Synthetic Field",
    isFTC: false,
    coordinateSystem: "wall-blue",
    rotations: [{ axis: "z", degrees: 180 }],
    position: [0.5, -0.25, 0],
    widthInches: 651.25,
    heightInches: 315.5,
    gamePieces: [
      { name: "Note", rotations: [{ axis: "x", degrees: 90 }], position: [0, 0, 0.025], stagedObjects: ["Note", "Note_1", "Note", "Cone_Group", "nothing"] },
      { name: "7", rotations: [], position: [0, 0, 0], stagedObjects: ["loose_carpet"] },
      { name: "Glass", rotations: [], position: [0, 0, 0], stagedObjects: [] },
      { name: "Note", rotations: [{ axis: "y", degrees: -15 }], position: [1, 0, 0], stagedObjects: ["part_3"] }
    ]
  };
  const pieceFiles: [string, FileSource][] = [
    ["model_0.glb", hexOf(pieceGlb(false))],
    ["model_1.glb", hexOf(pieceGlb(false))],
    ["model_2.glb", hexOf(pieceGlb(true))],
    ["model_3.glb", hexOf(names)]
  ];
  cases.push({ case: "field/synthetic", kind: "field", config: JSON.stringify(field), files: [["model.glb", hexOf(fieldGlb())], ...pieceFiles] });
  cases.push({ case: "field/synthetic-ftc", kind: "field", config: JSON.stringify({ ...field, isFTC: true }), files: [["model.glb", hexOf(fieldGlb())], ...pieceFiles] });
  cases.push({ case: "field/names", kind: "field", config: JSON.stringify({ ...field, gamePieces: [{ name: "n", rotations: [], position: [0, 0, 0], stagedObjects: ["Note_1_1", "constructor_NaN", "carpet_Area", "Note_2", "abcde", "Tab_Name_x_y_z", "Carpet_thing", "mesh_2", "mesh_3_1", "toString_NaN", "__proto___NaN", "shared_mesh_instance_1", "multi_shared_instance_0", "cam", "camnode", "Note_1_3", "Scene"] }] }), files: [["model.glb", hexOf(names)], ["model_0.glb", hexOf(materials)]] });
  cases.push({ case: "field/geometry", kind: "field", config: JSON.stringify({ ...field, gamePieces: [] }), files: [["model.glb", hexOf(geometry)]] });
  cases.push({ case: "field/nan", kind: "field", config: JSON.stringify({ ...field, gamePieces: [] }), files: [["model.glb", hexOf(nanFieldGlb())]], modes: once });
  cases.push({ case: "field/missing-piece", kind: "field", config: JSON.stringify(field), files: [["model.glb", hexOf(fieldGlb())], ["model_0.glb", hexOf(pieceGlb(false))]], modes: once });

  const robot = (name: string, files: [string, FileSource][]): AssetCase => ({ case: name, kind: "robot", config: JSON.stringify(ROBOT), files, modes: once });
  const valid = simpleBuilder().glb();
  cases.push(robot("error/bad-header", [["model.glb", hexOf(Buffer.from("glTF but not really, just text"))]]));
  cases.push(robot("error/not-json", [["model.glb", hexOf(Buffer.from("{not json"))]]));
  cases.push(robot("error/truncated", [["model.glb", hexOf(valid.subarray(0, 40))]]));
  {
    const b = simpleBuilder();
    b.json.asset.version = "1.0";
    cases.push(robot("error/version-1", [["model.glb", hexOf(b.glb())]]));
  }
  {
    const b = simpleBuilder();
    b.json.accessors[0].byteOffset = 2;
    cases.push(robot("error/misaligned", [["model.glb", hexOf(b.glb())]]));
  }
  {
    const b = simpleBuilder();
    b.json.accessors[1].count = 100000;
    cases.push(robot("error/out-of-range", [["model.glb", hexOf(b.glb())]]));
  }
  {
    const b = simpleBuilder();
    b.json.meshes[0].primitives[0].material = 3;
    cases.push(robot("error/missing-material", [["model.glb", hexOf(b.glb())]]));
  }
  {
    const b = simpleBuilder();
    b.json.extensionsUsed = ["KHR_draco_mesh_compression"];
    b.json.extensionsRequired = ["KHR_draco_mesh_compression"];
    cases.push(robot("error/draco", [["model.glb", hexOf(b.glb())]]));
  }
  {
    const b = simpleBuilder();
    b.json.extensionsUsed = ["EXT_meshopt_compression"];
    b.json.extensionsRequired = ["EXT_meshopt_compression"];
    cases.push(robot("meshopt-required", [["model.glb", hexOf(b.glb())]]));
  }
  {
    const b = simpleBuilder();
    b.json.extensionsUsed = ["ASRS_unknown", "KHR_texture_transform"];
    b.json.extensionsRequired = ["ASRS_unknown"];
    cases.push(robot("unknown-required-extension", [["model.glb", hexOf(b.glb())]]));
  }
  {
    const b = simpleBuilder();
    const text = b.gltf((body) => "data:application/octet-stream;base64," + body.toString("base64"));
    cases.push(robot("gltf/data-uri", [["model.glb", hexOf(text)]]));
  }
  {
    const b = simpleBuilder();
    let bin = Buffer.alloc(0);
    const text = b.gltf((body) => {
      bin = body;
      return "model%20data.bin";
    });
    cases.push(robot("gltf/external-buffer", [["model.glb", hexOf(text)], ["model data.bin", hexOf(bin)]]));
    cases.push(robot("error/missing-buffer", [["model.glb", hexOf(text)]]));
  }
  cases.push(robot("error/missing-component", [["model.glb", hexOf(valid)]]));
  cases[cases.length - 1].config = JSON.stringify({ ...ROBOT, components: [{ zeroedRotations: [], zeroedPosition: [0, 0, 0] }] });
  return cases;
}
