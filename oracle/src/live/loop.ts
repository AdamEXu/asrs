import { V8MathRandom } from "./random";

type Callback = (...args: any[]) => void;

interface Timer {
  id: number;
  due: number;
  repeat: number | null;
  level: number;
  precise: boolean;
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

const MIN_DELAY_MS = 4;
const MAX_UNCLAMPED_LEVEL = 5;
const PRECISE_DELAY_MS = 32;
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
 * Every task is followed by a full microtask drain, as in a browser. Timers carry the HTML
 * timer nesting level, as measured in Electron 39: a timer created while a timer task (or its
 * microtasks) runs at level n gets n + 1, one created by any other task gets 1, an interval
 * rises one level per repeat, and a delay under 4 ms is raised to 4 ms when the creating level
 * is above 5. `requestIdleCallback` fires at its `timeout` (50 ms without one),
 * `requestAnimationFrame` 16 ms later and `AbortSignal.timeout` at its delay, all three as
 * delayed tasks outside the nesting rules (level 0, never clamped) but in the timers' due-time
 * order. At equal due times precise timers, those with a delay from 1 to 31 ms, run before the
 * others: longer timers, the delayed tasks, and timers without a delay, which join the queue behind
 * what is already due. Each group runs in creation order (`live/net/timers`). The emulated main
 * process shares these rules although Node has no nesting clamp; main.ts never asks for a delay
 * under 4 ms. Exceptions escaping a task are swallowed, as a
 * page reports and continues.
 */
export class LiveLoop {
  now = 0;
  private timers: Timer[] = [];
  private tasks: Task[] = [];
  private nextId = 1;
  private nextSeq = 0;
  protected runningLevel = 0;
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
      this.postDelayedTask(() => callback({ didTimeout: true, timeRemaining: () => 0 }), options?.timeout ?? IDLE_DEFAULT_MS);
    g.cancelIdleCallback = (id: number) => this.clear(id);
    g.requestAnimationFrame = (callback: Callback) => this.postDelayedTask(() => callback(loop.now), FRAME_MS);
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

  protected add(callback: Callback, delay: number | undefined, repeat: boolean, args: unknown[], nested = true): number {
    const id = this.nextId++;
    let wait = Math.max(0, Number(delay) | 0);
    if (nested && this.runningLevel > MAX_UNCLAMPED_LEVEL && wait < MIN_DELAY_MS) wait = MIN_DELAY_MS;
    this.timers.push({
      id,
      due: this.now + wait,
      repeat: repeat ? Math.max(MIN_DELAY_MS, wait) : null,
      level: nested ? this.runningLevel + 1 : 0,
      precise: nested && wait > 0 && wait < PRECISE_DELAY_MS,
      callback,
      args
    });
    return id;
  }

  /** A one-shot delayed task that is not a `DOMTimer`: never clamped, run at nesting level 0. */
  postDelayedTask(callback: Callback, delay: number): number {
    return this.add(callback, delay, false, [], false);
  }

  protected clear(id: number): void {
    this.timers = this.timers.filter((t) => t.id !== id);
  }

  /** Queues a client task `delay` ms from now, behind every task already queued for that time. */
  deliver(delay: number, run: () => void): void {
    this.tasks.push({ due: this.now + delay, seq: this.nextSeq++, run });
  }

  private earliestTimer(): Timer | undefined {
    let best: Timer | undefined = undefined;
    for (const t of this.timers) {
      if (best === undefined || t.due < best.due || (t.due === best.due && this.runsBefore(t, best))) best = t;
    }
    return best;
  }

  /** The order of timers due at the same time. */
  private runsBefore(a: Timer, b: Timer): boolean {
    return a.precise === b.precise ? a.id < b.id : a.precise;
  }

  private earliestTask(): Task | undefined {
    let best: Task | undefined = undefined;
    for (const t of this.tasks) {
      if (best === undefined || t.due < best.due || (t.due === best.due && t.seq < best.seq)) best = t;
    }
    return best;
  }

  /** When the server, a delivery or a timer next has something to do. */
  protected nextDue(): number | null {
    const due = [this.server?.nextTime() ?? null, this.earliestTask()?.due ?? null, this.earliestTimer()?.due ?? null];
    const times = due.filter((t): t is number => t !== null);
    return times.length === 0 ? null : Math.min(...times);
  }

  /** The client task to run next at this time, if one is due. */
  protected takeDue(): (() => void) | null {
    const task = this.earliestTask();
    if (task !== undefined && task.due <= this.now) {
      this.tasks.splice(this.tasks.indexOf(task), 1);
      this.runningLevel = 0;
      return task.run;
    }
    const timer = this.earliestTimer();
    if (timer === undefined || timer.due > this.now) return null;
    this.runningLevel = timer.level;
    if (timer.repeat === null) {
      this.clear(timer.id);
    } else {
      timer.due = this.now + (timer.repeat - ((this.now - timer.due) % timer.repeat));
      timer.level++;
    }
    return () => timer.callback(...timer.args);
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
      const candidates = [nextStep < steps.length ? steps[nextStep].t : null, this.nextDue()].filter(
        (t): t is number => t !== null
      );
      if (candidates.length === 0) break;
      const t = Math.min(...candidates);
      if (t > endMs) break;
      if (t > this.now) this.now = t;

      if (this.server !== null && this.server.nextTime() === this.now) this.server.process(this.now);
      while (nextStep < steps.length && steps[nextStep].t <= this.now) {
        this.runningLevel = 0;
        await this.runTask(steps[nextStep++].run);
      }
      for (;;) {
        const run = this.takeDue();
        if (run === null) break;
        await this.runTask(run);
      }
    }
    if (endMs > this.now) this.now = endMs;
  }
}
