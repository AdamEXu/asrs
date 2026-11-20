import Log from "@upstream/shared/log/Log";
import { ByteSource, toBytes, utf8 } from "./bytes";
import { Corpus } from "./corpus";
import { dslog, dsevents, lvTime, LV_MATCH_START, randomRecord } from "./dslog";
import { exportBytes } from "./exporter";
import { RlogBuilder, r2Payload } from "./rlog";
import { randomRoadRunner } from "./roadrunner";
import { Rng } from "./rng";
import { WpilogBuilder, payload } from "./wpilog";

const SECOND = 1000000;

interface Embedded {
  name: string;
  type: string;
  bytes: ByteSource;
  chunks?: number;
  chunkTimestamps?: number[];
  start?: number;
  finish?: number;
}

/** A parent WPILOG with a plain field and each embedded log split into raw records. */
export function parentLog(children: Embedded[], extra = "child corpus"): Uint8Array {
  const b = new WpilogBuilder({ extra });
  b.start(0, 1, "/Parent/Value", "double", "");
  for (let i = 0; i < 5; i++) b.record(1, i * SECOND, payload.double(i));
  children.forEach((child, c) => {
    const id = c + 2;
    const bytes = toBytes(child.bytes);
    const chunks = child.chunks ?? 1;
    const size = Math.ceil(bytes.length / chunks);
    b.start(child.start ?? 0, id, child.name, child.type, "");
    for (let k = 0; k < chunks; k++) {
      const ts = child.chunkTimestamps?.[k] ?? SECOND + k * 1000;
      b.record(id, ts, bytes.slice(k * size, (k + 1) * size));
    }
    if (child.finish !== undefined) b.finish(child.finish, id);
  });
  return b.bytes();
}

export async function childWpilog(rng: Rng, prefix: string): Promise<Uint8Array> {
  const log = new Log();
  for (let i = 0; i < 30; i++) {
    log.putNumber(`${prefix}/Speed`, i * 0.02, rng.double());
    log.putBoolean(`${prefix}/Enabled`, i * 0.02, i > 10);
    if (i % 5 === 0) log.putString(`${prefix}/State`, i * 0.02, rng.ascii(6));
  }
  log.putStringArray("/Log1/LooksLikeMergePrefix", 0.1, ["merged?"]);
  return exportBytes(log, { format: "wpilog" });
}

function childRlog(): Uint8Array {
  const b = new RlogBuilder(2).cycle(0).key(1, "/RLOG/Value", "double").key(2, "/RLOG/Name", "string");
  for (let i = 0; i < 10; i++) b.cycle(i * 0.02).value(1, r2Payload.double(i * 1.5)).value(2, `n${i}`);
  return b.bytes();
}

export async function childMatrix(corpus: Corpus): Promise<void> {
  const rng = corpus.rng("child");
  const wpilog = await childWpilog(rng, "/Child");
  const rlog = childRlog();
  const csv = utf8("Timestamp,/CSV/A,/CSV/B\n0,1,true\n0.5,2,false\n");
  const rr = randomRoadRunner(corpus.rng("child/rr"), 40);
  const ds = dslog(lvTime(LV_MATCH_START), Array.from({ length: 50 }, () => randomRecord(rng, "rev")));
  const add = (id: string, about: string, bytes: Uint8Array) => corpus.add(`child/${id}`, about, [{ path: `child/${id}.wpilog`, bytes }]);

  add(
    "every-extension",
    "log:wpilog, log:rlog, log:csv and log:log children decoded in the renderer, each its own field prefix",
    parentLog([
      { name: "/Embedded/WPILOG", type: "log:wpilog", bytes: wpilog },
      { name: "/Embedded/RLOG", type: "log:rlog", bytes: rlog },
      { name: "/Embedded/CSV", type: "log:csv", bytes: csv },
      { name: "/Embedded/RoadRunner", type: "log:log", bytes: rr }
    ])
  );
  add(
    "via-main",
    "log:dslog, log:hoot and log:revlog children go through the main process as temp files; log:../x and log:a/b are rejected extensions",
    parentLog([
      { name: "/Embedded/DS", type: "log:dslog", bytes: ds },
      { name: "/Embedded/Hoot", type: "log:hoot", bytes: wpilog },
      { name: "/Embedded/Revlog", type: "log:revlog", bytes: "not a revlog" },
      { name: "/Embedded/Escape", type: "log:../x", bytes: wpilog },
      { name: "/Embedded/Slash", type: "log:a/b", bytes: wpilog }
    ])
  );
  add(
    "type-spelling",
    "log:WPILOG upper case lowercases to wpilog; log: with no extension; log:wpilog with spaces; logs: and LOG:wpilog are not child logs",
    parentLog([
      { name: "/Upper", type: "log:WPILOG", bytes: wpilog },
      { name: "/Empty", type: "log:", bytes: wpilog },
      { name: "/Spaced", type: "log: wpilog", bytes: wpilog },
      { name: "/NotChild", type: "logs:wpilog", bytes: wpilog },
      { name: "/NotChildEither", type: "LOG:wpilog", bytes: wpilog }
    ])
  );
  add(
    "chunked",
    "a child split into 64 raw records concatenated in timestamp order, plus one split into 2000 tiny records",
    parentLog([
      { name: "/Chunked", type: "log:wpilog", bytes: wpilog, chunks: 64 },
      { name: "/Tiny", type: "log:rlog", bytes: rlog, chunks: Math.min(2000, rlog.length) }
    ])
  );
  const n = 8;
  add(
    "chunk-timestamps",
    "chunks written out of time order are reassembled in time order; chunks sharing a timestamp overwrite each other and corrupt the child",
    parentLog([
      { name: "/Reversed", type: "log:wpilog", bytes: wpilog, chunks: n, chunkTimestamps: Array.from({ length: n }, (_, k) => (n - k) * SECOND) },
      { name: "/Shared", type: "log:wpilog", bytes: wpilog, chunks: n, chunkTimestamps: [1, 1, 2, 2, 3, 3, 4, 4].map((s) => s * SECOND) }
    ])
  );
  add(
    "short-lived",
    "a child entry finished within 1 s is deleted with its records",
    parentLog([{ name: "/Brief", type: "log:wpilog", bytes: wpilog, start: SECOND, finish: SECOND + 500000 }])
  );
  add(
    "invalid",
    "children whose bytes do not decode: garbage wpilog, rlog with a bad revision, csv with a bad header, an empty child",
    parentLog([
      { name: "/BadWpilog", type: "log:wpilog", bytes: "WPILOX" },
      { name: "/BadRlog", type: "log:rlog", bytes: [9, 0] },
      { name: "/BadCsv", type: "log:csv", bytes: "nope" },
      { name: "/EmptyChild", type: "log:wpilog", bytes: [] }
    ])
  );
  const grandchild = parentLog([{ name: "/Inner/RLOG", type: "log:rlog", bytes: rlog }], "grandchild");
  const middle = parentLog([{ name: "/Middle/WPILOG", type: "log:wpilog", bytes: grandchild }], "middle");
  add("nested", "a wpilog child inside a wpilog child that holds an rlog child (three levels of prefixes)", parentLog([{ name: "/Outer", type: "log:wpilog", bytes: middle }]));
  add(
    "key-collisions",
    "a child whose keys repeat the parent's own keys and a child under a key that already has children",
    parentLog([
      { name: "/Parent", type: "log:wpilog", bytes: await childWpilog(rng, "/Value") },
      { name: "/Parent/Value/Child", type: "log:csv", bytes: "Timestamp,/X\n0,1" }
    ])
  );

  const rlogParent = new RlogBuilder(2).cycle(0).key(1, "/Embedded", "log:wpilog").value(1, wpilog.slice(0, 30000));
  corpus.add("child/rlog-parent", "a log:wpilog field inside an RLOG file: RLOG loads are not partial so the child is never opened", [
    { path: "child/rlog-parent.rlog", bytes: rlogParent.bytes() }
  ]);
  corpus.add("child/dsevents-sibling", "a log:dsevents child goes through the main process, which only reads .dslog pairs", [
    { path: "child/dsevents-sibling.wpilog", bytes: parentLog([{ name: "/Events", type: "log:dsevents", bytes: dsevents(lvTime(LV_MATCH_START), [{ time: lvTime(LV_MATCH_START + 1n), text: "x" }]) }]) }
  ]);
}
