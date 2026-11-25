import { app, BrowserWindow, nativeTheme } from "electron";
import fs from "node:fs";

// Electron main process for `fieldText` (not run under ELECTRON_RUN_AS_NODE). For each of the
// fixture's controls, themes and cases it loads the control's markup into a page that is never shown, gives
// its input (#i) the case's value, focuses it and applies the case's selection: "all" (`select()`),
// "typed" (the value typed, which leaves the
// caret at its end in view) or [start, end]. It then records the pixels of the
// input's box in a page that is active, through Emulation.setFocusEmulationEnabled, and in one that
// is not, as rows of runs of colours. The caret is transparent, since it blinks.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
const TIMEOUT_MS = 600_000;

function fail(message: string): never {
  process.stderr.write(`fieldTextMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

app.setPath("userData", request.userData);
app.dock?.hide();
setTimeout(() => fail(`still running after ${TIMEOUT_MS / 1000} s`), TIMEOUT_MS);
app
  .whenReady()
  .then(async () => {
    const { fixture, scale } = request;
    const win = new BrowserWindow({
      width: 500,
      height: 100,
      show: false,
      frame: false,
      focusable: false,
      paintWhenInitiallyHidden: true,
      webPreferences: { backgroundThrottling: false, spellcheck: false }
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
    const painted = () => evaluate("new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(resolve, 50))))");
    const shot = async () => {
      const image = await win.webContents.capturePage();
      return { width: image.getSize(scale).width, bytes: image.toBitmap({ scaleFactor: scale }) };
    };
    const pixels = async ([x, y, w, h]: number[]) => {
      let { width, bytes } = await shot();
      for (let same = 0; same < 2; ) {
        const next = await shot();
        same = next.bytes.equals(bytes) ? same + 1 : 0;
        ({ width, bytes } = next);
      }
      const rows: [number, string][][] = [];
      for (let row = y; row < y + h; row++) {
        const runs: [number, string][] = [];
        for (let column = x; column < x + w; column++) {
          const at = (row * width + column) * 4;
          const color = [2, 1, 0].map((c) => bytes[at + c].toString(16).padStart(2, "0")).join("");
          if (runs.length > 0 && runs[runs.length - 1][1] === color) runs[runs.length - 1][0]++;
          else runs.push([1, color]);
        }
        rows.push(runs);
      }
      return rows;
    };
    const lines: string[] = [];
    for (const control of fixture.controls as any[]) {
      for (const theme of control.themes ?? ["light"]) {
        nativeTheme.themeSource = theme;
        const background = theme === "dark" ? "#202020" : "#ffffff";
        const css = control.css.map((sheet: string) => `<style>${sheet}</style>`).join("");
        for (const [value, selection] of control.cases as [string, any][]) {
          fs.writeFileSync(
            request.page,
            `<!doctype html><html><head><meta charset="utf-8"><base href="${request.base}">${css}<style>#i { caret-color: transparent }</style></head><body style="margin:0;background:${background}">${control.markup}</body></html>`
          );
          await win.loadFile(request.page);
          await send("Emulation.setFocusEmulationEnabled", { enabled: true });
          const text = JSON.stringify(value);
          const select = selection === "all" ? "i.select()" : Array.isArray(selection) ? `i.setSelectionRange(${selection.join(", ")})` : "";
          const fill = selection === "typed" ? "i.focus()" : `i.value = ${text}; i.focus(); ${select}`;
          await evaluate(`(() => { const i = document.getElementById("i"); ${fill} })()`);
          if (selection === "typed") {
            for (const char of value) win.webContents.sendInputEvent({ type: "char", keyCode: char });
            for (let i = 0; i < 1000 && (await evaluate("document.getElementById('i').value")) !== value; i++);
          }
          if ((await evaluate("document.getElementById('i').value")) !== value) fail(`${control.name}: ${value} is not a value`);
          const rect = await evaluate(`(() => { const r = document.getElementById("i").getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; })()`);
          const [left, top] = rect.slice(0, 2).map((v: number) => Math.round(v * scale));
          const box = [left, top, Math.round((rect[0] + rect[2]) * scale) - left, Math.round((rect[1] + rect[3]) * scale) - top];
          for (const active of [true, false]) {
            await send("Emulation.setFocusEmulationEnabled", { enabled: active });
            await painted();
            if ((await evaluate("document.hasFocus()")) !== active) fail(`${control.name}: document.hasFocus() is not ${active}`);
            lines.push(JSON.stringify({ control: control.name, type: control.type, theme, scale, value, selection, active, rect, rows: await pixels(box) }));
          }
        }
      }
    }
    process.stdout.write(lines.map((l) => l + "\n").join(""), () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
