const { app, BrowserWindow, nativeTheme } = require("electron");
const fs = require("fs");
const path = require("path");

const args = Object.fromEntries(
  process.argv
    .slice(2)
    .filter((a) => a.startsWith("--"))
    .map((a) => a.slice(2).split("="))
);
const theme = args.theme || "dark";
const out = args.out;
const cam = args.cam || "field";

app.whenReady().then(async () => {
  nativeTheme.themeSource = theme;
  const win = new BrowserWindow({
    width: 1100,
    height: 650,
    useContentSize: true,
    show: true,
    frame: false,
    webPreferences: { backgroundThrottling: false }
  });
  await win.loadFile(path.join(__dirname, "page.html"), { query: { cam } });
  const start = Date.now();
  while (!(await win.webContents.executeJavaScript("window.s3Ready === true"))) {
    if (Date.now() - start > 30000) {
      console.error("timeout waiting for render");
      app.exit(1);
      return;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  await new Promise((r) => setTimeout(r, 1500));
  const size = await win.webContents.executeJavaScript("[innerWidth, innerHeight, devicePixelRatio]");
  const image = await win.webContents.capturePage();
  fs.writeFileSync(out, image.toPNG());
  console.log(JSON.stringify({ out, size, image: image.getSize(), scale: image.getScaleFactors?.() }));
  app.exit(0);
});
