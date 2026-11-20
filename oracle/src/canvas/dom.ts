export interface DomScene {
  name: string;
  width: number;
  height: number;
  background: string;
  html: string;
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
    background,
    html:
      "<!doctype html><html><head><meta charset=\"utf-8\"><style>" +
      `html,body{margin:0;overflow:hidden;background:${background}}body{font-family:${FAMILY};color:${color};user-select:none}` +
      "</style></head><body>" +
      rows.join("") +
      "</body></html>"
  };
}

export function domScenes(): DomScene[] {
  return [textScene(false), textScene(true)];
}
