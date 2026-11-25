// Electron main process of `cargo xtask app chrome`: upstream's windows, which are never shown, as
// AppKit has them. windowChrome.m, loaded into it, answers with each one's native properties.
// With a captures folder the windows are also ordered in, inactive and off the screen, and
// captured with their frames.
const { app, BrowserWindow, nativeTheme } = require("electron");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

process.on("uncaughtException", (error) => {
  console.error(error);
  app.exit(1);
});

const [requestFile, out] = process.argv.slice(-2);
const request = JSON.parse(fs.readFileSync(requestFile, "utf8"));
// What updateFancyWindow() adds once main has told the page its platform.
const HUB_CLASSES = ["fancy-title-bar-mac", "fancy-side-bar-mac", ...(parseInt(os.release()) >= 27 ? ["fancy-side-bar-mac-27"] : [])]
  .map((name) => JSON.stringify(name))
  .join();
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

app.setPath("userData", `${out}.data`);
app.dock.hide();
app.on("window-all-closed", () => {});

async function chrome(win) {
  const file = `${out}.window.json`;
  fs.rmSync(file, { force: true });
  win.setRepresentedFilename(`asrs-chrome ${file}`);
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

async function capture(win, name) {
  const file = path.join(request.captures, `${name}.png`);
  fs.rmSync(file, { force: true });
  win.setRepresentedFilename(`asrs-capture ${file}`);
  for (let i = 0; i < 100 && !fs.existsSync(file); i++) await wait(100);
  await wait(300);
}

app.whenReady().then(async () => {
  await wait(request.settle);
  const records = [];
  for (const theme of request.captures ? ["light", "dark"] : ["light", "dark", "system"]) {
    nativeTheme.themeSource = theme;
    const parent = new BrowserWindow({ width: 1100, height: 650, show: false });
    for (const { name, page, options } of request.windows) {
      const owner = options.modal ? { parent } : {};
      const win = new BrowserWindow({ ...options, ...owner, show: false });
      await win.loadFile(path.join(request.www, page));
      if (name === "Hub") await win.webContents.executeJavaScript(`document.body.classList.add(${HUB_CLASSES}), 0`);
      await wait(request.settle);
      records.push({ name, theme, options, chrome: await chrome(win) });
      if (request.captures) {
        await capture(win, `${name}-${theme}-${process.env.ASRS_CHROME_ACTIVE ? "active" : "inactive"}`);
      }
      win.destroy();
    }
    parent.destroy();
  }
  // The themes after one another, on a window that is open.
  const win = new BrowserWindow({ width: 900, height: 500, show: false });
  const changes = [];
  for (const theme of ["light", "dark", "system", "dark", "light"]) {
    nativeTheme.themeSource = theme;
    await wait(request.settle);
    const { backgroundColor, effectiveAppearance, appAppearance } = await chrome(win);
    changes.push({ theme, backgroundColor, effectiveAppearance, appAppearance });
  }
  fs.writeFileSync(out, JSON.stringify({ macos: process.getSystemVersion(), windows: records, changes }));
  app.exit(0);
}).catch((error) => {
  console.error(error);
  app.exit(1);
});
