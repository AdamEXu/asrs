// Writes the form-control fixtures parity/specs/form/{number-input,range-input,select,css-value}
// .entry.json. Run from the repo root:
//   node oracle/tools/formSpecs.mjs
// Each fixture is a list of `form` steps (oracle/README.md, "Form vectors") over the markup
// upstream uses; the vectors record what Chromium reads back after each assignment.

import fs from "node:fs";
import path from "node:path";

const SPECS = "parity/specs/form";

const NUMBER_STRINGS = [
  "5", "-0.5", ".5", "-.5", "0", "-0", "00012", "0.1", "0.30000000000000004", "1e3", "1E+3", "2e-1",
  "1e-7", "1e21", "1e308", "1.7976931348623157e308", "1.7976931348623159e308", "1e400", "-1e400",
  "4.9e-324", "2e-324", "1e-400", "9007199254740993", "123456789012345678901234567890",
  "33.333333333333336", "59.639997482299805", "1.4999999999999999", "7.000000000000001",
  "5.", "1.e5", "1.E5", ".", "-", "+5", "--5", "e5", "1e", "1e+", "1.5e3.2", "0x10", "1_000", "1,5",
  " 5", "5 ", "\t5", "5\n", " 5", "١٢", "５", "abc", "NaN", "Infinity",
  "-Infinity", ""
];

const RANGE_VALUES = [
  "59.639997482299805", "59.5", "60.5", "1.5", "2.5", "7.000000000000001", "1.4999999999999999",
  "1e1", "abc", "", "1e400", "+5", "1e3", "301", "-5", "0", "45", "300", "1.e5", " 45", "45 "
];

const CSS_LENGTHS = [
  "33.333333333333336%", "100.00000000000001%", "66.66666666666667%", "0.1%", "1e-7%", "1e21%",
  "NaN%", "Infinity%", "-5%", "0%", "100%", "50.5px", "0.30000000000000004px", "1e21px", "1e-7px",
  "-0px", "0px", "12.5px", "500px", "", "initial", "auto", "5", "5 px"
];

const COLORS = [
  "#ff0000", "#FF000080", "#abc", "red", "rgb(255, 0, 0)", "rgba(1,2,3,0.5)", "hsl(120, 100%, 50%)",
  "transparent", "currentcolor", "notacolor", ""
];

const OPTION_TEXTS = ["Xbox Controller", "  Pad \t X  ", "Two\nLines", "a \r\n b", "   ", "", "Field <b>"];

const numberProbes = [
  ...NUMBER_STRINGS.map((s) => ({
    markup: '<input class="max-speed" type="number" value="5" title="Maximum speed (meters per second)">',
    ops: [["set", "value", s]],
    read: ["value", "valueAsNumber"]
  })),
  ...NUMBER_STRINGS.map((s) => ({
    markup: `<input type="number" value="${s.replace(/&/g, "&amp;").replace(/"/g, "&quot;")}">`,
    read: ["value", "valueAsNumber"]
  })),
  {
    markup: '<input class="data-table-jump-input" type="number" placeholder="0.000" title="Go to timestamp">',
    ops: NUMBER_STRINGS.map((s) => ["set", "value", s]),
    read: ["value", "valueAsNumber"]
  }
];

const SLIDER = '<input class="video-timeline-slider" id="timeline" type="range" value="0" min="1" max="1" step="1" title="Video timeline">';
const RANGE_READ = ["value", "valueAsNumber", "min", "max", "step"];
const rangeProbes = [
  {
    markup: SLIDER,
    ops: [["set", "max", "300"], ...RANGE_VALUES.map((v) => ["set", "value", v])],
    read: RANGE_READ
  },
  {
    markup: SLIDER,
    ops: [
      ["set", "max", "300"],
      ["set", "value", "300"],
      ["set", "max", "100"],
      ["set", "max", "10.5"],
      ["set", "value", "11"],
      ["set", "max", "0"],
      ["set", "value", "5"],
      ["set", "value", "abc"],
      ...["abc", "1e400", " 120", "121x", " 120", "120", "-10", "1e-3"].flatMap((max) => [
        ["set", "max", max],
        ["set", "value", "abc"]
      ])
    ],
    read: RANGE_READ
  },
  {
    markup: SLIDER,
    ops: [
      ["set", "max", "300"],
      ["set", "value", "150"],
      ["set", "min", "200"],
      ["set", "min", "abc"],
      ["set", "min", "-50"],
      ["set", "value", "-49.5"],
      ["setAttribute", "step", "any"],
      ["set", "value", "59.639997482299805"],
      ["setAttribute", "step", "0.5"],
      ["set", "value", "59.6"],
      ["set", "value", "59.75"],
      ["setAttribute", "step", "0"],
      ["set", "value", "59.6"],
      ["setAttribute", "step", "-1"],
      ["set", "value", "59.6"],
      ["setAttribute", "step", "abc"],
      ["set", "value", "59.6"],
      ["setAttribute", "value", "7"]
    ],
    read: RANGE_READ
  },
  { markup: '<input type="range">', ops: RANGE_VALUES.map((v) => ["set", "value", v]), read: RANGE_READ },
  { markup: '<input type="range" min="1" max="1" step="1" value="0">', read: RANGE_READ },
  { markup: '<input type="range" value="250" min="1" max="300" step="1">', read: RANGE_READ },
  { markup: '<input type="range" value="2.5" min="1" max="300" step="1">', read: RANGE_READ },
  {
    markup: '<input type="range" value="0.5" max="10">',
    ops: [
      ["set", "value", "3"],
      ["setAttribute", "value", "0.25"],
      ["set", "value", "3"],
      ["set", "min", "abc"],
      ["set", "min", "0"]
    ],
    read: RANGE_READ
  },
  {
    markup: '<input type="range" value="0.5" max="10">',
    ops: [
      ["setAttribute", "value", "4.25"],
      ["set", "max", "5"],
      ["setAttribute", "value", "abc"]
    ],
    read: RANGE_READ
  }
];

const optionReads = (n) =>
  ["value", "selectedIndex", "options.length"].concat(
    Array.from({ length: n }, (_, i) => [`options.${i}.value`, `options.${i}.text`, `options.${i}.textContent`, `options.${i}.innerText`, `options.${i}.label`]).flat()
  );
const selectProbes = [
  {
    markup: '<select title="Select joystick data source"></select>',
    ops: [
      ...OPTION_TEXTS.map((text) => ["appendElement", "option", [["innerText", text]]]),
      ["set", "value", "Pad X"],
      ["set", "value", "nonexistent"],
      ["set", "selectedIndex", 9],
      ["set", "selectedIndex", 1],
      ["set", "value", ""],
      ["remove", 1],
      ["remove", 0],
      ["set", "selectedIndex", -1],
      ["appendElement", "option", [["innerText", "Late"]]]
    ],
    read: optionReads(OPTION_TEXTS.length)
  },
  {
    markup: '<select class="field-select" title="Select field image"><option>A</option><option value="v">B</option></select>',
    ops: [
      ["set", "value", "B"],
      ["set", "value", "v"],
      ["set", "value", "A"],
      ["remove", 0],
      ["remove", 0],
      ["appendElement", "option", [["innerText", "C"]]],
      ["appendElement", "option", [["innerText", "D"]]]
    ],
    read: optionReads(2)
  },
  {
    markup: '<select class="time-range" title="Select time range"><option value="full">Full Log</option><option value="enabled">Enabled</option><option value="auto">Auto</option></select>',
    ops: [
      ["set", "value", "auto"],
      ["set", "value", "AUTO"],
      ["set", "value", "enabled"]
    ],
    read: optionReads(3)
  }
];

const cssProbes = [
  ...["width", "height", "left", "top", "right"].map((prop) => ({
    markup: "<div></div>",
    ops: CSS_LENGTHS.map((v) => ["set", `style.${prop}`, v]),
    read: [`style.${prop}`, "style.cssText"]
  })),
  ...["color", "backgroundColor"].map((prop) => ({
    markup: "<div></div>",
    ops: COLORS.map((v) => ["set", `style.${prop}`, v]),
    read: [`style.${prop}`, "style.cssText"]
  })),
  {
    markup: "<div></div>",
    ops: ["0.15", "0", "1", "1.5", "-1", "abc", "50%", ""].map((v) => ["set", "style.opacity", v]),
    read: ["style.opacity", "style.cssText"]
  },
  {
    markup: "<div></div>",
    ops: ["none", "initial", "", "block"].map((v) => ["set", "style.display", v]),
    read: ["style.display", "style.cssText"]
  }
];

function write(name, probes) {
  const spec = { entry: "dom", fixture: { boot: [], steps: probes.map((form) => ({ form })) } };
  const file = path.join(SPECS, `${name}.entry.json`);
  fs.mkdirSync(SPECS, { recursive: true });
  fs.writeFileSync(file, JSON.stringify(spec, null, 2) + "\n");
  console.log(`${file}: ${probes.length} probes`);
}

write("number-input", numberProbes);
write("range-input", rangeProbes);
write("select", selectProbes);
write("css-value", cssProbes);
