const { app, BrowserWindow } = require("electron");
const fs = require("fs");
const path = require("path");

const args = process.argv.slice(process.argv.indexOf("--") + 1);
const [scenesPath, measurePath, outDir] = args;

app.whenReady().then(async () => {
  try {
    const win = new BrowserWindow({
      show: false,
      width: 400,
      height: 300,
      webPreferences: { backgroundThrottling: false }
    });
    await win.loadFile(path.join(__dirname, "replay.html"));
    fs.mkdirSync(outDir, { recursive: true });

    const info = {
      electron: process.versions.electron,
      chrome: process.versions.chrome,
      gpuFeatureStatus: app.getGPUFeatureStatus(),
      scenes: []
    };
    const scenes = JSON.parse(fs.readFileSync(scenesPath, "utf8"));
    for (const scene of scenes) {
      for (const mode of ["gpu", "cpu"]) {
        const result = await win.webContents.executeJavaScript(
          `replay(${JSON.stringify(scene)}, ${mode === "cpu"})`
        );
        const file = path.join(outDir, `${scene.name}.${mode}.rgba`);
        fs.writeFileSync(file, Buffer.from(result.rgba, "base64"));
        info.scenes.push({ name: scene.name, mode, width: result.width, height: result.height });
      }
    }

    const measureReq = JSON.parse(fs.readFileSync(measurePath, "utf8"));
    const metrics = await win.webContents.executeJavaScript(
      `measure(${JSON.stringify(measureReq.fonts)}, ${JSON.stringify(measureReq.strings)})`
    );
    fs.writeFileSync(path.join(outDir, "measure.json"), JSON.stringify(metrics, null, 1));
    fs.writeFileSync(path.join(outDir, "info.json"), JSON.stringify(info, null, 1));
  } catch (e) {
    console.error(e);
    process.exitCode = 1;
  }
  app.quit();
});
