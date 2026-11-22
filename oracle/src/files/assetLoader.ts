import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import * as fakeFs from "../fakes/mainFs";
import { toTree } from "../cve1";

// Ground truth for asrs-services' asset loader: upstream's real `assetLoader.ts` (with the real
// `filterAndSortAssets` and parsers) against the fake fs. Each record is one operation:
//   {"case","step","op","args"?,"result"|"throw","writes","files"?,"dirs"?}
// Ops: reset, putDir, putFile, removeFile, setFailure (scenario inputs), createAssetFolders,
// getUserAssetsPath, loadAssets (upstream calls). `files` and `dirs` (the whole fake fs) follow
// createAssetFolders and the last op of each case. A file is {"hex"} up to INLINE_LIMIT bytes,
// otherwise {"len","sha256"}; a putFile of a real bundled asset is {"bundled": <path relative
// to upstream's bundledAssets>,"len","sha256"}.
//
// BUNDLED_ASSETS is `<harness dist>/../bundledAssets`, a machine path: every record has it
// replaced by BUNDLED_PLACEHOLDER. The run fails if encodeURIComponent would change any of its
// components, since the replacement would then miss encoded paths.

declare const __UPSTREAM_DIR__: string;

const INLINE_LIMIT = 4096;
const USER_DATA = "/fake/userData";
const PREFS = `${USER_DATA}/prefs.json`;
const AUTO = `${USER_DATA}/autoAssets`;
const USER = `${USER_DATA}/userAssets`;
const BUNDLED_PLACEHOLDER = "/fake/bundledAssets";
const REAL_BUNDLED = path.join(__UPSTREAM_DIR__, "bundledAssets");

let BUNDLED = "";
let loader: any;

const out = (value: object) => process.stdout.write(JSON.stringify(value).split(BUNDLED).join(BUNDLED_PLACEHOLDER) + "\n");

let rngState = 0x2545f491;
function nextU32(): number {
  rngState ^= rngState << 13;
  rngState ^= rngState >>> 17;
  rngState ^= rngState << 5;
  return rngState >>> 0;
}
const below = (n: number) => nextU32() % n;
const chance = (p: number) => nextU32() / 2 ** 32 < p;
const pick = <T>(items: readonly T[]): T => items[below(items.length)];

let caseIndex = -1;
let stepIndex = 0;

const fileValue = (bytes: Uint8Array) =>
  bytes.length <= INLINE_LIMIT
    ? { hex: Buffer.from(bytes).toString("hex") }
    : { len: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };

function dump() {
  const files = fakeFs.dumpFiles();
  return {
    files: Object.keys(files).map((name) => ({ name, ...fileValue(files[name]) })),
    dirs: fakeFs.dumpDirs()
  };
}

function record(op: string, args: unknown, run: () => unknown, withDump = false) {
  const r: any = { case: caseIndex, step: stepIndex++, op };
  if (args !== undefined) r.args = args;
  try {
    r.result = toTree(run());
  } catch (error) {
    r.throw = toTree(error);
  }
  r.writes = fakeFs.takeWrites();
  if (withDump) Object.assign(r, dump());
  out(r);
}

const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));

function reset(withUserData = true) {
  caseIndex++;
  stepIndex = 0;
  fakeFs.reset();
  record("reset", undefined, () => null);
  if (withUserData) putDir(USER_DATA);
}
function putDir(name: string) {
  record("putDir", { name }, () => {
    fakeFs.putDir(name);
    return null;
  });
}
function putBytes(name: string, data: Uint8Array) {
  record("putFile", { name, data: fileValue(data) }, () => {
    fakeFs.putFile(name, data);
    return null;
  });
}
const putText = (name: string, text: string) => putBytes(name, utf8(text));
function putBundled(rel: string) {
  const data = new Uint8Array(fs.readFileSync(path.join(REAL_BUNDLED, rel)));
  const name = `${BUNDLED}/${rel}`;
  record("putFile", { name, data: { bundled: rel, ...fileValue(data) } }, () => {
    fakeFs.putFile(name, data);
    return null;
  });
}
function removeFile(name: string) {
  record("removeFile", { name }, () => {
    fakeFs.removeFile(name);
    return null;
  });
}
function setFailure(name: string, kind: "read" | "write" | null) {
  record("setFailure", { name, kind }, () => {
    fakeFs.setFailure(name, kind);
    return null;
  });
}
const createAssetFolders = () => record("createAssetFolders", undefined, () => loader.createAssetFolders(), true);
const getUserAssetsPath = () => record("getUserAssetsPath", undefined, () => loader.getUserAssetsPath());
const loadAssets = () => record("loadAssets", undefined, () => loader.loadAssets());
const finish = () => record("loadAssets", undefined, () => loader.loadAssets(), true);

const prefs = (folder: unknown) => putText(PREFS, JSON.stringify({ theme: "system", userAssetsFolder: folder }));

function putBundledTree() {
  putDir(BUNDLED);
  for (const folder of fs.readdirSync(REAL_BUNDLED).sort()) {
    if (folder.startsWith(".")) continue;
    putDir(`${BUNDLED}/${folder}`);
    for (const file of fs.readdirSync(path.join(REAL_BUNDLED, folder)).sort()) {
      if (file.startsWith(".")) continue;
      putBundled(`${folder}/${file}`);
    }
  }
}

const field2d = (name: string, extra: object = {}) => JSON.stringify({ name, isFTC: false, topLeft: [1, 2], bottomRight: [3, 4], widthInches: 10, heightInches: 5, ...extra });
const field3d = (name: string, pieces: number, extra: object = {}) =>
  JSON.stringify({
    name,
    isFTC: false,
    widthInches: 600,
    heightInches: 300,
    gamePieces: Array.from({ length: pieces }, (_, i) => ({ name: `piece${i}`, rotations: [], position: [0, 0, 0], stagedObjects: [] })),
    ...extra
  });
const robot = (name: string, components: number, extra: object = {}) =>
  JSON.stringify({
    name,
    isFTC: false,
    rotations: [],
    position: [0, 0, 0],
    cameras: [],
    components: Array.from({ length: components }, () => ({ zeroedRotations: [], zeroedPosition: [0, 0, 0] })),
    ...extra
  });
const joystick = (name: string) => JSON.stringify({ name, components: [{ type: "button", isYellow: false, isEllipse: false, centerPx: [1, 1], sizePx: [2, 2], sdlSourceIndex: 0 }] });

function asset(parent: string, folder: string, config: string | null, files: string[]) {
  const dir = path.posix.join(parent, folder);
  putDir(dir);
  if (config !== null) putText(`${dir}/config.json`, config);
  for (const file of files) putBytes(`${dir}/${file}`, utf8(file));
}

function baseCase(folder: unknown = null) {
  reset();
  prefs(folder);
  putBundledTree();
}

function handWritten() {
  // Startup on a fresh install: README write, bundled fields and robots, then reloads.
  baseCase();
  getUserAssetsPath();
  createAssetFolders();
  loadAssets();
  loadAssets();
  asset(USER, "Field2d_Practice", field2d("Practice"), ["image.png"]);
  loadAssets();
  asset(AUTO, "Robot_2024KitBot", robot("2024 KitBot", 1), ["model.glb", "model_0.glb"]);
  loadAssets();
  removeFile(`${USER}/Field2d_Practice/image.png`);
  loadAssets();
  createAssetFolders();
  finish();

  // Preferences variants for getUserAssetsPath.
  for (const text of [null, "{", "null", "5", '"str"', "[]", "{}", '{"userAssetsFolder":7}', '{"userAssetsFolder":"/nowhere"}', "﻿{\"userAssetsFolder\":null}", '{"userAssetsFolder":"/custom"}']) {
    reset();
    putBundledTree();
    if (text !== null) putText(PREFS, text);
    getUserAssetsPath();
    createAssetFolders();
    finish();
  }
  reset();
  putBundledTree();
  prefs(null);
  setFailure(PREFS, "read");
  getUserAssetsPath();
  finish();

  // Custom folder: a directory, a file (readdir throws), a relative drive-like first component,
  // and first components whose decodeURIComponent differs from the raw text.
  for (const folder of ["/custom", "/custom/", "/fake/./custom/../custom", "C:", "rel/dir", "%41:", "%zz:", "/sp ace/#%?"]) {
    baseCase(folder);
    putDir(folder);
    asset(folder, "Field2d_Custom", field2d("Custom"), ["image.png"]);
    asset(folder, "Robot_Custom", robot("Custom", 0), ["model.glb"]);
    createAssetFolders();
    getUserAssetsPath();
    finish();
  }
  baseCase("/afile");
  putText("/afile", "x");
  createAssetFolders();
  finish();

  // createAssetFolders variants.
  reset();
  prefs(null);
  createAssetFolders();
  putText(`${USER}/README.txt`, "old");
  createAssetFolders();
  setFailure(`${USER}/README.txt`, "write");
  createAssetFolders();
  finish();
  reset();
  putDir(AUTO);
  prefs(null);
  createAssetFolders();
  finish();
  reset(false);
  createAssetFolders();
  finish();
  reset();
  putText(AUTO, "file");
  prefs(null);
  createAssetFolders();
  finish();

  // Every loader branch in one user folder.
  baseCase();
  createAssetFolders();
  asset(USER, ".Field2d_Hidden", field2d("Hidden"), ["image.png"]);
  putText(`${USER}/Field2d_File`, "not a folder");
  asset(USER, "Other_Thing", field2d("Other"), ["image.png"]);
  asset(USER, "Field2d_NoConfig", null, ["image.png"]);
  asset(USER, "Field2d_BadJson", "{oops", ["image.png"]);
  asset(USER, "Field2d_Bom", "﻿" + field2d("Bom"), ["image.png"]);
  asset(USER, "Field2d_Unreadable", field2d("Unreadable"), ["image.png"]);
  setFailure(`${USER}/Field2d_Unreadable/config.json`, "read");
  asset(USER, "Field2d_Skip", JSON.stringify({ name: "Old" }), ["image.png"]);
  asset(USER, "Field2d_Invalid", "5", ["image.png"]);
  asset(USER, "Field2d_NoImage", field2d("NoImage"), []);
  asset(USER, "Field3d_Skip", JSON.stringify({ name: "Old3d" }), ["model.glb"]);
  asset(USER, "Field3d_Invalid", "[]", ["model.glb"]);
  asset(USER, "Field3d_Pieces", field3d("Pieces", 2), ["model.glb", "model_0.glb", "model_1.glb"]);
  asset(USER, "Field3d_MissingPiece", field3d("MissingPiece", 2), ["model.glb", "model_0.glb"]);
  asset(USER, "Field3d_NoModel", field3d("NoModel", 0), []);
  asset(USER, "Robot_Invalid", "null", ["model.glb"]);
  asset(USER, "Robot_Parts", robot("Parts", 2), ["model.glb", "model_0.glb", "model_1.glb"]);
  asset(USER, "Robot_MissingPart", robot("MissingPart", 1), ["model.glb"]);
  asset(USER, "Joystick_Pad", joystick("Pad"), ["image.png"]);
  asset(USER, "Joystick_NoImage", joystick("NoImage"), []);
  asset(USER, "Joystick_Invalid", "true", ["image.png"]);
  asset(USER, "Field2d_Spaced Name #1%", field2d("Spaced"), ["image.png"]);
  asset(USER, "Robot_Ünïcode ☃", robot("Unicode", 0), ["model.glb"]);
  asset(USER, "Field3d_ConfigDir", null, ["model.glb"]);
  putDir(`${USER}/Field3d_ConfigDir/config.json`);
  finish();

  // Parser throws escape loadAssets.
  baseCase();
  createAssetFolders();
  asset(USER, "Field3d_Throws", JSON.stringify({ name: "x", isFTC: false, widthInches: 10, heightInches: 10, gamePieces: [5] }), ["model.glb"]);
  finish();
  baseCase();
  createAssetFolders();
  asset(USER, "Robot_Throws", JSON.stringify({ name: "r", isFTC: false, cameras: [null] }), ["model.glb"]);
  finish();

  // Priority, newest-version-first folder order and duplicate removal.
  baseCase();
  createAssetFolders();
  asset(USER, "Field2d_FRCEvergreen", field2d("Evergreen", { widthInches: 1 }), ["image.png"]);
  asset(AUTO, "Field2d_FRCEvergreen", field2d("Evergreen", { widthInches: 2 }), ["image.png"]);
  asset(AUTO, "Field3d_2025v1", field3d("2025 Field", 0), ["model.glb"]);
  asset(AUTO, "Field3d_2025v2", field3d("2025 Field", 0, { widthInches: 700 }), ["model.glb"]);
  asset(AUTO, "Field3d_2025v10", field3d("2025 Field", 0, { widthInches: 800 }), ["model.glb"]);
  asset(AUTO, "Field3d_2025FTC", field3d("2025 Field", 0, { isFTC: true }), ["model.glb"]);
  asset(AUTO, "Robot_KitBotA", robot("KitBot", 0), ["model.glb"]);
  asset(USER, "Robot_KitBotB", robot("KitBot", 0, { isFTC: true }), ["model.glb"]);
  asset(AUTO, "Joystick_A", joystick("Xbox"), ["image.png"]);
  asset(AUTO, "Joystick_B", joystick("Xbox"), ["image.png"]);
  asset(USER, "Robot_FRCKitBot", "{", ["model.glb"]);
  finish();

  // Sorting: Evergreen pinning, code-unit order, numeric collation, failure order.
  baseCase();
  createAssetFolders();
  for (const [i, name] of ["Evergreen", "b", "B", "Évora", "evergreen", "10 Field", "9 Field", "Evergreen", "zeta", "Alpha"].entries()) {
    asset(USER, `Field2d_S${i}`, field2d(name, { isFTC: i % 2 === 0 }), ["image.png"]);
    asset(AUTO, `Field3d_S${i}`, field3d(name, 0, { isFTC: i % 3 === 0 }), ["model.glb"]);
  }
  for (const [i, name] of ["2024 KitBot", "2025 KitBot", "10 Bot", "9 Bot", "KitBot", "kitbot", "Zebra", "éclair", "ecl", "Robot 2", "Robot 10", "0", "", "  9"].entries()) {
    asset(USER, `Robot_S${i}`, robot(name, 0), ["model.glb"]);
  }
  for (const [i, name] of ["Xbox", "PS5", "ps4", "Generic", "10", "2"].entries()) {
    asset(AUTO, `Joystick_S${i}`, joystick(name), ["image.png"]);
  }
  for (const name of ["Robot_10", "Robot_9", "Field2d_b", "Field2d_B", "Joystick_x2", "Joystick_x10", "Ñ", "n", "_", "1"]) {
    asset(USER, name, "{", []);
  }
  finish();
}

const NAMES = ["Evergreen", "KitBot", "2024 KitBot", "2025 KitBot", "10 Bot", "9 Bot", "Alpha", "alpha", "Beta", "Ünïcode", "Rebuilt", "Crescendo", "Reefscape", "x", "X 2", "X 10", ""];
const FOLDER_TAILS = ["A", "a", "B", "2024", "2025", "10", "9", "v1", "v2", "Ü", "z z", "%", "#", ""];
const KINDS = ["Field2d_", "Field3d_", "Robot_", "Joystick_", "Other_", ".Field2d_", ""];

function randomConfig(kind: string): string {
  const name = pick(NAMES);
  const variant = below(10);
  if (variant === 0) return pick(["{", "null", "[]", "7", '"s"', ""]);
  if (variant === 1 && kind !== "Robot_" && kind !== "Joystick_") return JSON.stringify({ name });
  const extra: any = chance(0.5) ? { isFTC: chance(0.5) } : {};
  switch (kind) {
    case "Field3d_":
      return field3d(name, below(3), extra);
    case "Robot_":
      return robot(name, below(3), extra);
    case "Joystick_":
      return joystick(name);
    default:
      return field2d(name, extra);
  }
}

function randomFolder(parent: string) {
  const kind = pick(KINDS);
  const folder = kind + pick(FOLDER_TAILS) + (chance(0.3) ? String(below(20)) : "");
  if (folder === "" || fakeFs.existsSync(path.posix.join(parent, folder))) return;
  if (chance(0.08)) {
    putText(path.posix.join(parent, folder), "file");
    return;
  }
  const files: string[] = [];
  const main = kind.includes("Field3d") || kind.includes("Robot") ? "model" : "image";
  const ext = main === "model" ? ".glb" : ".png";
  if (chance(0.85)) files.push(main + ext);
  for (let i = 0; i < 3; i++) if (chance(0.6)) files.push(`${main}_${i}${ext}`);
  asset(parent, folder, chance(0.9) ? randomConfig(kind) : null, files);
}

function randomCases(count: number) {
  for (let c = 0; c < count; c++) {
    reset();
    const custom = chance(0.3) ? "/custom" : null;
    prefs(custom);
    if (chance(0.5)) putBundledTree();
    else putDir(BUNDLED);
    createAssetFolders();
    if (custom !== null) putDir(custom);
    const parents = [custom ?? USER, AUTO, BUNDLED];
    const folders = 1 + below(12);
    for (let i = 0; i < folders; i++) randomFolder(pick(parents));
    loadAssets();
    const edits = below(4);
    for (let i = 0; i < edits; i++) {
      if (chance(0.5)) randomFolder(pick(parents));
      else {
        const files = Object.keys(fakeFs.dumpFiles()).filter((f) => !f.endsWith("README.txt") && f !== PREFS);
        if (files.length > 0) removeFile(pick(files));
      }
      loadAssets();
    }
    finish();
  }
}

export async function run(count: number, seed: number) {
  const constants = await import("@upstream/main/electron/ElectronConstants");
  BUNDLED = constants.BUNDLED_ASSETS;
  for (const component of BUNDLED.split("/")) {
    if (encodeURIComponent(component) !== component) throw new Error(`BUNDLED_ASSETS component ${component} needs encoding`);
  }
  if (constants.AUTO_ASSETS !== AUTO || constants.DEFAULT_USER_ASSETS !== USER || constants.PREFS_FILENAME !== PREFS) {
    throw new Error("unexpected asset folder constants");
  }
  loader = await import("@upstream/main/electron/assetLoader");
  rngState = seed >>> 0 || 1;
  handWritten();
  randomCases(count);
}
