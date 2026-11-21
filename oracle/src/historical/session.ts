import { HistoricalDataSource, HistoricalDataSourceStatus } from "@upstream/hub/dataSources/HistoricalDataSource";
import WorkerManager from "@upstream/hub/WorkerManager";
import Log from "@upstream/shared/log/Log";
import LoggableType from "@upstream/shared/log/LoggableType";
import { AKIT_TIMESTAMP_KEYS } from "@upstream/shared/log/RobotState";
import { cleanFloat, upperBound } from "@upstream/shared/util";
import { resolveWorkerScript, terminatedWorkersExited, setWorkerRouter } from "../shims";
import { dumpLog } from "./dump";
import { MainProcess } from "./mainProcess";
import { Scheduler } from "./scheduler";
import { FakeTimers, realClearTimeout, realSetTimeout } from "./timers";

const g = globalThis as any;

const EXPORT_STALL_MS = 600_000;
const MCAP_CHUNK_SIZE = 1024 * 1024;
const MCAP_TYPES = [
  LoggableType.Boolean,
  LoggableType.Number,
  LoggableType.String,
  LoggableType.BooleanArray,
  LoggableType.NumberArray
];

/**
 * An upper bound on the bytes the MCAP chunk holds when sampled export finishes: 2048 for the
 * schema records, then for every field of an MCAP type (a superset of what is exported) 64 plus
 * the key's UTF-8 length for its channel record, and 31 plus the UTF-8 length of
 * `JSON.stringify({ value })` for every timestamp that has a value at or before it.
 */
function mcapChunkBound(log: Log, timestamps: number[]): number {
  const encoder = new TextEncoder();
  let bound = 2048;
  for (const key of log.getFieldKeys()) {
    const type = log.getType(key);
    if (type === null || !MCAP_TYPES.includes(type)) continue;
    bound += 64 + encoder.encode(key).length;
    const data = log.getRange(key, -Infinity, Infinity);
    if (data === undefined) continue;
    let next = 0;
    for (const timestamp of timestamps) {
      next = upperBound(data.timestamps, timestamp, next);
      if (next !== 0) bound += 31 + encoder.encode(JSON.stringify({ value: data.values[next - 1] })).length;
    }
  }
  return bound;
}

/**
 * The export worker behind an `unhandledRejection` handler. Chromium logs a rejection nobody
 * handles inside a worker and the worker keeps running; a Node worker thread dies of one.
 * generateMCAP's sampled branch does not await `addValue`, so a message that throws there (a
 * timestamp too large for `BigInt`, a channel without a schema) is lost while the export
 * finishes, as in the app.
 */
function exportWorkerScript(): string {
  const worker = JSON.stringify(resolveWorkerScript("../bundles/exportWorker.js"));
  const source = `process.on("unhandledRejection", () => {});\nawait import(${worker});`;
  return `data:text/javascript,${encodeURIComponent(source)}`;
}

export interface ExportOptions {
  format: "csv-table" | "csv-list" | "wpilog" | "mcap";
  samplingMode: "changes" | "fixed" | "akit";
  samplingPeriod: number;
  prefixes: string;
  includeGenerated: boolean;
}

export type ExportOutcome =
  | { outcome: "ok"; bytes: Uint8Array }
  | { outcome: "failed" }
  | { outcome: "skipped"; reason: string };

/**
 * One historical log opened the way the hub opens it: a real `HistoricalDataSource` on a fresh
 * `Log`, the real worker bundles, and the main-process side emulated. Page work runs through
 * `Scheduler`, so the result does not depend on thread timing.
 */
export class HistoricalSession {
  readonly log = new Log();
  private timers = new FakeTimers();
  private scheduler = new Scheduler(this.timers);
  private source = new HistoricalDataSource();
  private status = HistoricalDataSourceStatus.Waiting;
  private closed = false;

  private constructor() {}

  /**
   * Decode order:
   * 1. `openFile` and settle; this is the "initial" phase.
   * 2. Fire every source's 50 ms interval (no active tab or sidebar fields) and settle, until a
   *    round creates no worker and posts no message. This is what the hub does before any user
   *    action.
   * 3. Call `loadAllFields()` on the root source, as export does, and settle; then repeat from
   *    step 2 until a full pass does nothing. Child sources created along the way get the same
   *    treatment. This is the "full" phase.
   */
  static async open(path: string): Promise<HistoricalSession> {
    const session = new HistoricalSession();
    try {
      session.install();
      const main = new MainProcess((message) => session.source.handleMainMessage(message));
      g.sendMainMessage = (name: string, data?: any) => main.handle(name, data);
      session.source.openFile(
        session.log,
        path,
        "",
        (status) => (session.status = status),
        () => {},
        () => {}
      );
      await session.scheduler.settle();
      if (main.unhandled.length > 0) throw new Error(`unexpected main messages: ${main.unhandled.join(", ")}`);
    } catch (error) {
      await session.close();
      throw error;
    }
    return session;
  }

  private install(): void {
    this.timers.install();
    setWorkerRouter(this.scheduler);
    g.log = this.log;
    g.tabs = { getActiveFields: () => new Set<string>() };
    g.sidebar = { getActiveFields: () => new Set<string>() };
  }

  dump(phase: string): string[] {
    return dumpLog(this.log.toSerialized(), {
      phase,
      status: HistoricalDataSourceStatus[this.status],
      loadingFields: Array.from(this.source.getLoadingFields())
    });
  }

  async loadEverything(): Promise<void> {
    for (;;) {
      const before = this.scheduler.activity();
      for (;;) {
        const tickStart = this.scheduler.activity();
        this.timers.tickIntervals();
        await this.scheduler.settle();
        if (this.scheduler.activity() === tickStart) break;
      }
      void this.source.loadAllFields();
      await this.scheduler.settle();
      if (this.scheduler.activity() === before) return;
    }
  }

  /**
   * Runs the real export worker on `log.toSerialized()`, as the hub's `prepare-export` does, and
   * returns the bytes main would write: a string result as UTF-8, a byte result as is.
   *
   * Two option sets are skipped because upstream would not finish:
   * - fixed sampling over a range needing more than `fixedSampleLimit` samples, or whose step
   *   vanishes in float precision;
   * - MCAP with fixed or AdvantageKit sampling once `mcapChunkBound` exceeds the writer's
   *   1 MiB chunk size. That branch calls `addMessage` without awaiting it, so after the
   *   threshold every message starts another `finalizeChunk` on an ever larger chunk.
   */
  async export(options: ExportOptions, fixedSampleLimit: number): Promise<ExportOutcome> {
    if (options.samplingMode === "fixed") {
      const range = this.log.getTimestampRange();
      const step = options.samplingPeriod / 1000;
      const samples = (range[1] - range[0]) / step;
      if (!(samples <= fixedSampleLimit) || !(range[0] + step > range[0]) || !(range[1] + step > range[1])) {
        return { outcome: "skipped", reason: "fixed sampling range too large" };
      }
    }
    if (options.format === "mcap" && options.samplingMode !== "changes") {
      const timestamps = this.sampleTimestamps(options);
      if (timestamps !== undefined && mcapChunkBound(this.log, timestamps) > MCAP_CHUNK_SIZE) {
        return { outcome: "skipped", reason: "unawaited MCAP writes past one chunk" };
      }
    }
    // One worker and no page timers: messages can be delivered as they arrive. Routing them
    // through the scheduler would cost an event-loop turn for each of the export worker's
    // per-sample progress messages.
    setWorkerRouter(null);
    let request: Promise<string | Uint8Array>;
    try {
      request = WorkerManager.request(exportWorkerScript(), { options, log: this.log.toSerialized() });
    } finally {
      setWorkerRouter(this.scheduler);
    }
    let timer: ReturnType<typeof realSetTimeout> | undefined;
    const stalled = new Promise<never>((_, reject) => {
      timer = realSetTimeout(() => reject(new Error("export worker did not answer")), EXPORT_STALL_MS);
    });
    try {
      const content = await Promise.race([request, stalled]);
      const bytes = typeof content === "string" ? new Uint8Array(Buffer.from(content, "utf8")) : content;
      return { outcome: "ok", bytes };
    } catch (error) {
      if (error instanceof Error && error.message === "export worker did not answer") throw error;
      return { outcome: "failed" };
    } finally {
      realClearTimeout(timer);
      await terminatedWorkersExited();
    }
  }

  /** The export worker's timestamp list for fixed or AdvantageKit sampling. */
  private sampleTimestamps(options: ExportOptions): number[] | undefined {
    if (options.samplingMode === "fixed") {
      const step = options.samplingPeriod / 1000;
      const range = this.log.getTimestampRange();
      const timestamps: number[] = [];
      for (let timestamp = range[0]; timestamp <= range[1]; timestamp += step) timestamps.push(cleanFloat(timestamp));
      return timestamps;
    }
    const key = this.log.getFieldKeys().find((k) => AKIT_TIMESTAMP_KEYS.includes(k));
    return key === undefined ? undefined : this.log.getNumber(key, -Infinity, Infinity)?.timestamps;
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.source.stop();
    this.scheduler.terminateAll();
    setWorkerRouter(null);
    this.timers.uninstall();
    await terminatedWorkersExited();
  }
}
