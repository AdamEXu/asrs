import "./shims";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import jsonfile from "jsonfile";
import { DEFAULT_PREFS, getLiveModeName, getRobotAddress, mergePreferences } from "@upstream/shared/Preferences";
import TabType, { getAllTabTypes, getDefaultTabTitle, getTabAccelerator, getTabIcon } from "@upstream/shared/TabType";
import { SelectionMode } from "@upstream/shared/Selection";
import * as buildConstants from "@upstream/shared/buildConstants";
import { toTree } from "./cve1";

// Ground truth for asrs-model's schemas and preferences.
//
//   genModel prefs <count> <seed>
//     The prefs.json flows of src/main/electron/main.ts, run through upstream's mergePreferences
//     and the real jsonfile 6.1.0 against a scratch file:
//     {"flow":"startup","platform":"darwin"|"linux","file":<bytes>|null,"out":<bytes>}
//       app.whenReady: a fresh DEFAULT_PREFS ("light" theme on Linux), written as is when there
//       is no file, otherwise merged with jsonfile.readFileSync, userAssetsFolder cleared when
//       fs.existsSync says no, then jsonfile.writeFileSync.
//     {"flow":"update","file":<bytes>,"data":"<JSON text>","out":<bytes>}
//       update-preferences: the raw file read back with jsonfile.readFileSync, merged with
//       JSON.parse(data), written with jsonfile.writeFileSync.
//     A throw replaces "out" with "throw":"<error name>".
//
//   genModel schemas
//     {"name":"<what>","value":<CVE-1>} for the enums, constants and pure functions of
//     Preferences.ts, TabType.ts, Selection.ts and buildConstants.ts.

const [mode, countArg, seedArg] = process.argv.slice(2);

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-genmodel-"));
const prefsFile = path.join(scratch, "prefs.json");

// The real app aliases DEFAULT_PREFS for its one startup; a copy gives every case its own.
const freshDefaults = () => structuredClone(DEFAULT_PREFS);

function runStartup(platform: string, file: Uint8Array | null): object {
  const record: any = { flow: "startup", platform, file: file === null ? null : toTree(file) };
  try {
    fs.rmSync(prefsFile, { force: true });
    let prefs = freshDefaults();
    if (platform === "linux") prefs.theme = "light";
    if (file === null) {
      jsonfile.writeFileSync(prefsFile, prefs);
    } else {
      fs.writeFileSync(prefsFile, file);
      mergePreferences(prefs, jsonfile.readFileSync(prefsFile));
      if (prefs.userAssetsFolder !== null && !fs.existsSync(prefs.userAssetsFolder)) {
        prefs.userAssetsFolder = null;
      }
      jsonfile.writeFileSync(prefsFile, prefs);
    }
    record.out = toTree(new Uint8Array(fs.readFileSync(prefsFile)));
  } catch (error) {
    record.throw = (error as Error).name;
  }
  return record;
}

// main.ts is bundled as sloppy-mode CommonJS, so in the app an assignment to a property of a
// number, string or boolean is silently dropped, while this strict-mode bundle would throw. Such
// a base file is skipped; startup has already thrown on it in the app.
function primitiveBase(file: Uint8Array): boolean {
  try {
    const value = JSON.parse(Buffer.from(file).toString("utf8").replace(/^\uFEFF/, ""));
    return value !== null && typeof value !== "object";
  } catch {
    return false;
  }
}

function runUpdate(file: Uint8Array, data: string): object | null {
  if (primitiveBase(file)) return null;
  const record: any = { flow: "update", file: toTree(file), data };
  try {
    fs.writeFileSync(prefsFile, file);
    let prefs = jsonfile.readFileSync(prefsFile);
    mergePreferences(prefs, JSON.parse(data));
    jsonfile.writeFileSync(prefsFile, prefs);
    record.out = toTree(new Uint8Array(fs.readFileSync(prefsFile)));
  } catch (error) {
    record.throw = (error as Error).name;
  }
  return record;
}

const utf8 = (text: string) => new Uint8Array(Buffer.from(text, "utf8"));

// Hand-picked files: legacy layouts, every key, wrong types, JSON edge cases.
const FILES: Uint8Array[] = [
  "{}",
  '{"theme":"dark","address":"10.63.28.2","rlogPort":5800,"liveDiscard":60}',
  '{"theme":"light","rioAddress":"roborio-6328-frc.local","rioPath":"/home/lvuser/logs","threeDimensionModeAc":"cinematic","threeDimensionModeBattery":"low-power","liveMode":"rlog"}',
  '{"address":"a","rioAddress":"b","robotAddress":"c","rioPath":"p","remotePath":"q"}',
  '{"robotAddress":"c","rioAddress":"b","address":"a","remotePath":"q","rioPath":"p"}',
  '{"field3dModeAc":"low-power","threeDimensionModeAc":"cinematic","field3dModeBattery":"standard","threeDimensionModeBattery":""}',
  '{"timestamps":"original","prefersFTC":true,"hasScrolledTimeline":true,"hasScrolledLineGraph":true,"systemcoreStaticAddress":"wifi","ctreLicenseAccepted":true,"skipFTCCoordinateSystemWarning":true,"skipFRCCoordinateSystemWarning":true,"skipHootNonProWarning":true,"userAssetsFolder":"/","tbaApiKey":"key","field3dAntialiasing":false,"field3dModeBattery":"cinematic","field3dModeAc":"cinematic","coordinateSystem":"wall-blue","rlogPort":5801,"publishFilter":"/Foo","liveDiscard":-1,"liveSubscribeMode":"logging","liveMode":"phoenix","remotePath":"/x","robotAddress":"y","theme":"dark"}',
  '{"theme":"Dark","liveMode":"NT4","liveSubscribeMode":"","coordinateSystem":"center","field3dModeAc":"","field3dModeBattery":null,"systemcoreStaticAddress":"USB","timestamps":"zero"}',
  '{"liveDiscard":"1200","rlogPort":null,"field3dAntialiasing":"false","tbaApiKey":5,"userAssetsFolder":null,"prefersFTC":1,"robotAddress":["a"],"remotePath":{"a":1}}',
  '{"liveDiscard":-0,"rlogPort":1e400}',
  '{"liveDiscard":-1e400,"rlogPort":1.5}',
  '{"liveDiscard":1e21,"rlogPort":5e-7}',
  '{"liveDiscard":123456789012345680000,"rlogPort":1e-7}',
  '{"liveDiscard":0.1,"rlogPort":-5800}',
  '{"userAssetsFolder":"/asrs-missing-assets-folder"}',
  '{"userAssetsFolder":""}',
  '{"userAssetsFolder":"/"}',
  '{"robotAddress":"\\ud800","remotePath":"x\\udc00y","publishFilter":"\\u2028\\u2029\\u0000\\u001f\\u007f","tbaApiKey":"\\"\\\\/\\b\\f\\n\\r\\t"}',
  '{"robotAddress":"roboté.local","remotePath":"/日本","publishFilter":"😀"}',
  '{"theme":"dark","theme":"light"}',
  '{"theme":"dark","theme":5}',
  '{"__proto__":{"theme":"dark"},"liveMode":"nt4-akit"}',
  '{"1":2,"theme":"dark","0":1,"extra":true}',
  "null",
  "5",
  '"prefs"',
  "true",
  "[]",
  '[{"theme":"dark"}]',
  "",
  "{",
  '{"theme":"dark",}',
  '  \n{"theme" : "dark"}\n\n',
  "﻿{\"theme\":\"dark\"}",
  "﻿﻿{\"theme\":\"dark\"}",
  '{"liveMode":"ftcdashboard","coordinateSystem":"center-rotated","systemcoreStaticAddress":"usb"}',
  '{"liveMode":"nt4-systemcore","coordinateSystem":"center-red","systemcoreStaticAddress":""}',
  '{"coordinateSystem":"wall-alliance","liveMode":"nt4"}'
].map(utf8);
FILES.push(new Uint8Array([0x7b, 0x22, 0x72, 0x6f, 0x62, 0x6f, 0x74, 0x41, 0x64, 0x64, 0x72, 0x65, 0x73, 0x73, 0x22, 0x3a, 0x22, 0xff, 0xc3, 0x28, 0xe2, 0x82, 0x22, 0x7d]));

// Partial updates as the hub and satellites send them, plus values only a broken sender makes.
const UPDATES: string[] = [
  "{}",
  '{"hasScrolledTimeline":true}',
  '{"hasScrolledLineGraph":true}',
  '{"liveMode":"rlog"}',
  '{"coordinateSystem":"wall-alliance"}',
  '{"prefersFTC":true,"coordinateSystem":"center-rotated"}',
  '{"theme":"Light","liveDiscard":"60"}',
  '{"rioAddress":"10.0.0.2","extra":1}',
  '{"userAssetsFolder":null}',
  '{"userAssetsFolder":"/asrs-missing-assets-folder"}',
  '{"liveDiscard":-0}',
  "null",
  "5",
  '"x"',
  "[]",
  "true"
];

let seed = Number(seedArg) >>> 0 || 0x2545f491;
function next(): number {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return seed >>> 0;
}
const below = (n: number) => next() % n;
const chance = (p: number) => next() / 2 ** 32 < p;
const pick = <T>(items: readonly T[]): T => items[below(items.length)];

const ENUM_VALUES: { [key: string]: string[] } = {
  theme: ["light", "dark", "system"],
  liveMode: ["nt4", "nt4-akit", "nt4-systemcore", "phoenix", "rlog", "ftcdashboard"],
  liveSubscribeMode: ["low-bandwidth", "logging"],
  coordinateSystem: ["automatic", "wall-alliance", "wall-blue", "center-rotated", "center-red"],
  field3dModeAc: ["cinematic", "standard", "low-power"],
  threeDimensionModeAc: ["cinematic", "standard", "low-power"],
  field3dModeBattery: ["", "cinematic", "standard", "low-power"],
  threeDimensionModeBattery: ["", "cinematic", "standard", "low-power"],
  systemcoreStaticAddress: ["", "usb", "wifi"],
  timestamps: ["start-zero", "original"]
};
const STRING_KEYS = ["robotAddress", "address", "rioAddress", "remotePath", "rioPath", "publishFilter", "tbaApiKey"];
const NUMBER_KEYS = ["liveDiscard", "rlogPort"];
const BOOLEAN_KEYS = [
  "field3dAntialiasing",
  "skipHootNonProWarning",
  "skipFRCCoordinateSystemWarning",
  "skipFTCCoordinateSystemWarning",
  "ctreLicenseAccepted",
  "hasScrolledLineGraph",
  "hasScrolledTimeline",
  "prefersFTC"
];
const JUNK_KEYS = ["extra", "version", "__proto__", "0", "7", "constructor", "Theme", ""];
const ALL_KEYS = [...Object.keys(ENUM_VALUES), ...STRING_KEYS, ...NUMBER_KEYS, ...BOOLEAN_KEYS, "userAssetsFolder", ...JUNK_KEYS];

const NUMBER_TOKENS = ["0", "-0", "1200", "60", "-1", "5800", "1.5", "0.1", "1e21", "1e-7", "5e-7", "1e400", "-1e400", "4.9e-324", "1.7976931348623157e308", "9007199254740993", "123e-20"];
const STRING_TOKENS = ['""', '"robot.local"', '"10.63.28.2"', '"/U/logs"', '"\\ud800"', '"\\udfff\\ud800"', '"\\u0000"', '"é日"', '"😀"', '"\\u2028"', '"a\\"b\\\\c"'];
const OTHER_TOKENS = ["null", "true", "false", "[]", "[1,\"a\"]", "{}", '{"theme":"dark"}'];
const PATH_TOKENS = ['"/"', '"/asrs-missing-assets-folder"', '""', "null"];

function valueToken(key: string): string {
  if (chance(0.15)) return pick([...NUMBER_TOKENS, ...STRING_TOKENS, ...OTHER_TOKENS]);
  if (Object.hasOwn(ENUM_VALUES, key)) {
    if (chance(0.2)) return JSON.stringify(pick(ENUM_VALUES[key]).toUpperCase() + pick(["", " "]));
    return JSON.stringify(pick(ENUM_VALUES[key]));
  }
  if (STRING_KEYS.includes(key)) return pick(STRING_TOKENS);
  if (NUMBER_KEYS.includes(key)) return pick(NUMBER_TOKENS);
  if (BOOLEAN_KEYS.includes(key)) return pick(["true", "false"]);
  if (key === "userAssetsFolder") return pick(PATH_TOKENS);
  return pick([...NUMBER_TOKENS, ...STRING_TOKENS, ...OTHER_TOKENS]);
}

function randomObjectText(maxKeys: number): string {
  const n = below(maxKeys + 1);
  const members: string[] = [];
  for (let i = 0; i < n; i++) {
    const key = pick(ALL_KEYS);
    members.push(JSON.stringify(key) + ":" + valueToken(key));
  }
  return "{" + members.join(",") + "}";
}

function randomFileText(): string {
  if (chance(0.03)) return pick(["null", "0", '"s"', "[]", "false"]);
  return randomObjectText(24);
}

function randomUpdateText(): string {
  if (chance(0.03)) return pick(["null", "1", '"s"', "[]", "true"]);
  return randomObjectText(4);
}

function startupOutput(record: any): Uint8Array | null {
  return record.out === undefined ? null : new Uint8Array(Buffer.from(record.out.b, "hex"));
}

function prefsMode(count: number) {
  for (const platform of ["darwin", "linux"]) {
    out(runStartup(platform, null));
    for (const file of FILES) out(runStartup(platform, file));
  }

  const written: Uint8Array[] = [];
  for (const file of FILES) {
    const record = runStartup("darwin", file);
    const bytes = startupOutput(record);
    written.push(bytes ?? file);
  }
  const bases = [...written, ...FILES];
  for (const base of bases) {
    for (const data of UPDATES) {
      const record = runUpdate(base, data);
      if (record !== null) out(record);
    }
  }

  for (let i = 0; i < count; i++) {
    const file = utf8(randomFileText());
    const platform = chance(0.25) ? "linux" : "darwin";
    const startup = runStartup(platform, file);
    out(startup);
    const base = chance(0.7) ? (startupOutput(startup) ?? file) : utf8(randomFileText());
    const update = runUpdate(base, randomUpdateText());
    if (update !== null) out(update);
  }
}

function schemasMode() {
  const emit = (name: string, value: unknown) => out({ name, value: toTree(value) });
  emit("DEFAULT_PREFS", DEFAULT_PREFS);
  for (const mode of ENUM_VALUES.liveMode) emit(`getLiveModeName ${mode}`, getLiveModeName(mode as any));
  for (const address of ["", "usb", "wifi"]) {
    for (const platform of ["darwin", "win32", "linux"]) {
      const prefs = { ...DEFAULT_PREFS, robotAddress: "10.63.28.2", systemcoreStaticAddress: address };
      emit(`getRobotAddress ${address} ${platform}`, getRobotAddress(prefs as any, platform));
    }
  }
  emit("TabType", TabType);
  emit("getAllTabTypes", getAllTabTypes());
  for (const type of getAllTabTypes()) {
    emit(`getDefaultTabTitle ${type}`, getDefaultTabTitle(type));
    emit(`getTabIcon ${type}`, getTabIcon(type));
    emit(`getTabAccelerator ${type}`, getTabAccelerator(type));
  }
  emit("SelectionMode", SelectionMode);
  emit("Distribution", buildConstants.Distribution);
  emit("DISTRIBUTION", buildConstants.DISTRIBUTION);
  emit("IS_LITE", buildConstants.IS_LITE);
  emit("LITE_VERSION", buildConstants.LITE_VERSION);
  emit("BUILD_DATE", buildConstants.BUILD_DATE);
  emit("COPYRIGHT", buildConstants.COPYRIGHT);
}

try {
  if (mode === "prefs") prefsMode(Number(countArg));
  else if (mode === "schemas") schemasMode();
  else throw new Error(`genModel: unknown mode ${mode}`);
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
