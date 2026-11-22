import * as fs from "node:fs";
import * as path from "node:path";
import { pathToFileURL } from "node:url";

export type ButtonState = "normal" | "blurred" | "disabled";

export interface UiButton {
  x: number;
  y: number;
  size: number;
  icon: string;
  state: ButtonState;
  fit?: { max: number; scale: number };
}

export interface UiProbe {
  x: number;
  y: number;
  size: number;
  chain: string;
  input: string;
  opacity: number;
}

export type ControlKind = "text" | "number" | "search" | "checkbox" | "segmented" | "range" | "progress";

export interface UiSegment {
  icon: string | null;
  label: string | null;
}

export interface UiControl {
  kind: ControlKind;
  preset: string;
  x: number;
  y: number;
  width: number;
  height: number;
  value: string | null;
  placeholder: string | null;
  checked: boolean;
  min: string | null;
  max: string | null;
  step: string | null;
  segments: UiSegment[];
  selected: number | null;
}

export interface UiScene {
  name: string;
  width: number;
  height: number;
  dark: boolean;
  background: [number, number, number];
  css: string[];
  buttons: UiButton[];
  probes: UiProbe[];
  patches: { x: number; y: number; color: string }[];
  controls: UiControl[];
  pointer: { x: number; y: number; down: boolean; selector: string } | null;
}

const FILTER_SOURCES = ["www/global.css", "www/hub.css", "www/renderers.css"];
export const PROBE_INPUTS = ["#000000", "#ffffff", "#2b66a2", "#808080", "#ff5900"];
const BUTTON = 28;
const PITCH = 34;
const COLUMNS = 26;

export function upstreamFilterChains(upstream: string): string[] {
  const chains: string[] = [];
  for (const file of FILTER_SOURCES) {
    const css = fs.readFileSync(path.join(upstream, file), "utf8");
    for (const m of css.matchAll(/(?:^|[;{\s])filter:\s*([^;]+);/g)) {
      const chain = m[1].trim().replace(/\s+/g, " ");
      if (!chains.includes(chain)) chains.push(chain);
    }
  }
  return chains;
}

export function symbolNames(upstream: string): string[] {
  const dir = path.join(upstream, "www/symbols");
  const top = fs.readdirSync(dir).filter((f) => f.endsWith(".svg"));
  const nested = fs
    .readdirSync(path.join(dir, "sourceList"))
    .filter((f) => f.endsWith(".svg"))
    .map((f) => `sourceList/${f}`);
  return [...top.sort(), ...nested.sort()];
}

function iconScene(upstream: string, dark: boolean): UiScene {
  const icons = symbolNames(upstream);
  const buttons: UiButton[] = [];
  let y = 10;
  for (const state of ["normal", "blurred", "disabled"] as ButtonState[]) {
    icons.forEach((icon, i) => {
      buttons.push({ x: 10 + (i % COLUMNS) * PITCH, y: y + Math.floor(i / COLUMNS) * PITCH, size: BUTTON, icon, state });
    });
    y += Math.ceil(icons.length / COLUMNS) * PITCH + 8;
  }
  const odd = ["add.svg", "play.svg", "tbalamp.svg", "fastforwardcircle.svg", "sourceList/360.svg"];
  [20, 25, 31, 40].forEach((size, row) => {
    odd.forEach((icon, i) => {
      buttons.push({ x: 10 + i * 48 + row, y: y + (i % 2), size, icon, state: "normal" });
    });
    y += size + 6;
  });
  const fits = [
    { size: 28, fit: { max: 1, scale: 0.9 } },
    { size: 40, fit: { max: 0.5, scale: 1.2 } }
  ];
  fits.forEach(({ size, fit }) => {
    odd.forEach((icon, i) => {
      buttons.push({ x: 10 + i * 48 + (i % 2), y, size, icon, state: "normal", fit });
    });
    y += size + 6;
  });
  const probes: UiProbe[] = [];
  y += 6;
  upstreamFilterChains(upstream).forEach((chain, row) => {
    const inputs: [string, number][] = [...PROBE_INPUTS.map((c): [string, number] => [c, 1]), ["#000000", 0.85]];
    inputs.forEach(([input, opacity], i) => {
      probes.push({ x: 10 + (row % 3) * 300 + i * 20, y: y + Math.floor(row / 3) * 20, size: 12, chain, input, opacity });
    });
  });
  y += Math.ceil(upstreamFilterChains(upstream).length / 3) * 20 + 10;
  const patches = ["#2b66a2", "#0047ce", "#af2437", "#808080"].map((color, i) => ({ x: 10 + i * 30, y, color }));
  return {
    name: `ui-icons-${dark ? "dark" : "light"}`,
    width: 10 + COLUMNS * PITCH + 4,
    height: y + 30,
    dark,
    background: dark ? [0x22, 0x22, 0x22] : [0xff, 0xff, 0xff],
    css: [],
    buttons,
    probes,
    patches,
    controls: [],
    pointer: null
  };
}

function pointerScene(dark: boolean, down: boolean): UiScene {
  return {
    name: `ui-${down ? "active" : "hover"}-${dark ? "dark" : "light"}`,
    width: 120,
    height: 60,
    dark,
    background: dark ? [0x22, 0x22, 0x22] : [0xff, 0xff, 0xff],
    buttons: [
      { x: 10, y: 10, size: BUTTON, icon: "play.svg", state: "normal" },
      { x: 50, y: 10, size: BUTTON, icon: "pause.svg", state: "normal" }
    ],
    css: [],
    probes: [],
    patches: [],
    controls: [],
    pointer: { x: 20, y: 30, down, selector: "button" }
  };
}

function control(kind: ControlKind, preset: string, x: number, y: number, width: number, height: number, extra: Partial<UiControl> = {}): UiControl {
  return {
    kind,
    preset,
    x,
    y,
    width,
    height,
    value: null,
    placeholder: null,
    checked: false,
    min: null,
    max: null,
    step: null,
    segments: [],
    selected: null,
    ...extra
  };
}

const SEGMENT_ICONS: UiSegment[] = [
  { icon: "rotateleft.svg", label: null },
  { icon: "rotateright.svg", label: null }
];
const SEGMENT_LABELS: UiSegment[] = ["30", "27", "24"].map((label) => ({ icon: null, label }));

function hubInputScene(dark: boolean): UiScene {
  const controls = [
    control("search", "sidebar-search", 0, 0, 300, 39, { placeholder: "Search" }),
    control("search", "sidebar-search", 0, 40, 300, 39, { placeholder: "Search", value: "Drive/Pose" }),
    control("search", "sidebar-search", 0, 80, 300, 39, { placeholder: "Search", value: "/RealOutputs/Odometry/Robot with a long key that overflows the box" }),
    control("number", "field-value", 0, 130, 300, 30, { placeholder: "12.345" }),
    control("number", "field-value", 0, 160, 300, 30, { placeholder: "12.345", value: "-0.5" }),
    control("number", "setting-block", 0, 200, 200, 40, { value: "5" }),
    control("number", "setting-block", 0, 245, 200, 40, { value: "0.65" }),
    control("segmented", "split", 320, 0, 200, 40, { segments: SEGMENT_ICONS, selected: null }),
    control("segmented", "split", 320, 45, 200, 40, { segments: SEGMENT_LABELS, selected: 1 }),
    control("segmented", "split", 320, 90, 200, 60, { segments: SEGMENT_LABELS, selected: 0 }),
    control("range", "video-timeline", 320, 160, 260, 35, { min: "1", max: "300", step: "1", value: "1" }),
    control("range", "video-timeline", 320, 200, 260, 35, { min: "1", max: "300", step: "1", value: "60" }),
    control("range", "video-timeline", 320, 240, 260, 35, { min: "1", max: "300", step: "1", value: "300" }),
    control("search", "console-filter", 0, 300, 400, 30, { placeholder: "Filter" }),
    control("search", "console-filter", 0, 335, 400, 30, { placeholder: "Filter", value: "warning" }),
    control("number", "table-jump", 420, 300, 175, 30, { placeholder: "0.000" }),
    control("number", "table-jump", 420, 335, 175, 30, { placeholder: "0.000", value: "12.5" })
  ];
  return inputScene(`ui-inputs-hub-${dark ? "dark" : "light"}`, 600, 375, dark, ["www/hub.css", "www/renderers.css"], controls);
}

function promptInputScene(dark: boolean): UiScene {
  const controls = [
    control("number", "prompt", 10, 10, 200, 21),
    control("number", "prompt", 10, 40, 200, 21, { value: "12.5" }),
    control("text", "prompt", 10, 70, 200, 21, { value: "Line Graph" }),
    control("text", "prompt", 10, 100, 200, 21, { value: "/U/logs", placeholder: "/U/logs" }),
    control("text", "prompt", 10, 130, 200, 21, { placeholder: "/U/logs" })
  ];
  return inputScene(`ui-inputs-prompts-${dark ? "dark" : "light"}`, 230, 160, dark, ["www/prompts.css"], controls);
}

function downloadInputScene(dark: boolean): UiScene {
  const controls = [0, 0.004, 0.4, 1].map((v, i) => control("progress", "download", 0, i * 50, 300, 45, { max: "1", min: "0", value: String(v) }));
  return inputScene(`ui-inputs-download-${dark ? "dark" : "light"}`, 300, 200, dark, ["www/download.css"], controls);
}

function betaInputScene(dark: boolean): UiScene {
  const controls = [false, true].map((checked, i) => control("checkbox", "beta", 10 + i * 40, 10, 30, 30, { checked }));
  return inputScene(`ui-inputs-beta-${dark ? "dark" : "light"}`, 100, 50, dark, ["www/betaWelcome.css"], controls);
}

function inputScene(name: string, width: number, height: number, dark: boolean, css: string[], controls: UiControl[]): UiScene {
  return {
    name,
    width,
    height,
    dark,
    background: dark ? [0x22, 0x22, 0x22] : [0xff, 0xff, 0xff],
    css,
    buttons: [],
    probes: [],
    patches: [],
    controls,
    pointer: null
  };
}

function hoverScene(dark: boolean): UiScene {
  const scene = inputScene(`ui-inputs-hover-${dark ? "dark" : "light"}`, 220, 50, dark, ["www/hub.css"], [
    control("segmented", "split", 0, 0, 200, 40, { segments: SEGMENT_LABELS, selected: 0 })
  ]);
  scene.pointer = { x: 100, y: 20, down: false, selector: "div.split > div" };
  return scene;
}

function searchHoverScene(dark: boolean): UiScene {
  const scene = inputScene(`ui-inputs-search-hover-${dark ? "dark" : "light"}`, 420, 80, dark, ["www/hub.css", "www/renderers.css"], [
    control("search", "sidebar-search", 0, 0, 300, 39, { placeholder: "Search", value: "Drive/Pose" }),
    control("search", "console-filter", 0, 45, 400, 30, { placeholder: "Filter", value: "warning" })
  ]);
  scene.pointer = { x: 150, y: 20, down: false, selector: "input.side-bar-search" };
  return scene;
}

export function uiScenes(upstream: string): UiScene[] {
  return [false, true].flatMap((dark) => [
    iconScene(upstream, dark),
    pointerScene(dark, false),
    pointerScene(dark, true),
    hubInputScene(dark),
    promptInputScene(dark),
    downloadInputScene(dark),
    betaInputScene(dark),
    hoverScene(dark),
    searchHoverScene(dark)
  ]);
}

export function probeSvg(probe: UiProbe): string {
  const s = probe.size;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${s}" height="${s}">` +
    `<rect width="${s}" height="${s}" fill="${probe.input}" fill-opacity="${probe.opacity}"/></svg>`
  );
}

function attr(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
}

function imgStyle(btn: UiButton): string {
  if (btn.fit === undefined) return "";
  const { max, scale } = btn.fit;
  return ` style="max-width:${Math.round(max * 100)}%;max-height:${Math.round(max * 100)}%;transform:translate(-50%, -50%) scale(${Math.round(scale * 100)}%)"`;
}

function esc(value: string): string {
  return attr(value).replace(/</g, "&lt;");
}

function valueAttrs(c: UiControl): string {
  const parts: string[] = [];
  for (const key of ["min", "max", "step", "value", "placeholder"] as const) {
    const v = c[key];
    if (v !== null) parts.push(` ${key}="${esc(v)}"`);
  }
  if (c.checked) parts.push(" checked");
  return parts.join("");
}

function controlMarkup(c: UiControl, href: (rel: string) => string): string {
  const a = valueAttrs(c);
  const tag = ` data-control`;
  switch (c.preset) {
    case "sidebar-search":
      return `<div class="side-bar-search-container"><input${tag} class="side-bar-search" type="search"${a}></div>`;
    case "field-value":
      return `<div class="field-value"><input${tag} type="number"${a}></div>`;
    case "setting-block":
      return `<div class="setting-blocks"><div class="input"><input${tag} type="number"${a}></div></div>`;
    case "split": {
      const segments = c.segments
        .map((s, i) => {
          const cls = i === c.selected ? ' class="selected"' : "";
          const body = s.icon !== null ? `<img class="invertable" src="${href(`www/symbols/${s.icon}`)}">` : `<span>${esc(s.label ?? "")}</span>`;
          return `<div tabindex="0"${cls}>${body}</div>`;
        })
        .join("");
      return `<div class="setting-blocks"><div class="input" style="height:${c.height}px"><div${tag} class="split split-${c.segments.length}">${segments}</div></div></div>`;
    }
    case "video-timeline":
      return `<div class="video-timeline-container" style="left:0px;right:0px"><input${tag} class="video-timeline-slider" type="range"${a}></div>`;
    case "console-filter":
      return `<table class="console-table" style="width:100%"><thead><tr><th style="height:${c.height}px"></th><th style="position:relative"><input${tag} class="filter" type="search"${a}></th></tr></thead></table>`;
    case "table-jump":
      return `<table class="data-table"><thead><tr><th style="height:${c.height}px"><input${tag} class="data-table-jump-input" type="number"${a}></th></tr></thead></table>`;
    case "prompt":
      return `<input${tag} type="${c.kind}"${a}>`;
    case "download":
      return `<progress${tag}${a}></progress>`;
    case "beta":
      return `<input${tag} type="checkbox"${a}>`;
    default:
      throw new Error(`unknown control preset ${c.preset}`);
  }
}

export function uiHtml(scene: UiScene, upstream: string): string {
  const href = (rel: string) => pathToFileURL(path.join(upstream, rel)).href;
  const [r, g, b] = scene.background;
  const parts: string[] = [];
  for (const btn of scene.buttons) {
    const cls = btn.state === "blurred" ? ' class="blurred"' : "";
    const disabled = btn.state === "disabled" ? " disabled" : "";
    parts.push(
      `<button${cls}${disabled} style="position:absolute;left:${btn.x}px;top:${btn.y}px;width:${btn.size}px;height:${btn.size}px">` +
        `<img${imgStyle(btn)} src="${href(`www/symbols/${btn.icon}`)}"></button>`
    );
  }
  for (const probe of scene.probes) {
    const src = "data:image/svg+xml," + encodeURIComponent(probeSvg(probe));
    parts.push(
      `<img data-probe style="position:absolute;left:${probe.x}px;top:${probe.y}px;filter:${attr(probe.chain)}" src="${src}">`
    );
  }
  for (const p of scene.patches) {
    parts.push(`<div data-patch="${p.color}" style="position:absolute;left:${p.x}px;top:${p.y}px;width:20px;height:20px;background:${p.color}"></div>`);
  }
  for (const c of scene.controls) {
    parts.push(
      `<div class="control-host" style="position:absolute;left:${c.x}px;top:${c.y}px;width:${c.width}px;height:${c.height}px">${controlMarkup(c, href)}</div>`
    );
  }
  const sheets = ["www/global.css", ...scene.css].map((css) => `<link rel="stylesheet" href="${href(css)}">`).join("");
  return (
    '<!doctype html><html><head><meta charset="utf-8">' +
    sheets +
    `<style>html,body{margin:0;overflow:hidden;background:rgb(${r},${g},${b})}` +
    ".control-host table,.control-host th{border:none!important;background:none!important;box-shadow:none!important}</style>" +
    "</head><body>" +
    parts.join("") +
    "</body></html>"
  );
}
