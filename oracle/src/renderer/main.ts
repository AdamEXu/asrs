import { app, BrowserWindow } from "electron";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

// Electron main process (not ELECTRON_RUN_AS_NODE): loads renderer.html in a hidden window with
// the hub's default web preferences and runs the request file given as the only argument.
const requestPath = process.argv[process.argv.length - 1];
const request = JSON.parse(fs.readFileSync(requestPath, "utf8"));
const page = path.join(path.dirname(fileURLToPath(import.meta.url)), "renderer.html");

app.dock?.hide();
app
  .whenReady()
  .then(async () => {
    const window = new BrowserWindow({ show: false, webPreferences: { backgroundThrottling: false } });
    await window.loadFile(page);
    const lines: string[] = await window.webContents.executeJavaScript(`runWorkerCheck(${JSON.stringify(request)})`);
    process.stdout.write(lines.map((l) => l + "\n").join(""), () => app.exit(0));
  })
  .catch((error) => {
    console.error(error);
    app.exit(1);
  });
