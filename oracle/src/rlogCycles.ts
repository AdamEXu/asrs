import "./shims";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { Bytes, hex, toBytes } from "./corpus/bytes";
import { RlogBuilder } from "./corpus/rlog";
import { OracleWorker, UPSTREAM_DIR, setWorkerRouter, terminatedWorkersExited } from "./shims";

// rlogCycles
//   The built RLOGDecoder in a worker, on logs whose negative lengths send the offset back. One
//   record per case: {name, isFile, frames (one `decode` call each, as hex), results}, where
//   results holds what each call returned, then "never" when a call had not returned after
//   STALL_MS; and, when every call returned, the fields as [key, type, timestamps].

const STALL_MS = 10_000;

/** RLOG records from 0, with values whose length reaches back to an earlier offset. */
class Cycles extends RlogBuilder {
  back(id: number, to: number): this {
    const header = new Bytes().u8(2).be(id, 2).finish().length + 2;
    return this.value(id, "", to - (this.length + header));
  }
  backR1(id: number, to: number): this {
    const value = new Bytes().u8(2).be(id, 2).u8(7).finish().length + 2;
    return this.rawBytes(new Bytes().u8(2).be(id, 2).u8(7).be(to - (this.length + value), 2).finish());
  }
  /** A value that returns into the one before it, which holds a key record spanning this one. */
  backOnce(): this {
    const start = this.length + 5;
    const record = new Bytes().u8(1).be(9, 2).be(5, 2).finish();
    return this.value(1, record).back(1, start).rawBytes(new Bytes().be(1, 2).raw("s").finish());
  }
  rawBytes(bytes: Uint8Array): this {
    (this as any).out.raw(bytes);
    return this;
  }
}

const file = (): Cycles => {
  const b = new Cycles(2);
  b.cycle(1).key(1, "/A", "string");
  return b;
};
const DESCRIPTOR = "proto:FileDescriptorProto";
const ANON = hex("22 11 0a 04 41 6e 6f 6e 12 09 0a 01 78 18 01 20 01 28 05");

const cases: { name: string; isFile: boolean; frames: Uint8Array[] }[] = [];
const add = (name: string, b: RlogBuilder) => cases.push({ name, isFile: true, frames: [b.bytes()] });

{
  const b = file();
  add("a value that returns to itself", b.back(1, b.length));
}
{
  const b = new Cycles(1);
  b.cycle(1).key(1, "/A");
  add("an r1 string that returns to itself", b.backR1(1, b.length));
}
{
  const b = file();
  const start = b.length;
  add("a value that returns to an earlier value", b.value(1, "x").value(1, "y").back(1, start));
}
{
  const b = file();
  const start = b.length;
  b.key(2, "/B", "string").value(2, "x").key(2, "/C", "string").value(2, "y");
  add("a cycle that renames its key", b.back(1, start));
}
{
  const b = file();
  const start = b.length;
  add("a key record that returns to an earlier one", b.key(2, "/B", "string").rawBytes(new Bytes().u8(1).be(3, 2).be(start - (b.length + 5), 2).finish()));
}
add("a value that returns once, to other records", file().backOnce().cycle(2));
{
  const b = file().backOnce();
  const start = b.length;
  add("a cycle after a return to other records", b.value(1, "x").back(1, start));
}
{
  const b = file().key(3, "/.schema/proto:anon", DESCRIPTOR) as Cycles;
  const start = b.length;
  add("a cycle that adds a descriptor twice", b.value(3, ANON).back(1, start));
}
{
  const b = file().key(3, "/.schema/proto:anon", DESCRIPTOR) as Cycles;
  b.backOnce();
  const start = b.length;
  add("a cycle that adds a descriptor twice, after a return to other records", b.value(3, ANON).back(1, start));
}
// Enough records of no known type to put the port's 64th watched state on the descriptor.
for (const skipped of [62, 63, 64]) {
  const b = file().key(3, "/.schema/proto:anon", DESCRIPTOR) as Cycles;
  b.backOnce().rawBytes(new Uint8Array(skipped).fill(9));
  const start = b.length;
  add(`a cycle that adds a descriptor twice, after ${skipped} skipped records`, b.value(3, ANON).back(1, start));
}
{
  const b = file().key(3, "/.schema/proto:anon", DESCRIPTOR) as Cycles;
  b.backOnce();
  const start = b.length;
  add("a cycle that adds an empty descriptor", b.value(3, "").back(1, start));
}
{
  const b = file();
  const start = b.length;
  add("a cycle through an earlier timestamp", b.cycle(2).value(1, "x").back(1, start));
}
{
  const b = file().cycle(2) as Cycles;
  add("a value that returns before the file", b.back(1, -5));
}
{
  const live = new Cycles(2);
  live.cycle(1).key(1, "/A", "string");
  const marker = new Uint8Array([0]);
  const cycle = new Cycles(2);
  const start = cycle.length;
  cycle.rawBytes(new Bytes().f64(2, false).finish()).value(1, "x").rawBytes(new Uint8Array([0]));
  cycle.back(1, start);
  const frame = cycle.bytes().slice();
  frame[0] = 0;
  const loop = new Cycles(2);
  loop.rawBytes(new Bytes().f64(3, false).finish());
  loop.back(1, loop.length);
  const loopFrame = loop.bytes().slice();
  loopFrame[0] = 0;
  cases.push({ name: "live: a frame with another timestamp", isFile: false, frames: [live.bytes(), frame] });
  cases.push({ name: "live: a frame whose value returns to itself", isFile: false, frames: [live.bytes(), marker, loopFrame] });
  const again = new Cycles(2);
  again.rawBytes(new Bytes().f64(4, false).finish());
  const back = again.length;
  again.value(1, "x").cycle(5).back(1, back);
  const againFrame = again.bytes().slice();
  againFrame[0] = 0;
  cases.push({ name: "live: a cycle through a timestamp", isFile: false, frames: [live.bytes(), againFrame] });
}

setWorkerRouter(null);
const bundle = (name: string) => JSON.stringify(pathToFileURL(path.join(UPSTREAM_DIR, "bundles", "chunk", name)).href);
const script = `data:text/javascript,${encodeURIComponent(`
import { RLOGDecoder } from ${bundle("RLOGDecoder.js")};
import { Log } from ${bundle("Log.js")};
console.warn = () => {};
self.onmessage = (event) => {
  postMessage("started");
  const decoder = new RLOGDecoder(event.data.isFile);
  const log = new Log(false);
  for (const frame of event.data.frames) postMessage(decoder.decode(log, frame));
  postMessage({ fields: Object.entries(log.fields).map(([key, f]) => [key, f.getType(), f.getTimestamps()]) });
};`)}`;

function run(c: { isFile: boolean; frames: Uint8Array[] }): Promise<{ results: unknown[]; fields: unknown }> {
  return new Promise((resolve, reject) => {
    const worker = new OracleWorker(script);
    const results: unknown[] = [];
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = (fields: unknown) => {
      clearTimeout(timer);
      worker.terminate();
      resolve({ results, fields });
    };
    const arm = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        results.push("never");
        finish(null);
      }, STALL_MS);
    };
    worker.onerror = (event) => reject(new Error(event.message));
    worker.onmessage = (event) => {
      if (typeof event.data === "object") return finish(event.data.fields);
      if (typeof event.data === "boolean") results.push(event.data);
      arm();
    };
    worker.postMessage({ isFile: c.isFile, frames: c.frames.map((f) => new Uint8Array(toBytes(f))) });
  });
}

for (const c of cases) {
  const { results, fields } = await run(c);
  await terminatedWorkersExited();
  const frames = c.frames.map((f) => Buffer.from(f).toString("hex"));
  process.stdout.write(JSON.stringify({ name: c.name, isFile: c.isFile, frames, results, fields }) + "\n");
}
process.exit(0);
