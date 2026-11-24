import field2dTraces from "./field2dTraces.json";
import histogramTraces from "./histogramTraces.json";
import lineGraphTraces from "./lineGraphTraces.json";
import { encodePng } from "./png";
import r07Traces from "./r07Traces.json";
import timelineTraces from "./timelineTraces.json";

export type Cmd = unknown[];

export type Gate = "pixel" | "text" | "calibration";

export interface Scene {
  name: string;
  group: string;
  gate: Gate;
  width: number;
  height: number;
  dpr: number;
  background: [number, number, number];
  cmds: Cmd[];
}

export const SYSTEM_FONT = "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";

export const LUM_LEVELS = [0x00, 0x24, 0x49, 0x6d, 0x92, 0xb6, 0xdb, 0xff];

const LIGHT: [number, number, number] = [0xff, 0xff, 0xff];
const DARK: [number, number, number] = [0x22, 0x22, 0x22];

class Rec {
  cmds: Cmd[] = [];
  set(prop: string, value: unknown) {
    this.cmds.push(["=", prop, value]);
  }
  call(name: string, ...args: unknown[]) {
    this.cmds.push([name, ...args]);
  }
}

function scene(
  name: string,
  group: string,
  gate: Gate,
  size: [number, number],
  dpr: number,
  background: [number, number, number],
  cmds: Cmd[]
): Scene {
  return { name, group, gate, width: size[0], height: size[1], dpr, background, cmds };
}

function scaleValue(v: number, from: [number, number], to: [number, number]) {
  return ((v - from[0]) / (from[1] - from[0])) * (to[1] - to[0]) + to[0];
}

function calibration(k: number): Scene {
  const level = LUM_LEVELS[k].toString(16).padStart(2, "0");
  const r = new Rec();
  r.call("scale", 2, 2);
  r.set("fillStyle", `#${level}${level}${level}`);
  r.set("textBaseline", "alphabetic");
  const lines = [
    "Sphinx of black quartz, judge my vow: 0123456789 +-.,/%()",
    "Pack my box with five dozen liquor jugs! ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "jackdaws love my big sphinx of quartz [x] {y} <z> #42 @7 &~"
  ];
  let y = 16;
  for (const size of [12, 14, 16]) {
    r.set("font", `${size}px ${SYSTEM_FONT}`);
    lines.forEach((line, i) => {
      for (let q = 0; q < 4; q++) {
        r.call("fillText", line, 5 + q * 0.125 + i * 0.03, y);
        y += size * 1.4;
      }
    });
  }
  return scene(`calib-${k}`, "calibration", "calibration", [520, 720], 2, [0x80, 0x80, 0x80], r.cmds);
}

function lineGraph(name: string, dpr: number, dark: boolean): Scene {
  const [width, height] = [800, 400];
  const fg = dark ? "#eee" : "#222";
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.call("clearRect", 0, 0, width, height);
  r.set("font", `12px ${SYSTEM_FONT}`);

  const graphTop = 20;
  const graphHeight = height - graphTop - 35;
  const discreteRows = 2;
  const graphHeightOpen = graphHeight - discreteRows * 20 - 5;
  const graphLeft = 25 + 40;
  const graphRight = 25 + 30;
  const graphWidth = width - graphLeft - graphRight;
  const timeRange: [number, number] = [0, 10];
  const xOf = (t: number) => scaleValue(t, timeRange, [graphLeft, graphLeft + graphWidth]);
  const yOf = (v: number) => scaleValue(v, [-1.5, 1.5], [graphTop + graphHeightOpen, graphTop]);

  r.set("globalAlpha", 1);
  r.set("textAlign", "left");
  r.set("textBaseline", "middle");
  r.set("lineWidth", 1);
  r.set("lineCap", "round");

  const stripes: [number, number, string, string][] = [
    [0, 1.5, "Disabled", "#808080"],
    [1.5, 3.25, "Autonomous", "#2b66a2"],
    [3.25, 9.6, "Teleoperated", "#2b66a2"],
    [9.6, 10, "Disabled", "#808080"]
  ];
  const topY = graphTop + graphHeight - 20;
  stripes.forEach(([s, e, text, color], i) => {
    const [sx, ex] = [xOf(s), xOf(e)];
    const toggle = i % 2 === 0;
    r.set("fillStyle", toggle ? color : "#5a8ec4");
    r.call("fillRect", sx, topY, ex - sx, 15);
    if (ex - sx > 10) {
      r.set("fillStyle", toggle ? "#ffffff" : "#000000");
      r.call("fillText", text, sx + 5, topY + 15 / 2, ex - sx - 10);
    }
  });
  const topY2 = graphTop + graphHeight - 40;
  r.call("beginPath");
  let toggle = false;
  const edges = [0, 0.7, 2.2, 2.9, 5, 6.35, 8.8, 10];
  for (let i = 0; i + 1 < edges.length; i++) {
    const [sx, ex] = [xOf(edges[i]), xOf(edges[i + 1])];
    toggle = !toggle;
    const [sy, ey] = toggle ? [topY2 + 15, topY2] : [topY2, topY2 + 15];
    r.call("moveTo", sx, sy);
    r.call("lineTo", sx, ey);
    r.call("lineTo", ex, ey);
    r.set("fillStyle", "#e5b31b");
    r.call("fillText", toggle ? "true" : "false", sx + 5, topY2 + 7.5, ex - sx - 10);
  }
  r.set("strokeStyle", "#e5b31b");
  r.call("stroke");

  const fields: [string, string, number][] = [
    ["smooth", "#2b66a2", 1],
    ["smooth", "#e48b32", 3],
    ["stepped", "#af2437", 1]
  ];
  const stepped = (t: number) => (Math.floor(t * 1.7) % 3) * 0.4 - 0.6;
  for (const [kind, color, lw] of fields) {
    r.set("strokeStyle", color);
    r.set("fillStyle", color);
    r.set("lineCap", "round");
    r.set("lineWidth", lw);
    r.call("beginPath");
    const n = 240;
    for (let i = 0; i <= n; i++) {
      const t = (i / n) * 10;
      const v =
        color === "#2b66a2"
          ? Math.sin(t * 1.3) * 1.1
          : color === "#e48b32"
            ? Math.cos(t * 0.6) * 0.8 + Math.sin(t * 4.1) * 0.15
            : stepped(t);
      const [x, y] = [xOf(t), yOf(v)];
      if (kind === "stepped" && i > 0) r.call("lineTo", x, yOf(stepped(((i - 1) / n) * 10)));
      r.call(i === 0 ? "moveTo" : "lineTo", x, y);
    }
    r.call("stroke");
  }
  r.set("fillStyle", "#3fa34d");
  r.call("beginPath");
  for (let i = 0; i < 30; i++) {
    const t = 0.2 + i * 0.33;
    const [x, y] = [xOf(t), yOf(-1.2 + Math.sin(t * 2) * 0.1)];
    r.call("moveTo", x + 3.2, y);
    r.call("arc", x, y, 3.2, 0, Math.PI * 2);
  }
  r.call("fill");

  const [zs, ze] = [xOf(6.1), xOf(7.4)];
  r.set("globalAlpha", 0.25);
  r.set("fillStyle", "yellow");
  r.call("fillRect", zs, graphTop, ze - zs, graphHeight);
  r.set("globalAlpha", 1);

  r.set("textBaseline", "top");
  r.set("textAlign", "center");
  const mark = (t: number, alpha: number) => {
    r.set("globalAlpha", alpha);
    r.set("lineWidth", 1);
    r.call("setLineDash", [5, 5]);
    r.set("strokeStyle", fg);
    r.set("fillStyle", fg);
    const x = xOf(t);
    r.call("beginPath");
    r.call("moveTo", x, graphTop);
    r.call("lineTo", x, graphTop + graphHeight);
    r.call("stroke");
    r.call("setLineDash", []);
    r.set("globalAlpha", 1);
  };
  mark(4.37, 0.35);
  mark(3.125, 1);
  const centered = (text: string, x: number, w: number, alpha: number) => {
    r.set("globalAlpha", alpha);
    r.set("strokeStyle", fg);
    r.set("fillStyle", fg);
    r.call("clearRect", x - w / 2 - 5, graphTop, w + 10, 19);
    r.call("strokeRect", x - w / 2 - 5, graphTop, w + 10, 19);
    r.call("fillText", text, x, graphTop + 5);
    r.set("globalAlpha", 1);
  };
  centered("+3.13s", xOf(3.125), 36, 1);
  centered("+4.37s", xOf(4.37), 36, 0.35);

  r.set("lineWidth", 1);
  r.set("strokeStyle", fg);
  r.call("clearRect", 0, 0, width, graphTop);
  r.call("clearRect", 0, graphTop + graphHeight, width, height - graphTop - graphHeight);
  r.call("clearRect", 0, graphTop, graphLeft, graphHeight);
  r.call("clearRect", graphLeft + graphWidth, graphTop, width - graphLeft - graphWidth, graphHeight);
  r.call("strokeRect", graphLeft, graphTop, graphWidth, graphHeight);

  r.set("lineWidth", 1);
  r.set("strokeStyle", fg);
  r.set("fillStyle", fg);
  r.set("textBaseline", "middle");
  r.set("textAlign", "right");
  ["1.5", "1", "0.5", "0", "-0.5", "-1", "-1.5"].forEach((label, i) => {
    const y = yOf(1.5 - i * 0.5);
    r.set("globalAlpha", 1);
    r.call("fillText", label, graphLeft - 15, y);
    r.call("beginPath");
    r.call("moveTo", graphLeft, y);
    r.call("lineTo", graphLeft - 5, y);
    r.call("stroke");
    r.set("globalAlpha", 0.1);
    r.call("beginPath");
    r.call("moveTo", graphLeft, y);
    r.call("lineTo", graphLeft + graphWidth, y);
    r.call("stroke");
  });
  r.set("textAlign", "left");
  ["12 m/s", "10 m/s", "8 m/s", "6 m/s", "4 m/s", "2 m/s", "0 m/s"].forEach((label, i) => {
    const y = scaleValue(12 - i * 2, [0, 12.5], [graphTop + graphHeightOpen, graphTop]);
    r.set("globalAlpha", 1);
    r.call("fillText", label, graphLeft + graphWidth + 15, y);
    r.call("beginPath");
    r.call("moveTo", graphLeft + graphWidth, y);
    r.call("lineTo", graphLeft + graphWidth + 5, y);
    r.call("stroke");
  });
  r.set("textAlign", "center");
  for (let i = 0; i <= 10; i++) {
    const x = xOf(i);
    r.set("globalAlpha", 1);
    r.call("fillText", `+${i}s`, x, graphTop + graphHeight + 15);
    r.call("beginPath");
    r.call("moveTo", x, graphTop + graphHeight);
    r.call("lineTo", x, graphTop + graphHeight + 5);
    r.call("stroke");
    r.set("globalAlpha", 0.1);
    r.call("beginPath");
    r.call("moveTo", x, graphTop);
    r.call("lineTo", x, graphTop + graphHeight);
    r.call("stroke");
  }
  return scene(name, "linegraph", "pixel", [width, height], dpr, dark ? DARK : LIGHT, r.cmds);
}

export const MEASURE_STRINGS = [
  "0", "1", "-1", "0.5", "-0.5", "1.5", "-1.5", "12.25", "-123.456", "1000000", "1e-7", "0.000123",
  "+0s", "+1s", "+10s", "+3.13s", "+4.37s", "+123.456s", "Δ+1.24s", "12 m/s", "-3.5 rad/s",
  "Disabled", "Autonomous", "Teleoperated", "true", "false", "Hello, World!", "AVWAY Tij fi",
  "Robot/Drive/LeftVelocity", "NT:/SmartDashboard/Auto Chooser/selected", "The quick brown fox jumps over the lazy dog",
  "WWWWWWWWWW", "iiiiiiiiii", "0123456789", "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz",
  "()[]{}<>!?@#$%^&*_+=|\\/:;\"',.~`", "Error: something went wrong", "x", " ", "  two  spaces  "
];

const EXTRA_MEASURE_STRINGS = [
  "", "Drag to zoom", "Click to add a field", "µs 45° ±", "café naïve", "日本語",
  "\u{1f916} robot", "Á", "\t tab", "fi fl ffi", "1.23e+21", "-0",
  "📖", "📉 Line Graph", "🗺 2D Field", "⚙️ Mechanism", "👀🔢"
];

export function measureFonts(): string[] {
  return [
    `12px ${SYSTEM_FONT}`,
    `200 12px ${SYSTEM_FONT}`,
    `200 14px ${SYSTEM_FONT}`,
    `16px ${SYSTEM_FONT}`,
    `bold 12px ${SYSTEM_FONT}`,
    "normal 12px 'Helvetica Neue', 'Helvetica', 'Arial', sans-serif",
    "normal bold 12px 'Helvetica Neue', 'Helvetica', 'Arial', sans-serif",
    "10px sans-serif",
    "italic 16px sans-serif",
    "13.5px sans-serif",
    "21.25px sans-serif"
  ];
}

export function measureStrings(): string[] {
  return [...MEASURE_STRINGS, ...EXTRA_MEASURE_STRINGS];
}

export const MEASURE_ALIGNS = ["left", "center", "right"];
export const MEASURE_BASELINES = ["alphabetic", "top", "middle", "bottom"];

function textGrid(name: string, dpr: number, dark: boolean): Scene {
  const [width, height] = [720, 460];
  const fg = dark ? "#eee" : "#222";
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.call("clearRect", 0, 0, width, height);
  r.set("font", `12px ${SYSTEM_FONT}`);
  r.set("fillStyle", fg);
  r.set("textAlign", "left");
  const baselines = ["alphabetic", "top", "middle", "bottom"];
  let y = 18;
  MEASURE_STRINGS.slice(0, 21).forEach((text, i) => {
    r.set("textBaseline", baselines[i % 4]);
    const x = 10 + (i % 3) * 0.25 + (i % 7) * 0.1;
    r.call("fillText", text, x, y + (i % 5) * 0.2);
    y += 20;
  });
  r.set("textBaseline", "middle");
  r.set("textAlign", "right");
  y = 18;
  for (const text of MEASURE_STRINGS.slice(21, 35)) {
    r.call("fillText", text, 470, y);
    y += 20;
  }
  r.set("textAlign", "center");
  ["#ff0000", "#2b66a2", "#e5b31b", "#ffffff", "#000000", "#808080"].forEach((color, i) => {
    r.set("fillStyle", color);
    r.call("fillText", "Teleoperated 12.5", 590, 18 + i * 20);
  });
  r.set("fillStyle", fg);
  r.set("globalAlpha", 0.35);
  r.call("fillText", "+4.37s alpha", 590, 140);
  r.set("globalAlpha", 1);
  r.set("textAlign", "left");
  r.call("fillText", "Teleoperated compressed", 500, 170, 60);
  r.call("fillText", "Autonomous", 500, 190, 40);
  r.set("font", `200 12px ${SYSTEM_FONT}`);
  r.call("fillText", "200 12px +1.5s 0123", 500, 220);
  r.set("font", `200 14px ${SYSTEM_FONT}`);
  r.call("fillText", "200 14px Drag to zoom", 500, 245);
  r.set("font", `16px ${SYSTEM_FONT}`);
  r.call("fillText", "16px Regular Wg", 500, 275);
  return scene(name, "text", "text", [width, height], dpr, dark ? DARK : LIGHT, r.cmds);
}

function shapes(name: string, dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.call("clearRect", 0, 0, 600, 300);
  r.set("strokeStyle", "#222");
  [1, 1.5, 2, 3, 6].forEach((lw, i) => {
    r.set("lineWidth", lw);
    ["butt", "round", "square"].forEach((cap, j) => {
      r.set("lineCap", cap);
      const x = 20 + j * 60;
      const y = 20 + i * 22 + 0.3 * j;
      r.call("beginPath");
      r.call("moveTo", x, y);
      r.call("lineTo", x + 45, y + 7);
      r.call("stroke");
    });
  });
  r.set("lineWidth", 1);
  r.set("lineCap", "butt");
  ["miter", "round", "bevel"].forEach((join, i) => {
    r.set("lineJoin", join);
    r.set("lineWidth", 5);
    const x = 220 + i * 60;
    r.call("beginPath");
    r.call("moveTo", x, 70);
    r.call("lineTo", x + 20, 20);
    r.call("lineTo", x + 40, 70);
    r.call("stroke");
  });
  r.set("lineJoin", "miter");
  r.set("lineWidth", 1);
  r.call("setLineDash", [5, 5]);
  r.call("beginPath");
  r.call("moveTo", 220, 100);
  r.call("lineTo", 400, 100);
  r.call("moveTo", 220.5, 110.5);
  r.call("lineTo", 400.5, 110.5);
  r.call("moveTo", 220, 120);
  r.call("lineTo", 400, 160);
  r.call("stroke");
  r.call("setLineDash", []);
  r.set("fillStyle", "#2b66a2");
  r.call("fillRect", 420, 20, 50.5, 30.25);
  r.set("globalAlpha", 0.4);
  r.set("fillStyle", "#e48b32");
  r.call("fillRect", 440.3, 35.7, 50, 30);
  r.set("globalAlpha", 1);
  r.call("clearRect", 450.5, 40.5, 10, 10);
  r.set("strokeStyle", "#af2437");
  r.set("lineWidth", 2);
  r.call("strokeRect", 500, 20, 60, 40);
  r.call("beginPath");
  r.call("arc", 60, 200, 40, 0, Math.PI * 2);
  r.set("fillStyle", "#3fa34d");
  r.call("fill");
  r.call("beginPath");
  r.call("arc", 160, 200, 40, 0.3, 2.5);
  r.call("stroke");
  r.call("beginPath");
  r.call("arc", 260, 200, 40, 0.3, 2.5, true);
  r.call("closePath");
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 330, 170);
  r.call("lineTo", 400, 240);
  r.call("lineTo", 340, 250);
  r.call("closePath");
  r.set("fillStyle", "#808080");
  r.call("fill");
  r.set("strokeStyle", "#222");
  r.set("lineWidth", 1);
  r.call("stroke");
  r.call("beginPath");
  for (let i = 0; i < 60; i++) {
    r.call(i === 0 ? "moveTo" : "lineTo", 420 + i * 2.5, 200 + Math.sin(i * 0.37) * 40);
  }
  r.set("lineWidth", 1);
  r.set("lineCap", "round");
  r.set("lineJoin", "round");
  r.set("strokeStyle", "#2b66a2");
  r.call("stroke");
  return scene(name, "shapes", "pixel", [600, 300], dpr, LIGHT, r.cmds);
}

function paths(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("strokeStyle", "#222");
  r.set("fillStyle", "#2b66a2");
  r.call("beginPath");
  r.call("rect", 10.5, 10.5, 60, 40);
  r.call("rect", 30, 30, 60, 40);
  r.call("fill");
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 120, 10);
  r.call("lineTo", 180, 70);
  r.call("lineTo", 120, 70);
  r.call("lineTo", 180, 10);
  r.call("closePath");
  r.set("fillStyle", "#e48b32");
  r.call("fill");
  r.call("stroke");
  const arcs: [number, number, boolean][] = [
    [0, Math.PI * 2, false],
    [0.25, 5.5, false],
    [5.5, 0.25, false],
    [0.25, 5.5, true],
    [-1, 9, false],
    [1, 1, false],
    [0, -Math.PI * 3, true]
  ];
  arcs.forEach(([start, end, ccw], i) => {
    r.call("beginPath");
    r.call("arc", 220 + i * 45, 40, 18, start, end, ccw);
    r.set("lineWidth", 1 + (i % 3));
    r.call("stroke");
  });
  r.set("lineWidth", 1);
  const ellipses: [number, number, number, number, number, boolean][] = [
    [30, 15, 0, 0, Math.PI * 2, false],
    [30, 15, 0.6, 0, Math.PI * 2, false],
    [12, 24, -0.4, 0.5, 4, false],
    [25, 10, 1.2, 0.5, 4, true],
    [8, 8, 0, 0, Math.PI * 2, false]
  ];
  ellipses.forEach(([rx, ry, rot, start, end, ccw], i) => {
    r.call("beginPath");
    r.call("ellipse", 40 + i * 75, 130, rx, ry, rot, start, end, ccw);
    r.set("fillStyle", i % 2 === 0 ? "rgba(63, 163, 77, 0.6)" : "#af2437");
    r.call("fill");
    r.call("stroke");
  });
  r.call("beginPath");
  r.call("moveTo", 400, 100);
  r.call("arc", 450, 130, 25, Math.PI, 0);
  r.call("lineTo", 500, 170);
  r.call("ellipse", 470, 170, 30, 12, 0, 0, Math.PI);
  r.call("closePath");
  r.set("lineWidth", 2);
  r.set("lineJoin", "round");
  r.call("stroke");
  r.set("lineJoin", "miter");
  r.set("lineWidth", 1);
  r.call("beginPath");
  r.call("moveTo", 10, 200);
  r.call("lineTo", 10, 200);
  r.call("moveTo", 30, 200);
  r.call("lineTo", 30.1, 260);
  r.call("lineTo", 30.2, 200);
  r.set("lineWidth", 4);
  r.call("stroke");
  r.call("beginPath");
  r.set("lineCap", "round");
  r.call("moveTo", 60, 220);
  r.call("lineTo", 60, 220);
  r.call("stroke");
  r.set("lineCap", "butt");
  r.set("lineWidth", 1);
  const widths = [0.25, 0.5, 0.75, 1, 1.25, 2.5];
  widths.forEach((lw, i) => {
    r.set("lineWidth", lw);
    r.call("beginPath");
    r.call("moveTo", 100, 200 + i * 10);
    r.call("lineTo", 300, 200 + i * 10 + i * 0.5);
    r.call("moveTo", 320 + i * 10, 200);
    r.call("lineTo", 320 + i * 10, 260);
    r.call("moveTo", 400 + i * 15, 200);
    r.call("lineTo", 420 + i * 15, 260);
    r.call("stroke");
  });
  r.set("lineWidth", 1);
  r.call("beginPath");
  r.call("moveTo", 520, 200);
  r.call("lineTo", 580, 200);
  r.call("lineTo", 520, 205);
  r.set("lineWidth", 6);
  r.set("lineJoin", "miter");
  r.call("stroke");
  return scene(`paths-${dpr}x`, "coverage", "pixel", [600, 280], dpr, LIGHT, r.cmds);
}

function transforms(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("font", `12px ${SYSTEM_FONT}`);
  r.set("fillStyle", "#2b66a2");
  r.set("strokeStyle", "#222");
  r.call("save");
  r.call("translate", 60, 60);
  for (let i = 0; i < 8; i++) {
    r.call("save");
    r.call("rotate", (i * Math.PI) / 8);
    r.set("globalAlpha", 0.3 + i * 0.08);
    r.call("fillRect", 10, -4, 40, 8);
    r.call("restore");
  }
  r.call("restore");
  r.call("save");
  r.call("translate", 180.5, 20.25);
  r.call("scale", 1.5, 0.75);
  r.call("strokeRect", 0, 0, 60, 60);
  r.call("beginPath");
  r.call("arc", 30, 30, 20, 0, Math.PI * 2);
  r.call("stroke");
  r.call("restore");
  r.call("save");
  r.call("translate", 320, 60);
  r.call("rotate", 0.3);
  r.set("lineWidth", 3);
  r.call("strokeRect", -30, -20, 60, 40);
  r.set("lineWidth", 1);
  r.set("textAlign", "center");
  r.set("textBaseline", "middle");
  r.set("fillStyle", "#222");
  r.call("fillText", "rotated", 0, 0);
  r.call("restore");
  r.call("save");
  r.call("scale", -1, 1);
  r.set("fillStyle", "#af2437");
  r.call("fillRect", -460, 30, 50, 20);
  r.call("restore");
  r.set("fillStyle", "#222");
  r.call("save");
  r.call("translate", 20.3, 140.6);
  r.set("textAlign", "left");
  r.set("textBaseline", "alphabetic");
  r.call("fillText", "translated text 0123", 0, 0);
  r.call("scale", 2, 2);
  r.call("fillText", "scaled 2x", 0, 20);
  r.call("restore");
  r.set("lineWidth", 2);
  r.call("save");
  r.set("strokeStyle", "#e48b32");
  r.set("lineWidth", 5);
  r.call("save");
  r.set("strokeStyle", "#3fa34d");
  r.call("restore");
  r.call("beginPath");
  r.call("moveTo", 300, 140);
  r.call("lineTo", 480, 180);
  r.call("stroke");
  r.call("restore");
  r.call("restore");
  r.call("beginPath");
  r.call("moveTo", 300, 200);
  r.call("lineTo", 480, 220);
  r.call("stroke");
  return scene(`transforms-${dpr}x`, "coverage", "pixel", [500, 240], dpr, LIGHT, r.cmds);
}

function rects(dpr: number, dark: boolean): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  const fills: [number, number, number, number][] = [
    [10, 10, 40, 20],
    [60.5, 10.5, 40, 20],
    [110.25, 10.75, 40.5, 20.25],
    [200, 30, -40, -20],
    [210, 10, 0, 20],
    [220, 10, 0.4, 20],
    [230.3, 10, 0.4, 20]
  ];
  r.set("fillStyle", dark ? "#eee" : "#222");
  for (const f of fills) r.call("fillRect", ...f);
  r.set("strokeStyle", dark ? "#eee" : "#222");
  [1, 2, 0.5, 3].forEach((lw, i) => {
    r.set("lineWidth", lw);
    r.call("strokeRect", 10 + i * 60, 50 + (i % 2) * 0.5, 45, 25);
  });
  r.set("lineWidth", 1);
  r.call("strokeRect", 260, 50, 0, 25);
  r.call("strokeRect", 270, 50, 20, 0);
  ["#2b66a2", "#e48b32", "#af2437", "#3fa34d"].forEach((color, i) => {
    r.set("globalAlpha", 0.2 + i * 0.25);
    r.set("fillStyle", color);
    r.call("fillRect", 10 + i * 25, 100, 50, 30);
  });
  r.set("globalAlpha", 1);
  r.set("fillStyle", "#2b66a2");
  r.call("fillRect", 160, 100, 120, 40);
  r.call("clearRect", 170, 105, 20, 10);
  r.call("clearRect", 200.5, 105.5, 20, 10);
  r.call("clearRect", 230.3, 105.7, 20.4, 10.2);
  r.set("globalAlpha", 0.5);
  r.call("clearRect", 255, 105, 15, 30);
  r.set("globalAlpha", 1);
  r.set("font", `12px ${SYSTEM_FONT}`);
  r.set("fillStyle", dark ? "#eee" : "#222");
  r.set("textBaseline", "top");
  r.call("fillText", "cleared after text", 10, 150);
  r.call("clearRect", 40, 152, 30, 6);
  r.set("strokeStyle", "#e5b31b");
  r.call("strokeRect", 130.5, 148.5, 60, 19);
  r.call("clearRect", 130.5, 148.5, 60, 19);
  r.call("strokeRect", 200, 148, 60, 19);
  return scene(`rects-${dark ? "dark" : "light"}-${dpr}x`, "coverage", "pixel", [300, 180], dpr, dark ? DARK : LIGHT, r.cmds);
}

function dashes(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("strokeStyle", "#222");
  const patterns: number[][] = [[5, 5], [10, 3, 2, 3], [3], [1, 1], [0, 4], [6, 0], [2.5, 2.5], [8, 4, 0, 4]];
  patterns.forEach((pattern, i) => {
    r.call("setLineDash", pattern);
    r.set("lineWidth", 1 + (i % 3));
    r.set("lineCap", ["butt", "round", "square"][i % 3]);
    r.call("beginPath");
    r.call("moveTo", 10, 15 + i * 18);
    r.call("lineTo", 290, 15 + i * 18 + (i % 2) * 0.5);
    r.call("stroke");
  });
  r.set("lineCap", "butt");
  r.set("lineWidth", 1);
  r.call("setLineDash", [5, 5]);
  r.call("beginPath");
  r.call("arc", 360, 60, 40, 0, Math.PI * 2);
  r.call("stroke");
  r.call("strokeRect", 320.5, 120.5, 80, 30);
  r.call("beginPath");
  r.call("moveTo", 320, 170);
  r.call("lineTo", 360, 160);
  r.call("lineTo", 400, 170);
  r.call("lineTo", 360, 160.2);
  r.call("stroke");
  r.call("setLineDash", []);
  r.call("beginPath");
  r.call("moveTo", 10, 165);
  r.call("lineTo", 290, 165);
  r.call("stroke");
  return scene(`dashes-${dpr}x`, "coverage", "pixel", [420, 180], dpr, LIGHT, r.cmds);
}

function colors(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  const styles = [
    "#f00", "#00ff00", "#2B66A2", "red", "yellow", "white", "black", "gray", "orange", "purple",
    "rgb(43, 102, 162)", "rgba(43, 102, 162, 0.5)", "rgba(0,0,0,1)", "rgba(0,0,0,0)", "rgba(228,139,50,0.25)",
    "hsl(120, 50%, 50%)", "#2b66a280", "transparent", "not a color", "#12345"
  ];
  styles.forEach((style, i) => {
    r.set("fillStyle", "#808080");
    r.set("fillStyle", style);
    r.call("fillRect", 10 + (i % 10) * 28, 10 + Math.floor(i / 10) * 28, 24, 24);
  });
  r.set("strokeStyle", "rgba(175, 36, 55, 0.7)");
  r.set("lineWidth", 3);
  r.call("strokeRect", 10, 70, 270, 20);
  return scene(`colors-${dpr}x`, "coverage", "pixel", [300, 100], dpr, LIGHT, r.cmds);
}

function gradients(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  const palette = { 0.25: "rgb(0,0,255)", 0.55: "rgb(0,255,0)", 0.85: "yellow", 1.0: "rgb(255,0,0)" };
  r.set("fillStyle", {
    linearGradient: [0, 0, 256, 1],
    stops: Object.entries(palette).map(([k, v]) => [Number(k), v])
  });
  r.call("fillRect", 0, 0, 256, 1);
  r.set("fillStyle", {
    linearGradient: [10, 10, 266, 40],
    stops: [
      [0, "#2b66a2"],
      [0.5, "rgba(255, 255, 255, 0)"],
      [1, "#af2437"]
    ]
  });
  r.call("fillRect", 10, 10, 256, 30);
  for (const [i, blur] of [0.15, 0.5, 0.85].entries()) {
    const [x, y, radius] = [40 + i * 70, 90, 25];
    r.set("fillStyle", {
      radialGradient: [x, y, radius * blur, x, y, radius],
      stops: [
        [0, "rgba(0,0,0,1)"],
        [1, "rgba(0,0,0,0)"]
      ]
    });
    r.call("fillRect", x - radius, y - radius, 2 * radius, 2 * radius);
  }
  r.set("fillStyle", {
    radialGradient: [260, 90, 2, 250, 85, 30],
    stops: [
      [0, "#e5b31b"],
      [0.6, "#e48b32"],
      [1, "rgba(63, 163, 77, 0.2)"]
    ]
  });
  r.call("beginPath");
  r.call("arc", 255, 90, 30, 0, Math.PI * 2);
  r.call("fill");
  r.set("strokeStyle", {
    linearGradient: [10, 130, 280, 130],
    stops: [
      [0, "#222"],
      [1, "#3fa34d"]
    ]
  });
  r.set("lineWidth", 4);
  r.call("beginPath");
  r.call("moveTo", 10, 130);
  r.call("lineTo", 280, 140);
  r.call("stroke");
  return scene(`gradients-${dpr}x`, "coverage", "pixel", [300, 150], dpr, LIGHT, r.cmds);
}

function testImage(width: number, height: number, alpha: boolean): string {
  const data = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      const check = (x >> 2) % 2 === (y >> 2) % 2;
      data[i] = check ? 0x2b : (x * 255) / (width - 1);
      data[i + 1] = check ? 0x66 : (y * 255) / (height - 1);
      data[i + 2] = check ? 0xa2 : 0x40;
      data[i + 3] = alpha ? Math.round(64 + (191 * x) / (width - 1)) : 255;
    }
  }
  return Buffer.from(data).toString("base64");
}

function images(dpr: number): Scene {
  const r = new Rec();
  r.call("image", "opaque", 24, 16, testImage(24, 16, false));
  r.call("image", "alpha", 24, 16, testImage(24, 16, true));
  r.call("scale", dpr, dpr);
  r.call("drawImage", "@opaque", 10, 10);
  r.call("drawImage", "@alpha", 40.5, 10.5);
  r.call("drawImage", "@opaque", 80, 10, 72, 48);
  r.set("imageSmoothingEnabled", false);
  r.call("drawImage", "@opaque", 160, 10, 72, 48);
  r.call("drawImage", "@opaque", 4, 2, 12, 10, 240, 10, 50.5, 41.7);
  r.set("imageSmoothingEnabled", true);
  r.call("drawImage", "@opaque", 4, 2, 12, 10, 10, 70, 50.5, 41.7);
  r.call("drawImage", "@alpha", 70, 70, 9, 6);
  r.set("globalAlpha", 0.4);
  r.call("drawImage", "@opaque", 90, 70, 48, 32);
  r.set("globalAlpha", 1);
  r.call(
    "canvas",
    "template",
    40,
    40,
    [
      ["=", "fillStyle", { radialGradient: [20, 20, 3, 20, 20, 20], stops: [[0, "rgba(0,0,0,1)"], [1, "rgba(0,0,0,0)"]] }],
      ["fillRect", 0, 0, 40, 40]
    ]
  );
  for (let i = 0; i < 6; i++) {
    r.set("globalAlpha", 0.15 + i * 0.15);
    r.call("drawImage", "@template", 150 + i * 12, 70 + (i % 2) * 8);
  }
  r.set("globalAlpha", 1);
  r.call("canvas", "small", 30, 20, [
    ["=", "fillStyle", "#af2437"],
    ["fillRect", 0, 0, 15, 10],
    ["=", "fillStyle", "rgba(63,163,77,0.5)"],
    ["fillRect", 10, 5, 20, 15]
  ]);
  r.call("drawImage", "@small", 240, 70, 60, 40);
  r.call("save");
  r.call("translate", 320, 30);
  r.call("rotate", 0.4);
  r.call("drawImage", "@opaque", 0, 0, 48, 32);
  r.call("restore");
  return scene(`images-${dpr}x`, "coverage", "pixel", [380, 120], dpr, LIGHT, r.cmds);
}

function imageData(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("fillStyle", "#2b66a2");
  r.call("fillRect", 10, 10, 40, 30);
  r.set("fillStyle", "rgba(228, 139, 50, 0.3)");
  r.call("fillRect", 30, 20, 40, 30);
  r.set("fillStyle", "rgba(0, 0, 0, 0.02)");
  r.call("fillRect", 60, 10, 20, 40);
  r.call("getImageData", 0, 0, 90 * dpr, 60 * dpr, "$copy");
  r.call("putImageData", "$copy", 100 * dpr, 0);
  r.call("putImageData", "$copy", 200 * dpr + 3, 5);
  r.call("putImageData", { width: 4, height: 2, rgba: testImage(4, 2, true) }, 5, 140);
  r.call("getImageData", -10, -10, 30, 30, "$edge");
  r.call("putImageData", "$edge", 10, 120);
  return scene(`imagedata-${dpr}x`, "coverage", "pixel", [300, 80], dpr, LIGHT, r.cmds);
}

function heatmap(dpr: number): Scene {
  const r = new Rec();
  const radius = 12;
  r.call("canvas", "tpl", radius * 2, radius * 2, [
    ["=", "fillStyle", { radialGradient: [radius, radius, radius * 0.85, radius, radius, radius], stops: [[0, "rgba(0,0,0,1)"], [1, "rgba(0,0,0,0)"]] }],
    ["fillRect", 0, 0, radius * 2, radius * 2]
  ]);
  const shadow: Cmd[] = [["clearRect", 0, 0, 120, 80]];
  for (let i = 0; i < 25; i++) {
    const x = 20 + ((i * 37) % 80);
    const y = 10 + ((i * 23) % 50);
    shadow.push(["=", "globalAlpha", Math.max(0.01, ((i * 7) % 10) / 10)]);
    shadow.push(["drawImage", "@tpl", x - radius, y - radius]);
  }
  r.call("canvas", "shadow", 120, 80, shadow);
  r.call("scale", dpr, dpr);
  r.call("drawImage", "@shadow", 10, 10, 240, 160);
  r.set("globalAlpha", 0.6);
  r.call("drawImage", "@shadow", 260, 10);
  return scene(`heatmap-${dpr}x`, "coverage", "pixel", [400, 180], dpr, LIGHT, r.cmds);
}

function textStyles(dpr: number, dark: boolean): Scene {
  const r = new Rec();
  const fg = dark ? "#eee" : "#222";
  r.call("scale", dpr, dpr);
  r.set("fillStyle", fg);
  const fonts = [
    `12px ${SYSTEM_FONT}`,
    `200 12px ${SYSTEM_FONT}`,
    `200 14px ${SYSTEM_FONT}`,
    `bold 12px ${SYSTEM_FONT}`,
    "10px sans-serif",
    "italic 16px sans-serif",
    "13.5px sans-serif",
    "21.25px sans-serif"
  ];
  fonts.forEach((font, i) => {
    r.set("font", font);
    r.set("textAlign", "left");
    r.set("textBaseline", "alphabetic");
    r.call("fillText", "Robot 0123 +4.37s Wg", 10, 24 + i * 26);
  });
  r.set("font", `12px ${SYSTEM_FONT}`);
  const aligns = ["start", "end", "left", "right", "center"];
  const baselines = ["alphabetic", "top", "middle", "bottom", "hanging", "ideographic"];
  aligns.forEach((align, i) => {
    baselines.forEach((baseline, j) => {
      r.set("textAlign", align);
      r.set("textBaseline", baseline);
      r.call("fillText", "Ag", 300 + i * 45, 20 + j * 22);
    });
  });
  r.set("textAlign", "center");
  r.set("textBaseline", "middle");
  [200, 100, 60, 30, 5, 0, -5].forEach((maxWidth, i) => {
    r.call("fillText", "Teleoperated maxWidth", 400, 160 + i * 14, maxWidth);
  });
  r.set("textAlign", "left");
  r.set("textBaseline", "alphabetic");
  r.call("fillText", "µs 45° ± café Δt", 10, 250);
  return scene(`textstyles-${dark ? "dark" : "light"}-${dpr}x`, "text", "text", [520, 270], dpr, dark ? DARK : LIGHT, r.cmds);
}

function textEmoji(dpr: number, dark: boolean): Scene {
  const r = new Rec();
  const fg = dark ? "#eee" : "#222";
  r.call("scale", dpr, dpr);
  r.set("fillStyle", fg);
  const fonts = [`16px ${SYSTEM_FONT}`, `12px ${SYSTEM_FONT}`, `bold 14px ${SYSTEM_FONT}`, "italic 16px sans-serif", "13.5px sans-serif"];
  const titles = ["📖", "📉 Line Graph", "🗺 2D Field", "👀 3D Field", "🔢 Table", "⚙️ Mechanism", "🦀 Swerve"];
  fonts.forEach((font, i) => {
    r.set("font", font);
    titles.forEach((title, j) => {
      r.call("fillText", title, 10 + (j % 4) * 130 + i * 0.25 + j * 0.1, 24 + i * 50 + Math.floor(j / 4) * 24);
    });
  });
  r.set("font", `16px ${SYSTEM_FONT}`);
  ["left", "center", "right"].forEach((align, i) => {
    r.set("textAlign", align);
    r.call("fillText", "👀🔢 align", 80 + i * 160, 280);
  });
  r.set("textAlign", "left");
  r.call("fillText", "📉 Line Graph compressed", 10, 310, 90);
  r.set("globalAlpha", 0.5);
  r.call("fillText", "💬 half alpha", 200, 310);
  r.set("globalAlpha", 1);
  r.set("fillStyle", "rgba(43, 102, 162, 0.6)");
  r.call("fillText", "🎮 translucent fill", 360, 310);
  return scene(`textemoji-${dark ? "dark" : "light"}-${dpr}x`, "text", "text", [540, 330], dpr, dark ? DARK : LIGHT, r.cmds);
}

function offCanvas(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("fillStyle", "#2b66a2");
  r.call("fillRect", -20.5, -10.25, 60, 40);
  r.call("fillRect", 270.3, 95.6, 60, 40);
  r.set("fillStyle", "rgba(228, 139, 50, 0.7)");
  r.call("beginPath");
  r.call("moveTo", -5000, 50);
  r.call("lineTo", 100, -3000);
  r.call("lineTo", 5000.5, 200);
  r.call("lineTo", 120, 70.5);
  r.call("closePath");
  r.call("fill");
  r.set("fillStyle", "#3fa34d");
  r.call("beginPath");
  r.call("moveTo", -12.3, 40);
  r.call("lineTo", 30.7, 52.1);
  r.call("lineTo", 25.2, 88.4);
  r.call("lineTo", -8.6, 79.9);
  r.call("closePath");
  r.call("fill");
  r.call("beginPath");
  r.call("arc", 310, 60, 25, 0, Math.PI * 2);
  r.call("fill");
  r.set("strokeStyle", "#222");
  r.set("lineWidth", 3);
  r.set("lineCap", "round");
  r.call("beginPath");
  r.call("moveTo", -10000, 30);
  r.call("lineTo", 10000, 35);
  r.call("stroke");
  r.set("lineWidth", 1);
  r.set("lineCap", "butt");
  r.call("beginPath");
  r.call("moveTo", -400, 100.5);
  r.call("lineTo", 700, 90);
  r.call("stroke");
  r.call("setLineDash", [6, 4]);
  r.call("beginPath");
  r.call("moveTo", -500, 60);
  r.call("lineTo", 900, 60);
  r.call("stroke");
  r.set("lineWidth", 2);
  r.call("beginPath");
  r.call("moveTo", 150, -300);
  r.call("lineTo", 150, 500);
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", -300, -100);
  r.call("lineTo", 600, 200);
  r.call("stroke");
  r.call("strokeRect", 200.5, -20.5, 140, 60);
  r.call("setLineDash", []);
  r.set("strokeStyle", "#af2437");
  r.set("lineWidth", 2.5);
  r.call("beginPath");
  r.call("arc", 150, 20000 + 75, 20000, 0, Math.PI * 2);
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 40, -50);
  r.call("lineTo", 80, 170);
  r.call("lineTo", 120, -40);
  r.set("lineJoin", "round");
  r.call("stroke");
  return scene(`offcanvas-${dpr}x`, "coverage", "pixel", [300, 120], dpr, LIGHT, r.cmds);
}

function gradientText(dpr: number, dark: boolean): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("font", `16px ${SYSTEM_FONT}`);
  r.set("textBaseline", "middle");
  r.set("fillStyle", {
    linearGradient: [10, 0, 290, 0],
    stops: [
      [0, "#2b66a2"],
      [0.5, dark ? "#eeeeee" : "#222222"],
      [1, "#af2437"]
    ]
  });
  r.call("fillText", "Gradient text 0123 +4.37s", 10, 20);
  r.set("font", `14px ${SYSTEM_FONT}`);
  r.set("fillStyle", {
    radialGradient: [150, 50, 5, 150, 50, 120],
    stops: [
      [0, "#e5b31b"],
      [1, "#3fa34d"]
    ]
  });
  r.call("fillText", "Radial gradient Wg", 30, 50);
  r.set("globalAlpha", 0.6);
  r.call("fillText", "with globalAlpha", 30, 72, 100);
  r.set("globalAlpha", 1);
  const fg = dark ? "#eeeeee" : "#222222";
  const odd = dark ? "#000000" : "#ffffff";
  r.set("fillStyle", {
    linearGradient: [10, 0, 290, 0],
    stops: [
      [0, fg],
      [0.5, fg],
      [0.5, odd],
      [0.5, odd],
      [0.5, odd],
      [0.5, fg],
      [1, fg]
    ]
  });
  r.set("font", `16px ${SYSTEM_FONT}`);
  r.call("fillText", "Duplicate stops 0123", 10, 96);
  return scene(`gradienttext-${dark ? "dark" : "light"}-${dpr}x`, "text", "text", [300, 110], dpr, dark ? DARK : LIGHT, r.cmds);
}

function curves(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("fillStyle", "#2b66a2");
  r.call("beginPath");
  r.call("moveTo", 10, 60);
  r.call("bezierCurveTo", 30, -10, 60, 120, 90, 40.5);
  r.call("bezierCurveTo", 100, 20, 70.3, 90.7, 10, 60);
  r.call("fill");
  r.set("fillStyle", "#e48b32");
  r.call("beginPath");
  r.call("moveTo", 100.5, 10.25);
  r.call("quadraticCurveTo", 160, 120, 190.3, 15);
  r.call("quadraticCurveTo", 150, 40, 100.5, 10.25);
  r.call("fill");
  r.set("fillStyle", "#3fa34d");
  r.call("beginPath");
  r.call("moveTo", 200, 10);
  r.call("bezierCurveTo", 260, 80, 190, 80, 250, 10);
  r.call("closePath");
  r.call("fill");
  r.set("fillStyle", "rgba(175, 36, 55, 0.7)");
  r.call("beginPath");
  r.call("moveTo", -40, 120);
  r.call("bezierCurveTo", 60, 300, 30, -150, 120, 190);
  r.call("lineTo", -30, 190);
  r.call("fill");
  r.call("beginPath");
  r.call("moveTo", 380, 30);
  r.call("bezierCurveTo", 700, -200, 460, 260, 390, 90);
  r.call("fill");
  r.set("strokeStyle", "#222");
  const strokes: [number, string, string][] = [
    [1, "butt", "miter"],
    [0.5, "butt", "miter"],
    [2.5, "round", "round"],
    [6, "square", "bevel"],
    [9, "butt", "miter"],
    [4, "round", "miter"]
  ];
  strokes.forEach(([lw, cap, join], i) => {
    const x = 270 + (i % 3) * 50;
    const y = 15 + Math.floor(i / 3) * 60;
    r.set("lineWidth", lw);
    r.set("lineCap", cap);
    r.set("lineJoin", join);
    r.call("beginPath");
    r.call("moveTo", x, y + 0.3);
    r.call("bezierCurveTo", x + 40, y, x - 5, y + 45, x + 35.5, y + 40);
    r.call("quadraticCurveTo", x + 15, y + 20, x + 38, y + 5);
    r.call("stroke");
  });
  r.set("lineWidth", 3);
  r.set("lineJoin", "round");
  r.set("lineCap", "butt");
  r.call("beginPath");
  r.call("moveTo", 20, 110);
  r.call("bezierCurveTo", 80, 170, 20, 170, 80, 110);
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 100, 110);
  r.call("bezierCurveTo", 160, 110, 100, 110, 160, 110.5);
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 100, 130);
  r.call("bezierCurveTo", 130, 130, 130, 130, 160, 130);
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 100, 150);
  r.call("bezierCurveTo", 100, 150, 160, 180, 160, 180);
  r.call("stroke");
  r.set("lineWidth", 12);
  r.set("lineJoin", "miter");
  r.call("beginPath");
  r.call("moveTo", 180, 110);
  r.call("bezierCurveTo", 260, 190, 170, 190, 250, 105);
  r.call("stroke");
  r.set("lineWidth", 1.5);
  r.call("setLineDash", [8, 4]);
  r.set("lineDashOffset", 2.5);
  r.call("beginPath");
  r.call("moveTo", 270, 130);
  r.call("bezierCurveTo", 320, 60, 360, 220, 410, 130);
  r.call("quadraticCurveTo", 390, 190, 300, 185);
  r.call("stroke");
  r.call("setLineDash", []);
  r.call("save");
  r.call("translate", 330, 150);
  r.call("rotate", 0.4);
  r.call("scale", 1.3, 0.7);
  r.set("lineWidth", 2);
  r.set("strokeStyle", "#af2437");
  r.call("beginPath");
  r.call("moveTo", -20, 0);
  r.call("bezierCurveTo", -10, -30, 10, 30, 20, 0);
  r.call("stroke");
  r.call("restore");
  r.set("lineWidth", 0.8);
  r.call("beginPath");
  r.call("bezierCurveTo", 15, 185, 60, 140, 90, 195);
  r.call("stroke");
  return scene(`curves-${dpr}x`, "coverage", "pixel", [420, 200], dpr, LIGHT, r.cmds);
}

function hugeCoords(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("strokeStyle", "#222");
  r.set("lineWidth", 2);
  r.call("beginPath");
  r.call("moveTo", 10, 10);
  r.call("lineTo", 1e39, 50);
  r.call("lineTo", 200, 40);
  r.call("lineTo", 3e38, 3e38);
  r.call("lineTo", 280, 15);
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 10, 30);
  r.call("lineTo", 3e38, 30.5);
  r.call("stroke");
  r.call("beginPath");
  r.call("moveTo", 10, 40.5);
  r.call("lineTo", 4e37, 40.5);
  r.call("stroke");
  r.set("fillStyle", "#2b66a2");
  r.call("fillRect", 10, 55, 1e39, 10);
  r.call("fillRect", -1e39, 70, 2e39, 10);
  r.call("fillRect", 20.5, 85, 4e37, 6);
  r.call("fillRect", 30.5, 95, 1e-7, 10);
  r.call("fillRect", 40000.5, 95, -39950, 10);
  r.call("beginPath");
  r.call("rect", 100, 95, 1e39, 10);
  r.call("rect", 120, 112, 30, 8);
  r.call("fill");
  r.set("strokeStyle", "#e48b32");
  r.call("strokeRect", 160.5, 95.5, 1e39, 1e39);
  r.call("strokeRect", 170.25, 100.5, 50000, 10);
  r.call("save");
  r.call("translate", 1e39, 0);
  r.set("fillStyle", "#3fa34d");
  r.call("fillRect", -1e39, 60, 30, 20);
  r.call("restore");
  r.call("save");
  r.call("scale", 1e39, 1);
  r.call("fillRect", 0, 85, 1e-36, 8);
  r.call("restore");
  r.set("lineWidth", 1e39);
  r.call("beginPath");
  r.call("moveTo", 250, 110);
  r.call("lineTo", 290, 112);
  r.call("stroke");
  r.set("lineWidth", 3);
  r.set("fillStyle", { linearGradient: [-1e39, 0, 1e39, 0], stops: [[0, "#af2437"], [1, "#3fa34d"]] });
  r.call("fillRect", 200, 25, 80, 10);
  r.call("beginPath");
  r.call("arc", 60, 105, 1e39, 0, 1);
  r.call("arc", 60, 105, 10, 0, 6);
  r.call("fill");
  r.call("beginPath");
  r.call("moveTo", 80, 100);
  r.call("bezierCurveTo", 1e39, 0, 90, 110, 95, 118);
  r.call("quadraticCurveTo", 90, 3e38, 85, 118);
  r.call("stroke");
  return scene(`hugecoords-${dpr}x`, "coverage", "pixel", [300, 120], dpr, LIGHT, r.cmds);
}

function strokeRects(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("strokeStyle", "#222");
  const widths = [0.3, 0.75, 1, 1.5, 2.25, 3.7, 7];
  const joins = ["miter", "round", "bevel"];
  joins.forEach((join, j) => {
    r.set("lineJoin", join);
    widths.forEach((lw, i) => {
      r.set("lineWidth", lw);
      const x = 8 + i * 42 + j * 0.25 + i * 0.13;
      const y = 8 + j * 34 + i * 0.37;
      r.call("strokeRect", x, y, 30.6, 20.3);
    });
  });
  r.set("strokeStyle", "#2b66a2");
  r.set("lineJoin", "round");
  widths.forEach((lw, i) => {
    r.set("lineWidth", lw);
    r.call("beginPath");
    r.call("rect", 8.4 + i * 42, 112.6, 30, 18.5);
    r.call("stroke");
  });
  r.set("lineWidth", 5);
  r.call("beginPath");
  r.call("rect", 300, 112, -20, 18);
  r.call("stroke");
  r.set("lineWidth", 30);
  r.call("strokeRect", 320, 110, 12, 10);
  r.set("lineJoin", "miter");
  r.set("miterLimit", 1.2);
  r.set("lineWidth", 4);
  r.call("strokeRect", 8.5, 145.5, 40, 20);
  r.set("miterLimit", 10);
  r.set("strokeStyle", "rgba(175, 36, 55, 0.6)");
  [0.4, 1.2, 2.6].forEach((lw, i) => {
    r.set("lineWidth", lw);
    r.call("strokeRect", 60.3 + i * 45, 145.7, 35.2, 20.1);
  });
  r.call("save");
  r.call("translate", 230, 155);
  r.call("rotate", 0.3);
  r.set("lineJoin", "round");
  r.set("lineWidth", 3);
  r.call("strokeRect", -15, -8, 30, 16);
  r.call("restore");
  r.call("save");
  r.call("translate", 290.5, 150.25);
  r.call("scale", 1.5, -0.75);
  r.set("lineJoin", "round");
  r.set("lineWidth", 2);
  r.call("strokeRect", 0, 0, 20, 16);
  r.call("restore");
  r.set("lineJoin", "miter");
  r.set("lineWidth", 1);
  r.call("strokeRect", 330.5, 145.5, 0.4, 20);
  return scene(`strokerects-${dpr}x`, "coverage", "pixel", [360, 175], dpr, LIGHT, r.cmds);
}

function dashedLines(r: Rec, y0: number) {
  r.set("strokeStyle", "#222");
  r.set("lineWidth", 2);
  r.call("setLineDash", [3, 3]);
  for (let i = 0; i < 8; i++) {
    r.call("beginPath");
    r.call("moveTo", 10.3 + i * 0.4, y0 + 10.6 + i * 9);
    r.call("lineTo", 180.7, y0 + 10.6 + i * 9);
    r.call("stroke");
    r.call("beginPath");
    r.call("moveTo", 195.6 + i * 12, y0 + 5.2 + i * 0.3);
    r.call("lineTo", 195.6 + i * 12, y0 + 80.4);
    r.call("stroke");
  }
  r.call("setLineDash", []);
}

function multiFrame(dpr: number, mode: "" | "-single" | "-resized"): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  r.set("fillStyle", "#2b66a2");
  for (let i = 0; i < 6; i++) {
    r.call("beginPath");
    r.call("rect", 10 + i * 20, 10, 12, 12);
    r.call("fill");
  }
  if (mode !== "-single") {
    r.call("frame");
    if (mode === "-resized") {
      r.call("resize", 300 * dpr + 1, 100 * dpr);
      r.call("resize", 300 * dpr, 100 * dpr);
    }
    r.call("scale", dpr, dpr);
  }
  dashedLines(r, 0);
  return scene(`multiframe${mode}-${dpr}x`, "coverage", "pixel", [300, 100], dpr, LIGHT, r.cmds);
}

// Width-1 strokes at 1x are antialiased hairlines. Skia halves them while longer than 511 px, and
// with round caps (outset pi/8) both halves blit the pixel where they meet. Lines leaving the
// canvas go through its clip blitter, which turns slanted lines' paired blits into plain ones.
function longHairlines(): Scene {
  const r = new Rec();
  r.set("lineWidth", 1);
  const lines: [string, number, number, number, number][] = [
    ["round", 55, 22.5, 735, 22.5],
    ["round", 105, 42.5, 735, 42.5],
    ["round", 0, 62.5, 790, 62.5],
    ["round", 55, 82.5, 335, 82.5],
    ["round", 20.3, 101.7, 780.6, 101.7],
    ["square", 55, 122.5, 735, 122.5],
    ["butt", 55.3, 142.5, 735.7, 142.5],
    ["round", 12.5, 160, 12.5, 790],
    ["round", 31.2, 158.6, 31.2, 797.3],
    ["square", 50.5, 160, 50.5, 790],
    ["round", 70, 170, 790, 260],
    ["round", 70, 280, 790, 520.4],
    ["round", 90, 300, 160, 790],
    ["round", 120, 790, 330, 220],
    ["round", 795, 540, 60, 560]
  ];
  for (const [color, from] of [["#222", 0], ["rgba(43, 102, 162, 0.6)", 7]] as [string, number][]) {
    r.set("strokeStyle", color);
    lines.forEach(([cap, x0, y0, x1, y1], i) => {
      const [dx, dy] = i < 7 ? [0, from] : [from, 0];
      r.set("lineCap", cap);
      r.call("beginPath");
      r.call("moveTo", x0 + dx, y0 + dy);
      r.call("lineTo", x1 + dx, y1 + dy);
      r.call("stroke");
    });
  }
  return scene("hairlines-1x", "coverage", "pixel", [800, 800], 1, LIGHT, r.cmds);
}

// Rect clips on and off the pixel grid, nested, under fills, strokes, text and clearRect, then
// rotated and curved clips.
function clipping(name: string, dpr: number): Scene {
  const r = new Rec();
  r.call("setTransform", dpr, 0, 0, dpr, 0, 0);
  const paint = (x: number, y: number) => {
    r.set("fillStyle", "#2b66a2");
    r.call("beginPath");
    r.call("rect", x - 4.3, y - 3.7, 70.6, 30.4);
    r.call("fill");
    r.set("fillStyle", "rgba(229, 179, 27, 0.6)");
    r.call("fillRect", x + 10.25, y + 5.5, 60, 60);
    r.set("strokeStyle", "#af2437");
    r.set("lineWidth", 3);
    r.call("beginPath");
    r.call("moveTo", x - 10, y + 40);
    r.call("lineTo", x + 90, y + 12.5);
    r.call("stroke");
    r.set("fillStyle", "#222");
    r.set("font", `12px ${SYSTEM_FONT}`);
    r.set("textBaseline", "middle");
    r.call("fillText", "Clipped text", x + 2, y + 30.4);
    r.call("clearRect", x + 30, y + 44, 12, 60);
  };
  [0, 0.05, 0.1, 0.2, 0.25, 0.4, 0.5, 0.75].forEach((offset, i) => {
    const x = 20 + (i % 4) * 95;
    const y = 15 + Math.floor(i / 4) * 85;
    r.call("save");
    r.call("beginPath");
    r.call("rect", x + offset, y + offset, 62 + offset, 56 - offset);
    r.call("clip");
    paint(x, y);
    r.call("restore");
  });
  r.call("save");
  r.call("beginPath");
  r.call("rect", 20.3, 190.6, 80.4, 60.2);
  r.call("clip");
  r.call("beginPath");
  r.call("rect", 30.7, 180.2, 80.4, 50.9);
  r.call("clip");
  paint(20, 190);
  r.call("restore");
  r.call("save");
  r.call("beginPath");
  r.call("arc", 160, 220, 31.4, 0, 2 * Math.PI);
  r.call("clip");
  paint(125, 190);
  r.call("restore");
  r.call("save");
  r.call("translate", 260, 190);
  r.call("rotate", 0.35);
  r.call("beginPath");
  r.call("rect", 0, 0, 50.5, 50.5);
  r.call("clip");
  r.call("resetTransform");
  r.call("setTransform", dpr, 0, 0, dpr, 0, 0);
  paint(230, 190);
  r.call("restore");
  r.call("save");
  r.call("beginPath");
  r.call("rect", 330, 190, 0, 0);
  r.call("clip");
  paint(330, 190);
  r.call("restore");
  return scene(name, "coverage", "pixel", [400, 280], dpr, LIGHT, r.cmds);
}

// Transforms whose f32 steps Skia takes and Blink's doubles do not: translations past half of
// FLT_MAX, long runs of small steps, and the matrices a readback makes Blink rebuild.
function floatMatrix(dpr: number): Scene {
  const r = new Rec();
  r.call("scale", dpr, dpr);
  const cell = (i: number) => r.call("setTransform", dpr, 0, 0, dpr, (10 + (i % 6) * 50) * dpr, (10 + Math.floor(i / 6) * 40) * dpr);
  r.set("fillStyle", "#2b66a2");
  ([
    [1e38, 1],
    [2e38, 1],
    [3e38, 1],
    [1.7e38, 2],
    [1.8e38, 2],
    [3.4e38, 0.5]
  ] as [number, number][]).forEach(([far, scale], i) => {
    r.call("save");
    cell(i);
    r.call("scale", scale, 1);
    r.call("translate", far, 0);
    r.call("translate", -far, 0);
    r.call("fillRect", 0, 0, 30 / scale, 20);
    r.call("restore");
  });
  r.set("strokeStyle", "#af2437");
  r.set("lineWidth", 2);
  for (let i = 0; i < 3; i++) {
    r.call("save");
    cell(6 + i);
    r.call("translate", [1e16, 16777216, 65536][i], 0);
    r.call("translate", -[1e16, 16777216, 65536][i] + 0.3, 0.3);
    r.call("strokeRect", 0, 0, 30.5, 20.25);
    r.call("beginPath");
    r.call("moveTo", 2, 2);
    r.call("lineTo", 28, 18);
    r.call("stroke");
    r.call("restore");
  }
  const steps = (i: number, step: (k: number) => void) => {
    r.call("save");
    cell(i);
    for (let k = 0; k < 400; k++) step(k);
    r.set("fillStyle", "rgba(63, 163, 77, 0.6)");
    r.call("fillRect", 0, 0, 20, 14);
    r.set("strokeStyle", "#222");
    r.set("lineWidth", 1);
    r.call("strokeRect", 0.5, 0.5, 24, 18);
    r.call("restore");
  };
  steps(9, () => r.call("translate", 0.025, 0.0125));
  steps(10, (k) => r.call("scale", k % 2 === 0 ? 1.1 : 1 / 1.1, k % 2 === 0 ? 0.9 : 1 / 0.9));
  steps(11, () => r.call("rotate", (2 * Math.PI) / 400));
  steps(12, (k) => {
    r.call("rotate", 0.1);
    r.call("translate", k % 2 === 0 ? 0.7 : -0.7, 0.01);
    r.call("rotate", -0.1);
  });
  r.call("save");
  cell(13);
  r.call("translate", 5.1, 3.3);
  r.call("rotate", Math.PI / 2);
  r.call("translate", 0, -16);
  r.set("fillStyle", "#e48b32");
  r.call("fillRect", 0.3, 0.3, 10.2, 6.6);
  r.call("getImageData", 0, 0, 1, 1, "$flush");
  r.set("fillStyle", "#e5b31b");
  r.call("fillRect", 12.3, 0.3, 10.2, 6.6);
  r.call("strokeRect", 12.3, 9.3, 10.2, 4.6);
  r.call("restore");
  r.call("save");
  cell(14);
  for (let k = 0; k < 300; k++) r.call("translate", 0.1, 0.05);
  r.call("fillRect", -30, -15, 12.5, 8.25);
  r.call("getImageData", 0, 0, 1, 1, "$flush");
  r.call("fillRect", -15, -15, 12.5, 8.25);
  r.call("beginPath");
  r.call("arc", -20, 2, 5, 0, Math.PI * 2);
  r.call("fill");
  r.call("restore");
  return scene(`floatmatrix-${dpr}x`, "coverage", "pixel", [310, 130], dpr, LIGHT, r.cmds);
}

// Quarter turns, which keep rects rects for Skia: through rotate(), setTransform() and flips, at
// fractional offsets, with fills, strokes, dashes, clips, clears and gradients, and the doubles of
// a matrix rebuilt after a readback, which are no quarter turn any more.
function quarterTurns(dpr: number): Scene {
  const r = new Rec();
  const cells: ((x: number, y: number) => void)[] = [
    () => r.call("rotate", Math.PI / 2),
    () => r.call("rotate", -Math.PI / 2),
    () => r.call("rotate", Math.PI),
    () => r.call("rotate", (5 * Math.PI) / 2),
    (x, y) => r.call("setTransform", 0, dpr, -dpr, 0, x, y),
    (x, y) => r.call("setTransform", 0, 1.5 * dpr, 0.75 * dpr, 0, x, y),
    () => {
      r.call("rotate", Math.PI / 2);
      r.call("scale", 1.3, -0.7);
    },
    () => {
      r.call("rotate", Math.PI / 4);
      r.call("rotate", Math.PI / 4);
    }
  ];
  cells.forEach((turn, i) => {
    r.call("save");
    const [x, y] = [(45.3 + (i % 4) * 90) * dpr, (45.7 + Math.floor(i / 4) * 90) * dpr];
    r.call("setTransform", dpr, 0, 0, dpr, x, y);
    turn(x, y);
    r.set("fillStyle", "#2b66a2");
    r.call("fillRect", -30.4, -30.2, 22.6, 14.3);
    r.set("fillStyle", "rgba(228, 139, 50, 0.6)");
    r.call("fillRect", -18.7, -24.1, 30.3, 12.8);
    r.call("fillRect", 16.2, -30, 0.4, 20);
    r.set("strokeStyle", "#222");
    [0.4, 1, 2.5].forEach((lw, k) => {
      r.set("lineWidth", lw);
      r.call("strokeRect", -30.3 + k * 17, -5.6, 12.4, 9.7);
    });
    r.set("lineJoin", "round");
    r.call("strokeRect", 21.3, -5.6, 9.4, 9.7);
    r.set("lineJoin", "miter");
    r.set("strokeStyle", "rgba(175, 36, 55, 0.7)");
    r.set("lineWidth", 2);
    r.call("setLineDash", [3, 3]);
    r.call("beginPath");
    r.call("moveTo", -30.5, 8.2);
    r.call("lineTo", 30.5, 8.2);
    r.call("stroke");
    r.call("setLineDash", []);
    r.set("fillStyle", { linearGradient: [-30, 0, 10, 0], stops: [[0, "#af2437"], [1, "rgba(63, 163, 77, 0.4)"]] });
    r.call("fillRect", -30.6, 12.3, 40.2, 8.4);
    r.call("clearRect", -24.4, 14.2, 6.3, 3.6);
    r.call("beginPath");
    r.call("rect", 12.4, 12.6, 18.2, 16.3);
    r.call("clip");
    r.set("fillStyle", "#3fa34d");
    r.call("fillRect", 8, 15.25, 30, 30);
    r.call("restore");
  });
  r.call("save");
  r.call("setTransform", dpr, 0, 0, dpr, 60.3 * dpr, 200.7 * dpr);
  r.call("rotate", Math.PI / 2);
  r.call("getImageData", 0, 0, 1, 1, "$flush");
  r.set("fillStyle", "#2b66a2");
  r.call("fillRect", -10.4, -20.2, 22.6, 34.3);
  r.set("strokeStyle", "#222");
  r.set("lineWidth", 2);
  r.call("strokeRect", -10.6, 20.4, 22.2, 24.3);
  r.call("restore");
  return scene(`quarterturns-${dpr}x`, "coverage", "pixel", [360, 225], dpr, LIGHT, r.cmds);
}

const GRADIENT_KINDS: [string, (x: number, y: number) => { linearGradient?: number[]; radialGradient?: number[] }][] = [
  ["linear", (x, y) => ({ linearGradient: [x, y, x + 34, y + 20] })],
  ["radial", (x, y) => ({ radialGradient: [x + 18, y + 14, 0, x + 18, y + 14, 22] })],
  ["ring", (x, y) => ({ radialGradient: [x + 18, y + 14, 4, x + 18, y + 14, 20] })],
  ["inside", (x, y) => ({ radialGradient: [x + 14, y + 12, 0, x + 18, y + 14, 22] })],
  ["cone", (x, y) => ({ radialGradient: [x + 6, y + 14, 3, x + 28, y + 14, 9] })],
  ["shrinking", (x, y) => ({ radialGradient: [x + 8, y + 14, 12, x + 26, y + 14, 5] })],
  ["strip", (x, y) => ({ radialGradient: [x + 6, y + 14, 8, x + 28, y + 14, 8] })],
  ["touching", (x, y) => ({ radialGradient: [x + 4, y + 14, 0, x + 14, y + 14, 10] })]
];

// Partly covered gradient pixels, which Chromium blends in float: every kind of gradient, opaque
// and not, over opaque, translucent and empty pixels, through fills, hairlines, strokes, text,
// and clips off the pixel grid.
function gradientEdges(dpr: number, dark: boolean): Scene {
  const r = new Rec();
  const stops = {
    opaque: [[0, "#2b66a2"], [0.4, "#e5b31b"], [1, "#af2437"]],
    translucent: [[0, "rgba(43, 102, 162, 0.9)"], [0.5, "rgba(229, 179, 27, 0.15)"], [1, "rgba(175, 36, 55, 0.55)"]]
  } as { [kind: string]: [number, string][] };
  r.call("scale", dpr, dpr);
  r.set("fillStyle", dark ? "#555" : "#ddd");
  r.call("fillRect", 0, 0, 340, 100);
  r.set("fillStyle", "rgba(63, 163, 77, 0.35)");
  r.call("fillRect", 0, 100, 340, 100);
  GRADIENT_KINDS.forEach(([, gradient], i) => {
    Object.values(stops).forEach((s, j) => {
      for (let k = 0; k < 3; k++) {
        const [x, y] = [4 + i * 42, 4 + j * 50 + k * 100];
        r.set("globalAlpha", k === 2 ? 0.7 : 1);
        r.set("fillStyle", { ...gradient(x, y), stops: s });
        r.call("beginPath");
        r.call("moveTo", x + 0.3, y + 20.4);
        r.call("lineTo", x + 17.7, y + 0.6);
        r.call("lineTo", x + 36.2, y + 15.1);
        r.call("bezierCurveTo", x + 30, y + 34, x + 12, y + 18, x + 0.3, y + 20.4);
        r.call("fill");
        r.call("fillRect", x + 0.6, y + 28.3, 14.5, 6.4);
        r.set("strokeStyle", { ...gradient(x, y + 24), stops: s });
        r.set("lineWidth", 0.6);
        r.call("beginPath");
        r.call("moveTo", x + 17, y + 28);
        r.call("lineTo", x + 37, y + 34.6);
        r.call("stroke");
        r.set("lineWidth", 2.3);
        r.call("strokeRect", x + 1.4, y + 37.3, 34.2, 5.1);
      }
    });
  });
  r.set("globalAlpha", 1);
  r.set("font", `16px ${SYSTEM_FONT}`);
  r.set("textBaseline", "middle");
  r.set("fillStyle", { linearGradient: [4, 310, 80, 330], stops: [[0, "#2b66a2"], [1, "#af2437"]] });
  r.call("fillText", "Gradient", 4, 320);
  r.call("save");
  r.call("beginPath");
  r.call("arc", 130, 322, 17.4, 0, Math.PI * 2);
  r.call("clip");
  r.set("fillStyle", { linearGradient: [110, 305, 150, 340], stops: stops.opaque });
  r.call("fillRect", 100, 300, 60, 45);
  r.call("restore");
  r.call("save");
  r.call("translate", 190, 322);
  r.call("rotate", 0.4);
  r.call("scale", 1.2, 0.8);
  r.set("fillStyle", { radialGradient: [0, 0, 2, 3, 1, 18], stops: stops.translucent });
  r.call("fillRect", -18.3, -14.2, 36.4, 28.7);
  r.call("restore");
  return scene(`gradientedges-${dark ? "dark" : "light"}-${dpr}x`, "coverage", "pixel", [340, 345], dpr, dark ? DARK : LIGHT, r.cmds);
}

// drawImage of canvases at other sizes: up and down, parts, mirrored, rotated, off the grid,
// translucent, clipped, smoothed and not, with sample points on pixel boundaries.
function scaledImages(dpr: number): Scene {
  const r = new Rec();
  r.call("image", "opaque", 24, 16, testImage(24, 16, false));
  r.call("image", "alpha", 24, 16, testImage(24, 16, true));
  r.call("image", "field", 180, 120, fieldImage(180, 120));
  r.call("image", "line", 1, 16, testImage(2, 8, true));
  r.call("canvas", "soft", 40, 40, [
    ["=", "fillStyle", { radialGradient: [20, 20, 3, 20, 20, 20], stops: [[0, "rgba(43,102,162,1)"], [1, "rgba(229,179,27,0)"]] }],
    ["fillRect", 0, 0, 40, 40]
  ]);
  r.call("scale", dpr, dpr);
  r.set("fillStyle", "#cfd8dc");
  r.call("fillRect", 0, 0, 400, 60);
  [true, false].forEach((smooth, j) => {
    const y = 6 + j * 150;
    r.set("imageSmoothingEnabled", smooth);
    r.call("drawImage", "@opaque", 6, y, 60, 40);
    r.call("drawImage", "@alpha", 72.3, y + 0.6, 50.5, 41.7);
    r.call("drawImage", "@soft", 128, y, 71.5, 33.25);
    r.call("drawImage", "@field", 205, y, 83, 55.333);
    r.call("drawImage", "@field", 20, 10, 101, 73, 294.4, y + 0.3, 47.6, 35.2);
    r.call("drawImage", "@opaque", 4, 2, 12, 10, 348, y, 30.4, 25.6);
    r.call("drawImage", "@opaque", 3.4, 2.3, 12.5, 9.2, 348, y + 30, 30.4, 25.6);
    r.call("drawImage", "@alpha", -4, -3, 20, 30, 382, y, 20, 24);
    r.set("globalAlpha", 0.4);
    r.call("drawImage", "@alpha", 6, y + 60, 48.5, 32.5);
    r.call("drawImage", "@opaque", 60.25, y + 60.5);
    r.call("drawImage", "@opaque", 90, y + 60);
    r.set("globalAlpha", 1);
    r.call("drawImage", "@opaque", 120.5, y + 60.5);
    r.call("drawImage", "@alpha", 5, 3, 14, 9, 150, y + 60, 14, 9);
    r.call("drawImage", "@line", 170, y + 60, 20, 40);
    r.call("drawImage", "@opaque", 195, y + 60, 0.5, 30);
    r.call("drawImage", "@opaque", 198.4, y + 60.3, 30, 0.3);
    r.call("save");
    r.call("translate", 250, y + 60);
    r.call("scale", -1, 1);
    r.call("drawImage", "@opaque", 0, 0, 40, 27);
    r.call("restore");
    r.call("save");
    r.call("translate", 260, y + 95);
    r.call("scale", 1.25, -1);
    r.call("drawImage", "@alpha", 0, 0);
    r.call("restore");
    r.call("save");
    r.call("translate", 320, y + 62);
    r.call("rotate", 0.4);
    r.call("drawImage", "@opaque", 0, 0, 48, 32);
    r.call("drawImage", "@alpha", 4, 2, 12, 10, 20, -12, 22, 14);
    r.call("restore");
    r.call("save");
    r.call("translate", 400, y + 60);
    r.call("rotate", Math.PI / 2);
    r.call("drawImage", "@field", 0, 0, 60.5, 40);
    r.set("globalAlpha", 0.5);
    r.call("drawImage", "@field", 30.3, 45.7, 60.5, 40);
    r.call("restore");
    r.call("save");
    r.call("beginPath");
    r.call("arc", 40, y + 115, 18.3, 0, Math.PI * 2);
    r.call("clip");
    r.call("drawImage", "@field", 18, y + 95, 46.5, 41);
    r.call("restore");
    r.call("save");
    r.call("beginPath");
    r.call("rect", 70.4, y + 100.3, 40.5, 30.2);
    r.call("clip");
    r.call("drawImage", "@opaque", 66, y + 98);
    r.call("drawImage", "@opaque", 92.5, y + 98.5);
    r.call("drawImage", "@soft", 80, y + 110, 60, 30);
    r.call("restore");
  });
  r.set("imageSmoothingEnabled", false);
  r.call("drawImage", "@field", 0, 0, 180, 120, 234 / dpr, 590 / dpr, 166 / dpr, 110 / dpr);
  r.call("drawImage", "@field", 0, 0, 180, 120, 10, 305, 45, 30);
  r.call("drawImage", "@field", 0, 0, 180, 120, 60, 305, 90, 60);
  r.call("drawImage", "@opaque", 160, 305, 36, 24);
  r.set("imageSmoothingEnabled", true);
  r.call("drawImage", "@field", 0, 0, 180, 120, 250, 305, 45, 30);
  r.call("drawImage", "@field", 300, 305, 90, 60);
  return scene(`scaledimages-${dpr}x`, "coverage", "pixel", [420, 370], dpr, LIGHT, r.cmds);
}

// drawImage of <img> elements, which Chromium draws from a mip level of the decoded image: whole
// and odd sizes, parts, every level down to a target that rounds to nothing, opaque and not.
function imageElements(dpr: number): Scene {
  const r = new Rec();
  const img = (id: string, width: number, height: number, rgba: string) =>
    r.call("img", id, width, height, rgba, encodePng(width, height, Buffer.from(rgba, "base64")).toString("base64"));
  img("field", 192, 128, fieldImage(192, 128));
  img("odd", 151, 97, fieldImage(151, 97));
  img("alpha", 61, 43, testImage(61, 43, true));
  img("thin", 200, 3, testImage(200, 3, true));
  r.call("scale", dpr, dpr);
  [true, false].forEach((smooth, j) => {
    const y = 5 + j * 130;
    r.set("imageSmoothingEnabled", smooth);
    let x = 5;
    for (const [k, id] of ["field", "odd", "alpha"].entries()) {
      x = 5;
      for (const width of [100, 70, 40, 24.5, 11, 5, 2, 0.8]) {
        r.call("drawImage", `@${id}`, x + k * 0.3, y + k * 40, width / (k + 1), 35 / (k + 1));
        x += width / (k + 1) + 4;
      }
    }
    r.call("drawImage", "@field", 40, 30, 100, 70, 290, y, 40, 28);
    r.call("drawImage", "@odd", 10.5, 7.25, 101.3, 80.1, 335.4, y + 0.4, 30.2, 25.6);
    r.call("drawImage", "@odd", -20, 30, 120, 90, 370, y, 20, 30);
    r.call("drawImage", "@thin", 290, y + 32, 50, 2);
    r.call("drawImage", "@thin", 290, y + 36, 50, 1);
    r.call("drawImage", "@thin", 290, y + 40, 400, 0.4);
    r.call("drawImage", "@odd", 290, y + 45, 0.2, 20);
    r.set("globalAlpha", 0.5);
    r.call("drawImage", "@alpha", 345, y + 40);
    r.call("drawImage", "@odd", 295, y + 50, 45, 30);
    r.set("globalAlpha", 1);
    r.call("save");
    r.call("translate", 330, y + 85);
    r.call("rotate", 0.3);
    r.call("drawImage", "@odd", 0, 0, 40, 26);
    r.call("restore");
    r.call("save");
    r.call("translate", 415, y + 80);
    r.call("rotate", Math.PI / 2);
    r.call("drawImage", "@field", 0, 0, 40, 27);
    r.call("restore");
  });
  return scene(`imgelements-${dpr}x`, "coverage", "pixel", [420, 265], dpr, LIGHT, r.cmds);
}

// Frames recorded from upstream's Timeline in dom/timeline vectors (asrs-hub's timeline
// traces equal them call for call), extracted to plain JSON numbers.
function timelineScenes(): Scene[] {
  return timelineTraces.map((t) =>
    scene(t.name, "timeline", "pixel", [t.width, t.height], t.dpr, t.dark ? DARK : LIGHT, t.cmds as Cmd[])
  );
}

// Frames recorded from upstream's LineGraphRenderer in dom/line-graph vectors, which
// asrs-tabs's renderer reproduces call for call.
function lineGraphScenes(): Scene[] {
  return lineGraphTraces.map((t) =>
    scene(t.name, "linegraph", "pixel", [t.width, t.height], t.dpr, t.dark ? DARK : LIGHT, t.cmds as Cmd[])
  );
}

// Frames recorded from upstream's StatisticsRenderer (Chart.js) in dom/histogram vectors, which
// asrs-tabs's histogram reproduces call for call.
function histogramScenes(): Scene[] {
  return histogramTraces.map((t) =>
    scene(t.name, "histogram", "pixel", [t.width, t.height], t.dpr, t.dark ? DARK : LIGHT, t.cmds as Cmd[])
  );
}

// A stand-in field image: carpet gradient, 2 px white lines every 48 px and two alliance
// blocks, so nearest-neighbour blits of it show any off-by-one in the source or destination
// rectangle.
function fieldImage(width: number, height: number): string {
  const data = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      let rgb = [60 + Math.floor((x * 60) / width), 90 + Math.floor((y * 50) / height), 70];
      if (x % 48 < 2 || y % 48 < 2) rgb = [235, 235, 235];
      if (x > width * 0.1 && x < width * 0.2 && y > height * 0.3 && y < height * 0.7) rgb = [200, 40, 40];
      if (x > width * 0.8 && x < width * 0.9 && y > height * 0.3 && y < height * 0.7) rgb = [40, 60, 200];
      data.set([...rgb, 255], i);
    }
  }
  return Buffer.from(data).toString("base64");
}

// Frames recorded from upstream's Field2dRenderer in dom/field-2d vectors, which asrs-tabs's
// renderer reproduces call for call. The field image is `fieldImage` at the recorded natural
// size; the heatmap canvas is left transparent (its pixels are `canvas heatmap`'s gate).
function field2dScenes(): Scene[] {
  return field2dTraces.map((t) => {
    const sources: Cmd[] = Object.entries(t.images as { [src: string]: number[] }).map(([src, [w, h]]) => [
      "image",
      src,
      w,
      h,
      fieldImage(w, h)
    ]);
    sources.push(["canvas", "heatmap", 1, 1, []]);
    return scene(t.name, "field2d", "pixel", [t.width, t.height], t.dpr, t.dark ? DARK : LIGHT, [...sources, ...(t.cmds as Cmd[])]);
  });
}

// Frames recorded from upstream's SwerveRenderer and JoysticksRenderer in dom/swerve and
// dom/joysticks vectors, which asrs-tabs's renderers reproduce call for call. Each layout image
// is `fieldImage` at its recorded size.
function r07Scenes(): Scene[] {
  return r07Traces.map((t) => {
    const sources: Cmd[] = Object.entries(t.images as { [src: string]: number[] }).map(([src, [w, h]]) => ["image", src, w, h, fieldImage(w, h)]);
    return scene(t.name, t.name.split("-")[0], "pixel", [t.width, t.height], t.dpr, t.dark ? DARK : LIGHT, [...sources, ...(t.cmds as Cmd[])]);
  });
}

export function allScenes(): Scene[] {
  const out: Scene[] = [
    lineGraph("linegraph-light-2x", 2, false),
    lineGraph("linegraph-dark-2x", 2, true),
    lineGraph("linegraph-light-1x", 1, false),
    lineGraph("linegraph-dark-1x", 1, true),
    textGrid("text-light-2x", 2, false),
    textGrid("text-dark-2x", 2, true),
    textGrid("text-light-1x", 1, false),
    textGrid("text-dark-1x", 1, true),
    shapes("shapes-2x", 2),
    shapes("shapes-1x", 1),
    longHairlines(),
    ...timelineScenes(),
    ...lineGraphScenes(),
    ...field2dScenes(),
    ...r07Scenes(),
    ...histogramScenes(),
    clipping("clip-1x", 1),
    clipping("clip-1.5x", 1.5),
    clipping("clip-2x", 2)
  ];
  for (const dpr of [1, 2]) {
    out.push(paths(dpr), transforms(dpr), rects(dpr, false), rects(dpr, true), dashes(dpr), colors(dpr));
    out.push(gradients(dpr), images(dpr), imageData(dpr), heatmap(dpr), textStyles(dpr, false), textStyles(dpr, true));
    out.push(textEmoji(dpr, false), textEmoji(dpr, true));
    out.push(offCanvas(dpr), gradientText(dpr, false), gradientText(dpr, true));
    out.push(curves(dpr), hugeCoords(dpr), strokeRects(dpr), multiFrame(dpr, ""), multiFrame(dpr, "-single"), multiFrame(dpr, "-resized"));
    out.push(floatMatrix(dpr), quarterTurns(dpr), gradientEdges(dpr, false), gradientEdges(dpr, true), scaledImages(dpr), imageElements(dpr));
  }
  return out;
}

export function calibrationScenes(): Scene[] {
  return LUM_LEVELS.map((_, k) => calibration(k));
}
