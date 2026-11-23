// Fixtures of the Table, Console, Statistics and Metadata renderers' DOM for
// `crates/asrs-tabs/tests/dom_tab_views.rs`. Writes parity/specs/dom/{table-view,console-view,
// statistics-view,metadata-view}/*.entry.json; edit this script, not the specs.

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SPECS = path.join(ROOT, "parity/specs/dom");
const WWW = path.join(ROOT, "reference/AdvantageScope/www");

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

class Code {
  constructor(source) {
    this.source = source;
  }
}
const code = (source) => new Code(source);

// JavaScript source for a value, so NaN, -0, undefined, missing keys and byte arrays survive.
function lit(v) {
  if (v === undefined) return "undefined";
  if (v === null) return "null";
  if (v instanceof Code) return v.source;
  if (typeof v === "number") return Object.is(v, -0) ? "-0" : String(v);
  if (typeof v === "string" || typeof v === "boolean") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map(lit).join(",") + "]";
  return "{" + Object.entries(v).map(([k, x]) => JSON.stringify(k) + ":" + lit(x)).join(",") + "}";
}

const KINDS = { 4: "table", 5: "console", 6: "statistics", 12: "metadata" };

// Brackets every `render` and every driven event of a renderer between {"r11": "begin"} and
// {"r11": "end"}, the latter with the DOM the renderer left. Selection calls inside a bracket
// are {"r11": "selection"} records; geometry and main-message records are the harness's own.
const RECORDER = String.raw`(() => {
  const w = window;
  const KINDS = ${JSON.stringify(KINDS)};
  const write = (record) => w.__oracleWrite(JSON.stringify(record));
  const tree = w.__oracleTree;
  const quiet = w.__oracleQuiet;
  const instances = [];
  let active = false;

  const innerText = (el) =>
    Array.from(el.childNodes, (n) => {
      if (n.nodeType === 3) return n.data;
      if (n.localName === "br") return "\n";
      throw new Error("unexpected " + n.nodeName);
    }).join("");
  const onlyText = (el) => {
    if (Array.from(el.childNodes).some((n) => n.nodeType !== 3)) throw new Error("unexpected child");
    return el.textContent;
  };
  const classes = (el) => Array.from(el.classList).sort();
  const node = (n) =>
    n.nodeType === 3 ? n.data : [n.localName, n.getAttributeNames().map((a) => [a, n.getAttribute(a)]), Array.from(n.childNodes, node)];
  const hand = (r) => [r.HAND_ICON.style.transition, r.HAND_ICON.style.opacity];

  const DUMP = {
    table: (r) => {
      const body = Array.from(r.TABLE_BODY.children);
      return {
        headers: Array.from(body[0].children).slice(1).map((th) => {
          const [key, close] = th.children;
          if (th.children.length !== 2 || key.children.length !== 1) throw new Error("unexpected header cell");
          const text = key.firstElementChild;
          return [th.title, innerText(text), text.style.textDecoration, classes(key), close.title, close.hidden];
        }),
        fillers: [body[1], body[body.length - 1]].map((el) => (el.__r11Height !== undefined ? el.__r11Height : el.style.height)),
        rows: body.slice(2, -1).map((tr) => [classes(tr), tr.hidden, Array.from(tr.children, onlyText)]),
        hand: hand(r),
        placeholder: r.INPUT_FIELD.placeholder
      };
    },
    console: (r) => ({
      field: innerText(r.FIELD_TEXT),
      decoration: r.FIELD_TEXT.style.textDecoration,
      deleteHidden: r.FIELD_DELETE.hidden,
      exportButton: r.EXPORT_BUTTON === null ? null : r.EXPORT_BUTTON.hidden,
      highlight: r.HIGHLIGHT_BUTTON.classList.contains("active"),
      filter: r.FILTER_INPUT.value,
      rows: Array.from(r.TABLE_BODY.children).slice(1).map((tr) => {
        const [time, value] = tr.children;
        if (tr.children.length !== 2) throw new Error("unexpected line");
        return [classes(tr), onlyText(time), Array.from(value.childNodes, node)];
      }),
      hand: hand(r),
      placeholder: r.JUMP_INPUT.placeholder
    }),
    statistics: (r) => ({
      rows: Array.from(r.VALUES_TABLE_BODY.children, (tr) => [
        classes(tr),
        Array.from(tr.children, (td) => [innerText(td), td.colSpan, td.__r11Color === undefined ? null : td.__r11Color])
      ])
    }),
    metadata: (r) => ({
      alertHidden: r.NO_DATA_ALERT.hidden,
      tableHidden: r.TABLE_CONTAINER.hidden,
      headers: Array.from(r.TABLE_BODY.firstElementChild.children, innerText),
      rows: Array.from(r.TABLE_BODY.children).slice(1).map((tr) => Array.from(tr.children, (td) => [innerText(td), classes(td)]))
    })
  };

  const shadow = (el, property, store) =>
    Object.defineProperty(el.style, property, {
      configurable: true,
      get() {
        return this.getPropertyValue(property);
      },
      set(value) {
        el[store] = String(value);
        this.setProperty(property, value);
      }
    });

  const WATCHED = ["setHoveredTime", "setSelectedTime", "goIdle", "play", "pause", "lock", "unlock"];
  const bracket = (instance, op, run) => {
    if (active) throw new Error("nested bracket");
    active = true;
    write({
      r11: "begin",
      id: tree(instance.id),
      op: tree(op),
      dark: w.matchMedia("(prefers-color-scheme: dark)").matches,
      timestamps: tree(w.preferences.timestamps)
    });
    const sel = w.selection;
    const saved = WATCHED.map((name) => sel[name]);
    WATCHED.forEach((name, i) => {
      sel[name] = (...args) => {
        write({ r11: "selection", call: tree([name, ...args]) });
        return saved[i].apply(sel, args);
      };
    });
    const createElement = document.createElement;
    document.createElement = function (...args) {
      const el = createElement.apply(this, args);
      if (instance.kind === "statistics" && args[0] === "td") shadow(el, "color", "__r11Color");
      return el;
    };
    const end = { r11: "end", id: tree(instance.id) };
    try {
      const result = run();
      if (op[0] === "save") end.result = tree(result);
    } catch (e) {
      end.threw = tree(e.name + ": " + e.message);
      throw e;
    } finally {
      document.createElement = createElement;
      WATCHED.forEach((name, i) => (sel[name] = saved[i]));
      active = false;
      if (instance.kind === "table") end.range = tree(instance.renderer.getTimestampRange());
      end.dom = tree(quiet(() => DUMP[instance.kind](instance.renderer)));
      write(end);
    }
  };

  const hook = (kind, renderer, hasController) => {
    if (instances.some((i) => i.renderer === renderer)) return;
    const instance = { id: instances.length, kind, renderer };
    instances.push(instance);
    if (kind === "table") {
      shadow(renderer.fillerUpper, "height", "__r11Height");
      shadow(renderer.fillerLower, "height", "__r11Height");
    }
    const render = renderer.render;
    renderer.render = function (command) {
      return bracket(instance, ["render", command], () => render.call(this, command));
    };
    write({ r11: "create", id: tree(instance.id), kind, hasController, dom: tree(quiet(() => DUMP[kind](renderer))) });
    return instance.id;
  };
  const hookAll = () =>
    w.tabs.tabList.forEach((t) => {
      if (KINDS[t.type] !== undefined) hook(KINDS[t.type], t.renderer, true);
    });
  hookAll();
  const addTab = w.tabs.addTab;
  w.tabs.addTab = function (...args) {
    const result = addTab.apply(this, args);
    hookAll();
    return result;
  };

  const drive = (id, op, run) => {
    try {
      bracket(instances[id], op, () => run(instances[id].renderer));
    } catch (e) {}
  };
  const rowsOf = (r) => Array.from(r.TABLE_BODY.children).slice(1);
  w.__r11 = {
    // Another renderer of the first tab's class, on the hub's markup or on the given one.
    make(type, hasController, html) {
      const tab = w.tabs.tabList.find((t) => t.type === type);
      let root;
      if (html === undefined) root = document.getElementById("renderer" + type).cloneNode(true);
      else {
        const template = document.createElement("template");
        template.innerHTML = html;
        root = template.content.firstElementChild;
      }
      root.removeAttribute("id");
      root.hidden = false;
      document.querySelector("div.renderer-content").appendChild(root);
      hook(KINDS[type], new tab.renderer.constructor(root, hasController), hasController);
    },
    render(id, command) {
      try {
        instances[id].renderer.render(command);
      } catch (e) {}
    },
    // The command the first tab's controller gives this frame, to a renderer of its type.
    share(id, type) {
      this.render(id, w.tabs.tabList.find((t) => t.type === type).controller.getCommand());
    },
    filter(id, value) {
      drive(id, ["filter", value], (r) => {
        r.FILTER_INPUT.value = value;
        r.FILTER_INPUT.dispatchEvent(new Event("input", { bubbles: true }));
      });
    },
    highlight: (id) => drive(id, ["highlight"], (r) => r.HIGHLIGHT_BUTTON.click()),
    restore: (id, state) => drive(id, ["restore", state], (r) => r.restoreState(state)),
    save: (id) => drive(id, ["save"], (r) => r.saveState()),
    row: (id, type, index) => drive(id, ["row", type, index], (r) => rowsOf(r)[index].dispatchEvent(new MouseEvent(type))),
    tableRow: (id, type, index) => drive(id, ["row", type, index], (r) => r.dataRows[index].dispatchEvent(new MouseEvent(type))),
    jump: (id, value, enter) =>
      drive(id, ["jump", value], (r) => {
        const input = r.JUMP_INPUT ?? r.INPUT_FIELD;
        input.value = value;
        if (enter) input.dispatchEvent(new KeyboardEvent("keydown", { code: "Enter" }));
        else input.nextElementSibling.click();
      }),
    export: (id) => drive(id, ["export"], (r) => r.EXPORT_BUTTON.click()),
    mousemove: (id, y) =>
      drive(id, ["mousemove", y], (r) => r.TABLE_BODY.dispatchEvent(new MouseEvent("mousemove", { clientY: y }))),
    mouseleave: (id) => drive(id, ["mouseleave"], (r) => r.TABLE_BODY.dispatchEvent(new MouseEvent("mouseleave"))),
    scroll: (id, top) => quiet(() => (instances[id].renderer.TABLE_CONTAINER.scrollTop = top)),
    size: (width, height) =>
      quiet(() => {
        const content = document.querySelector("div.renderer-content");
        content.style.width = width;
        content.style.height = height;
        content.style.bottom = "auto";
      }),
    tab: (type) => w.tabs.tabList.find((t) => t.type === type)
  };
})()`;

const step = (source) => ({ eval: source });
const r11 = (method, ...args) => step(`window.__r11.${method}(${args.map(lit).join(", ")})`);
const restoreTab = (type, state) => step(`window.__r11.tab(${type}).controller.restoreState(${lit(state)})`);
const select = (...call) => ({ selection: call });
const advance = (ms) => ({ advance: ms });
const frames = (n) => ({ frames: n });
const pref = (value) => step(`window.preferences.timestamps = ${JSON.stringify(value)}`);
const theme = (value) => step(`window.__oracleTheme = ${JSON.stringify(value)}`);

function spec(area, id, steps, extra = {}) {
  const { inputs, ...fixture } = extra;
  const body = { entry: "dom", ...(inputs ? { inputs } : {}), fixture: { ...fixture, steps } };
  const dir = path.join(SPECS, area);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, `${id}.entry.json`), JSON.stringify(body, null, 1) + "\n");
}

// ---- Table ----

const TABLE = 4;
const bytes = (n) => Array.from({ length: n }, (_, i) => ((i * 37 + 5) % 256).toString(16).padStart(2, "0")).join("");
const range = (n) => Array.from({ length: n }, (_, i) => i);

const typesLog = [
  ["putRaw", "/T/Raw", 5.25, { b: "" }],
  ["putRaw", "/T/Raw", 5.5, { b: "00ff10" }],
  ["putRaw", "/T/Raw", 6, { b: bytes(700) }],
  ["putBoolean", "/T/Bool", 5.25, true],
  ["putBoolean", "/T/Bool", 5.75, false],
  ["putBoolean", "/T/Bool", 7, true],
  ...[0, { n: "8000000000000000" }, { n: "7ff8000000000000" }, { n: "7ff0000000000000" }, { n: "fff0000000000000" }, 1e21, 0.1 + 0.2, 1e-7, -12.5]
    .map((v, i) => ["putNumber", "/T/Num", 5.25 + i * 0.25, v]),
  ...["", "plain", "a\nb\tc", 'quote " back \\', "\u{1f7e9} emoji", "\ud800 lone", "  \u0000 \u001f", "<b>&amp;</b>"]
    .map((v, i) => ["putString", "/T/Str", 5.25 + i * 0.25, v]),
  ["putBooleanArray", "/T/BoolArr", 5.3, []],
  ["putBooleanArray", "/T/BoolArr", 5.6, [true, false]],
  ["putNumberArray", "/T/NumArr", 5.3, [1, 2.5, -0.125]],
  ["putNumberArray", "/T/NumArr", 6.2, range(700)],
  ["putStringArray", "/T/StrArr", 5.4, ["a", 'b"', "\n"]],
  ["putStringArray", "/T/StrArr", 6.4, range(600).map(String)],
  ["putNumber", "/T/Late", 7.123456, 42],
  ["createBlankField", "/T/Empty", 7],
  ["createBlankField", "/T/Blank", 2]
];
const ALL_TYPES = ["/T/Raw", "/T/Bool", "/T/Num", "/T/Str", "/T/BoolArr", "/T/NumArr", "/T/StrArr", "/T/Late", "/T/Empty", "/T/Blank"];

const START_TABLE = (log) => [{ log }, { main: "new-tab", data: TABLE }, step(RECORDER), frames(2)];
const TABLE_FIXTURE = { geometry: "real", seed: 1111 };

spec(
  "table-view",
  "types",
  [
    ...START_TABLE(typesLog),
    restoreTab(TABLE, ALL_TYPES),
    frames(2),
    select("setSelectedTime", 5.25),
    frames(1),
    select("setSelectedTime", 6.1),
    select("setHoveredTime", 7.2),
    frames(1),
    select("setHoveredTime", 6.1),
    frames(1),
    select("setSelectedTime", 0),
    frames(1),
    select("setHoveredTime", 0),
    frames(1),
    select("setHoveredTime", 1e9),
    frames(1),
    select("setHoveredTime", 5),
    select("goIdle"),
    frames(1),
    pref("original"),
    frames(1),
    select("setSelectedTime", 6.5),
    frames(1),
    restoreTab(TABLE, ["/T/Late", "/T/Missing", "/T/Num", "/T/Num"]),
    frames(2),
    restoreTab(TABLE, ["/T/Missing"]),
    frames(1),
    restoreTab(TABLE, []),
    frames(2),
    pref("start-zero"),
    restoreTab(TABLE, ["/T/Bool"]),
    select("setHoveredTime", 5.9),
    frames(1),
    { log: [["deleteField", "/T/Bool"]] },
    frames(1),
    restoreTab(TABLE, ["/T/Bool", "/T/Str"]),
    frames(1),
    { log: [["putString", "/T/Bool", 1, "now a string"]] },
    frames(2)
  ],
  TABLE_FIXTURE
);

const command = (fields, timestamps, extra = {}) => ({
  timestamps,
  displayOffset: 0,
  fields,
  selectionMode: 0,
  selectedTime: null,
  hoveredTime: null,
  ...extra
});
const field = (key, isAvailable, type, timestamps, values) => ({
  key,
  isAvailable,
  data: timestamps === undefined ? null : { timestamps, values },
  type
});

spec(
  "table-view",
  "direct",
  [
    ...START_TABLE(typesLog),
    r11("make", TABLE, true),
    r11("make", TABLE, false),
    ...[1, 2].flatMap((id) => [
      r11("render", id, command([], [])),
      r11("render", id, command([field("a\nb\r\nc\rd", true, 2, [1], [5]), field("", true, 3, [], [])], [1, 2, 3])),
      r11("render", id, command([field(null, false, null), field(5, false, null), field("x", true, null, [1], [5])], [1, 2, 3])),
      r11("render", id, command([field(null, false, null), field(5, false, null), field("x", true, 2, [1], [5])], [1, 2, 3])),
      r11("render", id, command([field(null, true, 2, [2], [7])], [1, 2, 3], { displayOffset: -1.5 })),
      r11("render", id, command([field("u", false, null)], [-2.0005, -0.25, 0, 0.0004, 1e21], { hoveredTime: -0.25 })),
      r11("render", id, command([field("u", false, null)], [-2, -1, 1], { selectionMode: 1, selectedTime: -1, hoveredTime: 0 })),
      r11("render", id, command([field("u", false, null)], [-2, -1, 1], { selectionMode: 1, selectedTime: NaN, hoveredTime: -3 })),
      r11("render", id, command([field("u", false, null)], [-2, -1, 1], { selectionMode: 3, selectedTime: 1, hoveredTime: 1 })),
      r11("render", id, command([field("u", false, null)], [], { selectionMode: 3, selectedTime: 1, hoveredTime: 1 })),
      r11("render", id, command([field("u", false, null)], [4], { selectionMode: 1, selectedTime: null, hoveredTime: null }))
    ])
  ],
  TABLE_FIXTURE
);

const longLog = range(420).map((i) => ["putNumber", "/L/A", 2 + i * 0.1, i]);
const longLog2 = [...longLog, ...range(90).map((i) => ["putBoolean", "/L/B", 2.05 + i * 0.4, i % 3 === 0])];

spec(
  "table-view",
  "scroll",
  [
    ...START_TABLE(longLog2),
    restoreTab(TABLE, ["/L/A", "/L/B"]),
    frames(2),
    ...[1000, 333.3, 1e9, 0, 5200].flatMap((top) => [r11("scroll", 0, top), frames(2)]),
    r11("mousemove", 0, 300),
    frames(2),
    r11("mousemove", 0, 100),
    frames(1),
    r11("mouseleave", 0),
    frames(2),
    r11("tableRow", 0, "click", 7),
    frames(1),
    r11("jump", 0, "30", true),
    frames(2),
    r11("jump", 0, "", false),
    frames(1),
    r11("tableRow", 0, "contextmenu", 3),
    frames(1),
    r11("jump", 0, "", true),
    frames(2),
    select("setSelectedTime", 12),
    select("play"),
    advance(800),
    select("pause"),
    r11("size", "500px", "120px"),
    frames(2),
    r11("scroll", 0, 3000),
    frames(2),
    r11("size", "700px", "520px"),
    frames(2),
    r11("scroll", 0, 4000),
    frames(1),
    { log: [["clearBeforeTime", 8]] },
    frames(2),
    { log: [["clearBeforeTime", 36]] },
    frames(2),
    { log: [["clearBeforeTime", 43]] },
    frames(2)
  ],
  TABLE_FIXTURE
);

spec(
  "table-view",
  "live",
  [
    ...START_TABLE(longLog2),
    restoreTab(TABLE, ["/L/A", "/L/B"]),
    { liveTime: 44 },
    select("setLiveConnected"),
    frames(2),
    ...range(4).flatMap((i) => [
      { liveTime: 45 + i },
      { log: range(6).map((k) => ["putNumber", "/L/A", 44.5 + i + k * 0.1, i * 10 + k]) },
      frames(1)
    ]),
    select("setHoveredTime", 47),
    frames(1),
    r11("scroll", 0, 2000),
    frames(3),
    select("lock"),
    frames(2),
    restoreTab(TABLE, ["/L/B"]),
    frames(2),
    restoreTab(TABLE, ["/L/Nope"]),
    frames(2),
    { log: [["putBoolean", "/L/Nope", 46, true]] },
    frames(2),
    select("setLiveDisconnected"),
    frames(2)
  ],
  TABLE_FIXTURE
);

spec(
  "table-view",
  "satellite",
  [
    ...START_TABLE([...longLog2, ...typesLog]),
    r11("make", TABLE, false),
    restoreTab(TABLE, ["/L/A", "/T/Str", "/L/B"]),
    frames(2),
    r11("share", 1, TABLE),
    r11("scroll", 1, 6000),
    r11("share", 1, TABLE),
    step(`window.__r11.tab(4).controller.addRendererRange("sat", [26, 30])`),
    frames(1),
    r11("share", 1, TABLE),
    step(`window.__r11.tab(4).controller.addRendererRange("sat2", [4, 27])`),
    r11("share", 1, TABLE),
    frames(1),
    r11("mousemove", 1, 250),
    r11("share", 1, TABLE),
    r11("tableRow", 1, "click", 20),
    r11("share", 1, TABLE),
    restoreTab(TABLE, ["/T/Missing"]),
    r11("share", 1, TABLE)
  ],
  TABLE_FIXTURE
);

{
  const rand = mulberry32(4711);
  const pick = (list) => list[Math.floor(rand() * list.length)];
  const pool = ["/L/A", "/L/B", "/T/Str", "/T/Num", "/T/Bool", "/T/Missing", "/T/Raw"];
  const steps = [...START_TABLE([...longLog2, ...typesLog]), r11("make", TABLE, false), restoreTab(TABLE, ["/L/A"])];
  for (let i = 0; i < 90; i++) {
    const time = Math.round(rand() * 46000) / 1000;
    steps.push(
      pick([
        () => restoreTab(TABLE, range(Math.floor(rand() * 4)).map(() => pick(pool))),
        () => r11("scroll", pick([0, 1]), Math.round(rand() * 11000) / 2),
        () => select("setHoveredTime", pick([null, 0, time])),
        () => select("setSelectedTime", pick([0, time, time])),
        () => select("goIdle"),
        () => r11("mousemove", pick([0, 1]), Math.floor(rand() * 400)),
        () => r11("mouseleave", pick([0, 1])),
        () => r11("size", `${300 + Math.floor(rand() * 500)}px`, `${Math.floor(rand() * 520)}px`),
        () => pref(pick(["original", "start-zero"]))
      ])()
    );
    steps.push(pick([frames(1), r11("share", 1, TABLE)]));
  }
  spec("table-view", "random", steps, TABLE_FIXTURE);
}

// ---- Console ----

const CONSOLE = 5;
const satelliteHtml = fs.readFileSync(path.join(WWW, "satellite.html"), "utf8");
const satelliteConsole = (() => {
  const start = satelliteHtml.indexOf('<div id="renderer5"');
  const end = satelliteHtml.indexOf("<!-- Statistics -->");
  if (start < 0 || end < start) throw new Error("satellite.html has no console renderer");
  return satelliteHtml.slice(start, end).trim().replace(/\n\s+/g, "\n");
})();

const CONSOLE_FIXTURE = { geometry: "real", seed: 1112 };
const START_CONSOLE = (log) => [{ log }, { main: "new-tab", data: CONSOLE }, step(RECORDER), frames(1)];
const line = (timestamp, value, flags = "") => ({
  timestamp,
  value,
  isError: flags.includes("e"),
  isWarning: flags.includes("w")
});
const lines = (list, extra = {}) => ({
  key: "/Console",
  keyAvailable: true,
  lines: list,
  displayOffset: 0,
  selectionMode: 0,
  selectedTime: null,
  hoveredTime: null,
  ...extra
});

const ANSI = [
  "plain text",
  "",
  "\x1b[31mred\x1b[0m and \x1b[1;32mbold green\x1b[0m",
  "\\x1b[33mescaped\\x1b[0m \\033[4munderline \\e[3mitalic\\e[0m",
  "\x1b[38;5;208m256\x1b[48;2;10;20;30mrgb\x1b[2mfaint\x1b[0m",
  "two\nlines\\nthree\\r\\nfour\r\nfive\rsix",
  "<b>html</b> & \"quotes\" 'single' a>b",
  "\x1b]8;;https://example.com/?a=1&b=2\x07link\x1b]8;;\x07 after",
  "nul\u0000 tab\t ls  lone\ud800 emoji\u{1f600}",
  "\x1b[7mreverse\x1b[27m \x1b[9mstrike",
  "unterminated \x1b[31",
  "Error: WARNING amp & lt < gt >"
];
const ansiLines = ANSI.map((v, i) => line(i * 0.5, v, ["", "e", "w", "ew"][i % 4]));

spec(
  "console-view",
  "ansi",
  [
    ...START_CONSOLE([]),
    r11("make", CONSOLE, true),
    r11("make", CONSOLE, false, satelliteConsole),
    ...[1, 2].flatMap((id) => [
      r11("render", id, lines(ansiLines)),
      ...["e", "E", "amp", "&", ";", "<", "lt", "\n", "a\nm\rp", "quot", "!", "!e", "!zzz", "br", "span", "ERROR", "\u{1f600}", "\ud83d", "#x27", ""]
        .map((f) => r11("filter", id, f)),
      r11("highlight", id),
      r11("filter", id, "t"),
      r11("highlight", id),
      r11("filter", id, "")
    ])
  ],
  CONSOLE_FIXTURE
);

// Upstream keeps `renderedValues` at its longest length, so rows a filter removed come back
// against the values of other rows.
const A = line(1, "alpha one");
const B = line(2, "beta two");
const C = line(2, "gamma two");
spec(
  "console-view",
  "stale",
  [
    ...START_CONSOLE([]),
    r11("make", CONSOLE, true),
    r11("render", 1, lines([A, B, line(3, "alpha three"), line(4, "beta four")])),
    r11("filter", 1, "beta"),
    r11("filter", 1, "BETA"),
    r11("filter", 1, ""),
    r11("filter", 1, "alpha"),
    r11("filter", 1, "a"),
    r11("filter", 1, ""),
    r11("render", 1, lines([A])),
    r11("render", 1, lines([A, B])),
    r11("render", 1, lines([A, C])),
    r11("render", 1, lines([A, B])),
    r11("render", 1, lines([])),
    r11("render", 1, lines([B, A])),
    r11("render", 1, lines([A, A, A])),
    r11("render", 1, lines([A, C, C])),
    r11("render", 1, lines([A])),
    r11("render", 1, lines([A, C])),
    r11("render", 1, lines([A, B])),
    r11("render", 1, lines([A, line(2, "alpha one")])),
    r11("render", 1, lines([line(1, "İx"), line(2, "i̇x")])),
    r11("filter", 1, "İ"),
    r11("filter", 1, "i̇")
  ],
  CONSOLE_FIXTURE
);

const consoleLog = range(120).map((i) => [
  "putString",
  "/Console",
  3 + i * 0.25,
  ["Robot program starting", "\x1b[31mERROR\x1b[0m something failed", "Warning: loop overrun", "multi\nline\nvalue", `value ${i}`][i % 5] + ` #${i}`
]);
spec(
  "console-view",
  "controller",
  [
    ...START_CONSOLE([...consoleLog, ["putNumber", "/Num", 3, 1]]),
    r11("save", 0),
    r11("export", 0),
    restoreTab(CONSOLE, "/Console"),
    frames(2),
    r11("export", 0),
    select("setSelectedTime", 5),
    frames(1),
    select("setHoveredTime", 9.9),
    frames(1),
    select("setHoveredTime", 0.5),
    frames(1),
    select("setHoveredTime", 0),
    frames(1),
    r11("row", 0, "mouseenter", 4),
    frames(1),
    r11("row", 0, "mouseleave", 4),
    frames(1),
    r11("row", 0, "click", 9),
    frames(1),
    r11("filter", 0, "error"),
    frames(1),
    r11("row", 0, "mouseenter", 4),
    r11("row", 0, "click", 4),
    frames(1),
    r11("jump", 0, "12", true),
    r11("jump", 0, "12.1", false),
    r11("filter", 0, ""),
    ...["12", "0", "3", "3.2", "-4", "1e9", "", "abc"].map((v, i) => r11("jump", 0, v, i % 2 === 0)),
    r11("row", 0, "contextmenu", 2),
    frames(1),
    r11("jump", 0, "", true),
    r11("highlight", 0),
    r11("save", 0),
    frames(1),
    r11("restore", 0, { highlight: false }),
    r11("restore", 0, { highlight: "yes" }),
    r11("restore", 0, null),
    r11("restore", 0, 5),
    r11("restore", 0, []),
    r11("restore", 0, { highlight: true }),
    pref("original"),
    frames(1),
    { liveTime: 40 },
    select("setLiveConnected"),
    frames(2),
    { log: [["putString", "/Console", 39, "live error"]] },
    frames(2),
    r11("scroll", 0, 100),
    frames(2),
    select("lock"),
    frames(2),
    select("setLiveDisconnected"),
    frames(1),
    pref("start-zero"),
    frames(1),
    restoreTab(CONSOLE, "/Missing"),
    frames(1),
    restoreTab(CONSOLE, "/Num"),
    frames(1),
    restoreTab(CONSOLE, null),
    frames(1)
  ],
  CONSOLE_FIXTURE
);

spec(
  "console-view",
  "direct",
  [
    ...START_CONSOLE([]),
    r11("make", CONSOLE, true),
    r11("make", CONSOLE, false, satelliteConsole),
    ...[1, 2].flatMap((id) => [
      r11("render", id, lines([], { key: null, keyAvailable: false })),
      r11("render", id, lines([A, B], { key: "a\nb\r\nc\rd" })),
      r11("render", id, lines([A, B], { key: "", keyAvailable: false })),
      r11("render", id, lines([A, B], { key: "" })),
      r11("render", id, lines([line(-1.5, "neg"), line(0, "zero"), line(0.0004, "tiny"), line(1e21, "huge")], { displayOffset: -0.25 })),
      r11("render", id, lines([line(-1.5, "neg"), line(0, "zero"), line(0.0004, "tiny"), line(1e21, "huge")], { displayOffset: -0.25, hoveredTime: -9 })),
      r11("jump", id, "-0.25", true),
      r11("render", id, lines([line(-1.5, "neg"), line(0, "zero")], { selectionMode: 1, selectedTime: null, hoveredTime: NaN })),
      r11("jump", id, "", true),
      r11("render", id, lines([line(-1.5, "neg"), line(0, "zero")], { selectionMode: 2, selectedTime: -1, hoveredTime: -1 })),
      r11("render", id, lines([], { selectionMode: 1, selectedTime: 3, hoveredTime: 3 })),
      r11("jump", id, "5", true),
      r11("render", id, lines([A], { selectionMode: 1, selectedTime: 3, hoveredTime: 3 })),
      r11("render", id, lines([A, B], { selectionMode: 1, selectedTime: 3, hoveredTime: 1 })),
      r11("render", id, lines([A], { selectionMode: 3, selectedTime: 3, hoveredTime: 3 })),
      r11("render", id, lines([line(NaN, "nan")], { hoveredTime: 2 })),
      r11("render", id, lines([line(NaN, "nan")], { hoveredTime: 2 }))
    ])
  ],
  CONSOLE_FIXTURE
);

for (const [id, seed, satellite] of [
  ["random", 99, false],
  ["random-satellite", 100, true]
]) {
  const rand = mulberry32(seed);
  const pick = (list) => list[Math.floor(rand() * list.length)];
  const words = ["a", "b", "ab", "A&b", "\x1b[31mab\x1b[0m", "b\na", "<a>", ""];
  let current = lines([]);
  const steps = [...START_CONSOLE([]), satellite ? r11("make", CONSOLE, false, satelliteConsole) : r11("make", CONSOLE, true)];
  for (let i = 0; i < 220; i++) {
    const t = () => Math.floor(rand() * 4);
    const op = pick(["lines", "lines", "lines", "push", "filter", "filter", "highlight", "select", "offset", "key", "pref"]);
    if (op === "filter") steps.push(r11("filter", 1, pick(["", "", "a", "A", "ab", "!a", "!", "b", "&", "\x1b", "31"])));
    else if (op === "highlight") steps.push(r11("highlight", 1));
    else if (op === "pref") steps.push(pref(pick(["original", "start-zero"])));
    else {
      current = { ...current };
      if (op === "lines") {
        current.lines = range(Math.floor(rand() * 6))
          .map(() => line(t(), pick(words), pick(["", "", "e", "w", "ew"])))
          .sort((x, y) => x.timestamp - y.timestamp);
      } else if (op === "push") {
        current.lines = [...current.lines, line(4 + i * 0.01, pick(words), pick(["", "e", "w"]))];
      } else if (op === "select") {
        current.selectionMode = Math.floor(rand() * 4);
        current.selectedTime = pick([null, 0, t(), t() + 0.5]);
        current.hoveredTime = pick([null, 0, t(), t() + 0.5]);
      } else if (op === "offset") current.displayOffset = pick([0, -1.25, 3]);
      else {
        current.key = pick([null, "/Console", "/Other"]);
        current.keyAvailable = rand() < 0.5;
      }
      steps.push(r11("render", 1, current));
    }
  }
  spec("console-view", id, steps, CONSOLE_FIXTURE);
}

// ---- Statistics ----

const STATISTICS = 6;
const STATS = [
  "count", "min", "max", "mean", "median", "mode", "geometricMean", "harmonicMean", "quadraticMean", "standardDeviation",
  "medianAbsoluteDeviation", "interquartileRange", "skewness", "percentile01", "percentile05", "percentile10", "percentile25",
  "percentile50", "percentile75", "percentile90", "percentile95", "percentile99"
];
const stats = (values) => Object.fromEntries(STATS.map((name, i) => [name, values[i % values.length]]));
const statField = (title, color, values) => ({ title, color, histogramCounts: [1, 2], stats: stats(values) });
let counter = 100;
const statsCommand = (fields, extra = {}) => ({ changeCounter: counter++, bins: [0, 1], stepSize: 1, fields, ...extra });
const NUMBERS = [0, -0, 1, -1, 0.0005, 0.0004999, 1.0005, 2.5, 1e21, 1.5e21, 123456.7895, -0.00001, NaN, Infinity, -Infinity, 1 / 3, 5e-324, 1e20, 8.345, 1.255];
const COLORS = ["#2b66a2", "#e5b31b", "#af2437", "#80588e", "#e48b32", "#c0b487", "#858584", "#3b875a", "#d993aa", "#eb987e", "#5f4528", "#ffffff", "#000000", "#FF00ff", "#abc", "", "red", "#12"];

spec(
  "statistics-view",
  "direct",
  [
    { main: "new-tab", data: STATISTICS },
    step(RECORDER),
    r11("make", STATISTICS, true),
    r11("render", 1, statsCommand([], { changeCounter: 0 })),
    r11("render", 1, statsCommand([statField("same counter", "#2b66a2", [1])], { changeCounter: 0 })),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", NUMBERS)])),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", NUMBERS), statField("/B", "#e5b31b", NUMBERS.slice(1)), statField("/C", "#af2437", NUMBERS.slice(2))])),
    ...COLORS.map((c) => r11("render", 1, statsCommand([statField("/A", c, [1.5, NaN])]))),
    theme("dark"),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1])], { changeCounter: counter - 1 })),
    ...COLORS.map((c) => r11("render", 1, statsCommand([statField("/A", c, [1.5, NaN])]))),
    theme("light"),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1])], { changeCounter: counter - 1 })),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1, undefined, 2])])),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1, 2, null])])),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1, "5"])])),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1, "x"])])),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1, true])])),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [1]), statField("/B", undefined, [2])])),
    r11("render", 1, statsCommand([statField("/A", null, [1])])),
    r11("render", 1, statsCommand([statField("/A", "#2b66a2", [3])]))
  ],
  { seed: 1113 }
);

const statsSource = (logKey, type, color, extra = {}) => ({ type, logKey, logType: "Number", visible: true, options: { color }, ...extra });
for (const dark of [false, true]) {
  spec(
    "statistics-view",
    dark ? "controller-dark" : "controller",
    [
      { log: range(200).map((i) => ["putNumber", "/S/A", i * 0.05, Math.sin(i) * 5]) },
      { log: range(80).map((i) => ["putNumber", "/S/B", 1 + i * 0.1, (i * 7) % 11]) },
      { main: "new-tab", data: STATISTICS },
      step(RECORDER),
      advance(300),
      restoreTab(STATISTICS, {
        sources: [statsSource("/S/A", "independent", "#2b66a2"), statsSource("/S/B", "independent", "#e5b31b")],
        timeRange: "full",
        rangeMin: 0,
        rangeMax: 10,
        stepSize: 1
      }),
      advance(300),
      { log: [["putNumber", "/S/B", 20, 400]] },
      advance(300),
      restoreTab(STATISTICS, {
        sources: [statsSource("/S/A", "reference", "#000000"), statsSource("/S/B", "relativeError", "#ffffff"), statsSource("/S/B", "absoluteError", "#af2437")],
        timeRange: "full"
      }),
      advance(300),
      restoreTab(STATISTICS, { sources: [] }),
      advance(300)
    ],
    { seed: 1114, window: { theme: dark ? "dark" : "light" } }
  );
}

// ---- Metadata ----

const METADATA = 12;
const slot = (generic = null, real = null, replay = null) => ({ generic, real, replay });
const metaCommands = [
  {},
  {},
  { "/A": slot("1") },
  { "/A": slot("1") },
  { "/A": slot("1"), "/B": slot(null, "r") },
  { "/A": slot("1"), "/B": slot(null, "r", "p"), "/C": slot("x", "y", "z") },
  { "/B": slot(null, "r", "p") },
  { "/B": slot(null, null, "p") },
  { "/B": slot() },
  { "/A": slot(undefined), "/B": slot("v") },
  { "/A": slot(undefined) },
  { "/A": slot("a\nb\r\nc\rd", "", " spaced  text ") },
  { "/Multi\nline": slot("\u{1f600}\ud800\u0000<b>&") },
  { "": slot("empty"), X: slot("one"), "5": slot("index"), "10": slot("ten"), "/": slot("slash") },
  {
    "/Log0/GitSHA": slot("a"), "/Log1/GitSHA": slot("b"), "/GitSHA": slot("c"), "/Log0/Branch": slot("d"), "/Log10/Branch": slot("e"),
    "/Branch": slot("f"), "/Log": slot("g"), "/Logger": slot("h"), "/LogX/Zed": slot("i"), "/Log1": slot("j"), "/log0/lower": slot("k")
  },
  Object.fromEntries(
    ["/b", "/B", "/a", "/A", "/é", "/e", "/E", "/z", "/_x", "/-x", "/1", "/10", "/2", "/a b", "/ab", "/a-b", "/中", "/\u{1f600}", "/ss", "/ß"].map(
      (k, i) => [k, slot(i % 3 === 0 ? null : String(i), i % 2 === 0 ? null : "r" + i)]
    )
  ),
  {}
];
const rand = mulberry32(12);
for (let i = 0; i < 30; i++) {
  const keys = range(Math.floor(rand() * 9)).map(
    () => (rand() < 0.4 ? `/Log${Math.floor(rand() * 3)}` : "") + "/" + range(1 + Math.floor(rand() * 3)).map(() => "aAbB_1-z"[Math.floor(rand() * 8)]).join("")
  );
  const value = () => (rand() < 0.5 ? null : "v" + Math.floor(rand() * 5));
  metaCommands.push(Object.fromEntries(keys.map((k) => [k, slot(value(), value(), value())])));
}

spec(
  "metadata-view",
  "direct",
  [{ main: "new-tab", data: METADATA }, step(RECORDER), r11("make", METADATA, true), ...metaCommands.map((c) => r11("render", 1, c))],
  { seed: 1115 }
);

spec(
  "metadata-view",
  "controller",
  [
    { main: "new-tab", data: METADATA },
    step(RECORDER),
    frames(2),
    { log: [["putString", "/Metadata/Generic", 0, "g"], ["putString", "/RealOutputs/Other", 0, "x"]] },
    frames(2),
    {
      log: [
        ["putString", "/RealMetadata/GitSHA", 0, "real"],
        ["putString", "/ReplayMetadata/GitSHA", 0, "replay"],
        ["putString", "/Log0/RealMetadata/GitSHA", 0, "merged"],
        ["putString", "/Log1/Metadata/Generic", 0, "merged1"],
        ["putNumber", "/Metadata/Number", 0, 5]
      ]
    },
    frames(2),
    { log: [["createBlankField", "/Metadata/Blank", 3], ["putString", "/Metadata/Generic", 5, "later\nvalue"]] },
    frames(2)
  ],
  { seed: 1116 }
);

console.log("r11: specs written");
