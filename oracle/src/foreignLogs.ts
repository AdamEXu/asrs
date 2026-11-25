import "./shims";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import WorkerManager from "@upstream/hub/WorkerManager";
import { ByteSource } from "./corpus/bytes";
import { RlogBuilder, r1Body } from "./corpus/rlog";
import { RrBuilder, rr } from "./corpus/roadrunner";
import { WpilogBuilder, payload } from "./corpus/wpilog";
import { runWithLog } from "./logRun";
import { OracleWorker, UPSTREAM_DIR, resolveWorkerScript, setWorkerRouter, terminatedWorkersExited } from "./shims";

// foreignLogs decode
//   The built decode workers on files whose field names Object.prototype defines. One record per
//   case: {name, worker, file (hex), requests (field keys), replies}, with one list of what the
//   worker did per request, "start" first: ["initial", isPartial, fields], ["failed"],
//   ["fields", fields] or ["threw", message]. A field is [key, type, timestamps, values,
//   structuredType, wpilibType, metadataString]; progress messages are left out.
//
// foreignLogs exports
//   The real export worker on logs with such names and with values of other types than their
//   field's. One record per case and export: {name, ops, options, bytes (hex, or null when the
//   worker rejected)}. Ops are logRun's, with plain JSON values.

const NAMES = Object.getOwnPropertyNames(Object.prototype);

interface DecodeCase {
  name: string;
  worker: string;
  file: Uint8Array;
  requests: string[];
}

function wpilog(entries: [string, string, ByteSource][], finishFirst?: number): Uint8Array {
  const b = new WpilogBuilder();
  entries.forEach(([name, type], i) => b.start(0, i + 1, name, type));
  if (finishFirst !== undefined) b.finish(finishFirst, 1);
  entries.forEach(([, , data], i) => {
    if (finishFirst === undefined || i > 0) b.record(i + 1, 1000000 * (i + 1), data);
  });
  return b.bytes();
}

function decodeCases(): DecodeCase[] {
  const cases: DecodeCase[] = [];
  const wpi = (name: string, file: Uint8Array, ...requests: string[]) =>
    cases.push({ name: `wpilog: ${name}`, worker: "wpilogWorker", file, requests });
  const a: [string, string, ByteSource] = ["a", "double", payload.double(2)];
  const schema: [string, string, ByteSource] = ["/.schema/struct:S", "structschema", "int8 x"];
  for (const key of NAMES) {
    wpi(key, wpilog([[key, "double", payload.double(1)], a]), "a", key, "/" + key, "a");
    wpi(`${key}, then a schema`, wpilog([[key, "double", payload.double(1)], schema, a]), "a");
    wpi(`${key} with a slash`, wpilog([["/" + key, "double", payload.double(1)], a]), "/" + key, key);
    wpi(`${key}/.type`, wpilog([[key + "/.type", "string", "Pose2d"], [key + "/x", "double", payload.double(3)], a]), key + "/.type", key + "/x", "a");
  }
  wpi("a name that ends before a second", wpilog([["toString", "double", payload.double(1)], schema, a], 500000), "a");
  wpi("a name that ends after a second", wpilog([["toString", "double", payload.double(1)], schema, a], 1500000), "a");
  wpi("a struct", wpilog([["valueOf", "struct:S", [1]], schema, a]), "a");
  wpi("a string array", wpilog([["constructor", "string[]", payload.stringArray(["x"])], a]), "constructor", "a");
  wpi("an enabled key", wpilog([["toString", "boolean", payload.boolean(true)], ["/DriverStation/Enabled", "boolean", payload.boolean(true)], a]), "a");
  wpi("an empty log with a name", wpilog([["toString", "raw", []]]), "toString");

  const csv = (name: string, text: string) =>
    cases.push({ name: `csv: ${name}`, worker: "csvWorker", file: new TextEncoder().encode(text), requests: [] });
  const values = ["1", "true", '"s"', "(empty)", "01-02", "[1; 2]", '[1; "a"]', "[]", "null", '{"a":1}', "0a"];
  for (const key of NAMES) csv(key, `Timestamp,a,${key}\n0,1,2\n1,2,3`);
  for (const value of values) {
    csv(`table ${value}`, `Timestamp,a,toString\n0,1,${value}\n1,2,3`);
    csv(`list ${value}`, `Timestamp,Key,Value\n0,a,1\n0,__proto__,${value}\n1,a,2`);
    csv(`child ${value}`, `Timestamp,Key,Value\n0,a,1\n0,a/valueOf,${value}\n1,a,2`);
  }

  const rlog = (name: string, file: Uint8Array) =>
    cases.push({ name: `rlog: ${name}`, worker: "rlogWorker", file, requests: [] });
  const r2: [string, ByteSource][] = [
    ["boolean", payload.boolean(true)],
    ["double", new Uint8Array(8)],
    ["string", "s"],
    ["boolean[]", payload.booleanArray([true])],
    ["double[]", new Uint8Array(8)],
    ["string[]", new Uint8Array(4)],
    ["json", '{"a":1}'],
    ["msgpack", [0x81, 0xa1, 0x61, 1]],
    ["struct:S", [1]],
    ["struct:S[]", [1]],
    ["photonstruct:S", [1]],
    ["proto:p.S", [1]],
    ["proto:FileDescriptorProto", []],
    ["raw", [1]],
    ["URCL", [1]]
  ];
  for (const [type, data] of r2) {
    const b = new RlogBuilder(2).cycle(1).key(1, "/a", "string").value(1, "x").key(2, "toString", type).value(2, data);
    rlog(`r2 ${type}`, b.cycle(2).value(1, "y").bytes());
  }
  for (const key of NAMES) {
    rlog(`r2 ${key} without a value`, new RlogBuilder(2).cycle(1).key(1, key, "string").key(2, "/a", "string").value(2, "x").bytes());
    rlog(`r2 ${key}/.type`, new RlogBuilder(2).cycle(1).key(1, key + "/.type", "string").value(1, "Pose2d").cycle(2).key(2, "/a", "string").value(2, "x").bytes());
  }
  const r1: [number, ByteSource][] = [
    [0, []],
    [1, r1Body.boolean(true)],
    [9, [1]],
    [3, [0, 0, 0, 1]],
    [5, new Uint8Array(8)],
    [7, [0, 1, 0x73]],
    [2, [0, 1, 1]],
    [10, [0, 1, 1]],
    [4, [0, 0]],
    [6, [0, 0]],
    [8, [0, 0]]
  ];
  for (const [tag, body] of r1) {
    const b = new RlogBuilder(1).cycle(1).key(1, "/a").r1(1, 7, [0, 1, 0x78]).key(2, "valueOf").r1(2, tag, body);
    rlog(`r1 ${tag}`, b.cycle(2).r1(1, 7, [0, 1, 0x79]).bytes());
  }

  const road = (name: string, b: RrBuilder) =>
    cases.push({ name: `road runner: ${name}`, worker: "roadRunnerWorker", file: b.bytes(), requests: [] });
  const pose = rr.struct(["x", rr.double], ["y", rr.double], ["heading", rr.double]);
  for (const key of ["toString", "__proto__"]) {
    const messages: [string, any, any][] = [
      ["double", rr.double, 1],
      ["boolean", rr.boolean, true],
      ["string", rr.string, "s"],
      ["long", rr.long, 1n],
      ["struct", rr.struct(["a", rr.double]), { a: 1 }],
      ["pose", pose, { x: 1, y: 2, heading: 3 }],
      ["array", rr.array(rr.double), [1, 2]],
      ["empty array", rr.array(rr.double), []]
    ];
    for (const [name, schema, value] of messages) {
      const b = new RrBuilder();
      const first = b.channel("a", rr.double);
      const second = b.channel(key, schema);
      road(`${key} ${name}`, b.message(first, 1).message(second, value).message(first, 2));
    }
  }
  return cases;
}

function fieldOf([key, data]: [string, any]): unknown {
  return [key, data.type, data.timestamps, data.values, data.structuredType, data.wpilibType, data.metadataString];
}

function replyOf(message: any): unknown {
  switch (message.type) {
    case "initial":
      return ["initial", message.isPartial, Object.entries(message.log.fields).map(fieldOf as any)];
    case "fields":
      return ["fields", message.fields.map((field: any) => fieldOf([field.key, field.data]))];
    default:
      return [message.type];
  }
}

async function decode(c: DecodeCase): Promise<unknown[]> {
  const url = JSON.stringify(pathToFileURL(path.join(UPSTREAM_DIR, "bundles", c.worker + ".js")).href);
  const worker = new OracleWorker(
    `data:text/javascript,${encodeURIComponent(`
console.warn = console.error = () => {};
await import(${url});
const handle = self.onmessage;
self.onmessage = async (event) => {
  try {
    await handle(event);
  } catch (e) {
    postMessage({ threw: String(e) });
  }
  postMessage({ done: true });
};`)}`
  );
  const requests = [
    { type: "start", data: [new Uint8Array(c.file)] },
    ...c.requests.map((key) => ({ type: "parseField", key }))
  ];
  const replies: unknown[] = [];
  for (const request of requests) {
    const reply: unknown[] = [];
    await new Promise<void>((resolve, reject) => {
      worker.onerror = (event) => reject(new Error(`${c.name}: ${event.message}`));
      worker.onmessage = (event) => {
        if (event.data.done) resolve();
        else if (event.data.threw !== undefined) reply.push(["threw", event.data.threw]);
        else if (event.data.type !== "progress") reply.push(replyOf(event.data));
      };
      worker.postMessage(request);
    });
    replies.push(reply);
  }
  worker.terminate();
  await terminatedWorkersExited();
  return replies;
}

const VALUES = [true, false, 0, 1, 1.5, 1e21, "", "5", "a,b", "é\n", [], [1], [1, "a", null], [[1], { b: 2 }], {}, { a: 1, b: [true] }];

function exportCases(): { name: string; ops: any[][] }[] {
  const cases: { name: string; ops: any[][] }[] = [];
  for (const method of ["putBoolean", "putNumber", "putString"]) {
    cases.push({ name: `${method} of each value`, ops: VALUES.map((v, i) => [method, "k", i, v]) });
    const ops = VALUES.map((v, i) => [method, `k${i}`, i / 2, v]);
    cases.push({ name: `${method}, a field per value`, ops: [...ops, ["putNumber", "/RealOutputs/Timestamp", 0, 1]] });
    cases.push({
      name: `${method} before and after its type`,
      ops: [
        [method, "k", 0, { putBoolean: true, putNumber: 7, putString: "g" }[method]],
        [method, "k", 1, [1]],
        [method, "k", 2, { putBoolean: false, putNumber: 8, putString: "h" }[method]]
      ]
    });
  }
  const tree = (name: string, ...keys: string[]) =>
    cases.push({ name, ops: keys.map((key, i) => ["putNumber", key, i, i]) });
  for (const key of NAMES) {
    tree(`a key that ends at ${key}`, "a/b", `a/${key}`, "c");
    tree(`a key through ${key}`, "a/b", `a/${key}/d`, "c");
  }
  tree("fullKey before __proto__", "a/fullKey", "a/__proto__", "c");
  tree("fullKey after __proto__", "a/__proto__", "b/fullKey", "c");
  tree("a key through fullKey after __proto__", "a/__proto__", "b/fullKey/d", "c");
  tree("fullKey and a slash after __proto__", "a/__proto__", "b/fullKey/", "c");
  tree("fullKey after __proto__ and a slash", "a/__proto__/", "b/fullKey", "c");
  tree("an existing fullKey after __proto__", "b/fullKey", "a/__proto__", "b/fullKey/d");
  return cases;
}

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
setWorkerRouter(null);
switch (process.argv[2]) {
  case "decode":
    for (const c of decodeCases()) {
      const replies = await decode(c);
      out({ name: c.name, worker: c.worker, file: Buffer.from(c.file).toString("hex"), requests: c.requests, replies });
    }
    break;
  case "exports":
    for (const { name, ops } of exportCases()) {
      const { log } = runWithLog(true, ops);
      for (const format of ["csv-table", "csv-list", "wpilog", "mcap"]) {
        for (const samplingMode of ["changes", "fixed", "akit"]) {
          const options = { format, samplingMode, samplingPeriod: 500, prefixes: "", includeGenerated: true };
          const worker = JSON.stringify(resolveWorkerScript("../bundles/exportWorker.js"));
          const script = `data:text/javascript,${encodeURIComponent(
            `process.on("unhandledRejection", () => {});\nawait import(${worker});`
          )}`;
          const content: any = await WorkerManager.request(script, { options, log: log.toSerialized() }).catch(() => null);
          const bytes = content === null ? null : Buffer.from(content).toString("hex");
          out({ name, ops, options, bytes });
        }
      }
    }
    break;
  default:
    throw new Error("usage: foreignLogs decode|exports");
}
process.exit(0);
