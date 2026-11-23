// Fixtures of the Console table's layout for `crates/asrs-app/tests/console_layout.rs`. Writes
// parity/specs/dom/console-layout/*.entry.json and the log they share; edit this script, not
// the specs.

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SPECS = path.join(ROOT, "parity/specs/dom");

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const random = mulberry32(1303);
const int = (n) => Math.floor(random() * n);
const pick = (list) => list[int(list.length)];
const ASCII = Array.from({ length: 94 }, (_, i) => String.fromCharCode(33 + i));
const PUNCTUATION = Array.from("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~");
const WORDS = ["Robot", "periodic()", "error", "warning:", "0.02s", "/Drive/Module0", "frc.robot.Main", "-1.5", "x", "[0,1,2]"];
const OTHER = ["é", "°", "µ", "→", "✓", "—", "…", " ", "\u{1f600}"];
const SGR = ["0", "1", "2", "3", "4", "22", "31", "1;32", "3;33", "38;5;208", "41;97", "0", "0"];

function chunk() {
  switch (int(9)) {
    case 0:
      return " ".repeat(1 + int(6));
    case 1:
      return "\t".repeat(1 + int(2));
    case 2:
      return Array.from({ length: 1 + int(12) }, () => pick(PUNCTUATION)).join("");
    case 3:
      return Array.from({ length: 1 + int(8) }, () => pick(ASCII)).join("");
    case 4:
      return `\u001b[${pick(SGR)}m`;
    case 5:
      return int(5) === 0 ? pick(["\n", "\r\n", "\\n", "\n\n"]) : pick(WORDS);
    case 6:
      return int(4) === 0 ? pick(OTHER) : pick(WORDS) + " ";
    default:
      return pick(WORDS) + pick([" ", ", ", "", ": "]);
  }
}

const lines = [
  "",
  " ",
  "\n",
  "end\n",
  "\nstart",
  "a\n\n\nb",
  "x".repeat(400),
  " ".repeat(300),
  "\t".repeat(40),
  "(".repeat(250),
  "\"".repeat(120) + "x" + ")".repeat(120),
  "ab ".repeat(90),
  "word ".repeat(19) + " ".repeat(10) + "after",
  "\u001b[1m" + "bold text ".repeat(12) + "\u001b[0m" + "plain text ".repeat(12)
];
while (lines.length < 220) {
  let line = "";
  const target = 10 + int(260);
  while (line.length < target) line += chunk();
  lines.push(line);
}
const log = lines.map((line, i) => ["putString", "/Wrap", i * 0.5 * (i > 200 ? 40000 : 1), line]);
fs.writeFileSync(path.join(SPECS, "logs/console-wrap.json"), JSON.stringify(log) + "\n");

// Every row: its clientHeight, then each cell's text nodes with their client rects and the
// cell's inline element boxes, all relative to the cell's left edge and the row's top.
const ROWS = String.raw`/*console-layout:rows*/ window.__oracleQuiet(() => {
  const range = document.createRange();
  const body = document.querySelector("div.renderer-content > :not([hidden]) table.console-table > tbody");
  return Array.from(body.children).slice(1).map((row) => {
    const top = row.getBoundingClientRect().top;
    return [row.clientHeight, ...Array.from(row.children).map((cell) => {
      const left = cell.getBoundingClientRect().left;
      const out = [];
      const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
        range.selectNodeContents(node);
        out.push([node.data, Array.from(range.getClientRects(), (r) => [r.x - left, r.y - top, r.width, r.height])]);
      }
      return [cell.getBoundingClientRect().width, out];
    })];
  });
})`;

function spec(window) {
  return {
    entry: "dom",
    inputs: ["../logs/console-wrap.json"],
    fixture: {
      geometry: "real",
      window,
      seed: 1304,
      steps: [
        { log: "../logs/console-wrap.json" },
        { main: "new-tab", data: 5 },
        { eval: "window.tabs.tabList[window.tabs.selectedTab].controller.restoreState(\"/Wrap\")" },
        { frames: 3 },
        { capture: [{ kind: "eval", expr: ROWS }], label: "plain" },
        { event: { target: "input.filter", type: "input", props: { value: "ro" } } },
        { frames: 2 },
        { capture: [{ kind: "eval", expr: ROWS }], label: "filtered" }
      ]
    }
  };
}

const write = (name, value) =>
  fs.writeFileSync(path.join(SPECS, `console-layout/${name}.entry.json`), JSON.stringify(value, null, 2) + "\n");
write("wrap", spec({ width: 1100, height: 650 }));
write("wrap-2x", spec({ width: 900, height: 650, devicePixelRatio: 2 }));
write("wrap-narrow", spec({ width: 520, height: 400 }));

// Scroll anchoring: lines of different heights filtered away above the first row in view.
const CONTAINER = 'document.querySelector("div.renderer-content > :not([hidden]) div.console-table-container")';
const anchorLines = Array.from({ length: 80 }, (_, i) => {
  const value = i === 0 ? "tall\n".repeat(9) + "tall" : i % 7 === 3 ? `row ${i} pair\nsecond` : `row ${i}`;
  return ["putString", "/Wrap", i, value];
});
const top = { capture: [{ kind: "eval", expr: `/*console-layout:top*/ window.__oracleQuiet(() => ${CONTAINER}.scrollTop)` }] };
fs.writeFileSync(path.join(SPECS, "logs/console-anchor.json"), JSON.stringify(anchorLines) + "\n");
const anchorSteps = [
  { log: "../logs/console-anchor.json" },
  { main: "new-tab", data: 5 },
  { eval: "window.tabs.tabList[window.tabs.selectedTab].controller.restoreState(\"/Wrap\")" },
  { frames: 3 }
];
for (const start of [0, 10, 100, 190, 191, 200, 301, 655, 2000]) {
  for (const filter of ["!tall", "!pair", "row 2", "absent"]) {
    anchorSteps.push(
      { eval: `/*console-layout:scroll ${start}*/ window.__oracleQuiet(() => { ${CONTAINER}.scrollTop = ${start}; })` },
      { frames: 2 },
      top
    );
    for (const value of [filter, ""]) {
      anchorSteps.push({ event: { target: "input.filter", type: "input", props: { value } } }, { frames: 2 }, top);
    }
  }
}
for (const [first, second] of [[200, 300], [300, 650], [1000, 191], [250, 251]]) {
  const scroll = (to) => [
    { eval: `/*console-layout:scroll ${to}*/ window.__oracleQuiet(() => { ${CONTAINER}.scrollTop = ${to}; })` },
    { frames: 2 },
    top
  ];
  const filtered = (value) => [{ event: { target: "input.filter", type: "input", props: { value } } }, { frames: 2 }, top];
  anchorSteps.push(...scroll(first), ...filtered("!tall"), ...scroll(second), ...filtered(""), ...scroll(0), ...filtered("!pair"));
  anchorSteps.push(...filtered(""));
}
write("anchor", { entry: "dom", inputs: ["../logs/console-anchor.json"], fixture: { geometry: "real", seed: 1305, steps: anchorSteps } });
