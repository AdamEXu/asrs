import { HistoricalDataSource, HistoricalDataSourceStatus } from "@upstream/hub/dataSources/HistoricalDataSource";
import WorkerManager from "@upstream/hub/WorkerManager";
import Log from "@upstream/shared/log/Log";
import { setWorkerRouter } from "../shims";
import { dumpLog } from "./dump";
import { MainProcess } from "./mainProcess";
import { Scheduler } from "./scheduler";
import { FakeTimers, realClearTimeout, realSetTimeout } from "./timers";

const g = globalThis as any;

const EXPORT_STALL_MS = 600_000;

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
      session.close();
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
   * Fixed-period sampling over a range that would take more than `fixedSampleLimit` samples, or
   * whose step vanishes in float precision (upstream would not finish), is skipped.
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
    // One worker and no page timers: messages can be delivered as they arrive. Routing them
    // through the scheduler would cost an event-loop turn for each of the export worker's
    // per-sample progress messages.
    setWorkerRouter(null);
    let request: Promise<string | Uint8Array>;
    try {
      request = WorkerManager.request("../bundles/exportWorker.js", { options, log: this.log.toSerialized() });
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
    }
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.source.stop();
    this.scheduler.terminateAll();
    setWorkerRouter(null);
    this.timers.uninstall();
  }
}
