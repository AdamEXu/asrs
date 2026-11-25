import { app, BrowserWindow, nativeImage, nativeTheme } from "electron";
import fs from "node:fs";
import path from "node:path";

// Electron main process for `scrollers` (not run under ELECTRON_RUN_AS_NODE). Each step loads its
// markup into an offscreen page that emulates focus and applies its ops:
//   ["key", name, modifiers?, commands?], where modifiers are "alt", "ctrl", "meta" and "shift" and
//   commands are the editing commands AppKit's key bindings send along,
//   ["wheel", x, y, dx, dy], ["move", x, y], ["press", x, y], ["release", x, y], ["click", x, y],
//   ["eval", script], ["wait", ms], ["shot", name] (only with a shot dir),
//   ["pixels", name, [x, y, width, height]], which records the rows of that box as runs of colours,
//   ["fade", [x, y, width]], which records when that row of pixels first changes and turns white,
//   to the nearest 100 and 250 ms.
// An op that ends with "quick" has no record and does not settle.
// The positions an op ends at are read once they stay the same for SETTLE_MS.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
const SETTLE_MS = 250;
const FADE_MS = 1500;
const TIMEOUT_MS = 600_000;
const KEYS: { [name: string]: [string, number, string?] } = {
  ArrowLeft: ["ArrowLeft", 37],
  ArrowUp: ["ArrowUp", 38],
  ArrowRight: ["ArrowRight", 39],
  ArrowDown: ["ArrowDown", 40],
  PageUp: ["PageUp", 33],
  PageDown: ["PageDown", 34],
  End: ["End", 35],
  Home: ["Home", 36],
  Space: [" ", 32, " "],
  Tab: ["Tab", 9]
};
const MODIFIERS: { [name: string]: number } = { alt: 1, ctrl: 2, meta: 4, shift: 8 };

function fail(message: string): never {
  process.stderr.write(`scrollersMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

app.setPath("userData", request.userData);
app.dock?.hide();
setTimeout(() => fail(`still running after ${TIMEOUT_MS / 1000} s`), TIMEOUT_MS);
app
  .whenReady()
  .then(async () => {
    const { fixture, style, scale } = request;
    const win = new BrowserWindow({
      width: fixture.window?.[0] ?? 500,
      height: fixture.window?.[1] ?? 400,
      show: false,
      frame: false,
      webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false }
    });
    win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
    await win.loadURL("about:blank");
    win.webContents.debugger.attach("1.3");
    const send = (method: string, params: object = {}) => win.webContents.debugger.sendCommand(method, params);
    const evaluate = async (expression: string): Promise<any> => {
      const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails !== undefined) fail(`${expression}: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
      return result.result.value;
    };
    const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    const lines: string[] = [];
    for (const theme of fixture.themes ?? ["light"]) {
      nativeTheme.themeSource = theme;
      for (const [index, step] of (fixture.steps as any[]).entries()) {
        const css = fixture.css.map((sheet: string) => `<style>${sheet}</style>`).join("");
        fs.writeFileSync(request.page, `<!doctype html><html><head><meta charset="utf-8">${css}</head><body style="margin:0">${step.markup}</body></html>`);
        await win.loadFile(request.page);
        await send("Emulation.setFocusEmulationEnabled", { enabled: true });
        await evaluate(`window.__read = () => ({
          active: document.activeElement === document.body ? "body" : document.activeElement?.id ?? null,
          scroll: [document.scrollingElement, ...document.querySelectorAll(".s")].map((e) => [e.scrollLeft, e.scrollTop])
        })`);
        await sleep(SETTLE_MS);
        const write = async (op: any) => {
          let last = JSON.stringify(await evaluate("window.__read()"));
          for (let same = Date.now(); Date.now() - same < SETTLE_MS; ) {
            await sleep(20);
            const next = JSON.stringify(await evaluate("window.__read()"));
            if (next !== last) same = Date.now();
            last = next;
          }
          lines.push(JSON.stringify({ style, scale, theme, step: step.name ?? index, op, ...JSON.parse(last) }));
        };
        const boxes = await evaluate(
          `[document.scrollingElement, ...document.querySelectorAll(".s")].map((e) => [e.clientWidth, e.clientHeight, e.scrollWidth, e.scrollHeight, e.offsetWidth, e.offsetHeight])`
        );
        lines.push(JSON.stringify({ style, scale, theme, step: step.name ?? index, devicePixelRatio: await evaluate("devicePixelRatio"), boxes }));
        let pressed = false;
        const mouse = (type: string, x: number, y: number) =>
          send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" && !pressed ? "none" : "left", buttons: pressed ? 1 : 0, clickCount: type === "mouseMoved" ? 0 : 1 });
        for (const op of step.ops as any[]) {
          const [kind] = op;
          if (kind === "key") {
            const [key, code, text] = KEYS[op[1]] ?? fail(`unknown key ${op[1]}`);
            const modifiers = (op[2] ?? []).reduce((sum: number, m: string) => sum | MODIFIERS[m], 0);
            const event = { key, code: op[1], windowsVirtualKeyCode: code, nativeVirtualKeyCode: code, modifiers };
            await send("Input.dispatchKeyEvent", { type: text === undefined ? "rawKeyDown" : "keyDown", text, commands: process.platform === "darwin" ? op[3] ?? [] : [], ...event });
            await send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
          } else if (kind === "wheel") await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: op[1], y: op[2], deltaX: op[3], deltaY: op[4] });
          else if (kind === "move") await mouse("mouseMoved", op[1], op[2]);
          else if (kind === "press" || kind === "release" || kind === "click") {
            if (kind !== "release") {
              pressed = true;
              await mouse("mousePressed", op[1], op[2]);
            }
            if (kind !== "press") {
              pressed = false;
              await mouse("mouseReleased", op[1], op[2]);
            }
          } else if (kind === "eval") await evaluate(op[1]);
          else if (kind === "wait") {
            await sleep(op[1]);
            continue;
          } else if (kind === "pixels") {
            const [x, y, w, h] = op[2].map((v: number) => v * scale);
            const { data } = await send("Page.captureScreenshot", { format: "png" });
            const image = nativeImage.createFromBuffer(Buffer.from(data, "base64"));
            const { width } = image.getSize();
            const bytes = image.toBitmap();
            const rows: [number, string][] = [];
            for (let row = y; row < y + h; row++) {
              const runs: [number, string][] = [];
              for (let column = x; column < x + w; column++) {
                const at = (row * width + column) * 4;
                const color = [2, 1, 0].map((c) => bytes[at + c].toString(16).padStart(2, "0")).join("");
                if (runs.length > 0 && runs[runs.length - 1][1] === color) runs[runs.length - 1][0]++;
                else runs.push([1, color]);
              }
              const text = runs.map(([n, color]) => `${n}:${color}`).join(" ");
              if (rows.length > 0 && rows[rows.length - 1][1] === text) rows[rows.length - 1][0]++;
              else rows.push([1, text]);
            }
            lines.push(JSON.stringify({ style, scale, theme, step: step.name ?? index, pixels: op[1], rows }));
            continue;
          } else if (kind === "fade") {
            const [x, y, w] = op[1].map((v: number) => v * scale);
            const start = Date.now();
            let first: string | null = null;
            let begins: number | null = null;
            let ends: number | null = null;
            while (Date.now() - start < FADE_MS) {
              const at = Date.now() - start;
              const { data } = await send("Page.captureScreenshot", { format: "png" });
              const image = nativeImage.createFromBuffer(Buffer.from(data, "base64"));
              const row = y * image.getSize().width + x;
              const strip = image.toBitmap().subarray(row * 4, (row + w) * 4);
              const text = strip.toString("hex");
              if (first === null) first = text;
              else if (begins === null && text !== first) begins = at;
              if (ends === null && strip.every((v) => v === 255)) ends = at;
            }
            const round = (ms: number | null, to: number) => (ms === null ? null : Math.round(ms / to) * to);
            lines.push(JSON.stringify({ style, scale, theme, step: step.name ?? index, fade: [round(begins, 100), round(ends, 250)] }));
            continue;
          } else if (kind === "shot") {
            if (request.shots !== null) {
              const { data } = await send("Page.captureScreenshot", { format: "png" });
              fs.writeFileSync(path.join(request.shots, `${step.name ?? index}-${op[1]}-${style}-${theme}-${scale}x.png`), Buffer.from(data, "base64"));
            }
            continue;
          } else fail(`unknown op ${JSON.stringify(op)}`);
          if (op[op.length - 1] !== "quick") await write(op);
        }
      }
    }
    process.stdout.write(lines.map((l) => l + "\n").join(""), () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
