type Callback = (...args: any[]) => void;

interface Timeout {
  id: number;
  due: number;
  callback: Callback;
  args: unknown[];
}

interface Interval {
  id: number;
  callback: Callback;
  args: unknown[];
}

const g = globalThis as any;

export const realSetImmediate: typeof setImmediate = g.setImmediate.bind(globalThis);
export const realSetTimeout: typeof setTimeout = g.setTimeout.bind(globalThis);
export const realClearTimeout: typeof clearTimeout = g.clearTimeout.bind(globalThis);

/**
 * Replaces the page-side timer globals. Timeouts run only from `runNextTimeout`, earliest due
 * time first and then in scheduling order; intervals run only from `tickIntervals`. Nothing
 * fires on its own, so the order of page work does not depend on wall-clock time.
 */
export class FakeTimers {
  private nextId = 1;
  private now = 0;
  private timeouts: Timeout[] = [];
  private intervals: Interval[] = [];
  private saved: { [name: string]: unknown } | null = null;

  install(): void {
    if (this.saved !== null) return;
    this.saved = {
      setTimeout: g.setTimeout,
      clearTimeout: g.clearTimeout,
      setInterval: g.setInterval,
      clearInterval: g.clearInterval
    };
    g.setTimeout = (callback: Callback, delay?: number, ...args: unknown[]) => {
      const id = this.nextId++;
      const wait = Number(delay);
      this.timeouts.push({ id, due: this.now + (wait > 0 ? wait : 0), callback, args });
      return id;
    };
    g.clearTimeout = (id: number) => {
      this.timeouts = this.timeouts.filter((t) => t.id !== id);
    };
    g.setInterval = (callback: Callback, _period?: number, ...args: unknown[]) => {
      const id = this.nextId++;
      this.intervals.push({ id, callback, args });
      return id;
    };
    g.clearInterval = (id: number) => {
      this.intervals = this.intervals.filter((t) => t.id !== id);
    };
  }

  uninstall(): void {
    if (this.saved === null) return;
    Object.assign(g, this.saved);
    this.saved = null;
  }

  runNextTimeout(): boolean {
    if (this.timeouts.length === 0) return false;
    let next = 0;
    for (let i = 1; i < this.timeouts.length; i++) {
      if (this.timeouts[i].due < this.timeouts[next].due) next = i;
    }
    const [timeout] = this.timeouts.splice(next, 1);
    this.now = timeout.due;
    timeout.callback(...timeout.args);
    return true;
  }

  /** Fires every registered interval once, in registration order. */
  tickIntervals(): void {
    const snapshot = this.intervals.slice();
    for (const interval of snapshot) {
      if (this.intervals.includes(interval)) interval.callback(...interval.args);
    }
  }
}
