// Electron main process of `cargo xtask app windows`: windows that are never shown, on the display
// as it is. Writes one JSON object to the file named last on the command line: the displays, where
// each of the options puts a window, and on macOS, with windowEvents.m loaded, which of a window's
// notifications Electron's `focus` and `blur` events follow.
const { app, BrowserWindow, screen } = require("electron");
const fs = require("node:fs");

// Electron would show an alert.
process.on("uncaughtException", (error) => {
  console.error(error);
  app.exit(1);
});

const [requestFile, out] = process.argv.slice(-2);
const request = JSON.parse(fs.readFileSync(requestFile, "utf8"));
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const rect = ({ x, y, width, height }) => [x, y, width, height];

app.setPath("userData", `${out}.data`);
app.dock?.hide();
app.on("window-all-closed", () => {});

async function placement() {
  const records = [];
  for (const options of request.options) {
    const win = new BrowserWindow({ ...options, show: false });
    await wait(request.settle);
    records.push({ options, bounds: rect(win.getBounds()), content: rect(win.getContentBounds()) });
    win.destroy();
  }
  return records;
}

async function focus() {
  const parent = new BrowserWindow({ width: 800, height: 600, show: false });
  const kinds = {
    hub: { titleBarStyle: "hiddenInset" },
    prompt: { parent, modal: true, resizable: false, useContentSize: true },
    download: { width: 500, height: 500, resizable: false, alwaysOnTop: true, useContentSize: true }
  };
  const notifications = ["BecomeKey", "BecomeMain", "ResignKey", "ResignMain"].map((name) => `NSWindowDid${name}Notification`);
  const order = [0, 1, 2, 3, 1, 0, 3, 2, 1, 1, 2, 0, 3, 3];
  const records = [];
  for (const [window, options] of Object.entries(kinds)) {
    const win = new BrowserWindow({ ...options, show: false });
    win.setRepresentedFilename("asrs-main");
    await wait(50);
    const main = win.getRepresentedFilename();
    let events = [];
    win.on("focus", () => events.push("focus"));
    win.on("blur", () => events.push("blur"));
    const steps = [];
    for (const i of order) {
      events = [];
      win.setRepresentedFilename(`asrs-post ${notifications[i]}`);
      await wait(50);
      steps.push({ post: notifications[i], events });
    }
    records.push({ window, canBecomeMain: main.endsWith("/yes"), steps });
    win.destroy();
  }
  parent.destroy();
  return records;
}

app.whenReady().then(async () => {
  await wait(request.settle);
  const displays = screen.getAllDisplays().map((d) => ({ bounds: rect(d.bounds), workArea: rect(d.workArea), scale: d.scaleFactor }));
  const primary = screen.getPrimaryDisplay().id;
  const record = {
    displays,
    primary: screen.getAllDisplays().findIndex((d) => d.id === primary),
    placement: await placement()
  };
  if (process.platform === "darwin") record.focus = await focus();
  fs.writeFileSync(out, JSON.stringify(record));
  app.exit(0);
}).catch((error) => {
  console.error(error);
  app.exit(1);
});
