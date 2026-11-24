import { app, BrowserWindow, Menu } from "electron";
import fs from "node:fs";

// Electron main process for `focusRings` (not run under ELECTRON_RUN_AS_NODE). Loads the fixture's
// markup with upstream's global.css in an offscreen window for each of the fixture's sequences and
// applies its ops, then as many seeded random ones as `random` asks for:
//   ["active", bool]        Emulation.setFocusEmulationEnabled, off at the start of a sequence
//   ["key", name, mods]     a key pressed and released; ["down", ...] and ["up", ...] are its halves
//   ["press", id, button]   a mouse press and release on an element; ["move", id] moves there
//   ["focus", id], ["blur"] script
// Input is webContents.sendInputEvent's, so it is trusted. After each op it writes {"sequence",
// "op", "active", "focus", "visible", "ring"}: the id of document.activeElement, whether it matches
// :focus and :focus-visible, and whether its computed style has the rule's outline.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
const fixture = request.fixture;

// The DevTools protocol's input would focus the page.
const KEY_CODES: { [name: string]: string } = { " ": "Space", ArrowLeft: "Left", ArrowRight: "Right" };
const CHARS: { [name: string]: string } = { Enter: "\r", " ": " " };
const MODIFIERS: { [name: string]: string } = { alt: "alt", ctrl: "control", meta: "meta", shift: "shift" };
const MODIFIER_KEYS: { [name: string]: string } = { Alt: "alt", Control: "control", Meta: "meta", Shift: "shift" };

function fail(message: string): never {
  process.stderr.write(`focusRingsMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

function mulberry32(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const COUNT = `(() => {
  window.__seen = {};
  for (const type of ["keydown", "keyup", "mousedown", "mouseup", "mousemove"]) {
    window.__seen[type] = 0;
    window.addEventListener(type, () => window.__seen[type]++, true);
  }
})()`;

const READ = `(() => {
  const active = document.activeElement;
  const matches = (pseudo) => active !== null && active.matches(pseudo);
  return {
    active: active === null || active === document.body ? null : active.id,
    focus: matches(":focus"),
    visible: matches(":focus-visible"),
    ring: active !== null && getComputedStyle(active).outline === "rgb(68, 108, 227) solid 2px"
  };
})()`;

app.setPath("userData", request.userData);
app.dock?.hide();
Menu.setApplicationMenu(null);
app
  .whenReady()
  .then(async () => {
    const win = new BrowserWindow({
      width: 500,
      height: 400,
      show: false,
      focusable: false,
      webPreferences: { backgroundThrottling: false, spellcheck: false }
    });
    win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
    // Emulation commands wait for a page.
    await win.loadURL("about:blank");
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
    const centre = (id: string): Promise<[number, number]> =>
      evaluate(`(() => { const r = document.getElementById(${JSON.stringify(id)}).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()`);
    // Runs `send` and waits for the page to have had the event.
    const until = async (type: string, send: () => void) => {
      const seen = await evaluate(`window.__seen.${type}`);
      send();
      for (let i = 0; i < 1000; i++) {
        if ((await evaluate(`window.__seen.${type}`)) > seen) return;
      }
      fail(`no ${type}`);
    };
    const lines: string[] = [];

    const perform = async (op: any[]) => {
      const [kind, name, extra] = op;
      if (kind === "active") {
        await send("Emulation.setFocusEmulationEnabled", { enabled: name });
      } else if (kind === "key" || kind === "down" || kind === "up") {
        const held = ((extra ?? []) as string[]).map((m) => MODIFIERS[m] ?? fail(`unknown modifier ${m}`));
        const own = MODIFIER_KEYS[name];
        const keyCode = KEY_CODES[name] ?? name;
        const key = (type: "keyDown" | "char" | "keyUp", code: string, modifiers: string[]) => win.webContents.sendInputEvent({ type, keyCode: code, modifiers: modifiers as any });
        if (kind !== "up") {
          const modifiers = own === undefined ? held : [...held, own];
          await until("keydown", () => key("keyDown", keyCode, modifiers));
          const char = CHARS[name] ?? (name.length === 1 ? name : undefined);
          if (char !== undefined && held.every((m) => m === "shift")) key("char", char, held);
        }
        if (kind !== "down") await until("keyup", () => key("keyUp", keyCode, held));
      } else if (kind === "press" || kind === "move") {
        const [x, y] = (await centre(name)).map(Math.round);
        if (kind === "move") await until("mousemove", () => win.webContents.sendInputEvent({ type: "mouseMove", x, y, movementX: 1 } as any));
        else {
          const button = extra ?? "left";
          await until("mousedown", () => win.webContents.sendInputEvent({ type: "mouseDown", x, y, button, clickCount: 1 }));
          await until("mouseup", () => win.webContents.sendInputEvent({ type: "mouseUp", x, y, button, clickCount: 1 }));
        }
      } else if (kind === "focus") {
        await evaluate(`document.getElementById(${JSON.stringify(name)}).focus()`);
      } else if (kind === "blur") {
        await evaluate("document.activeElement.blur()");
      } else fail(`unknown op ${JSON.stringify(op)}`);
    };

    const random = mulberry32(fixture.random.seed);
    const pick = <T>(list: T[]): T => list[Math.floor(random() * list.length)];
    const sequences: any[][] = [...fixture.sequences];
    for (let i = 0; i < fixture.random.sequences; i++) {
      const ops: any[] = [["active", true]];
      while (ops.length < fixture.random.length) ops.push(pick(fixture.random.ops));
      sequences.push(ops);
    }
    const css = fixture.css.join("\n");
    for (const [sequence, ops] of sequences.entries()) {
      await send("Emulation.setFocusEmulationEnabled", { enabled: false });
      const html = `<!doctype html><html><head><meta charset="utf-8"><style>${css}</style></head><body>${fixture.markup}</body></html>`;
      fs.writeFileSync(request.page, html);
      await win.loadFile(request.page);
      await evaluate(COUNT);
      for (const op of ops) {
        await perform(op);
        await settle();
        lines.push(JSON.stringify({ sequence, op, ...(await evaluate(READ)) }));
      }
    }
    process.stdout.write(lines.map((l) => l + "\n").join(""), () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
