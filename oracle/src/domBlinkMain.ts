import { app, BrowserWindow, nativeTheme, net, protocol } from "electron";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Electron main process for `domBlink` (not run under ELECTRON_RUN_AS_NODE). Loads upstream's
// www/hub.html in an offscreen window and serves the harness in place of bundles/hub.js: the
// request's fixture, then domPage.js, whose first module (dom/blinkBoot.ts) sets up Blink. Stylesheets load from upstream. Every other request (images, <object> icons,
// iframes, workers, fetches, http and https) gets a response whose body never arrives: a request
// left unanswered fails after Electron's 30 s timeout, and one answered at once fails or loads
// at a time no step controls. Page load therefore never completes, so the page is driven over
// the DevTools protocol, which does not wait for it as executeJavaScript does.

const requestPath = process.argv[process.argv.length - 1];
const request = JSON.parse(fs.readFileSync(requestPath, "utf8"));
const fixture = request.fixture;
const view = fixture.window ?? {};
const width: number = view.width ?? 1100;
const height: number = view.height ?? 650;
const real = (p: string) => {
  try {
    return fs.realpathSync(p);
  } catch {
    return p;
  }
};
const www = real(path.join(request.upstream, "www"));
const hubHtml = path.join(www, "hub.html");
const hubJs = real(path.join(request.upstream, "bundles", "hub.js"));

const script =
  `window.__oracleBackend = "blink";\n` +
  `window.__oracleFixture = JSON.parse(${JSON.stringify(JSON.stringify(fixture))});\n` +
  `window.__oracleZone = ${JSON.stringify(request.zone ?? "")};\n` +
  fs.readFileSync(request.page, "utf8");

function fail(message: string): never {
  process.stderr.write(`domBlinkMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

const never = () => new Response(new ReadableStream({ start() {} }));

app.setPath("userData", request.userData);
app.dock?.hide();
app
  .whenReady()
  .then(async () => {
    nativeTheme.themeSource = view.theme ?? "light";
    protocol.handle("file", (req) => {
      const file = real(fileURLToPath(req.url));
      if (file === hubJs) return new Response(script, { headers: { "content-type": "text/javascript" } });
      if (file === hubHtml || (file.startsWith(www + path.sep) && path.extname(file) === ".css")) {
        return net.fetch(req, { bypassCustomProtocolHandlers: true });
      }
      return never();
    });
    protocol.handle("http", never);
    protocol.handle("https", never);
    const win = new BrowserWindow({
      width,
      height,
      useContentSize: true,
      show: false,
      webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false }
    });
    win.webContents.setFrameRate(240);
    win.webContents.on("console-message", (event) => {
      process.stderr.write(`page ${event.level}: ${event.message}\n`);
    });
    win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
    const ready = new Promise<void>((resolve) => win.webContents.once("dom-ready", () => resolve()));
    win.loadFile(hubHtml).catch(() => {});
    await ready;
    win.webContents.debugger.attach("1.3");
    const evaluate = async (expression: string): Promise<any> => {
      const result = await win.webContents.debugger.sendCommand("Runtime.evaluate", {
        expression,
        awaitPromise: true,
        returnByValue: true
      });
      if (result.exceptionDetails !== undefined) {
        fail(`${expression}: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
      }
      return result.result.value;
    };
    const problems: string[] = await evaluate("window.__oracleReady()");
    if (problems.length > 0) fail(problems.join("; "));
    const length: number = await evaluate("window.__oracleRun().then(() => (window.__oracleText = window.__oracleOutput()).length)");
    const chunks: string[] = [];
    for (let at = 0; at < length; at += 1 << 22) {
      chunks.push(await evaluate(`window.__oracleText.slice(${at}, ${at + (1 << 22)})`));
    }
    process.stdout.write(chunks.join(""), () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
