import { LiveLoop } from "./loop";
import { V8MathRandom } from "./random";

/**
 * [`LiveLoop`]'s server, deliveries and timers inside a page that has its own clock
 * (`src/dom/env.ts`), for hub sessions. The page's `setTimeout`, `setInterval` and
 * `requestIdleCallback` calls after `install` run under Blink's rules here, as do the emulated main
 * process's. The page's clock drives it: one page timer is armed for whatever is due next, and each
 * firing runs one task, so every task has the page's own microtask drain after it.
 */
export class PageLoop extends LiveLoop {
  private armed: { due: number; id: number } | null = null;
  private setPageTimeout: (run: () => void, ms: number) => number;
  private clearPageTimeout: (id: number) => void;

  constructor(
    private page: any,
    private swallowed: (error: unknown) => void
  ) {
    super(page.Date.now(), new V8MathRandom(1n));
    this.setPageTimeout = page.setTimeout.bind(page);
    this.clearPageTimeout = page.clearTimeout.bind(page);
    Object.defineProperty(this, "now", { get: () => page.Date.now() - this.epochMs });
  }

  override install(): void {
    const timers: { [name: string]: unknown } = {
      setTimeout: (callback: () => void, delay?: number, ...args: unknown[]) => this.add(callback, delay, false, args),
      setInterval: (callback: () => void, delay?: number, ...args: unknown[]) => this.add(callback, delay, true, args),
      clearTimeout: (id: number) => this.clear(id),
      clearInterval: (id: number) => this.clear(id)
    };
    Object.assign(globalThis, timers);
    Object.assign(this.page, timers, {
      requestIdleCallback: (callback: (deadline: object) => void, options?: { timeout?: number }) =>
        this.postDelayedTask(() => callback({ didTimeout: true, timeRemaining: () => 0 }), options?.timeout ?? 50)
    });
  }

  /** The end of a page task: what it left due next is armed, and the next task starts a new nesting chain. */
  taskEnded(): void {
    this.runningLevel = 0;
    const due = this.nextDue();
    if (due === (this.armed?.due ?? null)) return;
    if (this.armed !== null) this.clearPageTimeout(this.armed.id);
    this.armed = due === null ? null : { due, id: this.setPageTimeout(() => this.pump(), due - this.now) };
  }

  private pump(): void {
    this.armed = null;
    if (this.server !== null && (this.server.nextTime() ?? Infinity) <= this.now) this.server.process(this.server.nextTime()!);
    const run = this.takeDue();
    if (run === null) return;
    try {
      run();
    } catch (error) {
      this.swallowed(error);
    }
  }
}
