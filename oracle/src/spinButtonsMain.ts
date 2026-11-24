import { app, BrowserWindow } from "electron";
import fs from "node:fs";

// Electron main process for `spinButtons` (not run under ELECTRON_RUN_AS_NODE). Each step loads its
// markup into an offscreen page that emulates focus and applies its ops to the <input>:
//   ["move", place], where place is "up" or "down" (the middle of that button), "text", "beside-up"
//   or "beside-down" (left of the buttons), "above" or "outside",
//   ["put", place], which puts the pointer there without a move,
//   ["press"], ["release"], ["hold", ms], ["key", "ArrowUp" | "ArrowDown"], ["type", text],
//   ["blur"].
// After the markup and every op it writes {"step", "op", "value", "focused", "selected", "events"}.
// The events of a hold are {"initial", "period", "values"} instead: the delay from the press, to
// the next 50 ms, and between repeats, to the next 10 ms, and what the first repeats left. From then
// on a step has no values.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
const MODES = ["ArrowUp", "ArrowDown"];
const REPEATS = 4;
const SETTLE_MS = 60;
const TIMEOUT_MS = 120_000;

function fail(message: string): never {
  process.stderr.write(`spinButtonsMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

const PAGE_SCRIPT = `(() => {
  const input = document.querySelector("input");
  window.__events = [];
  for (const type of ["input", "change"]) input.addEventListener(type, () => window.__events.push([type, performance.now(), input.value]));
  window.__pressed = 0;
  addEventListener("mousedown", () => (window.__pressed = performance.now()), true);
  window.__read = () => {
    const events = window.__events;
    window.__events = [];
    let selected;
    const s = getSelection();
    selected = s.toString();
    return { value: input.value, focused: document.activeElement === input, selected, events, pressed: window.__pressed };
  };
  window.__box = () => {
    const r = input.getBoundingClientRect();
    const css = getComputedStyle(input);
    const edge = (side) => parseFloat(css["border" + side + "Width"]) + parseFloat(css["padding" + side]);
    return { x: r.x, y: r.y, right: r.right - edge("Right"), top: r.y + edge("Top"), bottom: r.bottom - edge("Bottom") };
  };
})()`;

app.setPath("userData", request.userData);
app.dock?.hide();
setTimeout(() => fail(`still running after ${TIMEOUT_MS / 1000} s`), TIMEOUT_MS);
app
  .whenReady()
  .then(async () => {
    const win = new BrowserWindow({
      width: 400,
      height: 200,
      show: false,
      webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false }
    });
    win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
    await win.loadURL("about:blank");
    win.webContents.debugger.attach("1.3");
    const send = (method: string, params: object) => win.webContents.debugger.sendCommand(method, params);
    const evaluate = async (expression: string): Promise<any> => {
      const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails !== undefined) fail(`${expression}: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
      return result.result.value;
    };
    const settle = () => evaluate(`new Promise((r) => setTimeout(r, ${SETTLE_MS}))`);
    const lines: string[] = [];
    for (const [index, step] of (request.fixture.steps as any[]).entries()) {
      const html = `<!doctype html><html><head><meta charset="utf-8"></head><body style="margin:20px">${step.markup}</body></html>`;
      await win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(html));
      await send("Emulation.setFocusEmulationEnabled", { enabled: true });
      await evaluate(PAGE_SCRIPT);
      await settle();
      const box = await evaluate("window.__box()");
      const half = Math.floor((box.bottom - box.top) / 2);
      const spin = box.right - 7;
      const places: { [name: string]: [number, number] } = {
        up: [spin, box.top + half / 2],
        down: [spin, box.top + half + half / 2],
        text: [box.x + 10, (box.top + box.bottom) / 2],
        "beside-up": [box.right - 40, box.top + half / 2],
        "beside-down": [box.right - 40, box.top + half + half / 2],
        above: [spin, box.y - 10],
        outside: [5, 150]
      };
      let at = places.outside;
      let pressed = false;
      let timed = false;
      const mouse = (type: string) =>
        send("Input.dispatchMouseEvent", { type, x: at[0], y: at[1], button: type === "mouseMoved" && !pressed ? "none" : "left", buttons: pressed ? 1 : 0, clickCount: type === "mouseMoved" ? 0 : 1 });
      const write = async (op: any) => {
        const { events, pressed: since, ...state } = await evaluate("window.__read()");
        const held = op !== null && op[0] === "hold";
        const inputs = events.filter((e: any) => e[0] === "input");
        let recorded: unknown = events.map((e: any) => (timed ? [e[0]] : [e[0], e[2]]));
        if (held && inputs.length > REPEATS) {
          const gaps = inputs.slice(1).map((e: any, i: number) => e[1] - inputs[i][1]).sort((a: number, b: number) => a - b);
          recorded = {
            initial: Math.round((inputs[0][1] - since) / 50) * 50,
            period: Math.round(gaps[Math.floor(gaps.length / 2)] / 10) * 10,
            values: inputs.slice(0, REPEATS).map((e: any) => e[2])
          };
        }
        if (held) timed = true;
        // How often it repeated by then differs between runs.
        if (timed) delete state.value;
        lines.push(JSON.stringify({ step: index, op, ...state, events: recorded }));
      };
      await write(null);
      for (const op of step.ops as any[]) {
        const [kind, argument] = op;
        if (kind === "put") at = places[argument] ?? fail(`unknown place ${argument}`);
        else if (kind === "move") {
          at = places[argument] ?? fail(`unknown place ${argument}`);
          await mouse("mouseMoved");
        } else if (kind === "press" || kind === "release") {
          pressed = kind === "press";
          await mouse(pressed ? "mousePressed" : "mouseReleased");
        } else if (kind === "hold") await new Promise((r) => setTimeout(r, argument));
        else if (kind === "type") await send("Input.insertText", { text: argument });
        else if (kind === "blur") await evaluate("document.querySelector('input').blur()");
        else if (kind === "key" && MODES.includes(argument)) {
          const keyCode = argument === "ArrowUp" ? 38 : 40;
          const key = { key: argument, code: argument, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode };
          await send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...key });
          await send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
        } else fail(`unknown op ${JSON.stringify(op)}`);
        if (kind !== "press" && kind !== "hold") await settle();
        else await evaluate("new Promise((r) => setTimeout(r, 0))");
        await write(op);
      }
    }
    process.stdout.write(lines.map((l) => l + "\n").join(""), () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
