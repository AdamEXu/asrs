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

export interface UiScene {
  name: string;
  width: number;
  height: number;
  dark: boolean;
  background: [number, number, number];
  buttons: UiButton[];
  probes: UiProbe[];
  patches: { x: number; y: number; color: string }[];
  pointer: { x: number; y: number; down: boolean } | null;
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
    buttons,
    probes,
    patches,
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
    probes: [],
    patches: [],
    pointer: { x: 20, y: 30, down }
  };
}

export function uiScenes(upstream: string): UiScene[] {
  return [false, true].flatMap((dark) => [iconScene(upstream, dark), pointerScene(dark, false), pointerScene(dark, true)]);
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
  return (
    '<!doctype html><html><head><meta charset="utf-8">' +
    `<link rel="stylesheet" href="${href("www/global.css")}">` +
    `<style>html,body{margin:0;overflow:hidden;background:rgb(${r},${g},${b})}</style>` +
    "</head><body>" +
    parts.join("") +
    "</body></html>"
  );
}
