// Fake `Date`, `setInterval` and `setTimeout` for the `files` harness: upstream code registers
// its save cadence and reads the clock through these globals. Intervals are captured in
// registration order so a scenario tick can run the real callback.

const RealDate = Date;

let nowMs = 0;
const intervals: { callback: (...args: any[]) => any; ms: number }[] = [];

export function setNow(ms: number): void {
  nowMs = ms;
}

export function install(): void {
  const g = globalThis as any;
  g.setInterval = (callback: (...args: any[]) => any, ms: number): number => {
    intervals.push({ callback, ms });
    return intervals.length;
  };
  g.clearInterval = () => {};
  g.setTimeout = () => 0;
  g.clearTimeout = () => {};
  g.Date = class extends RealDate {
    constructor(...args: any[]) {
      // @ts-expect-error spread construct
      super(...(args.length === 0 ? [nowMs] : args));
    }
    static now(): number {
      return nowMs;
    }
    static parse = RealDate.parse;
    static UTC = RealDate.UTC;
  };
}

export function intervalCount(): number {
  return intervals.length;
}

export function tickInterval(index: number): void {
  intervals[index]?.callback();
}
