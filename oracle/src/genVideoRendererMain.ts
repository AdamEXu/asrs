import { app, BrowserWindow } from "electron";
import fs from "node:fs";

// Electron main process for `genVideoRenderer` (not run under ELECTRON_RUN_AS_NODE). Each step
// is {"render": command} or {"load": true}, which waits for the image's load or error event.
// After each one it writes what upstream's renderer and its <img> read back.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));

function fail(message: string): never {
  process.stderr.write(`genVideoRendererMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

const PAGE_SCRIPT = `(() => {
  const VideoRenderer = ${request.renderer};
  const image = document.querySelector("img");
  const renderer = new VideoRenderer(document.getElementById("root"));
  const read = () => ({
    aspectRatio: renderer.getAspectRatio(),
    state: renderer.saveState(),
    hidden: image.hidden,
    src: image.getAttribute("src"),
    natural: [image.naturalWidth, image.naturalHeight]
  });
  let events = 0;
  image.addEventListener("load", () => events++);
  image.addEventListener("error", () => events++);
  window.__render = (command) => {
    events = 0;
    renderer.render(command === "__undefined" ? undefined : command);
    return read();
  };
  window.__load = async () => {
    while (events === 0) await new Promise((resolve) => setTimeout(resolve, 5));
    return read();
  };
  return true;
})()`;

const hex = (value: number) => {
  const bits = new DataView(new ArrayBuffer(8));
  bits.setFloat64(0, value);
  return bits.getBigUint64(0).toString(16).padStart(16, "0");
};

app.setPath("userData", request.userData);
app.dock?.hide();
app
  .whenReady()
  .then(async () => {
    const win = new BrowserWindow({ width: 400, height: 300, show: false, webPreferences: { offscreen: true, backgroundThrottling: false } });
    win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
    await win.loadFile(request.page);
    await win.webContents.executeJavaScript(PAGE_SCRIPT);
    const lines: string[] = [];
    for (const step of request.fixture.steps as ({ render: unknown } | { load: true })[]) {
      const record: any = await win.webContents.executeJavaScript("render" in step ? `window.__render(${JSON.stringify(step.render)})` : "window.__load()");
      const { aspectRatio, ...rest } = record;
      lines.push(JSON.stringify({ step, aspectRatio: aspectRatio === null ? null : { n: hex(aspectRatio) }, ...rest }));
    }
    process.stdout.write(lines.join("\n") + "\n", () => app.exit(0));
  })
  .catch((error) => fail(String(error?.stack ?? error)));
