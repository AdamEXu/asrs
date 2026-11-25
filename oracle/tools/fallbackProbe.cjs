// Measures fractional font sizes, their baselines and font fallback in Electron for `windows_vectors.rs`:
//   electron oracle/tools/fallbackProbe.cjs parity/vectors/canvas/fallback-windows.json
const { app, BrowserWindow } = require("electron");
const fs = require("fs");

const SIZES = [
  39.10909090909091, 24.443181818181817, 19.554545454545455, 26.072727272727278, 46.1337890625, 22.21276595744681,
  13.882978723404253, 14.829545454545453, 10.1953125, 23.727272727272727
];
const BASELINE_SIZES = [29.659090909090907, 47.45454545454545, ...SIZES];
const BASELINES = ["top", "middle", "bottom"];
const FONTS = ["12px system-ui", "bold 14px system-ui", "italic 16px sans-serif", "13px monospace"];
const TEXTS = ["日本語", "かな", "한국", "∞→⇒", "☃⚙", "Ωж", "ไทย", "ا", "１２", "𠀀", "א", "✓•", "漢字かな交じり"];
const FIELDS = [
  "width",
  "actualBoundingBoxLeft",
  "actualBoundingBoxRight",
  "actualBoundingBoxAscent",
  "actualBoundingBoxDescent"
];

function measure(cases, fields) {
  const context = document.createElement("canvas").getContext("2d");
  return cases.map(([font, text]) => {
    context.font = font;
    const metrics = context.measureText(text);
    return { font, text, metrics: fields.map((f) => metrics[f]) };
  });
}

function baselines(sizes, names) {
  const context = document.createElement("canvas").getContext("2d");
  return sizes.map((size) => {
    const font = `${size}px sans-serif`;
    context.font = font;
    const offset = (name) => {
      context.textBaseline = name;
      return context.measureText("0").alphabeticBaseline;
    };
    return { font, text: "0", metrics: names.map(offset) };
  });
}

app.commandLine.appendSwitch("force-color-profile", "srgb");
app.commandLine.appendSwitch("lang", "en-US");
app.whenReady().then(async () => {
  const window = new BrowserWindow({ show: false });
  await window.loadURL("data:text/html,<meta charset=utf-8>");
  const run = (cases) =>
    window.webContents.executeJavaScript(`(${measure})(${JSON.stringify(cases)}, ${JSON.stringify(FIELDS)})`);
  const sizes = await run(SIZES.map((s) => [`${s}px sans-serif`, "Robot 0"]));
  const offsets = await window.webContents.executeJavaScript(
    `(${baselines})(${JSON.stringify(BASELINE_SIZES)}, ${JSON.stringify(BASELINES)})`
  );
  const fallback = await run(FONTS.flatMap((font) => TEXTS.map((text) => [font, text])));
  const lines = (rows) => rows.map((r) => `    ${JSON.stringify(r)}`).join(",\n");
  fs.writeFileSync(
    process.argv[process.argv.length - 1],
    `{\n  "electron": "${process.versions.electron}",\n  "sizes": [\n${lines(sizes)}\n  ],\n  "baselines": [\n${lines(offsets)}\n  ],\n  "fallback": [\n${lines(fallback)}\n  ]\n}\n`
  );
  app.quit();
});
