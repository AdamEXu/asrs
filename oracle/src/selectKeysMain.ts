import { app, BrowserWindow } from "electron";
import fs from "node:fs";

// Electron main process for `selectKeys` (not run under ELECTRON_RUN_AS_NODE). Each step loads
// its markup into an offscreen page, focuses the first <select> or <input>, and applies the ops: keys and
// mouse presses go through the DevTools protocol's Input domain, which makes them trusted input
// with the timestamps the fixture gives. After the markup and after every op it writes
// {"step", "op", "value", "selectedIndex", "focused", "events"}, where events lists the DOM
// events the element received since the previous record. Ops that would open the native popup
// (ArrowUp, ArrowDown, F4, Space outside a type-ahead session, a press on an enabled select)
// block in AppKit's menu loop, so fixtures avoid them.

const requestPath = process.argv[process.argv.length - 1];
const request = JSON.parse(fs.readFileSync(requestPath, "utf8"));
const fixture = request.fixture;

const KEYS: { [name: string]: { code: string; keyCode: number; text?: string } } = {
  Enter: { code: "Enter", keyCode: 13, text: "\r" },
  Escape: { code: "Escape", keyCode: 27 },
  Home: { code: "Home", keyCode: 36 },
  End: { code: "End", keyCode: 35 },
  PageUp: { code: "PageUp", keyCode: 33 },
  PageDown: { code: "PageDown", keyCode: 34 },
  ArrowUp: { code: "ArrowUp", keyCode: 38 },
  ArrowDown: { code: "ArrowDown", keyCode: 40 },
  ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
  ArrowRight: { code: "ArrowRight", keyCode: 39 },
  Backspace: { code: "Backspace", keyCode: 8 }
};
const MODIFIERS: { [name: string]: number } = { alt: 1, ctrl: 2, meta: 4, shift: 8 };
const EPOCH_SECONDS = 1_000_000;

function fail(message: string): never {
  process.stderr.write(`selectKeysMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

const PAGE_SCRIPT = `(() => {
  const select = document.querySelector("select, input");
  window.__events = [];
  for (const type of ["keydown", "keypress", "keyup", "input", "change", "focus", "blur", "mousedown", "mouseup", "click"]) {
    select.addEventListener(type, () => window.__events.push(type));
  }
  window.__read = () => {
    const events = window.__events;
    window.__events = [];
    return { value: select.value, selectedIndex: select.selectedIndex, focused: document.activeElement === select, events };
  };
  window.__select = select;
  return true;
})()`;

app.setPath("userData", request.userData);
app.dock?.hide();
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
    win.webContents.debugger.attach("1.3");
    const send = (method: string, params: object) => win.webContents.debugger.sendCommand(method, params);
    const evaluate = async (expression: string): Promise<any> => {
      const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails !== undefined) {
        fail(`${expression}: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
      }
      return result.result.value;
    };
    const settle = () => evaluate("new Promise((resolve) => setTimeout(resolve, 0))");
    const lines: string[] = [];
    for (const [index, step] of (fixture.steps as any[]).entries()) {
      const html = `<!doctype html><html><head><meta charset="utf-8"></head><body style="margin:0">${step.markup}</body></html>`;
      await win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(html));
      await evaluate(PAGE_SCRIPT);
      if (step.focus !== false) await evaluate("window.__select.focus()");
      await settle();
      const write = async (op: unknown) => lines.push(JSON.stringify({ step: index, op, ...(await evaluate("window.__read()")) }));
      await write(null);
      for (const op of step.ops as any[]) {
        const [kind, name, time, modifierNames] = op;
        const modifiers = ((modifierNames ?? []) as string[]).reduce((m, n) => m | (MODIFIERS[n] ?? fail(`unknown modifier ${n}`)), 0);
        const timestamp = EPOCH_SECONDS + (time ?? 0) / 1000;
        if (kind === "char") {
          await send("Input.dispatchKeyEvent", { type: "keyDown", key: name, text: name, unmodifiedText: name, modifiers, timestamp });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: name, modifiers, timestamp });
        } else if (kind === "key") {
          const key = KEYS[name] ?? fail(`unknown key ${name}`);
          const down = { key: name, code: key.code, windowsVirtualKeyCode: key.keyCode, nativeVirtualKeyCode: key.keyCode, modifiers, timestamp };
          await send("Input.dispatchKeyEvent", key.text === undefined ? { type: "rawKeyDown", ...down } : { type: "keyDown", text: key.text, ...down });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code: key.code, windowsVirtualKeyCode: key.keyCode, modifiers, timestamp });
        } else if (kind === "press") {
          const box = await evaluate("(() => { const r = window.__select.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()");
          const mouse = { x: box[0], y: box[1], button: "left", clickCount: 1, timestamp };
          await send("Input.dispatchMouseEvent", { type: "mousePressed", ...mouse });
          await send("Input.dispatchMouseEvent", { type: "mouseReleased", ...mouse });
        } else if (kind === "set") {
          await evaluate(`window.__select[${JSON.stringify(name)}] = ${JSON.stringify(time)}`);
        } else if (kind === "blur" || kind === "focus") {
          await evaluate(`window.__select.${kind}()`);
        } else fail(`unknown op ${JSON.stringify(op)}`);
        await settle();
        await write(op);
      }
    }
    process.stdout.write(lines.map((l) => l + "\n").join(""), () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
