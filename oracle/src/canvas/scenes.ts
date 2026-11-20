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
  "\u{1f916} robot", "Á", "\t tab", "fi fl ffi", "1.23e+21", "-0"
];

export function measureFonts(): string[] {
  return [
    `12px ${SYSTEM_FONT}`,
    `200 12px ${SYSTEM_FONT}`,
    `200 14px ${SYSTEM_FONT}`,
    `16px ${SYSTEM_FONT}`,
    `bold 12px ${SYSTEM_FONT}`,
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
    shapes("shapes-1x", 1)
  ];
  for (const dpr of [1, 2]) {
    out.push(paths(dpr), transforms(dpr), rects(dpr, false), rects(dpr, true), dashes(dpr), colors(dpr));
    out.push(gradients(dpr), images(dpr), imageData(dpr), heatmap(dpr), textStyles(dpr, false), textStyles(dpr, true));
  }
  return out;
}

export function calibrationScenes(): Scene[] {
  return LUM_LEVELS.map((_, k) => calibration(k));
}
