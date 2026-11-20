import fs from "node:fs";
import path from "node:path";
import "./shims";
import { UPSTREAM_DIR } from "./shims";
import {
  parseField2d,
  parseField3d,
  parseJoystick,
  parseRobot
} from "@upstream/main/assetUtil";
import {
  ALLIANCE_STATION_WIDTH,
  BuiltIn3dFields,
  DEFAULT_DRIVER_STATIONS_FRC,
  DEFAULT_DRIVER_STATIONS_FTC,
  FRC_STANDARD_FIELD_LENGTH,
  FRC_STANDARD_FIELD_WIDTH,
  FTC_STANDARD_FIELD_LENGTH,
  FTC_STANDARD_FIELD_WIDTH
} from "@upstream/shared/AdvantageScopeAssets";
import { numberToBits, toTree } from "./cve1";

// Emits asrs-model asset parser ground truth: each record is
//   {"id":<label>, "kind":"field2d"|"field3d"|"robot"|"joystick", "doc":<config.json text>,
//    "parse":{"ok":<CVE-1>|"throw":<CVE-1>}, "out":{"ok":<CVE-1>|"throw":<CVE-1>}}
// where "out" is the real parser's result ("invalid" / "skip" / the config object) or the
// TypeError it threw. "out" is absent when JSON.parse failed.
//   genAssets bundled
//   genAssets dir <repo-relative dir>...
//   genAssets malformed <count> <seed>
//   genAssets builtin
//   genAssets requests
//     emits {"fn":"main/assetUtil:<parser>","args":[<CVE-1>]} request lines for the fn
//     oracle over the bundled + release fixtures and args JSON.parse cannot produce
//     (undefined, holes, BigInt, NaN, -0, bytes, dates).

const REPO_ROOT = path.resolve(UPSTREAM_DIR, "..", "..", "..");
const BUNDLED = path.join(UPSTREAM_DIR, "bundledAssets");

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const PARSERS: { [kind: string]: (raw: unknown) => unknown } = {
  field2d: parseField2d,
  field3d: parseField3d,
  robot: parseRobot,
  joystick: parseJoystick
};

const PREFIXES: [string, string][] = [
  ["Field2d_", "field2d"],
  ["Field3d_", "field3d"],
  ["Robot_", "robot"],
  ["Joystick_", "joystick"]
];

function kindOf(folder: string): string | undefined {
  return PREFIXES.find(([prefix]) => folder.startsWith(prefix))?.[1];
}

const attempt = <T>(f: () => T): { ok: T } | { throw: unknown } => {
  try {
    return { ok: f() };
  } catch (e) {
    return { throw: e };
  }
};

const result = (r: { ok: unknown } | { throw: unknown }) =>
  "ok" in r ? { ok: toTree(r.ok) } : { throw: toTree(r.throw) };

function emit(id: string, kind: string, doc: string) {
  const record: { [key: string]: unknown } = { id, kind, doc };
  const parsed = attempt(() => JSON.parse(doc));
  record.parse = result(parsed);
  if ("ok" in parsed) record.out = result(attempt(() => PARSERS[kind](parsed.ok)));
  out(record);
}

function visit(dir: string, files: string[]) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) visit(full, files);
    else if (entry.name === "config.json") files.push(full);
  }
}

function emitConfigs(base: string, files: string[]) {
  files.sort();
  for (const file of files) {
    const folder = path.basename(path.dirname(file));
    const kind = kindOf(folder);
    const id = path.relative(base, path.dirname(file));
    if (kind === undefined) {
      out({ id, error: `folder ${folder} matches no asset prefix` });
      continue;
    }
    emit(id, kind, fs.readFileSync(file, "utf8"));
  }
}

function genBundled() {
  const files: string[] = [];
  visit(BUNDLED, files);
  emitConfigs(BUNDLED, files);
}

function genDirs(dirs: string[]) {
  for (const dir of dirs) {
    const base = path.join(REPO_ROOT, dir);
    const files: string[] = [];
    visit(base, files);
    emitConfigs(base, files);
  }
}

let seed = 0;
function next(): number {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return seed >>> 0;
}
const below = (n: number) => next() % n;
const chance = (p: number) => next() / 2 ** 32 < p;
const pick = <T>(items: readonly T[]): T => items[below(items.length)];

const KINDS = ["field2d", "field3d", "robot", "joystick"];

const BASES: { [kind: string]: object } = {
  field2d: {
    name: "Practice Field",
    isFTC: false,
    coordinateSystem: "wall-blue",
    useGrid: false,
    sourceUrl: "https://example.com/field.png",
    topLeft: [10, 20],
    bottomRight: [1000, 500],
    widthInches: 648,
    heightInches: 324
  },
  field3d: {
    name: "Rebuilt",
    isFTC: false,
    coordinateSystem: "wall-blue",
    useGrid: true,
    rotations: [{ axis: "z", degrees: 90 }],
    position: [0.1, 0.2, 0.3],
    widthInches: 648,
    heightInches: 324,
    driverStations: [
      [8, -1.5],
      [8, 0],
      [8, 1.5],
      [-8, 1.5],
      [-8, 0],
      [-8, -1.5]
    ],
    gamePieces: [
      {
        name: "Fuel",
        rotations: [{ axis: "x", degrees: 45 }],
        position: [1, 2, 3],
        stagedObjects: ["a.glb", "b.glb"]
      }
    ],
    aprilTags: [{ variant: "36h11-6.5in", id: 3, rotations: [], position: [1, 0, 2] }]
  },
  robot: {
    name: "KitBot",
    isFTC: false,
    disableSimplification: true,
    rotations: [{ axis: "z", degrees: 90 }],
    position: [0.12, 3.17, 0],
    cameras: [
      { name: "Front", rotations: [{ axis: "y", degrees: 20 }], position: [0.2, 0, 0.8], resolution: [960, 720], fov: 100 }
    ],
    components: [{ zeroedRotations: [{ axis: "x", degrees: -10 }], zeroedPosition: [1, 2, 3] }]
  },
  joystick: {
    name: "Xbox",
    path: "",
    components: [
      { type: "button", isYellow: true, isEllipse: true, centerPx: [1, 2], sizePx: [3, 4], sdlSourceIndex: 0, sdlSourcePov: "up", niSourceIndex: 1, niSourcePov: "down" },
      { type: "joystick", isYellow: false, centerPx: [10, 20], radiusPx: 30, sdlXSourceIndex: 0, sdlXSourceInverted: true, sdlYSourceIndex: 1, sdlYSourceInverted: false, sdlButtonSourceIndex: 2, niXSourceIndex: 3, niXSourceInverted: true, niYSourceIndex: 4, niYSourceInverted: false, niButtonSourceIndex: 5 },
      { type: "axis", isYellow: false, centerPx: [5, 6], sizePx: [7, 8], sdlSourceIndex: 2, sdlSourceRange: [-1, 1], niSourceIndex: 3, niSourceRange: [1, -1] },
      { type: "touchpad", isYellow: true, centerPx: [9, 9], sizePx: [4, 4], sdlSourceIndex: 7 }
    ]
  }
};

const FIXED: [string, string][] = [
  // top-level non-objects and empty inputs
  ...KINDS.flatMap((kind): [string, string][] => [
    [kind, "null"],
    [kind, "5"],
    [kind, '"str"'],
    [kind, "true"],
    [kind, "[]"],
    [kind, "{}"]
  ]),
  // missing keys
  ["field2d", '{"name":"x"}'],
  ["field3d", '{"name":"x"}'],
  ["robot", '{"name":"x"}'],
  ["joystick", '{"name":"x"}'],
  ["joystick", '{"name":"x","components":[]}'],
  ["robot", '{"name":"r","cameras":[]}'],
  ["field2d", '{"isFTC":false,"name":"","topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"isFTC":true,"name":"x","topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field3d", '{"isFTC":true,"name":"x","widthInches":1,"heightInches":1}'],
  // wrong types for scalars
  ["field2d", '{"name":42,"isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":"true","topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":0,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"useGrid":1,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"useGrid":false,"sourceUrl":7,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"coordinateSystem":"bogus","topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"coordinateSystem":"CENTER-RED","topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field3d", '{"name":"x","isFTC":false,"useGrid":"yes","widthInches":10,"heightInches":10}'],
  ["robot", '{"name":"r","isFTC":"no","disableSimplification":"yes"}'],
  // wrong types for arrays
  ["field2d", '{"name":"x","isFTC":false,"topLeft":"3,4","bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[0,0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[0,"a"],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[-1,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":-1,"heightInches":1}'],
  ["field3d", '{"name":"x","isFTC":false,"position":[0,0],"widthInches":10,"heightInches":10}'],
  ["field3d", '{"name":"x","isFTC":false,"rotations":{},"widthInches":10,"heightInches":10}'],
  ["field3d", '{"name":"x","isFTC":false,"rotations":[{"axis":"q","degrees":1}],"widthInches":10,"heightInches":10}'],
  ["field3d", '{"name":"x","isFTC":false,"rotations":[{"axis":"x"}],"widthInches":10,"heightInches":10}'],
  ["field3d", '{"name":"x","isFTC":false,"rotations":[{"axis":"x","degrees":"90"}],"widthInches":10,"heightInches":10}'],
  ["field3d", '{"name":"x","isFTC":false,"rotations":[{"degrees":1,"axis":"x","extra":[1,2]}],"widthInches":10,"heightInches":10}'],
  ["field3d", '{"name":"x","isFTC":false,"rotations":[null],"widthInches":10,"heightInches":10}'],
  ["field3d", '{"name":"x","isFTC":false,"rotations":[5],"widthInches":10,"heightInches":10}'],
  // primitives inside forEach arrays throw TypeErrors
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"gamePieces":[null]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"gamePieces":[5]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"gamePieces":[true]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"aprilTags":[null]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"aprilTags":["v"]}'],
  ["robot", '{"name":"r","cameras":[null]}'],
  ["robot", '{"name":"r","components":[0]}'],
  ["joystick", '{"name":"j","components":[5]}'],
  ["joystick", '{"name":"j","components":[null]}'],
  // gamePiece and camera element coverage
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"gamePieces":[{"name":3,"rotations":[{"axis":"x","degrees":1,"junk":true}],"position":[1,2,3,4],"stagedObjects":["a",1]}]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"gamePieces":[{"name":"g","stagedObjects":[]}]}'],
  ["robot", '{"name":"r","cameras":[{"name":"c","resolution":[1],"fov":"wide"},{"name":""}]}'],
  ["robot", '{"name":"r","cameras":[{"name":"c","resolution":[640,480],"fov":1e999}]}'],
  // aprilTag variant aliasing and parseFloat semantics
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"aprilTags":[{"variant":"frc-36h11"},{"variant":"frc-16h5"},{"variant":"ftc-2in"},{"variant":"ftc-3in"},{"variant":"ftc-4in"},{"variant":"ftc-5in"}]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"aprilTags":[{"variant":"36h11- 6.5in"},{"variant":"36h11-.5in"},{"variant":"36h11-0x10in"},{"variant":"36h11-Infinityin"},{"variant":"16h5-.5e2in"},{"variant":"36h11-in"},{"variant":"16h5-e3in"},{"variant":"36h11--1in"},{"variant":"36h11-6.5inx"},{"variant":"36h11-6.5in-extra"},{"variant":"36H11-6.5in"},{"variant":"bogus"}]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"aprilTags":[{"id":1},{"variant":5},{"variant":"16h5-6in","id":"3"},{"variant":"16h5-6in","id":3,"rotations":[{"axis":"z","degrees":180}],"position":[0,0,0]}]}'],
  // driverStations counts: FRC wants 6, FTC wants 4
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"driverStations":[[1,2],[3,4],[5,6],[7,8]]}'],
  ["field3d", '{"name":"x","isFTC":true,"widthInches":10,"heightInches":10,"driverStations":[[1,2],[3,4],[5,6],[7,8]]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"driverStations":[[1,2],[3,4],[5,6],[7,8],[9,10],[11,12]]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"driverStations":[[1,2],[3,4],[5,6],[7,8],[9,10],[11]]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"driverStations":[0,1,2,3,4,5]}'],
  // NaN-ish and extreme numbers (1e999 parses to Infinity; still typeof number)
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1e999,"heightInches":10}'],
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":-1e999,"heightInches":10}'],
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":5e-324,"heightInches":0.1}'],
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[9007199254740993,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":1.7976931348623158e308,"heightInches":10}'],
  ["robot", '{"name":"r","cameras":[{"name":"c","fov":-0,"resolution":[5e-324,1e999]}]}'],
  // integer-like keys reorder ahead of named keys in the parsed object
  ["field2d", '{"2":"a","1":"b","name":"x","isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":1,"heightInches":1,"gamePieces":[{"9":1,"name":"g"}]}'],
  // duplicate keys: last wins
  ["field2d", '{"name":"first","name":"second","isFTC":false,"isFTC":true,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1}'],
  ["field2d", '{"name":"x","isFTC":false,"widthInches":0,"widthInches":50,"heightInches":50,"topLeft":[0,0],"bottomRight":[1,1]}'],
  ["joystick", '{"name":"j","components":[{"type":"button","type":"axis","centerPx":[0,0],"sizePx":[2,2],"sourceIndex":3}]}'],
  // extra keys and deep nesting
  ["field2d", '{"name":"x","isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1,"extra":{"deep":[{"k":[1,2,{"v":null}]}]}}'],
  ["robot", '{"name":"r","components":[{"zeroedRotations":[{"axis":"y","degrees":30}],"zeroedPosition":[1,2,3],"bogus":{"a":[{"b":[{}]}]}}]}'],
  ["field3d", '{"name":"x","isFTC":false,"widthInches":10,"heightInches":10,"deep":[[[[[[[[[[{"k":[[[[[{"v":[[[[[[[[[[0]]]]]]]]]]}]}]]]]]]]]}]]]]]]]]]]}'],
  // legacy joystick key aliases
  ["joystick", '{"name":"j","components":[{"type":"button","sizePx":[2,2],"sourceIndex":3,"sourcePov":"left"}]}'],
  ["joystick", '{"name":"j","components":[{"type":"button","sizePx":[2,2],"niSourceIndex":3,"sourceIndex":9,"niSourcePov":"up","sourcePov":"down"}]}'],
  ["joystick", '{"name":"j","components":[{"type":"joystick","radiusPx":10,"xSourceIndex":0,"xSourceInverted":true,"ySourceIndex":1,"ySourceInverted":false,"buttonSourceIndex":2}]}'],
  ["joystick", '{"name":"j","components":[{"type":"axis","sizePx":[2,2],"sourceIndex":3,"sourceRange":[1,-1]}]}'],
  ["joystick", '{"name":"j","components":[{"type":"axis","sizePx":[2,2],"niSourceIndex":3,"sourceIndex":4}]}'],
  // joystick validation edge cases
  ["joystick", '{"name":"j","components":[{"type":"button","sizePx":[0,2],"sdlSourceIndex":1},{"type":"joystick","radiusPx":5,"sdlXSourceIndex":0},{"type":"touchpad","sizePx":[1,1],"sdlSourceIndex":-1}]}'],
  ["joystick", '{"name":"j","components":[{"type":"button","sizePx":[2,2],"niSourceIndex":1},{"type":"joystick","radiusPx":5,"niXSourceIndex":0,"niYSourceIndex":1},{"type":"touchpad","sizePx":[1,1],"sdlSourceIndex":0}]}'],
  ["joystick", '{"name":"j","components":[{"type":"weird"},{"type":5},{"type":"button","sizePx":[2,2],"sdlSourceIndex":1}]}'],
  ["joystick", '{"name":"j","components":[{"type":"joystick","radiusPx":5,"sdlXSourceIndex":0,"sdlYSourceIndex":1,"niXSourceIndex":2,"niYSourceIndex":3,"isYellow":true}]}']
];

const JUNK: unknown[] = [
  0, -0, 1, -1, 5e-324, 1e21, "x", "", "number", true, false, null, [], [1, "a"], {},
  { axis: "x" }, { axis: "x", degrees: 1 }, [0, 0], [0, 0, 0], [0, 0, 0, 0], ["a", "b"]
];

const NUMBER_TEXT = ["0", "-0", "1", "-1", "0.5", "-0.5", "1e999", "-1e999", "5e-324", "9007199254740993", "1.7976931348623158e308", "0.30000000000000004", "123e-2", "4294967296", "1e-400"];

function pathsInto(value: unknown, prefix: string[] = []): string[][] {
  const paths = [prefix];
  if (Array.isArray(value)) {
    value.forEach((item, i) => pathsInto(item, [...prefix, String(i)]).forEach((p) => paths.push(p)));
  } else if (typeof value === "object" && value !== null) {
    for (const key of Object.keys(value)) pathsInto((value as any)[key], [...prefix, key]).forEach((p) => paths.push(p));
  }
  return paths;
}

function getAt(value: unknown, path: string[]): unknown {
  let v = value;
  for (const key of path) v = (v as any)[key];
  return v;
}

function setAt(value: unknown, path: string[], replacement: unknown) {
  let v = value;
  for (const key of path.slice(0, -1)) v = (v as any)[key];
  (v as any)[path[path.length - 1]] = replacement;
}

function deleteAt(value: unknown, path: string[]) {
  let v = value;
  for (const key of path.slice(0, -1)) v = (v as any)[key];
  const last = path[path.length - 1];
  if (Array.isArray(v)) v.splice(Number(last), 1);
  else delete (v as any)[last];
}

function mutateValue(kind: string): object {
  const base = JSON.parse(JSON.stringify(BASES[kind]));
  const mutations = 1 + below(4);
  for (let i = 0; i < mutations; i++) {
    const paths = pathsInto(base).filter((p) => p.length > 0);
    const path = pick(paths);
    switch (below(5)) {
      case 0:
        if (path.length > 0) deleteAt(base, path);
        break;
      case 1:
        setAt(base, path, pick(JUNK));
        break;
      case 2: {
        const holder = getAt(base, path.slice(0, -1));
        if (typeof holder === "object" && holder !== null && !Array.isArray(holder)) {
          deleteAt(base, path);
        } else {
          setAt(base, path, pick(JUNK));
        }
        break;
      }
      case 3: {
        const holder = getAt(base, path.slice(0, -1));
        if (typeof holder === "object" && holder !== null && !Array.isArray(holder)) {
          (holder as any)[chance(0.5) ? String(below(20)) : "extra" + below(100)] = pick(JUNK);
        }
        break;
      }
      default: {
        const holder = getAt(base, path.slice(0, -1));
        if (Array.isArray(holder)) {
          if (chance(0.5) && holder.length > 0) holder.splice(below(holder.length), 1);
          else holder.splice(below(holder.length + 1), 0, pick(JUNK));
        }
      }
    }
  }
  return base;
}

const TEXT_TOKENS = ["1e999", "-1e999", "5e-324", "0.1", "9007199254740993", "e5", ".", "-", ",", ":", "{", "}", "[", "]", '"x"', "true", "null", "NaN", '"\\ud800"'];

function mutateText(doc: string): string {
  const at = below(doc.length + 1);
  switch (below(4)) {
    case 0:
      return doc.slice(0, at) + pick(TEXT_TOKENS) + doc.slice(at);
    case 1:
      return doc.slice(0, at) + doc.slice(at + 1 + below(3));
    case 2:
      return doc.slice(0, at);
    default:
      return mutateText(mutateText(doc));
  }
}

function dupKeyDoc(kind: string): string {
  const base = BASES[kind] as { [key: string]: unknown };
  const keys = Object.keys(base);
  const key = pick(keys);
  const dup = JSON.stringify(key) + ":" + pick(NUMBER_TEXT.concat(['"junk"', "true", "null", "[]", "{}"])) + ",";
  const doc = JSON.stringify(base);
  return doc.slice(0, 1) + dup + doc.slice(1);
}

function deepDoc(): string {
  const depth = 20 + below(200);
  let open = "";
  let close = "";
  for (let i = 0; i < depth; i++) {
    if (chance(0.5)) {
      open += "[";
      close = "]" + close;
    } else {
      open += '{"' + pick(["a", "0", "1", "name", "rotations"]) + '":';
      close = "}" + close;
    }
  }
  return open + pick(NUMBER_TEXT) + close;
}

function genMalformed(count: number) {
  let i = 0;
  for (const [kind, doc] of FIXED) {
    emit(`malformed/fixed-${i}`, kind, doc);
    i++;
  }
  for (; i < count; i++) {
    const kind = pick(KINDS);
    let doc: string;
    switch (below(6)) {
      case 0:
      case 1:
        doc = JSON.stringify(mutateValue(kind));
        break;
      case 2:
        doc = mutateText(JSON.stringify(BASES[kind]));
        break;
      case 3:
        doc = dupKeyDoc(kind);
        break;
      case 4:
        doc = '{"name":"x","isFTC":false,"topLeft":[0,0],"bottomRight":[1,1],"widthInches":1,"heightInches":1,"deep":' + deepDoc() + "}";
        break;
      default: {
        const base = mutateValue(kind);
        doc = chance(0.5) ? mutateText(JSON.stringify(base)) : JSON.stringify(base);
      }
    }
    emit(`malformed/${i}`, kind, doc);
  }
}

const FN_NAMES: { [kind: string]: string } = {
  field2d: "parseField2d",
  field3d: "parseField3d",
  robot: "parseRobot",
  joystick: "parseJoystick"
};

const n = (x: number) => ({ n: numberToBits(x) });
const o = (pairs: [string, unknown][]) => ({ o: pairs });

// Request args written as literal CVE-1 trees: shapes JSON.parse cannot produce
// (undefined, BigInt, NaN, -0, bytes, dates).
const NONJSON_ARGS: [string, unknown][] = [
  ["field2d", { u: null }],
  ["field2d", n(NaN)],
  ["field2d", { bi: "5" }],
  ["field2d", { d: numberToBits(1000) }],
  [
    "field3d",
    o([
      ["name", "x"],
      ["isFTC", false],
      ["widthInches", n(10)],
      ["heightInches", n(10)],
      ["gamePieces", [{ u: null }]]
    ])
  ],
  [
    "field3d",
    o([
      ["name", "x"],
      ["isFTC", false],
      ["widthInches", n(10)],
      ["heightInches", n(10)],
      ["rotations", [null]]
    ])
  ],
  [
    "field3d",
    o([
      ["name", "x"],
      ["isFTC", false],
      ["widthInches", n(NaN)],
      ["heightInches", n(10)]
    ])
  ],
  [
    "field3d",
    o([
      ["name", "x"],
      ["isFTC", false],
      ["widthInches", n(Infinity)],
      ["heightInches", n(Infinity)],
      ["aprilTags", [o([["variant", "36h11-6.5in"], ["id", { bi: "7" }]])]]
    ])
  ],
  [
    "field3d",
    o([
      ["name", "x"],
      ["isFTC", true],
      ["widthInches", n(10)],
      ["heightInches", n(10)],
      ["driverStations", [[n(1), n(-0)], [n(1), n(0)], [n(1), n(1.5)], [n(-1), n(1.5)]]]
    ])
  ],
  ["robot", o([["name", "r"], ["components", [{ u: null }]]])],
  ["joystick", o([["name", "j"], ["components", [{ u: null }]]])],
  [
    "joystick",
    o([
      ["name", "j"],
      ["components", [o([["type", "touchpad"], ["sizePx", [n(1), n(1)]], ["sdlSourceIndex", n(NaN)]])]]
    ])
  ],
  ["joystick", { b: "00ff" }]
];

function genRequests() {
  const request = (kind: string, arg: unknown) => out({ fn: `main/assetUtil:${FN_NAMES[kind]}`, args: [arg] });
  const dirs = [BUNDLED, path.join(REPO_ROOT, "parity/specs/assets/release")];
  for (const dir of dirs) {
    const files: string[] = [];
    visit(dir, files);
    files.sort();
    for (const file of files) {
      const kind = kindOf(path.basename(path.dirname(file)));
      if (kind === undefined) continue;
      const parsed = attempt(() => JSON.parse(fs.readFileSync(file, "utf8")));
      if ("ok" in parsed) request(kind, toTree(parsed.ok));
    }
  }
  for (const [kind, doc] of FIXED) {
    const parsed = attempt(() => JSON.parse(doc));
    if ("ok" in parsed) request(kind, toTree(parsed.ok));
  }
  for (const [kind, arg] of NONJSON_ARGS) request(kind, arg);
}

function genBuiltin() {
  const record = (id: string, value: unknown) => out({ id, out: { ok: toTree(value) } });
  record("constants/FRC_STANDARD_FIELD_LENGTH", FRC_STANDARD_FIELD_LENGTH);
  record("constants/FRC_STANDARD_FIELD_WIDTH", FRC_STANDARD_FIELD_WIDTH);
  record("constants/FTC_STANDARD_FIELD_LENGTH", FTC_STANDARD_FIELD_LENGTH);
  record("constants/FTC_STANDARD_FIELD_WIDTH", FTC_STANDARD_FIELD_WIDTH);
  record("constants/ALLIANCE_STATION_WIDTH", ALLIANCE_STATION_WIDTH);
  record("constants/DEFAULT_DRIVER_STATIONS_FRC", DEFAULT_DRIVER_STATIONS_FRC);
  record("constants/DEFAULT_DRIVER_STATIONS_FTC", DEFAULT_DRIVER_STATIONS_FTC);
  BuiltIn3dFields.forEach((field, i) => record(`BuiltIn3dFields/${i}`, field));
}

const [mode, ...rest] = process.argv.slice(2);
switch (mode) {
  case "bundled":
    genBundled();
    break;
  case "dir":
    genDirs(rest);
    break;
  case "malformed": {
    const count = Number(rest[0]);
    seed = (Number(rest[1]) >>> 0) || 0x6a09e667;
    genMalformed(count);
    break;
  }
  case "builtin":
    genBuiltin();
    break;
  case "requests":
    genRequests();
    break;
  default:
    throw new Error(
      "usage: genAssets bundled | dir <rel>... | malformed <count> <seed> | builtin | requests"
    );
}
