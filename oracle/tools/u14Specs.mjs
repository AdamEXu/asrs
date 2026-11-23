// Fixtures of the Statistics and Metadata tabs' layout and of Blink's line breaking, for
// `crates/asrs-app/tests/statistics_metadata_layout.rs` and
// `crates/asrs-canvas/tests/line_break.rs`. Writes
// parity/specs/dom/{statistics-layout,metadata-layout,line-break}/*.entry.json; edit this script,
// not the specs.

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

function write(family, name, fixture) {
  const file = path.join(SPECS, family, `${name}.entry.json`);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify({ entry: "dom", fixture }, null, 2) + "\n");
}

const range = (n, f) => Array.from({ length: n }, (_, i) => f(i));
const quiet = (body) => `window.__oracleQuiet(() => { ${body} })`;
const RENDERER = 'document.querySelector("div.renderer-content > :not([hidden])")';

// Rows as [height, cells], a cell as [x, y, width, height, text nodes], a text node as
// [data, line rects], all relative to `origin`.
const ROWS = String.raw`
  const box = (r) => [r.x - origin.x, r.y - origin.y, r.width, r.height];
  const range = document.createRange();
  const text = (cell) => {
    const out = [];
    const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
      range.selectNodeContents(node);
      out.push([node.data, Array.from(range.getClientRects(), box)]);
    }
    return out;
  };
  const rows = (body) => Array.from(body.children, (row) =>
    [row.getBoundingClientRect().height, Array.from(row.children, (cell) => [...box(cell.getBoundingClientRect()), text(cell)])]);
  const sizes = (el) => [el.offsetWidth, el.clientWidth, el.clientHeight, el.scrollWidth, el.scrollHeight];
`;

const STATISTICS = `/*statistics-layout*/ ${quiet(`
  const root = ${RENDERER};
  const container = root.querySelector("div.stats-values-container");
  const origin = container.getBoundingClientRect();
  ${ROWS}
  const table = container.firstElementChild;
  container.scrollTo(1e6, 1e6);
  const end = [container.scrollLeft, container.scrollTop];
  container.scrollTo(0, 0);
  return {
    root: box(root.parentElement.getBoundingClientRect()),
    end,
    container: [origin.x, origin.y, origin.width, origin.height],
    sizes: sizes(container),
    table: box(table.getBoundingClientRect()),
    histogram: box(root.querySelector("div.stats-histogram-container").getBoundingClientRect()),
    canvas: box(root.querySelector("canvas").getBoundingClientRect()),
    rows: rows(table.firstElementChild)
  };
`)}`;

const METADATA = `/*metadata-layout*/ ${quiet(`
  const root = ${RENDERER};
  const container = root.querySelector("div.metadata-table-container");
  const alert = root.querySelector("div.tab-centered");
  const origin = root.parentElement.getBoundingClientRect();
  ${ROWS}
  const table = container.firstElementChild;
  return {
    root: [origin.x, origin.y, origin.width, origin.height],
    alert: alert.hidden ? null : [...box(alert.getBoundingClientRect()), text(alert)],
    container: container.hidden ? null : box(container.getBoundingClientRect()),
    sizes: sizes(container),
    rows: container.hidden ? [] : rows(table.firstElementChild)
  };
`)}`;

const STATS = [
  "count", "min", "max", "mean", "median", "mode", "geometricMean", "harmonicMean", "quadraticMean", "standardDeviation",
  "medianAbsoluteDeviation", "interquartileRange", "skewness", "percentile01", "percentile05", "percentile10", "percentile25",
  "percentile50", "percentile75", "percentile90", "percentile95", "percentile99"
];
const COLORS = ["#2b66a2", "#e5b31b", "#af2437", "#80588e", "#e48b32", "#c0b487"];
const VALUES = [0, -0.0004, 1, -12.5, 123.4567, 98765.4321, -1234567.891, 4503599627370496.5, 0.25];
const EXTREME = [NaN, Infinity, -Infinity, 1e21, -1.5e21, 1e20, -0.001];

// A number as JavaScript source, so NaN and the infinities survive.
const source = (v) => (Array.isArray(v) ? `[${v.map(source).join(",")}]` : typeof v === "number" ? String(v) : typeof v === "string" ? JSON.stringify(v) : `{${Object.entries(v).map(([k, x]) => `${JSON.stringify(k)}:${source(x)}`).join(",")}}`);
const random = mulberry32(1401);
let counter = 0;
function statistics(count, pool) {
  const fields = range(count, (i) => ({
    title: `/Layout/Field${i}`,
    color: COLORS[i % COLORS.length],
    histogramCounts: [1, 2],
    stats: Object.fromEntries(STATS.map((name) => [name, pool[Math.floor(random() * pool.length)]]))
  }));
  return { changeCounter: ++counter, bins: [0, 1], stepSize: 1, fields };
}
const command = (value) =>
  `/*layout:command*/ (() => { const command = ${source(value)}; window.tabs.tabList[window.tabs.selectedTab].controller.getCommand = () => command; return command; })()`;
const shown = (value, expr, label) => [
  { capture: [{ kind: "eval", expr: command(value) }], label: `${label} command` },
  { frames: 4 },
  { capture: [{ kind: "eval", expr }], label }
];

const WINDOWS = {
  "1x": { width: 1100, height: 650 },
  "2x": { width: 1100, height: 650, devicePixelRatio: 2 },
  hidpi: { width: 1103, height: 651, devicePixelRatio: 1.5 },
  narrow: { width: 700, height: 420 }
};

for (const [name, window] of Object.entries(WINDOWS)) {
  write("statistics-layout", name, {
    geometry: "real",
    window,
    seed: 1402,
    steps: [
      { main: "new-tab", data: 6 },
      { frames: 4 },
      ...[[0, VALUES], [1, [0, 1]], [1, VALUES], [2, VALUES], [3, VALUES], [5, VALUES], [9, VALUES], [2, EXTREME], [4, EXTREME]].flatMap(([count, pool], i) =>
        shown(statistics(count, pool), STATISTICS, `${i}: ${count} fields`)
      )
    ]
  });
}

const WORDS = ["main", "frc-6328", "2026-11-22", "16:04:05", "(dirty)", "src/main/java/frc/robot", "a1b2c3d4e5f60718293a4b5c6d7e8f9001122334", "Robot", "v1.2.3", "WPILib,", "don't", "\"quoted\"", "50%", "x=y+z;", "https://github.com/Mechanical-Advantage/AdvantageScope/blob/main/README.md?plain=1#L10-L20", "All", "changes", "committed", "--", "[1,2,3]", "{a:b}", "C++", "$5", "e-mail", "-5", "3-4", "A-1"];
const OTHER = ["café", "µs", "20°C", "→", "✓", "em—dash", "wait…", "no break", "日本語のテキスト、改行。", "\u{1f916}\u{1f600}", "«quoted»", "zero\u200bwidth"];
const SOFT_HYPHEN = "soft\u00adhyphen";
// Blink's advance for CJK in Courier differs from the port's (#273), which at 1.5x moves a line break.
let others = OTHER;
const pick = (list) => list[Math.floor(random() * list.length)];
function value() {
  const kind = random();
  if (kind < 0.06) return "";
  if (kind < 0.12) return pick(WORDS);
  const words = range(1 + Math.floor(random() * 24), () => (random() < 0.12 ? pick(others) : pick(WORDS)));
  return words.join(pick([" ", " ", " ", "  ", "/", "_", "\t", "\n"]));
}
function metadata(rows, columns, text = value) {
  const out = {};
  for (let i = 0; i < rows; i++) {
    const key = pick(["/Project", "/GitSHA", "/Build Date", "/Runtime-Type/Long", "/superlongmetadatakeywithoutbreaks", "/Two words here that wrap", "/Log0/GitBranch"]) + i;
    const cell = (shownColumn) => (shownColumn && random() < 0.9 ? text() : null);
    out[key] = { generic: cell(columns.includes("g")), real: cell(columns.includes("r")), replay: cell(columns.includes("p")) };
  }
  return out;
}
const metadataSource = (value) => JSON.stringify(value);
const metadataShown = (value, label) => [
  {
    capture: [{ kind: "eval", expr: `/*layout:command*/ (() => { const command = ${metadataSource(value)}; window.tabs.tabList[window.tabs.selectedTab].controller.getCommand = () => command; return command; })()` }],
    label: `${label} command`
  },
  { frames: 3 },
  { capture: [{ kind: "eval", expr: METADATA }], label }
];
for (const [name, window] of Object.entries(WINDOWS)) {
  others = name === "hidpi" ? OTHER.filter((s) => !/[\u3000-\u9fff]/.test(s)) : OTHER;
  write("metadata-layout", name, {
    geometry: "real",
    window,
    seed: 1403,
    steps: [
      { main: "new-tab", data: 12 },
      { frames: 3 },
      ...metadataShown({}, "empty"),
      ...metadataShown(metadata(3, "g", () => pick(WORDS)), "short"),
      ...metadataShown(
        Object.fromEntries(
          ["", "end\n", "\n", "\n\n", "a\n\nb", "\nstart", " \n x ", "a\n  ", "  ", "\t\n"].map((text, i) => [`/Line${i}`, { generic: text, real: null, replay: null }])
        ),
        "lines"
      ),
      ...metadataShown(metadata(40, "g"), "generic"),
      ...metadataShown(metadata(30, "rp"), "real and replay"),
      ...metadataShown(metadata(30, "grp"), "all")
    ]
  });
}

// Line breaking at width 0: per string, where Blink starts a new line.
const BREAKS = String.raw`
  const div = document.createElement("div");
  div.style.cssText = "position: absolute; left: 0px; top: 0px; width: 0px; font: 16px/20px Courier; white-space: normal;";
  document.body.appendChild(div);
  const range = document.createRange();
  const breaks = (text) => {
    div.textContent = text;
    const node = div.firstChild;
    const out = [];
    let last = null;
    let at = 0;
    for (const c of text) {
      if (c !== " " && c !== "​" && c !== "­") {
        range.setStart(node, at);
        range.setEnd(node, at + c.length);
        const rects = range.getClientRects();
        const top = rects[rects.length - 1].y;
        if (last !== null && top - last > 10) out.push(at);
        last = top;
      }
      at += c.length;
    }
    return out;
  };
  try {
    return body();
  } finally {
    div.remove();
  }
`;
const breaks = (label, body) => ({
  capture: [{ kind: "eval", expr: `/*line-break*/ ${quiet(`const body = () => ${body}; ${BREAKS}`)}` }],
  label
});
const sentence = (words) => range(2 + Math.floor(random() * 9), () => pick(words)).join(pick([" ", "", "-", "/", ","]));
const strings = [...WORDS, ...OTHER, SOFT_HYPHEN, ...range(150, () => sentence(WORDS)), ...range(150, () => sentence([...WORDS, ...OTHER, ...OTHER, SOFT_HYPHEN]))];
strings.push(
  ...["x-", "x|", "1-", "x -", "-", "|", "x?", "x!", "x)", "x/"].map((s) => `${s}\u00a0y`),
  "\u00a0-x",
  "\u00a0|x",
  "\u00a0(x",
  "\u00a0-1",
  "a\u00a0日\u00a0b",
  ...["きゃっと", "キャッシュー", "人々の", "あぁ、ー。", "한국어 텍스트입니다.", "ภาษาไทยทดสอบ", "カタカナ・テスト", "x\u2010y", "1\u20132", "a\u2060b", "a\u2028b", "x\u3000y", "e\u0301-\u0301x", "👩‍💻👍🏽🇺🇸🇩🇪"]
);
// No currency, percent or degree signs, and one dictionary script a string: icu_segmenter breaks both otherwise.
const SCRIPTS = ["\u00a0", "\u00a0", "ぁ", "ゃ", "ッ", "ー", "々", "ゝ", "・", "、", "。", "「", "」", "日", "あ", "カ", "한", "글", "é", "µ", "→", "…", "—", "\u2013", "\u2010", "«", "»", "“", "”", "½", "א", "ب", "-", "|", "/", "(", ")", "!", "?", "1", "x", " "];
const DICTIONARY = [["ไทย", "ประเทศ"], ["ພາສາ", "ລາວ"], ["ភាសា", "ខ្មែរ"], ["မြန်မာ", "ဘာသာ"]];
const scripts = mulberry32(1404);
strings.push(
  ...range(400, (i) => {
    const words = [...SCRIPTS, ...DICTIONARY[i % 4], ...DICTIONARY[i % 4]];
    return `x${range(3 + Math.floor(scripts() * 10), () => words[Math.floor(scripts() * words.length)]).join("")}x`;
  })
);
write("line-break", "pairs", {
  geometry: "real",
  steps: [
    breaks(
      "pairs",
      String.raw`Array.from({ length: 94 }, (_, i) => Array.from({ length: 94 }, (_, j) => (breaks("x" + String.fromCharCode(33 + i, 33 + j) + "x").includes(2) ? "1" : "0")).join(""))`
    )
  ]
});
write("line-break", "strings", {
  geometry: "real",
  steps: strings.map((text, i) => breaks(`string ${i}`, `[${JSON.stringify(text)}, breaks(${JSON.stringify(text)})]`))
});

// The arrow keys in the number inputs upstream sets `step` on.
const STEPS = ["", "1", "0.1", "0.01", "0.001", "10", "1e-7", "1e+21", "2.5", "any", "0", "-1", "abc"];
const NUMBERS = ["0", "10", "0.25", "-0.05", "3.14159", "", "1e21", "-1e21", "0.30000000000000004", "123456789.123", "1e-10", "9007199254740993", "-7"];
const keys = (...names) => names.map((name) => ["key", name]);
const input = (markup, ops) => ({ markup, ops });
const RANGE_MIN = '<input class="range-min" type="number" title="Minimum histogram value" />';
const numberKeys = [
  ...STEPS.map((step) =>
    input(RANGE_MIN, [
      ["set", "step", step],
      ...NUMBERS.flatMap((n) => [["set", "value", n], ...keys("ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown", "ArrowDown")])
    ])
  ),
  input('<input type="number" value="5">', keys("ArrowUp", "ArrowDown", "ArrowDown")),
  input('<input type="number" value="5.5" step="2">', [...keys("ArrowUp"), ["set", "value", "6"], ...keys("ArrowUp"), ["set", "value", "6"], ...keys("ArrowDown")]),
  input('<input type="number" value="x" step="0.5">', [["set", "value", "0.7"], ...keys("ArrowDown", "ArrowUp")]),
  input(RANGE_MIN, [["char", "1"], ["char", "2"], ...keys("ArrowUp"), ["char", "5"], ["blur"], ["focus"], ...keys("ArrowDown"), ["blur"]]),
  input(RANGE_MIN, [["char", "1"], ["char", "e"], ...keys("ArrowUp")]),
  input(RANGE_MIN, ["shift", "alt", "ctrl"].flatMap((m) => ["ArrowUp", "ArrowDown"].map((k) => ["key", k, 0, [m]])))
];
fs.writeFileSync(
  path.join(ROOT, "parity/specs/form/number-keys.entry.json"),
  JSON.stringify({ entry: "selectKeys", fixture: { steps: numberKeys } }, null, 1) + "\n"
);
