import * as THREE from "three";
import makeAxesField from "@upstream/shared/renderers/field3d/AxesField";
import makeAxesTemplate from "@upstream/shared/renderers/field3d/AxesTemplate";
import makeEvergreenField from "@upstream/shared/renderers/field3d/EvergreenField";
import { toTree } from "../cve1";

// Builds one upstream procedural field and returns every object under it in traverse order
// (parent first), as the state three's toJSON serializes, read from the live objects:
//
//   args: {specular: hex, shininess, isFTC?, coordinateSystem?}
//   node: [parent index | null, type, name, visible, castShadow, receiveShadow, frustumCulled,
//          renderOrder, position, quaternion, scale, matrixWorld, geometry, material]
//   geometry: null | [rank of geometry.id among the field's geometries, FNV-1a 64 digest of the
//             CVE-1 text of [index, [[name, itemSize, normalized, array]...], groups]]
//   material: null | [rank of material.id among the field's materials, type, ...properties]
//
// Ranks make shared geometries and materials (Object3D.clone shares both) and their creation
// order visible without depending on three's global id counters. matrixWorld is read after
// root.updateMatrixWorld(true), as the first render computes it.

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

function ranks(ids: number[]): Map<number, number> {
  const sorted = [...new Set(ids)].sort((a, b) => a - b);
  return new Map(sorted.map((id, rank) => [id, rank]));
}

function materialOf(m: THREE.Material, rank: number): unknown[] {
  if (m instanceof THREE.MeshPhongMaterial) {
    return [
      rank,
      m.type,
      m.color.toArray(),
      m.specular.toArray(),
      m.shininess,
      m.emissive.toArray(),
      m.emissiveIntensity,
      m.opacity,
      m.transparent,
      m.side,
      m.depthTest,
      m.depthWrite,
      m.vertexColors,
      m.visible,
      m.map === null
    ];
  }
  if (m instanceof THREE.LineBasicMaterial) {
    return [rank, m.type, m.color.toArray(), m.opacity, m.transparent, m.vertexColors, m.depthTest, m.depthWrite, m.visible];
  }
  throw new Error(`field3dFieldsRun: unexpected material ${m.type}`);
}

export function run(kind: string, args: any): unknown {
  const specular = new THREE.Color(args.specular);
  let root: THREE.Object3D;
  switch (kind) {
    case "evergreen":
      root = makeEvergreenField(specular, args.shininess, args.isFTC);
      break;
    case "axes":
      root = makeAxesField(specular, args.shininess, args.coordinateSystem, args.isFTC);
      break;
    case "template":
      root = makeAxesTemplate(specular, args.shininess);
      break;
    default:
      throw new Error(`field3dFieldsRun: unknown field ${JSON.stringify(kind)}`);
  }
  root.updateMatrixWorld(true);

  const nodes: THREE.Object3D[] = [];
  root.traverse((o) => nodes.push(o));
  const drawables = nodes.filter((o) => (o as any).isMesh || (o as any).isLine) as THREE.Mesh[];
  for (const o of drawables) {
    if (Array.isArray(o.material)) throw new Error("field3dFieldsRun: material array");
  }
  const geometryRanks = ranks(drawables.map((o) => o.geometry.id));
  const materialRanks = ranks(drawables.map((o) => (o.material as THREE.Material).id));

  return nodes.map((o) => {
    const drawable = o as THREE.Mesh;
    const isDrawable = drawables.includes(drawable);
    return [
      o === root ? null : nodes.indexOf(o.parent!),
      o.type,
      o.name,
      o.visible,
      o.castShadow,
      o.receiveShadow,
      o.frustumCulled,
      o.renderOrder,
      o.position.toArray(),
      o.quaternion.toArray(),
      o.scale.toArray(),
      o.matrixWorld.toArray(),
      isDrawable ? [geometryRanks.get(drawable.geometry.id), geometryDigest(drawable.geometry)] : null,
      isDrawable
        ? materialOf(drawable.material as THREE.Material, materialRanks.get((drawable.material as THREE.Material).id)!)
        : null
    ];
  });
}
