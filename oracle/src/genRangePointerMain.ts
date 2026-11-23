import { app, BrowserWindow } from "electron";
import fs from "node:fs";

// Electron main process for `genRangePointer` (not run under ELECTRON_RUN_AS_NODE). Each step
// puts upstream's timeline container in an offscreen page with the slider `width` px wide at
// `left`, sets the attributes it lists, and applies its ops: ["press" | "move" | "release", x]
// in px from the slider's left edge, ["key", name], ["set", property, value], ["focus"] and
// ["blur"]. After the markup and after every op it writes
// {"step", "op", "value", "focused", "events"}.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));

const KEYS: { [name: string]: number } = { End: 35, Home: 36, PageUp: 33, PageDown: 34, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40 };
const TOP = 40;

function fail(message: string): never {
  process.stderr.write(`genRangePointerMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

const PAGE_SCRIPT = `(() => {
  const input = document.querySelector("input");
  window.__input = input;
  window.__events = [];
  for (const type of ["keydown", "input", "change", "focus", "blur"]) input.addEventListener(type, () => window.__events.push(type));
  window.__read = () => {
    const events = window.__events;
    window.__events = [];
    return { value: input.value, focused: document.activeElement === input, events };
  };
  return true;
})()`;

app.setPath("userData", request.userData);
app.dock?.hide();
app
  .whenReady()
  .then(async () => {
    const win = new BrowserWindow({
      width: 1200,
      height: 200,
      show: false,
      webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false }
    });
    win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
    win.webContents.debugger.attach("1.3");
    const send = (method: string, params: object) => win.webContents.debugger.sendCommand(method, params);
    const evaluate = async (expression: string): Promise<any> => {
      const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails !== undefined) {
        fail(`${expression}: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
      }
      return result.result.value;
    };
    const settle = () => evaluate("new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)))");
    const lines: string[] = [];
    for (const [index, step] of (request.fixture.steps as any[]).entries()) {
      const container = `left:${step.left}px;right:auto;top:${TOP}px;height:33px;width:${step.width}px`;
      fs.writeFileSync(
        request.page,
        `<!doctype html><html><head><meta charset="utf-8"><style>${request.css}</style></head><body style="margin:0">` +
          `<div class="video-timeline-container" style="${container}"><input class="video-timeline-slider" id="timeline" type="range" value="0" min="1" max="1" step="1"></div></body></html>`
      );
      await win.loadFile(request.page);
      const ratio = await evaluate("devicePixelRatio");
      if (ratio !== (request.fixture.scale ?? 1)) fail(`devicePixelRatio is ${ratio}`);
      await evaluate(PAGE_SCRIPT);
      for (const [property, value] of step.set) await evaluate(`window.__input[${JSON.stringify(property)}] = ${JSON.stringify(value)}`);
      await settle();
      await evaluate("window.__read()");
      const write = async (op: unknown) => lines.push(JSON.stringify({ step: index, op, ...(await evaluate("window.__read()")) }));
      await write(null);
      let held = false;
      for (const op of step.ops as any[]) {
        const [kind, arg, value] = op;
        if (kind === "press" || kind === "move" || kind === "release") {
          const type = { press: "mousePressed", move: "mouseMoved", release: "mouseReleased" }[kind as "press"];
          if (kind === "press") held = true;
          const button = kind === "move" && !held ? "none" : "left";
          await send("Input.dispatchMouseEvent", { type, x: step.left + arg, y: TOP + 12, button, buttons: held && kind !== "release" ? 1 : 0, clickCount: kind === "move" ? 0 : 1 });
          if (kind === "release") held = false;
        } else if (kind === "key") {
          const keyCode = KEYS[arg] ?? fail(`unknown key ${arg}`);
          await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: arg, code: arg, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: arg, code: arg, windowsVirtualKeyCode: keyCode });
        } else if (kind === "set") {
          await evaluate(`window.__input[${JSON.stringify(arg)}] = ${JSON.stringify(value)}`);
        } else if (kind === "focus" || kind === "blur") {
          await evaluate(`window.__input.${kind}()`);
        } else fail(`unknown op ${JSON.stringify(op)}`);
        await settle();
        await write(op);
      }
    }
    process.stdout.write(lines.map((l) => l + "\n").join(""), () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
