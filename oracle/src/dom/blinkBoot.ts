// Runs in the Blink page before domPage.js (see blinkMain.ts) and provides what dom.ts provides
// in jsdom: the output sink, the yield between steps, the exception records and a readiness
// check (window size and scale, zone, locale, no web fonts to load asynchronously). It captures the real requestAnimationFrame, MessageChannel and window geometry before
// env.ts replaces them.

const w = window as any;
const realFrame = window.requestAnimationFrame.bind(window);
const channel = new MessageChannel();
const pending: (() => void)[] = [];
channel.port1.onmessage = () => pending.shift()?.();
const log = console.log.bind(console);

const descriptor = (name: string) => {
  for (let o: object | null = window; o !== null; o = Object.getPrototypeOf(o)) {
    const d = Object.getOwnPropertyDescriptor(o, name);
    if (d !== undefined) return d;
  }
  throw new Error(`window.${name} has no descriptor`);
};
const real = Object.fromEntries(
  ["innerWidth", "innerHeight", "devicePixelRatio"].map((name) => [name, descriptor(name).get!.bind(window)])
);

// Blink runs the rendering steps (scroll events, layout, observers) only inside a real frame,
// so every yield waits for one real frame and then for one task: whatever upstream's last
// callback scheduled has run before the next step, at the same point on every run.
w.__oracleYield = async () => {
  await new Promise<void>((resolve) => realFrame(() => resolve()));
  await new Promise<void>((resolve) => {
    pending.push(resolve);
    channel.port2.postMessage(null);
  });
};

const lines: string[] = [];
w.__oracleWrite = (line: string) => lines.push(line);
w.__oracleOutput = () => lines.map((line) => line + "\n").join("");
w.__oracleLog = (text: string) => log(text);
w.__oracleOuter = { TextEncoder, TextDecoder };

const errorText = (error: unknown, message: string) =>
  error instanceof Error ? `${error.name}: ${error.message}` : String(error ?? message);
window.addEventListener("error", (event) => {
  lines.push(JSON.stringify({ exception: "event", error: errorText(event.error, event.message) }));
});
window.addEventListener("unhandledrejection", (event) => {
  event.preventDefault();
  lines.push(JSON.stringify({ exception: "unhandledRejection", error: errorText(event.reason, "") }));
});

// CSS transitions and animations run on the real clock; with them off, every geometry read
// sees the end state.
const style = document.createElement("style");
style.textContent = "*, *::before, *::after { transition: none !important; animation: none !important; }";
document.head.appendChild(style);

w.__oracleReady = (): string[] => {
  const fixture = w.__oracleFixture;
  const view = fixture.window ?? {};
  const problems: string[] = [];
  const expect = (what: string, got: unknown, want: unknown) => {
    if (got !== want) problems.push(`${what} is ${JSON.stringify(got)}, expected ${JSON.stringify(want)}`);
  };
  expect("innerWidth", real.innerWidth(), view.width ?? 1100);
  expect("innerHeight", real.innerHeight(), view.height ?? 650);
  expect("devicePixelRatio", real.devicePixelRatio(), view.devicePixelRatio ?? 1);
  expect("web fonts", document.fonts.size, 0);
  const zone = w.__oracleZone;
  const resolved = Intl.DateTimeFormat().resolvedOptions();
  expect("time zone", resolved.timeZone, zone === "UTC" ? "UTC" : zone);
  expect("Intl.DateTimeFormat locale", resolved.locale, "en-US");
  expect("Intl.Collator locale", new Intl.Collator().resolvedOptions().locale, "en-US");
  expect("Intl.NumberFormat locale", new Intl.NumberFormat().resolvedOptions().locale, "en-US");
  expect("navigator.language", navigator.language, "en-US");
  return problems;
};
