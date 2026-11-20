import { V8MathRandom } from "./random";

type Callback = (...args: any[]) => void;

interface Timer {
  id: number;
  due: number;
  repeat: number | null;
  callback: Callback;
  args: unknown[];
}

interface Task {
  due: number;
  seq: number;
  run: () => void;
}

export interface ClientStep {
  t: number;
  run: () => void;
}

/** The mock server's side of the loop: work it has queued, and running all of it for one time. */
export interface ServerSide {
  nextTime(): number | null;
  process(t: number): void;
}

const MIN_REPEAT_MS = 4;
const IDLE_DEFAULT_MS = 50;
const FRAME_MS = 16;

const g = globalThis as any;
const realSetImmediate: (cb: () => void) => unknown = g.setImmediate;

/**
 * One fake-clock event loop for a whole live session: the renderer, the emulated main process
 * and the mock server all run on it. Times are integer milliseconds since the session start;
 * `Date.now()` is `epochMs` plus that. At each time the loop runs, in this order:
 *
 * 1. the server: every client message arriving now (in send order), then the session's server
 *    steps for now (in file order);
 * 2. the session's client steps for now (in file order);
 * 3. client tasks until none is due: a delivery (server message, IPC, socket event) due now,
 *    oldest first, ahead of any timer; otherwise the next due timer under Blink's rules
 *    (see `./clock.ts`, which these rules extend).
 *
 * Every task is followed by a full microtask drain, as in a browser. `requestIdleCallback`
 * fires at its `timeout` (50 ms without one) and `requestAnimationFrame` 16 ms later, both as
 * ordinary timers. Exceptions escaping a task are swallowed, as a page reports and continues.
 */
export class LiveLoop {
  now = 0;
  private timers: Timer[] = [];
  private tasks: Task[] = [];
  private nextId = 1;
  private nextSeq = 0;
  private saved: { [name: string]: unknown } | null = null;
  server: ServerSide | null = null;

  constructor(
    readonly epochMs: number,
    private random: V8MathRandom
  ) {}

  wallNow(): number {
    return this.epochMs + this.now;
  }

  install(): void {
    if (this.saved !== null) return;
    const names = [
      "setTimeout",
      "clearTimeout",
      "setInterval",
      "clearInterval",
      "requestIdleCallback",
      "cancelIdleCallback",
      "requestAnimationFrame",
      "cancelAnimationFrame",
      "Date"
    ];
    this.saved = Object.fromEntries(names.map((name) => [name, g[name]]));
    this.saved.random = Math.random;
    const loop = this;
    const RealDate = Date;
    class FakeDate extends RealDate {
      constructor(...args: any[]) {
        if (args.length === 0) super(loop.wallNow());
        else super(...(args as [any]));
      }
      static now(): number {
        return loop.wallNow();
      }
    }
    g.Date = FakeDate;
    g.setTimeout = (callback: Callback, delay?: number, ...args: unknown[]) => this.add(callback, delay, false, args);
    g.setInterval = (callback: Callback, delay?: number, ...args: unknown[]) => this.add(callback, delay, true, args);
    g.clearTimeout = (id: number) => this.clear(id);
    g.clearInterval = (id: number) => this.clear(id);
    g.requestIdleCallback = (callback: Callback, options?: { timeout?: number }) =>
      this.add(() => callback({ didTimeout: true, timeRemaining: () => 0 }), options?.timeout ?? IDLE_DEFAULT_MS, false, []);
    g.cancelIdleCallback = (id: number) => this.clear(id);
    g.requestAnimationFrame = (callback: Callback) => this.add(() => callback(loop.now), FRAME_MS, false, []);
    g.cancelAnimationFrame = (id: number) => this.clear(id);
    Math.random = () => this.random.next();
  }

  uninstall(): void {
    if (this.saved === null) return;
    const { random, ...globals } = this.saved;
    Object.assign(g, globals);
    Math.random = random as () => number;
    this.saved = null;
  }

  private add(callback: Callback, delay: number | undefined, repeat: boolean, args: unknown[]): number {
    const id = this.nextId++;
    const wait = Math.max(0, Number(delay) | 0);
    this.timers.push({
      id,
      due: this.now + wait,
      repeat: repeat ? Math.max(MIN_REPEAT_MS, wait) : null,
      callback,
      args
    });
    return id;
  }

  private clear(id: number): void {
    this.timers = this.timers.filter((t) => t.id !== id);
  }

  /** Queues a client task `delay` ms from now, behind every task already queued for that time. */
  deliver(delay: number, run: () => void): void {
    this.tasks.push({ due: this.now + delay, seq: this.nextSeq++, run });
  }

  private earliestTimer(): Timer | undefined {
    let best: Timer | undefined = undefined;
    for (const t of this.timers) {
      if (best === undefined || t.due < best.due || (t.due === best.due && t.id < best.id)) best = t;
    }
    return best;
  }

  private earliestTask(): Task | undefined {
    let best: Task | undefined = undefined;
    for (const t of this.tasks) {
      if (best === undefined || t.due < best.due || (t.due === best.due && t.seq < best.seq)) best = t;
    }
    return best;
  }

  private async runTask(run: () => void): Promise<void> {
    try {
      run();
    } catch {}
    await new Promise<void>((resolve) => realSetImmediate(resolve));
  }

  async run(endMs: number, steps: ClientStep[]): Promise<void> {
    let nextStep = 0;
    for (;;) {
      const candidates = [
        this.server?.nextTime() ?? null,
        nextStep < steps.length ? steps[nextStep].t : null,
        this.earliestTask()?.due ?? null,
        this.earliestTimer()?.due ?? null
      ].filter((t): t is number => t !== null);
      if (candidates.length === 0) break;
      const t = Math.min(...candidates);
      if (t > endMs) break;
      if (t > this.now) this.now = t;

      if (this.server !== null && this.server.nextTime() === this.now) this.server.process(this.now);
      while (nextStep < steps.length && steps[nextStep].t <= this.now) {
        await this.runTask(steps[nextStep++].run);
      }
      for (;;) {
        const task = this.earliestTask();
        if (task !== undefined && task.due <= this.now) {
          this.tasks.splice(this.tasks.indexOf(task), 1);
          await this.runTask(task.run);
          continue;
        }
        const timer = this.earliestTimer();
        if (timer !== undefined && timer.due <= this.now) {
          if (timer.repeat === null) {
            this.clear(timer.id);
          } else {
            timer.due = this.now + (timer.repeat - ((this.now - timer.due) % timer.repeat));
          }
          await this.runTask(() => timer.callback(...timer.args));
          continue;
        }
        break;
      }
    }
    if (endMs > this.now) this.now = endMs;
  }
}
