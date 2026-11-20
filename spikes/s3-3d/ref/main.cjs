const { app, BrowserWindow, nativeTheme } = require("electron");
const zlib = require("zlib");
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

function crc32(buf) {
  let c = ~0;
  for (const b of buf) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

function encodePng(width, height, rgba) {
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) rgba.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0))
  ]);
}

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
  fs.writeFileSync(out.replace(/\.png$/, "_capture.png"), image.toPNG());
  await win.webContents.executeJavaScript("window.s3GrabRequested = true");
  let grab = null;
  while (grab === null) {
    await new Promise((r) => setTimeout(r, 50));
    grab = await win.webContents.executeJavaScript(
      "window.s3Grab && { width: s3Grab.width, height: s3Grab.height, data: Array.from(s3Grab.data) }"
    );
  }
  fs.writeFileSync(out, encodePng(grab.width, grab.height, Buffer.from(grab.data)));
  console.log(JSON.stringify({ out, size, capture: image.getSize(), framebuffer: [grab.width, grab.height] }));
  app.exit(0);
});
