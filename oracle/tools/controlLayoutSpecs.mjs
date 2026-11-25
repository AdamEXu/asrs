// Fixtures of the controls' layout at whole and fractional display scales, for the
// `controls_lie_where_blink_puts_them` tests of crates/asrs-app/src/views. Writes
// parity/specs/dom/control-layout/*.entry.json; edit this script, not the specs.

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SPECS = path.join(ROOT, "parity/specs/dom/control-layout");

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

const WIDTH = 1300;
const TABS = { lineGraph: 1, field2d: 2, field3d: 3, statistics: 6, joysticks: 8, swerve: 9, mechanism: 10, points: 11 };
const ALERT = "3D field not found.</br>Please check that it is installed.";
const PARTS = [
  "div.source-list", "div.source-list > div.title", "div.setting-blocks", "div.setting-blocks > div", "div.setting-blocks div.half", "div.setting-blocks div.split",
  "div.setting-blocks div.xr-button", "div.setting-blocks div.xr-button img", "div.setting-blocks input", "div.setting-blocks select", "div.setting-blocks a", "table.joystick-selector td", "table.joystick-selector select"
];
const RENDERED = ["canvas.field-3d-canvas", "div.field-3d-alert", "div.line-graph-help-container", "div.line-graph-help-row", "div.line-graph-help-row > span", "img.line-graph-help-icon"];
const MEASURE = `/*control-layout:measure*/ window.__controlLayout = () => window.__oracleQuiet(() => {
  const range = document.createRange();
  const box = (r) => [r.x, r.y, r.width, r.height];
  const lines = (el) => {
    const out = [];
    for (const node of el.childNodes) {
      if (node.nodeType !== 3 || node.data.trim() === "") continue;
      range.selectNodeContents(node);
      for (const rect of range.getClientRects()) if (rect.width > 0) out.push(box(rect));
    }
    return out;
  };
  const tab = window.tabs.tabList[window.tabs.selectedTab];
  const shown = (el) => el.getClientRects().length > 0;
  const parts = (root, selectors) => selectors.map((selector) => [selector, Array.from(root.querySelectorAll(selector)).filter(shown).map((el) => [el.className, box(el.getBoundingClientRect()), lines(el)])]).filter(([, found]) => found.length > 0);
  const content = document.querySelector("div.controls-content");
  const renderer = document.querySelector("div.renderer-content");
  const list = document.createElement("div");
  list.className = "source-list";
  const name = document.createElement("div");
  name.className = "type-name";
  name.innerText = "W".repeat(200);
  list.appendChild(name);
  document.body.appendChild(list);
  const nameWidth = name.clientWidth;
  list.remove();
  const select = document.querySelector("div.controls-content table.joystick-selector select");
  let selectWidth = null;
  if (select !== null && shown(select)) {
    const clone = select.cloneNode(true);
    clone.style.width = "auto";
    document.body.appendChild(clone);
    selectWidth = clone.getBoundingClientRect().width;
    clone.remove();
  }
  const canvas = renderer.querySelector("canvas.field-3d-canvas.fixed");
  const canvasStyle = canvas === null ? null : [canvas.style.width, canvas.style.height];
  return { type: tab.type, selectWidth, canvasStyle, window: window.innerWidth, nameWidth, controls: box(content.getBoundingClientRect()), renderer: box(renderer.getBoundingClientRect()), parts: [...parts(content, ${JSON.stringify(PARTS)}), ...parts(renderer, ${JSON.stringify(RENDERED)})] };
})`;

// The 3D field's canvas takes a fixed aspect ratio, and its alert shows, by script: the WebGL renderer is
// not part of the harness.
const FIXED = (width, height) =>
  `window.__oracleQuiet(() => { const root = document.querySelector("div.renderer-content > :not([hidden])"); const canvas = root.querySelector("canvas.field-3d-canvas"); canvas.classList.add("fixed"); canvas.parentElement.classList.add("fixed"); canvas.style.width = "${width}%"; canvas.style.height = "${height}%"; const alert = root.querySelector("div.field-3d-alert"); alert.hidden = false; alert.innerHTML = ${JSON.stringify(ALERT)}; })`;

function spec(scale, seed) {
  const random = mulberry32(seed);
  const int = (low, high) => low + Math.floor(random() * (high - low + 1));
  const steps = [{ eval: MEASURE }];
  for (const type of Object.values(TABS)) steps.push({ main: "new-tab", data: type });
  // A whole number of device px, which a window's edges are and the side bar's here stand in for.
  const device = (css) => Math.round(css * scale) / scale;
  const places = [];
  for (let i = 0; i < 24; i++) places.push([int(240, 860), int(100, 330)]);
  for (let i = 0; i < 8; i++) places.push([device(int(240, 860) + random()), device(int(100, 330) + random())]);
  for (const [tab, type] of Object.entries(TABS)) {
    const index = `window.tabs.tabList.findIndex((tab) => tab.type === ${type})`;
    steps.push({ eval: `window.tabs.setSelected(${index})` }, { advance: 50 });
    for (const [side, height] of places) {
      if (tab === "field3d") {
        const part = Math.round(random() * 1e6) / 1e4;
        steps.push({ eval: random() < 0.5 ? FIXED(100, part) : FIXED(part, 100) });
      }
      if (tab === "field2d") {
        steps.push({ eval: `document.querySelector("div.controls-content a.field-source").hidden = ${random() < 0.3}` });
      }
      steps.push(
        { eval: `window.__oracleQuiet(() => { document.documentElement.style.setProperty("--side-bar-width", "${side}px"); document.documentElement.style.setProperty("--tab-controls-height", "${height}px"); })` },
        { capture: [{ kind: "eval", expr: `/*control-layout*/ window.__controlLayout()` }], label: tab }
      );
    }
  }
  return {
    entry: "dom",
    inputs: ["../assets/minimal.json"],
    fixture: { geometry: "real", window: { width: WIDTH, height: 900, devicePixelRatio: scale }, seed: 419, assets: "../assets/minimal.json", steps }
  };
}

fs.mkdirSync(SPECS, { recursive: true });
for (const [name, scale] of [["1x", 1], ["1.25x", 1.25], ["1.5x", 1.5], ["2x", 2], ["3x", 3]]) {
  fs.writeFileSync(path.join(SPECS, `${name}.entry.json`), JSON.stringify(spec(scale, 41900 + scale * 100), null, 1) + "\n");
}
