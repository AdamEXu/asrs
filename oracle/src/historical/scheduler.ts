import { EXITED, OracleWorker, WorkerRouter } from "../shims";
import { FakeTimers, realClearTimeout, realSetImmediate, realSetTimeout } from "./timers";

interface Slot {
  worker: OracleWorker;
  inbox: unknown[];
  wake: (() => void) | null;
}

function turn(): Promise<void> {
  return new Promise((resolve) => realSetImmediate(resolve));
}

/**
 * Runs page-side work in one fixed order, whatever the thread timing:
 *
 * 1. Let pending microtasks finish.
 * 2. If a timeout is due, run the earliest one and start again.
 * 3. Otherwise take the oldest worker (creation order) that has an undelivered message or is
 *    still handling a posted message, wait for its next message, deliver it and start again.
 * 4. Stop when no timeout is queued and no worker is busy.
 *
 * Interval callbacks never run here; `FakeTimers.tickIntervals` fires them explicitly.
 */
export class Scheduler implements WorkerRouter {
  private slots: Slot[] = [];

  constructor(
    private timers: FakeTimers,
    private stallMs = 120_000
  ) {}

  created(worker: OracleWorker): void {
    this.slots.push({ worker, inbox: [], wake: null });
  }

  received(worker: OracleWorker, raw: unknown): void {
    const slot = this.slots.find((s) => s.worker === worker);
    if (slot === undefined) return;
    if (raw !== EXITED) slot.inbox.push(raw);
    const wake = slot.wake;
    slot.wake = null;
    wake?.();
  }

  /** Workers created so far and messages posted to them, to detect whether a step did anything. */
  activity(): string {
    return `${this.slots.length}:${this.slots.reduce((sum, s) => sum + s.worker.posted, 0)}`;
  }

  workers(): OracleWorker[] {
    return this.slots.map((s) => s.worker);
  }

  async settle(): Promise<void> {
    for (;;) {
      await turn();
      if (this.timers.runNextTimeout()) continue;
      const slot = this.slots.find((s) => s.inbox.length > 0 || s.worker.busy);
      if (slot === undefined) return;
      if (slot.inbox.length === 0) {
        await this.waitFor(slot);
        continue;
      }
      slot.worker.deliver(slot.inbox.shift());
    }
  }

  private waitFor(slot: Slot): Promise<void> {
    return new Promise((resolve, reject) => {
      const timer = realSetTimeout(() => {
        slot.wake = null;
        reject(new Error(`worker ${slot.worker.script} sent nothing for ${this.stallMs} ms while busy`));
      }, this.stallMs);
      slot.wake = () => {
        realClearTimeout(timer);
        resolve();
      };
    });
  }

  terminateAll(): void {
    for (const slot of this.slots) slot.worker.terminate();
  }
}
