type Callback = (...args: any[]) => void;

interface Timer {
  id: number;
  due: number;
  repeat: number | null;
  callback: Callback;
  args: unknown[];
}

const g = globalThis as any;
const MIN_REPEAT_MS = 4;

/**
 * A scripted clock behind `Date.now`, `setTimeout` and `setInterval` that follows Blink's timer
 * rules, as measured in Electron 39's renderer (the live sources run there, not under Node):
 *
 * - delays are WebIDL `long`s, so `1000 / 60` runs every 16 ms and negative delays are 0;
 * - timers due at the same time run in creation order, and an interval keeps the position of
 *   its `setInterval` call on every repeat;
 * - an interval is rescheduled before its callback runs, drift free: a late interval fires
 *   once and returns to its grid;
 * - intervals repeat no faster than every 4 ms (Blink's nesting clamp, reduced to one rule).
 *
 * `asrs_live::timers::TimerQueue` implements the same rules for the port.
 */
export class ScriptedClock {
  private nextId = 1;
  private timers: Timer[] = [];
  private saved: { [name: string]: unknown } | null = null;

  constructor(public now: number) {}

  install(): void {
    if (this.saved !== null) return;
    this.saved = {
      setTimeout: g.setTimeout,
      clearTimeout: g.clearTimeout,
      setInterval: g.setInterval,
      clearInterval: g.clearInterval,
      dateNow: Date.now
    };
    g.setTimeout = (callback: Callback, delay?: number, ...args: unknown[]) => this.add(callback, delay, false, args);
    g.setInterval = (callback: Callback, delay?: number, ...args: unknown[]) => this.add(callback, delay, true, args);
    g.clearTimeout = (id: number) => this.clear(id);
    g.clearInterval = (id: number) => this.clear(id);
    Date.now = () => this.now;
  }

  uninstall(): void {
    if (this.saved === null) return;
    const { dateNow, ...timers } = this.saved;
    Object.assign(g, timers);
    Date.now = dateNow as () => number;
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

  private earliest(): Timer | undefined {
    let best: Timer | undefined = undefined;
    for (const t of this.timers) {
      if (best === undefined || t.due < best.due || (t.due === best.due && t.id < best.id)) best = t;
    }
    return best;
  }

  /** Moves the clock forward by `ms`, running every timer that falls due on the way at its due time. */
  advance(ms: number): void {
    const target = this.now + ms;
    for (;;) {
      const timer = this.earliest();
      if (timer === undefined || timer.due > target) break;
      if (timer.due > this.now) this.now = timer.due;
      if (timer.repeat === null) {
        this.clear(timer.id);
      } else {
        timer.due = this.now + (timer.repeat - ((this.now - timer.due) % timer.repeat));
      }
      timer.callback(...timer.args);
    }
    if (target > this.now) this.now = target;
  }
}
