const { app, BrowserWindow } = require("electron");

const page = `<!doctype html><canvas id=c width=64 height=64></canvas><script>
const gl = document.getElementById("c").getContext("webgl2", { antialias: true });
let info = { webgl2: !!gl };
if (gl) {
  const ext = gl.getExtension("WEBGL_debug_renderer_info");
  info.renderer = ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
  info.vendor = ext ? gl.getParameter(ext.UNMASKED_VENDOR_WEBGL) : gl.getParameter(gl.VENDOR);
  info.samples = gl.getParameter(gl.SAMPLES);
  info.maxSamples = gl.getParameter(gl.MAX_SAMPLES);
}
console.log("WEBGL " + JSON.stringify(info));
</script>`;

app.whenReady().then(async () => {
  const win = new BrowserWindow({ width: 200, height: 200, show: false });
  win.webContents.on("console-message", (e) => {
    if (e.message.startsWith("WEBGL ")) {
      process.stdout.write(e.message + "\n");
      process.stdout.write("FEATURES " + JSON.stringify(app.getGPUFeatureStatus()) + "\n");
      app.getGPUInfo("basic").then((i) => {
        process.stdout.write("GPUINFO " + JSON.stringify(i.auxAttributes ? { gl: i.auxAttributes.glImplementationParts, renderer: i.auxAttributes.glRenderer } : i) + "\n");
        app.quit();
      });
    }
  });
  win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(page));
  setTimeout(() => app.quit(), 20000);
});
