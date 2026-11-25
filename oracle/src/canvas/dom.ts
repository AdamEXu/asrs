import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { LUM_LEVELS } from "./scenes";

/** Fill and background of a text box, both opaque. */
export interface TextColors {
  color: string;
  background: string;
}

export interface DomScene {
  name: string;
  width: number;
  height: number;
  html: string;
  /** `prefers-color-scheme`, where the scene's style sheets read it. */
  dark?: boolean;
  /** A Windows 11 window with the material of upstream's hub window, whose page is not opaque. */
  acrylic?: boolean;
  /** The text boxes whose text is not grey on grey. */
  colors?: Record<string, TextColors>;
}

export interface CalibrationRow {
  id: string;
  text: string;
  font: string;
  left: number;
  top: number;
}

export interface CalibrationScene extends DomScene {
  kind: "grey" | "colour";
  color: string;
  background: string;
  rows: CalibrationRow[];
}

const FAMILY = "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
const MONO = "Courier, monospace";

const STYLES: [string, string][] = [
  ["16px", `16px ${FAMILY}`],
  ["12px", `12px ${FAMILY}`],
  ["12px-600", `600 12px ${FAMILY}`],
  ["12px-bold", `bold 12px ${FAMILY}`],
  ["13px", `13px ${FAMILY}`],
  ["14px", `14px ${FAMILY}`],
  ["14px-bold", `bold 14px ${FAMILY}`],
  ["14px-900", `900 14px ${FAMILY}`],
  ["15px", `15px ${FAMILY}`],
  ["16px-bold", `bold 16px ${FAMILY}`],
  ["12px-200", `200 12px ${FAMILY}`],
  ["25px", `25px ${FAMILY}`],
  ["30px-bold", `bold 30px ${FAMILY}`],
  ["15px-mono", `15px ${MONO}`],
  ["16px-mono-bold", `bold 16px ${MONO}`]
];

const SAMPLES = [
  "Robot/Drive/LeftVelocity",
  "Teleoperated 0123456789 +4.37s",
  "AVWAY Tij fi Wg",
  "NT:/SmartDashboard/Auto Chooser"
];

function escape(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function textScene(dark: boolean): DomScene {
  const color = dark ? "#eee" : "#222";
  const background = dark ? "#222222" : "#ffffff";
  const rows: string[] = [];
  const rowColors: Record<string, TextColors> = {};
  let y = 8;
  STYLES.forEach(([id, font], i) => {
    const size = Number(/(\d+)px/.exec(font)![1]);
    SAMPLES.forEach((sample, j) => {
      const left = j % 2 === 0 ? 10 : 460.3 + (i % 4) * 0.25;
      const top = y + (j >= 2 ? Math.ceil(size * 1.4) : 0);
      rows.push(
        `<div data-row="${id}/${j}" style="position:absolute;left:${left}px;top:${top}px;font:${font};white-space:pre">${escape(sample)}</div>`
      );
    });
    y += Math.ceil(size * 1.4) * 2 + 4;
  });
  const colors = ["#2b66a2", "#af2437", "#e5b31b", "#3fa34d", dark ? "#ffffff" : "#000000"];
  colors.forEach((c, i) => {
    rowColors[`color/${i}`] = { color: c, background };
    rows.push(
      `<div data-row="color/${i}" style="position:absolute;left:${10 + i * 120}px;top:${y}px;font:13px ${FAMILY};color:${c};white-space:pre">Color ${c}</div>`
    );
  });
  y += 30;
  ["#2b66a2", "#0047ce", "#af2437", "#808080"].forEach((c, i) => {
    rows.push(`<div data-patch="${c}" style="position:absolute;left:${10 + i * 30}px;top:${y}px;width:20px;height:20px;background:${c}"></div>`);
  });
  return {
    name: `dom-text-${dark ? "dark" : "light"}`,
    width: 920,
    height: y + 30,
    colors: rowColors,
    html:
      "<!doctype html><html><head><meta charset=\"utf-8\"><style>" +
      `html,body{margin:0;overflow:hidden;background:${background}}body{font-family:${FAMILY};color:${color};user-select:none}` +
      "</style></head><body>" +
      rows.join("") +
      "</body></html>"
  };
}

interface EligibilityCase {
  id: string;
  /** Style of the element around the text. */
  outer?: string;
  /** Style of the text box. */
  style?: string;
  size?: number;
}

const ELIGIBILITY: EligibilityCase[] = [
  ...[23, 23.5, 24, 24.02, 24.5, 25, 47, 48, 48.02, 48.5, 49].map((size) => ({ id: `size-${size}`, size })),
  { id: "opaque-box", outer: "background:OPAQUE" },
  { id: "translucent-box", outer: "background:TINT" },
  { id: "translucent-text", style: "color:FADED" },
  { id: "opacity-text", style: "opacity:0.5" },
  { id: "opacity-parent", outer: "opacity:0.5" },
  { id: "opacity-parent-opaque", outer: "opacity:0.5;background:OPAQUE" },
  { id: "opacity-parent-two-boxes", outer: "opacity:0.5;border-bottom:1px solid #808080" },
  { id: "opacity-composited", outer: "opacity:0.5;will-change:opacity" },
  { id: "will-change-transform", outer: "will-change:transform" },
  { id: "will-change-transform-opaque", outer: "will-change:transform;background:OPAQUE" },
  { id: "translate-whole", outer: "transform:translate(3px,2px)" },
  { id: "translate-fraction", outer: "transform:translate(3.3px,2.4px)" },
  { id: "translate-3d", outer: "transform:translateZ(0)" },
  { id: "translate-3d-opaque", outer: "transform:translateZ(0);background:OPAQUE" },
  { id: "scaled", outer: "transform:scale(1.25);transform-origin:0 0" },
  { id: "rotated", outer: "transform:rotate(4deg);transform-origin:0 0" },
  { id: "filter", outer: "filter:brightness(1.1)" },
  { id: "backdrop-filter", outer: "backdrop-filter:blur(2px)" },
  { id: "mask", outer: "mask-image:linear-gradient(to right,#000 80%,transparent)" },
  { id: "blend-mode", outer: "mix-blend-mode:difference" },
  { id: "rounded-clip", outer: "overflow:hidden;border-radius:6px" },
  { id: "rounded-clip-opaque", outer: "overflow:hidden;border-radius:6px;background:OPAQUE" },
  { id: "scroller", outer: "overflow:auto;scrollbar-width:none;height:20px" },
  { id: "scroller-opaque", outer: "overflow:auto;scrollbar-width:none;height:20px;background:OPAQUE" },
  { id: "shadowed", style: "text-shadow:1px 1px 1px #808080" },
  { id: "isolated", outer: "isolation:isolate" },
  { id: "fixed", outer: "position:fixed" }
];

/** One text box per way a page can take LCD text away. */
function eligibilityScene(dark: boolean): DomScene {
  const background = dark ? "#222222" : "#ffffff";
  const fill: Record<string, string> = {
    OPAQUE: dark ? "#444444" : "#cccccc",
    TINT: dark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)",
    FADED: dark ? "rgba(255,255,255,0.25)" : "rgba(0,0,0,0.25)"
  };
  const css = (style = "") => style.replace(/OPAQUE|TINT|FADED/g, (name) => fill[name]);
  const big = ELIGIBILITY.filter((c) => c.size !== undefined);
  const small = ELIGIBILITY.filter((c) => c.size === undefined);
  const box = (c: EligibilityCase, left: number, top: number, width: number, height: number) =>
    `<div style="position:absolute;left:${left}px;top:${top}px;width:${width}px;height:${height}px">` +
    `<div style="${css(c.outer)}"><span data-row="${c.id}" style="display:inline-block;font:${c.size ?? 14}px ${FAMILY};white-space:pre;${css(c.style)}">${c.size === undefined ? "Auto Chooser 4.37" : "Wolf 47"}</span></div></div>`;
  const parts = big.map((c, i) => box(c, 10 + (i % 3) * 300, 10 + Math.floor(i / 3) * 72, 280, 70));
  const top = 10 + Math.ceil(big.length / 3) * 72;
  parts.push(...small.map((c, i) => box(c, 10 + (i % 3) * 300, top + Math.floor(i / 3) * 40, 280, 38)));
  return {
    name: `dom-lcd-${dark ? "dark" : "light"}`,
    width: 920,
    height: top + Math.ceil(small.length / 3) * 40 + 10,
    dark,
    html: page(background, dark ? "#eee" : "#222", parts.join(""))
  };
}

function page(background: string, color: string, body: string): string {
  return (
    "<!doctype html><html><head><meta charset=\"utf-8\"><style>" +
    `html,body{margin:0;overflow:hidden;background:${background}}body{font-family:${FAMILY};color:${color};user-select:none}` +
    `</style></head><body>${body}</body></html>`
  );
}

const FIELDS = [
  { label: "AdvantageKit", key: false },
  { label: "RealOutputs", key: false, indent: 20 },
  { label: "LeftVelocity", key: true, indent: 40, type: "Number", value: "4.37" },
  { label: "MeasuredPose", key: true, indent: 40, type: "Pose2d", value: "" },
  { label: "Timestamp", key: true, indent: 40, type: "Number", value: "152.03", generated: true },
  { label: "Enabled", key: true, indent: 20, type: "Boolean", value: "true" },
  { label: "SystemStats", key: true, indent: 20, loading: true }
];

function fieldMarkup(field: {
  label: string;
  key: boolean;
  indent?: number;
  type?: string;
  value?: string;
  generated?: boolean;
  loading?: boolean;
}): string {
  const label = ["field-item-label", ...(field.key ? ["known", "full-key"] : []), ...(field.loading ? ["loading"] : [])];
  const type = field.type === undefined ? "" : `<span class="field-item-type-label"> \u2013 ${field.type}</span>`;
  return (
    `<div class="field-item-container" style="--indent:${field.indent ?? 0}px;--value-width:80px">` +
    `<div class="field-item${field.generated ? " generated" : ""}"><div class="${label.join(" ")}"><span>${field.label}</span>${type}</div></div>` +
    `<div class="field-value">${field.value ?? ""}</div></div>`
  );
}

/** Upstream's hub page with its own style sheets, the classes it gives itself on this platform and
 *  stand-ins for what its scripts fill in. Every piece of text is a text box. */
function hubScene(upstream: string, dark: boolean): DomScene {
  const www = path.join(upstream, "www");
  const markup = fs.readFileSync(path.join(www, "hub.html"), "utf8");
  const acrylic = process.platform === "win32" && Number(os.release().split(".").pop()) >= 22621;
  const classes = {
    darwin: ["fancy-title-bar-mac", "fancy-side-bar-mac"],
    win32: ["fancy-title-bar-win", ...(acrylic ? ["fancy-side-bar-win"] : [])],
    linux: ["fancy-title-bar-linux"]
  }[process.platform as string];
  const replace = (html: string, from: string | RegExp, to: string) => {
    const out = html.replace(from, () => to);
    if (out === html) throw new Error(`hub.html has no ${from}`);
    return out;
  };
  const style = "<style>img,svg{visibility:hidden}*{transition:none!important;animation:none!important}</style>";
  const tabs = '<div class="tab tab-selected">Line Graph</div><div class="tab">Field 2D</div><div class="tab">Statistics</div>';
  const result = '<div class="search-results" style="top:520px;min-width:200px"><div class="search-results-item">/RealOutputs/LeftVelocity</div></div>';
  let html = replace(markup, /<meta\s+http-equiv="Content-Security-Policy"[^>]*>/, `<base href="${pathToFileURL(www).href}/" />`);
  html = replace(html, /<meta\s+http-equiv="X-Content-Security-Policy"[^>]*>/, style);
  html = replace(html, /<script[^>]*bundles\/hub\.js"><\/script>/, "");
  html = replace(html, "<body>", `<body class="${(classes ?? []).join(" ")}" data-text-boxes>`);
  html = replace(html, '<span id="fieldList"></span>', `<span id="fieldList">${FIELDS.map(fieldMarkup).join("")}</span>`);
  html = replace(html, '<div class="tab-bar"></div>', `<div class="tab-bar">${tabs}</div>`);
  html = replace(html, '<div class="search-results" hidden></div>', result);
  return { name: `dom-hub-${dark ? "dark" : "light"}`, width: 1100, height: 650, html, dark, acrylic };
}

export function domScenes(upstream: string): DomScene[] {
  return [false, true].flatMap((dark) => [textScene(dark), eligibilityScene(dark), hubScene(upstream, dark)]);
}

const CALIBRATION_LINES = [
  "Sphinx of black quartz, judge my vow: 0123456789 +-.,/%()",
  "Pack my box with five dozen liquor jugs! ABCDEFGHIJKLMNOPQRSTUVWXYZ",
  "jackdaws love my big sphinx of quartz [x] {y} <z> #42 @7 &~"
];

function hex(rgb: number[]): string {
  return "#" + rgb.map((v) => v.toString(16).padStart(2, "0")).join("");
}

function calibrationScene(kind: "grey" | "colour", k: number): CalibrationScene {
  const levels = kind === "grey" ? [k, k, k] : [k, (k + 3) % 8, (k + 5) % 8];
  const color = hex(levels.map((l) => LUM_LEVELS[l]));
  const background = hex(levels.map((l) => (l < 4 ? 0xff : 0x00)));
  const rows: CalibrationRow[] = [];
  let top = 8;
  for (const size of [12, 14, 16]) {
    CALIBRATION_LINES.forEach((text, i) => {
      for (let q = 0; q < 4; q++) {
        rows.push({ id: `${size}/${i}/${q}`, text, font: `${size}px ${FAMILY}`, left: 8 + q * 0.125 + i * 0.03125, top });
        top += Math.ceil(size * 1.5);
      }
    });
  }
  const body = rows.map(
    (r) => `<div data-row="${r.id}" style="position:absolute;left:${r.left}px;top:${r.top}px;font:${r.font};white-space:pre">${escape(r.text)}</div>`
  );
  return {
    name: `lcd-${kind}-${k}`,
    kind,
    width: 680,
    height: top + 8,
    color,
    background,
    rows,
    colors: Object.fromEntries(rows.map((r) => [r.id, { color, background }])),
    html: page(background, color, body.join(""))
  };
}

/** DOM text on opaque backgrounds: grey at each 3-bit luminance, and colours whose channels fall
 *  in different ones. */
export function domCalibrationScenes(): CalibrationScene[] {
  return (["grey", "colour"] as const).flatMap((kind) => LUM_LEVELS.map((_, k) => calibrationScene(kind, k)));
}
