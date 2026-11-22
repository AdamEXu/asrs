// Records how Chromium turns wheel input into ScrollSensor overlay scrolls, for
// crates/asrs-model/tests/wheel_calibration.rs. Run from the repo root with upstream's Electron:
//   "$(cargo xtask oracle env | jq -r .electron)" oracle/tools/wheelCalibration.cjs \
//     [--force-device-scale-factor=2] parity/calibration/wheel-<platform>-<scale>x.json
// The page holds one overlay built like upstream's (`overflow: scroll`, hidden scrollbars, a
// 1000000 px child) and recentres it the way ScrollSensor.reset does before each case. Every
// case sends its events through webContents.sendInputEvent, whose deltaX/deltaY are
// WebMouseWheelEvent's delta_x/delta_y, and records the overlay's position at each scroll
// event. Precise events are sent one at a time, each after the last one's scroll event, so
// each gets a scroll event of its own; notched ticks animate on the compositor clock, so only
// where they settle is reproducible, and only for whole-pixel ticks: sub-pixel offsets at 1x
// and fractional ticks land on run-dependent pixels (#194). A notched tick scrolls at most
// the overlay's client size on each axis, so the overlay's size is part of the calibration.

const { app, BrowserWindow } = require("electron");
const fs = require("node:fs");

const out = process.argv[process.argv.length - 1];
const SETTLE_MS = 600;

const PAGE = `<!doctype html><html><body style="margin:0">
<div id="o" style="position:absolute;left:100px;top:50px;width:400px;height:40px;overflow:scroll">
<div style="width:1000000px;height:1000000px"></div></div>
<style>#o::-webkit-scrollbar{display:none}</style>
<script>
const o = document.getElementById("o");
window.events = [];
o.addEventListener("scroll", () => events.push([o.scrollLeft, o.scrollTop]));
window.reset = () => { o.scrollLeft = 500000; o.scrollTop = 500000; };
</script></body></html>`;

const precise = (x, y) => ({ deltaX: x, deltaY: y, hasPreciseScrollingDeltas: true });
const notch = (x, y) => ({ deltaX: x * 40, deltaY: y * 40, wheelTicksX: x, wheelTicksY: y });

const CASES = {
  "trackpad-flick": [0, 1, 3, 7, 12, 9, -2, 11, 6, 2, 1].map((p) => precise(0, -p)),
  "trackpad-x": [precise(-4, 0), precise(-9, 0), precise(6, 0)],
  "trackpad-diagonal": [precise(-20, -10), precise(5, 3)],
  "windows-lines": [precise(0, -33.33333206176758), precise(0, -33.33333206176758), precise(0, 100), precise(-33.33333206176758, 0)],
  "notch-down": [notch(0, -1)],
  "notch-up": [notch(0, 1)],
  "notch-3": [notch(0, -3)],
  "notch-5-up": [notch(0, 5)],
  "notch-x": [notch(-1, 0)],
  "notch-2-capped": [notch(0, -2)],
  "notch-x15-capped": [notch(-15, 0)],
  "notch-train": [notch(0, -1), notch(0, 1), notch(0, -0.5)]
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

app.whenReady().then(async () => {
  const win = new BrowserWindow({ width: 640, height: 200, x: 20, y: 20, show: true, webPreferences: { backgroundThrottling: false } });
  await win.loadURL("data:text/html;base64," + Buffer.from(PAGE).toString("base64"));
  const wc = win.webContents;
  const ev = (s) => wc.executeJavaScript(s);
  const result = { electron: process.versions.electron, platform: process.platform, devicePixelRatio: await ev("devicePixelRatio"), overlay: [400, 40], cases: {} };
  for (const [name, events] of Object.entries(CASES)) {
    await ev("reset()");
    await sleep(200);
    await ev("events.length = 0");
    const steps = [];
    for (const e of events) {
      wc.sendInputEvent({ type: "mouseWheel", x: 300, y: 70, canScroll: true, wheelTicksX: 0, wheelTicksY: 0, ...e });
      if (e.hasPreciseScrollingDeltas) {
        await sleep(120);
        steps.push({ wheel: [e.deltaX, e.deltaY], precise: true, scrolls: await ev("events.splice(0)") });
      } else {
        await sleep(SETTLE_MS);
        steps.push({ wheel: [e.deltaX, e.deltaY], precise: false, scrolls: await ev("events.splice(0)") });
      }
    }
    result.cases[name] = steps;
  }
  fs.mkdirSync(require("node:path").dirname(out), { recursive: true });
  fs.writeFileSync(out, JSON.stringify(result, null, 1) + "\n");
  app.quit();
});
