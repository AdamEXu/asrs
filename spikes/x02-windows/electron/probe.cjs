// X02: what upstream's hub window looks like on Windows, and Chromium's wheel deltas.
// usage: electron probe.cjs <upstream dir> <out dir> <light|dark> <x02-shell.exe>
const { app, BrowserWindow, screen, desktopCapturer } = require("electron");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");

const [upstream, out, theme, shell] = process.argv.slice(-4);
fs.mkdirSync(out, { recursive: true });
const userData = path.join(out, `userdata-${theme}`);
fs.rmSync(userData, { recursive: true, force: true });
fs.mkdirSync(userData, { recursive: true });
fs.writeFileSync(path.join(userData, "prefs.json"), JSON.stringify({ theme }));
app.setPath("userData", userData);

const report = {};
const log = (k, v) => {
  report[k] = v;
  console.log(k, JSON.stringify(v));
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const finish = (code) => {
  fs.writeFileSync(path.join(out, `electron-${theme}.json`), JSON.stringify(report, null, 1));
  app.exit(code);
};
setTimeout(() => {
  console.error("timeout");
  finish(1);
}, 120000);

let done = false;
app.on("browser-window-created", (_e, win) => {
  win.webContents.on("devtools-opened", () => win.webContents.closeDevTools());
  win.webContents.once("did-finish-load", async () => {
    if (done || !win.webContents.getURL().endsWith("/www/hub.html")) return;
    done = true;
    try {
      win.setBounds({ x: 200, y: 100, width: 1100, height: 650 });
      win.focus();
      await sleep(3000);
      const bounds = win.getBounds();
      const content = win.getContentBounds();
      log("bounds", bounds);
      log("contentBounds", content);
      log("scaleFactor", screen.getDisplayMatching(bounds).scaleFactor);
      log("menuBarVisible", win.isMenuBarVisible());
      log("menuBarAutoHide", win.isMenuBarAutoHide());
      log("alwaysOnTop", win.isAlwaysOnTop());
      log("bodyClasses", await win.webContents.executeJavaScript("document.body.className"));
      const capture = await win.webContents.capturePage();
      fs.writeFileSync(path.join(out, `hub-${theme}-capture.png`), capture.toPNG());
      log("captureSize", capture.getSize());
      execFileSync(shell, ["screen", path.join(out, `hub-${theme}-screen.png`), bounds.x - 20, bounds.y - 20, bounds.width + 40, bounds.height + 40]);
      log("winfoHub", execFileSync(shell, ["winfo", "AdvantageScope"]).toString().trim().split("\n"));

      const top = new BrowserWindow({ width: 300, height: 200, alwaysOnTop: true, title: "x02 electron alwaysOnTop", show: true });
      await top.loadURL("data:text/html,<title>x02 electron alwaysOnTop</title>top");
      await sleep(800);
      log("winfoAlwaysOnTop", execFileSync(shell, ["winfo", "x02 electron"]).toString().trim().split("\n"));
      top.destroy();

      await win.webContents.executeJavaScript(`
        window.__x02wheel = [];
        window.addEventListener("wheel", (e) => window.__x02wheel.push({ deltaX: e.deltaX, deltaY: e.deltaY, deltaMode: e.deltaMode, wheelDeltaY: e.wheelDeltaY, wheelDeltaX: e.wheelDeltaX }), { capture: true, passive: true });
        true`);
      win.focus();
      const cx = content.x + 700;
      const cy = content.y + 400;
      const sender = spawn(shell, ["send-wheel", String(cx), String(cy)]);
      await new Promise((r) => sender.on("exit", r));
      await sleep(800);
      log("wheel", await win.webContents.executeJavaScript("window.__x02wheel"));
      finish(0);
    } catch (e) {
      console.error(e);
      finish(1);
    }
  });
});

require(path.join(upstream, "bundles", "main.js"));
