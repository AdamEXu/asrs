const fs = require("fs");
const { app, BrowserWindow } = require("electron");

const [file, out] = process.argv.slice(-2);
app.whenReady().then(async () => {
  const win = new BrowserWindow({ width: 920, height: 798, useContentSize: true, show: true });
  await win.loadFile(file);
  await new Promise((r) => setTimeout(r, 1500));
  const image = await win.webContents.capturePage();
  fs.writeFileSync(out, image.toPNG());
  app.quit();
});
