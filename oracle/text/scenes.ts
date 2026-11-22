// DOM text scenes for asrs-ui's Text element: every font size, weight and style that
// www/global.css and www/hub.css give text (plus the UA button font), and the layout features
// those rules use: line-height, text-align, direction: rtl, white-space and text-overflow.

export interface TextBox {
  id: string;
  x: number;
  y: number;
  width: number | null;
  height: number | null;
  font: string;
  lineHeight: string;
  align: string;
  direction: string;
  whiteSpace: string;
  overflow: string;
  color: string;
  opacity: number;
  text: string;
  /** Set for fonts from global.css and hub.css, whose box sizes are gated at 0.05 px. */
  exact: boolean;
}

export interface TextScene {
  name: string;
  width: number;
  height: number;
  dark: boolean;
  background: [number, number, number];
  boxes: TextBox[];
  patches: { x: number; y: number; color: string }[];
}

const SYSTEM = "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
const COURIER = "Courier, monospace";

/** [id, font shorthand, the rule that uses it]; user agent fonts are not size-gated. */
export const FONTS: [string, string, string][] = [
  ["16px", `16px ${SYSTEM}`, "body"],
  ["16px-600", `600 16px ${SYSTEM}`, "div.title-bar-text"],
  ["16px-bold", `bold 16px ${SYSTEM}`, "div.source-list div.title"],
  ["16px-italic", `italic 16px ${SYSTEM}`, "div.tab-centered"],
  ["15px", `15px ${SYSTEM}`, "div.field-item-label"],
  ["15px-italic", `italic 15px ${SYSTEM}`, "div.field-item-label.full-key"],
  ["14px", `14px ${SYSTEM}`, "div.source-list div.key-container"],
  ["14px-bold", `bold 14px ${SYSTEM}`, "div.source-list div.type-name"],
  ["13.33px", `13.333333px ${SYSTEM}`, "button (user agent)"],
  ["13px", `13px ${SYSTEM}`, "input.side-bar-search"],
  ["12px", `12px ${SYSTEM}`, "div.search-results-item"],
  ["12px-bold", `bold 12px ${SYSTEM}`, "div.side-bar-title"],
  ["11px-bold", `bold 11px ${SYSTEM}`, "hub.css:1433"],
  ["15px-courier", `15px ${COURIER}`, "div.field-value"],
  ["16px-courier-bold", `bold 16px ${COURIER}`, "hub.css:291"]
];

const SAMPLES = [
  "Robot/Drive/LeftVelocity",
  "Teleoperated 0123456789 +4.37s",
  "AVWAY Tij fi Wg",
  "NT:/SmartDashboard/Auto Chooser",
  "No data for “Drive” — 3 values"
];

function box(partial: Partial<TextBox> & Pick<TextBox, "id" | "x" | "y" | "font" | "text" | "color">): TextBox {
  return {
    width: null,
    height: null,
    lineHeight: "normal",
    align: "left",
    direction: "ltr",
    whiteSpace: "nowrap",
    overflow: "visible",
    opacity: 1,
    exact: true,
    ...partial
  };
}

function styleScene(dark: boolean): TextScene {
  const color = dark ? "#eeeeee" : "#222222";
  const boxes: TextBox[] = [];
  let y = 8;
  FONTS.forEach(([id, font, rule], i) => {
    const size = Number(/([\d.]+)px/.exec(font)![1]);
    const step = Math.ceil(size * 1.4);
    SAMPLES.forEach((text, j) => {
      const x = j % 2 === 0 ? 10 : 400.3 + (i % 4) * 0.25;
      const exact = !rule.endsWith("(user agent)");
      boxes.push(box({ id: `${id}/${j}`, x: j === 4 ? 790 : x, y: y + (j >= 2 && j < 4 ? step : 0), font, text, color, exact }));
    });
    y += step * 2 + 4;
  });
  const extras: [string, Partial<TextBox>][] = [
    ["opacity-half", { font: `bold 12px ${SYSTEM}`, opacity: 0.5, text: "Field Selection" }],
    ["grey-value", { font: `14px ${SYSTEM}`, color: "#777777", text: "X: 1.234m, Y: 5.678m, θ: 90.00°" }],
    ["link", { font: `16px ${SYSTEM}`, color: dark ? "#58a7ff" : "#0000ee", text: "Open documentation" }]
  ];
  extras.forEach(([id, partial], i) => {
    boxes.push(box({ id, x: 10 + i * 300, y, font: partial.font!, text: partial.text!, color, ...partial }));
  });
  y += 30;
  return finish(`text-styles-${dark ? "dark" : "light"}`, dark, 1100, y, boxes);
}

function layoutScene(dark: boolean): TextScene {
  const color = dark ? "#eeeeee" : "#222222";
  const boxes: TextBox[] = [];
  const key = "/RealOutputs/Drive/Module0/DriveVelocityRadPerSec";
  const add = (partial: Partial<TextBox> & Pick<TextBox, "id" | "x" | "y" | "font" | "text">) =>
    boxes.push(box({ color, ...partial }));
  add({ id: "title-bar", x: 10, y: 0, width: 300, height: 38, font: `600 16px ${SYSTEM}`, lineHeight: "38px", overflow: "ellipsis", text: "AdvantageScope — akit_24-03-15_12-34-56_e8.wpilog" });
  add({ id: "title-bar-short", x: 330, y: 0, width: 300, height: 38, font: `600 16px ${SYSTEM}`, lineHeight: "38px", overflow: "ellipsis", text: "AdvantageScope" });
  add({ id: "side-bar-title", x: 10, y: 50, width: 191, height: 20, font: `bold 12px ${SYSTEM}`, lineHeight: "20px", align: "center", overflow: "ellipsis", opacity: 0.5, text: "Open a log or connect to view data" });
  add({ id: "side-bar-title-fits", x: 220, y: 50, width: 191, height: 20, font: `bold 12px ${SYSTEM}`, lineHeight: "20px", align: "center", overflow: "ellipsis", opacity: 0.5, text: "12 fields, 2m runtime" });
  add({ id: "search-result", x: 430, y: 50, width: 180, height: 25, font: `12px ${SYSTEM}`, lineHeight: "25px", overflow: "ellipsis", text: "/AdvantageKit/RealOutputs/Odometry/Robot" });
  add({ id: "source-title", x: 10, y: 90, width: 233, height: 30, font: `bold 16px ${SYSTEM}`, lineHeight: "30px", align: "center", overflow: "ellipsis", text: "Left Axis — Velocity and Position" });
  add({ id: "source-title-fits", x: 260, y: 90, width: 233, height: 30, font: `bold 16px ${SYSTEM}`, lineHeight: "30px", align: "center", overflow: "ellipsis", text: "Poses" });
  add({ id: "type-name", x: 510, y: 90, width: 120, height: 30, font: `bold 14px ${SYSTEM}`, lineHeight: "30px", overflow: "ellipsis", text: "Ghost (Zebra Translation)" });
  add({ id: "key-rtl", x: 10, y: 130, width: 190, height: 30, font: `14px ${SYSTEM}`, lineHeight: "30px", align: "right", direction: "rtl", overflow: "ellipsis", text: key });
  add({ id: "key-rtl-fits", x: 220, y: 130, width: 190, height: 30, font: `14px ${SYSTEM}`, lineHeight: "30px", align: "right", direction: "rtl", overflow: "ellipsis", text: "/Drive/Pose" });
  add({ id: "value", x: 430, y: 130, width: 200, height: 15, font: `14px ${SYSTEM}`, lineHeight: "15px", color: "#777777", overflow: "ellipsis", text: "X: 1.234m, Y: 5.678m, θ: 90.00°, extra precision" });
  add({ id: "field-label-clip", x: 10, y: 170, width: 140, height: 20, font: `15px ${SYSTEM}`, overflow: "clip", text: "DriveVelocityRadPerSec" });
  add({ id: "field-value", x: 170, y: 170, width: 120, height: 30, font: `15px ${COURIER}`, lineHeight: "30px", align: "right", text: "-12.345" });
  add({ id: "badge", x: 310, y: 170, width: null, height: null, font: `bold 11px ${SYSTEM}`, lineHeight: "1", text: "A1" });
  add({ id: "setting-block", x: 340, y: 170, width: 151, height: 40, font: `16px ${SYSTEM}`, align: "center", overflow: "ellipsis", text: "Field Relative Rotation" });
  add({ id: "center-odd", x: 510, y: 170, width: 101, height: 20, font: `12px ${SYSTEM}`, align: "center", text: "Center me" });
  add({ id: "tab-centered", x: 10, y: 220, width: 260, height: null, font: `italic 16px ${SYSTEM}`, align: "center", whiteSpace: "normal", text: "Drag a field here to begin, or open the documentation for this tab type." });
  add({ id: "paragraph", x: 290, y: 220, width: 330, height: null, font: `14px ${SYSTEM}`, whiteSpace: "normal", text: "This field contains values that were logged as a sub-key of a struct-encoded value. Values are read-only and match the robot-side representation exactly." });
  add({ id: "pre-lines", x: 640, y: 50, width: null, height: null, font: `12px ${COURIER}`, whiteSpace: "pre", text: "Line one\n  indented line\nLast" });
  return finish(`text-layout-${dark ? "dark" : "light"}`, dark, 760, 330, boxes);
}

/** Upstream's tab titles (TabType.ts): each tab type's emoji, then its name. */
const TAB_TITLES = [
  "📖 ",
  "📉 Line Graph",
  "🗺 2D Field",
  "👀 3D Field",
  "🔢 Table",
  "💬 Console",
  "📊 Statistics",
  "🎬 Video",
  "🎮 Joysticks",
  "🦀 Swerve",
  "⚙️ Mechanism",
  "📍 Points",
  "🔍 Metadata"
];

function emojiScene(dark: boolean): TextScene {
  const color = dark ? "#eeeeee" : "#222222";
  const boxes: TextBox[] = [];
  const fonts: [string, string][] = [
    ["16px", `16px ${SYSTEM}`],
    ["12px", `12px ${SYSTEM}`],
    ["14px-bold", `bold 14px ${SYSTEM}`],
    ["16px-italic", `italic 16px ${SYSTEM}`],
    ["15px-courier", `15px ${COURIER}`]
  ];
  fonts.forEach(([id, font], i) => {
    TAB_TITLES.forEach((text, j) => {
      const x = 10 + (j % 4) * 190 + (i % 4) * 0.25 + (j % 3) * 0.125;
      const y = 8 + i * 110 + Math.floor(j / 4) * 26;
      boxes.push(box({ id: `${id}/${j}`, x, y, font, text, color }));
    });
  });
  const y = 8 + fonts.length * 110;
  boxes.push(box({ id: "ellipsis", x: 10, y, width: 70, height: 20, font: `16px ${SYSTEM}`, overflow: "ellipsis", text: "📉 Line Graph" }));
  boxes.push(box({ id: "ellipsis-emoji", x: 100, y, width: 12, height: 20, font: `16px ${SYSTEM}`, overflow: "ellipsis", text: "📖📉🗺" }));
  boxes.push(box({ id: "wrap", x: 200, y, width: 160, height: null, font: `14px ${SYSTEM}`, whiteSpace: "normal", text: "Drag 📉 here, or 🗺 and 👀 tabs 🎮🦀 too" }));
  boxes.push(box({ id: "centered", x: 400, y, width: 150, height: 20, font: `16px ${SYSTEM}`, align: "center", text: "🔢🔢 Table" }));
  boxes.push(box({ id: "opacity", x: 580, y, font: `16px ${SYSTEM}`, opacity: 0.5, text: "💬 Console" }));
  return finish(`text-emoji-${dark ? "dark" : "light"}`, dark, 780, y + 70, boxes.map((b) => ({ ...b, color })));
}

function finish(name: string, dark: boolean, width: number, height: number, boxes: TextBox[]): TextScene {
  const patches = ["#2b66a2", "#0047ce", "#af2437", "#808080"].map((color, i) => ({ x: 10 + i * 30, y: height, color }));
  return {
    name,
    width,
    height: height + 30,
    dark,
    background: dark ? [0x22, 0x22, 0x22] : [0xff, 0xff, 0xff],
    boxes,
    patches
  };
}

export function textScenes(): TextScene[] {
  return [
    styleScene(false),
    styleScene(true),
    layoutScene(false),
    layoutScene(true),
    emojiScene(false),
    emojiScene(true)
  ];
}

function escape(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function boxStyle(b: TextBox): string {
  const parts = [
    "position:absolute",
    `left:${b.x}px`,
    `top:${b.y}px`,
    `font:${b.font}`,
    `line-height:${b.lineHeight}`,
    `text-align:${b.align}`,
    `direction:${b.direction}`,
    `white-space:${b.whiteSpace}`,
    `color:${b.color}`
  ];
  if (b.width !== null) parts.push(`width:${b.width}px`);
  if (b.height !== null) parts.push(`height:${b.height}px`);
  if (b.overflow !== "visible") parts.push("overflow:hidden", `text-overflow:${b.overflow}`);
  if (b.opacity !== 1) parts.push(`opacity:${b.opacity}`);
  return parts.join(";");
}

export function sceneHtml(scene: TextScene): string {
  const [r, g, b] = scene.background;
  const background = `rgb(${r},${g},${b})`;
  const boxes = scene.boxes.map((b) => `<div data-box="${b.id}" style="${boxStyle(b)}">${escape(b.text)}</div>`);
  const patches = scene.patches.map(
    (p) => `<div data-patch="${p.color}" style="position:absolute;left:${p.x}px;top:${p.y}px;width:20px;height:20px;background:${p.color}"></div>`
  );
  return (
    '<!doctype html><html><head><meta charset="utf-8"><style>' +
    `html,body{margin:0;overflow:hidden;background:${background}}body{font-family:${SYSTEM};user-select:none}` +
    "</style></head><body>" +
    boxes.join("") +
    patches.join("") +
    "</body></html>"
  );
}

/** Runs in the page: each box's border box and the client rects of its text, one per line fragment. */
export const BOXES_SCRIPT = `(() => {
  const boxes = Array.from(document.querySelectorAll("[data-box]")).map((el) => {
    const r = el.getBoundingClientRect();
    const lines = [];
    const text = el.firstChild;
    if (text !== null && text.nodeType === Node.TEXT_NODE) {
      const range = document.createRange();
      range.selectNodeContents(text);
      for (const q of Array.from(range.getClientRects())) lines.push([q.x, q.y, q.width, q.height]);
    }
    return { id: el.dataset.box, rect: [r.x, r.y, r.width, r.height], lines };
  });
  const patches = Array.from(document.querySelectorAll("[data-patch]")).map((el) => {
    const r = el.getBoundingClientRect();
    return { color: el.dataset.patch, rect: [r.x, r.y, r.width, r.height] };
  });
  return { devicePixelRatio: window.devicePixelRatio, boxes, patches };
})()`;
