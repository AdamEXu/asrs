const { app, BrowserWindow } = require("electron");

const secs = Number(process.argv[process.argv.length - 1]) || 30;
const page = `<!doctype html><body style="margin:0;height:100vh;background:#fff">
<script>
addEventListener("wheel", (e) => {
  console.log("dom wheel deltaX=" + e.deltaX + " deltaY=" + e.deltaY + " deltaMode=" + e.deltaMode +
    " wheelDeltaY=" + e.wheelDeltaY + " at=(" + e.clientX + "," + e.clientY + ")");
}, { passive: true });
</script></body>`;

app.whenReady().then(() => {
  const win = new BrowserWindow({ x: 100, y: 100, width: 600, height: 400, title: "x03-wheel-electron" });
  win.webContents.on("console-message", (event) => {
    process.stdout.write(event.message + "\n");
  });
  win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(page));
  win.webContents.once("did-finish-load", () => {
    process.stdout.write("electron window open dpr=" + require("electron").screen.getPrimaryDisplay().scaleFactor + "\n");
  });
  setTimeout(() => app.quit(), secs * 1000);
});
