#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/sidebar/<case>.entry.json` against the ported
//! `asrs_hub::sidebar::Sidebar` and compares the emitted records with the
//! committed `dom/sidebar/<case>` vector, line for line. The runner only
//! converts fixture inputs to model calls and the resulting view state back to
//! CVE-1 nodes; all sidebar behavior lives in `asrs-hub`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_hub::sidebar::{
    DragPayload, EventOutcome, INDENT_SIZE_PX, RowRect, RowValue, Sidebar, SidebarEnv, SidebarRow,
    Tuner,
};
use asrs_js::string::JsString;
use asrs_js::value::{JsValue, ToJs};
use asrs_log::log::{Log, LoggableType, RawValue};
use asrs_model::hub_state::SidebarState;
use asrs_model::selection::{Selection, SelectionMode, TimeSupplier};
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors;

fn err(msg: impl Into<String>) -> String {
    msg.into()
}

fn number(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| err(format!("expected a number, got {}", v.kind())))
}

fn opt_number(v: &Value) -> Result<Option<f64>, String> {
    Ok(match v {
        Value::Null | Value::Undefined => None,
        _ => Some(number(v)?),
    })
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null | Value::Undefined => false,
        Value::Number(bits) => {
            let n = f64::from_bits(*bits);
            n != 0.0 && !n.is_nan()
        }
        _ => true,
    }
}

fn js_string(v: &Value) -> Result<JsString, String> {
    match v {
        Value::String(s) => Ok(JsString::from_units(s.units().to_vec())),
        other => Err(err(format!("expected a string, got {}", other.kind()))),
    }
}

fn utf16(s: &JsString) -> Utf16String {
    Utf16String::from_units(s.units().to_vec())
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn cve_string(s: &JsString) -> Value {
    Value::String(utf16(s))
}

fn json_to_cve(j: &Json) -> Value {
    Value::from_json_lenient(j)
}

// --- Scripted selection -----------------------------------------------------

/// `window.selection`: `SelectionImpl` semantics for the modes fixtures drive.
/// `now` is the runner clock for playback math; `log` supplies timestamp ranges.
struct ScriptedSelection {
    mode: SelectionMode,
    hovered_time: Option<f64>,
    static_time: f64,
    live_connected: bool,
    live_time: Option<TimeSupplier>,
    playback_start_log: f64,
    playback_start_real: f64,
    playback_speed: f64,
    playback_looping: bool,
    now: Rc<Cell<f64>>,
    range: Rc<Cell<[f64; 2]>>,
}

impl ScriptedSelection {
    fn range(&self) -> [f64; 2] {
        self.range.get()
    }
    fn set_mode(&mut self, mode: SelectionMode) {
        self.mode = mode;
    }
}

impl Selection for ScriptedSelection {
    fn get_mode(&self) -> SelectionMode {
        self.mode
    }
    fn get_hovered_time(&self) -> Option<f64> {
        self.hovered_time
    }
    fn set_hovered_time(&mut self, value: Option<f64>) {
        self.hovered_time = value;
    }
    fn get_selected_time(&self) -> Option<f64> {
        match self.mode {
            SelectionMode::Idle => None,
            SelectionMode::Static => Some(self.static_time.max(self.range()[0])),
            SelectionMode::Playback => {
                let mut time = (self.now.get() - self.playback_start_real) / 1000.0
                    * self.playback_speed
                    + self.playback_start_log;
                let mut max_time = self.range()[1];
                if let Some(live) = &self.live_time {
                    max_time = max_time.max(live());
                }
                if time > max_time {
                    Some(max_time.max(self.range()[0]))
                } else {
                    time = time.max(self.range()[0]);
                    Some(time)
                }
            }
            SelectionMode::Locked => Some(self.live_time.as_ref().map_or(0.0, |f| f())),
        }
    }
    fn set_selected_time(&mut self, time: f64) {
        match self.mode {
            SelectionMode::Idle => {
                self.set_mode(SelectionMode::Static);
                self.static_time = time;
            }
            SelectionMode::Static => self.static_time = time,
            SelectionMode::Playback => {
                self.playback_start_log = time;
                self.playback_start_real = self.now.get();
            }
            SelectionMode::Locked => {}
        }
    }
    fn go_idle(&mut self) {
        if self.mode == SelectionMode::Static {
            self.set_mode(SelectionMode::Idle);
        }
    }
    fn play(&mut self) {
        match self.mode {
            SelectionMode::Idle => {
                self.set_mode(SelectionMode::Playback);
                self.playback_start_log = self.range()[0];
                self.playback_start_real = self.now.get();
            }
            SelectionMode::Static => {
                let mut max_time = self.range()[1];
                if let Some(live) = &self.live_time {
                    max_time = max_time.max(live());
                }
                if self.static_time < max_time {
                    self.set_mode(SelectionMode::Playback);
                    self.playback_start_log = self.static_time.max(self.range()[0]);
                    self.playback_start_real = self.now.get();
                }
            }
            _ => {}
        }
    }
    fn pause(&mut self) {
        if matches!(self.mode, SelectionMode::Playback | SelectionMode::Locked) {
            let selected = self.get_selected_time();
            self.set_mode(SelectionMode::Static);
            self.static_time = selected.unwrap_or(0.0);
        }
    }
    fn toggle_playback(&mut self) {
        if matches!(self.mode, SelectionMode::Playback | SelectionMode::Locked) {
            self.pause();
        } else {
            self.play();
        }
    }
    fn lock(&mut self) {
        if self.live_connected {
            self.set_mode(SelectionMode::Locked);
        }
    }
    fn unlock(&mut self) {
        if self.mode == SelectionMode::Locked {
            let selected = self.get_selected_time();
            self.set_mode(SelectionMode::Static);
            self.static_time = selected.unwrap_or(0.0);
        }
    }
    fn toggle_lock(&mut self) {
        if self.mode == SelectionMode::Locked {
            self.unlock();
        } else {
            self.lock();
        }
    }
    fn step_cycle(&mut self, is_forward: bool) {
        self.static_time += if is_forward { 0.02 } else { -0.02 };
    }
    fn set_live_connected(&mut self, time_supplier: TimeSupplier) {
        let new_connection = !self.live_connected;
        self.live_connected = true;
        self.live_time = Some(time_supplier);
        if new_connection {
            self.set_mode(SelectionMode::Locked);
        }
    }
    fn set_live_disconnected(&mut self) {
        self.unlock();
        self.live_connected = false;
        self.live_time = None;
    }
    fn get_current_live_time(&self) -> Option<f64> {
        self.live_time.as_ref().map(|f| f())
    }
    fn get_render_time(&self) -> Option<f64> {
        let selected = self.get_selected_time();
        match self.mode {
            SelectionMode::Playback | SelectionMode::Locked => selected,
            _ => self.hovered_time.or(selected),
        }
    }
    fn set_playback_speed(&mut self, speed: f64) {
        if self.mode == SelectionMode::Playback {
            let selected = self.get_selected_time();
            self.playback_start_log = selected.unwrap_or(0.0);
            self.playback_start_real = self.now.get();
        }
        self.playback_speed = speed;
    }
    fn set_playback_looping(&mut self, looping: bool) {
        self.playback_looping = looping;
    }
    fn set_grab_zoom_range(&mut self, _range: Option<[f64; 2]>) {}
    fn get_grab_zoom_range(&self) -> Option<[f64; 2]> {
        None
    }
    fn finish_grab_zoom(&mut self) {}
    fn get_timeline_range(&self) -> [f64; 2] {
        self.range()
    }
    fn get_timeline_is_max_zoom(&self) -> bool {
        true
    }
    fn set_timeline_range(&mut self, _range: [f64; 2], _lock_max_zoom: bool) {}
    fn apply_timeline_scroll(&mut self, _dx: f64, _dy: f64, _width_pixels: f64) {}
}

// --- Stub tuner -------------------------------------------------------------

struct StubTuner {
    keys: HashSet<JsString>,
    events: Rc<RefCell<Vec<Emit>>>,
}

impl Tuner for StubTuner {
    fn has_tunable_fields(&self) -> bool {
        !self.keys.is_empty()
    }
    fn is_tunable(&self, key: &JsString) -> bool {
        self.keys.contains(key)
    }
    fn publish(&self, key: &JsString, value: &JsValue) {
        self.events.borrow_mut().push(Emit {
            values: vec![(
                "tuner",
                Value::Array(vec![
                    cve_string(&js("publish")),
                    cve_string(key),
                    js_json::from_js(value),
                ]),
            )],
        });
    }
    fn unpublish(&self, key: &JsString) {
        self.events.borrow_mut().push(Emit {
            values: vec![(
                "tuner",
                Value::Array(vec![cve_string(&js("unpublish")), cve_string(key)]),
            )],
        });
    }
}

// --- Pseudo-DOM for layout rules -------------------------------------------

/// The element shape `env.ts::rectOf` reasons about: tag, class list, hidden
/// flag, position among same-tag siblings and parent for ancestor checks.
struct ElDesc {
    tag: &'static str,
    classes: &'static [&'static str],
    hidden: bool,
    pos: usize,
    parent: Option<Box<ElDesc>>,
}

fn el(
    tag: &'static str,
    classes: &'static [&'static str],
    hidden: bool,
    pos: usize,
    parent: ElDesc,
) -> ElDesc {
    ElDesc {
        tag,
        classes,
        hidden,
        pos,
        parent: Some(Box::new(parent)),
    }
}

fn matches_compound(el: &ElDesc, compound: &str) -> bool {
    let (compound, nth) = match compound.split_once(":nth-of-type(") {
        Some((head, arg)) => (head, Some(arg.trim_end_matches(')'))),
        None => (compound, None),
    };
    let mut parts = compound.split('.');
    let tag = parts.next().unwrap_or("");
    if !tag.is_empty() && tag != el.tag {
        return false;
    }
    for class in parts {
        if !el.classes.iter().any(|c| c == &class) {
            return false;
        }
    }
    if let Some(arg) = nth {
        let arg = arg.trim();
        let pos = el.pos as f64;
        let ok = match arg {
            "odd" => pos % 2.0 == 1.0,
            "even" => pos % 2.0 == 0.0,
            _ if arg.contains('n') => {
                let mut it = arg.splitn(2, 'n');
                let a_raw = it.next().unwrap_or("").trim().trim_end_matches('+');
                let a = match a_raw {
                    "" | "+" => 1.0,
                    "-" => -1.0,
                    _ => a_raw.parse::<f64>().unwrap_or(f64::NAN),
                };
                let b = it
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let k = (pos - b) / a;
                a != 0.0 && k >= 0.0 && k.fract() == 0.0
            }
            _ => pos == arg.parse::<f64>().unwrap_or(f64::NAN),
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Descendant combinators only: each earlier compound must match some
/// ancestor, scanning nearest-first like CSS.
fn matches_selector(el: &ElDesc, selector: &str) -> bool {
    let compounds: Vec<&str> = selector.split_whitespace().collect();
    let Some(last) = compounds.last() else {
        return false;
    };
    if !matches_compound(el, last) {
        return false;
    }
    let mut cursor = el.parent.as_deref();
    for compound in compounds[..compounds.len() - 1].iter().rev() {
        let mut found = false;
        while let Some(parent) = cursor {
            if matches_compound(parent, compound) {
                found = true;
                cursor = parent.parent.as_deref();
                break;
            }
            cursor = parent.parent.as_deref();
        }
        if !found {
            return false;
        }
    }
    true
}

fn is_hidden(el: &ElDesc) -> bool {
    let mut cursor = Some(el);
    while let Some(e) = cursor {
        if e.hidden {
            return true;
        }
        cursor = e.parent.as_deref();
    }
    false
}

struct Layout {
    rules: Vec<(String, [f64; 4])>,
}

impl Layout {
    fn rect_of(&self, el: &ElDesc) -> RowRect {
        if is_hidden(el) {
            return RowRect::default();
        }
        for (selector, rect) in &self.rules {
            if matches_selector(el, selector) {
                return RowRect {
                    x: rect[0],
                    y: rect[1],
                    width: rect[2],
                    height: rect[3],
                };
            }
        }
        RowRect::default()
    }
}

/// Ancestors of a `.field-item`: `html > body > div.side-bar > #fieldList`,
/// then one `childSpan` per enclosing level (hidden when that ancestor row is
/// collapsed), then the row's own `.field-item-container`.
fn field_item_el(row: &SidebarRow, ancestors_collapsed: &[bool]) -> ElDesc {
    let html = ElDesc {
        tag: "html",
        classes: &[],
        hidden: false,
        pos: 1,
        parent: None,
    };
    let body = el("body", &[], false, 1, html);
    let side_bar = el("div", &["side-bar"], false, 1, body);
    let mut top = el("span", &[], false, 1, side_bar);
    for &collapsed in ancestors_collapsed {
        top = el("span", &[], collapsed, 1, top);
    }
    let container = el("div", &["field-item-container"], false, row.position, top);
    let classes: &[&str] = if row.generated {
        &["field-item", "generated"]
    } else {
        &["field-item"]
    };
    el("div", classes, false, 1, container)
}

fn menu_button_rect(layout: &Layout) -> RowRect {
    let menu = ElDesc {
        tag: "div",
        classes: &["title-bar-menu"],
        hidden: false,
        pos: 1,
        parent: None,
    };
    let table = el("table", &[], false, 1, menu);
    let button = el("button", &[], false, 1, table);
    layout.rect_of(&button)
}

// --- Emit --------------------------------------------------------------------

struct Emit {
    values: Vec<(&'static str, Value)>,
}

fn node_tuple(
    depth: usize,
    tag: &str,
    class: &str,
    hidden: bool,
    text: &JsString,
    extra_pairs: Vec<(&str, Value)>,
) -> Value {
    let extra = Value::object(extra_pairs);
    Value::Array(vec![
        Value::number(depth as f64),
        cve_string(&js(tag)),
        cve_string(&js(class)),
        Value::Bool(hidden),
        cve_string(text),
        extra,
    ])
}

/// The `extra` object: asked `attributes` that are present (request order),
/// then `value`/`checked` for form elements.
fn extra<'a>(
    attrs: &'a [&'a str],
    present: impl Fn(&str) -> Option<Value>,
    form_value: Option<Value>,
) -> Vec<(&'a str, Value)> {
    let mut out: Vec<(&'a str, Value)> = Vec::new();
    for name in attrs {
        if let Some(v) = present(name) {
            out.push((*name, v));
        }
    }
    if let Some(v) = form_value {
        out.push(("value", v));
    }
    out
}

fn dom_emit(selector: &str, nodes: Vec<Value>) -> Emit {
    Emit {
        values: vec![
            ("dom", cve_string(&js(selector))),
            ("nodes", Value::Array(vec![Value::Array(nodes)])),
        ],
    }
}

fn project_field_list(sidebar: &Sidebar, attrs: &[&str]) -> Vec<Value> {
    let mut nodes = vec![node_tuple(0, "span", "", false, &JsString::new(), vec![])];
    for row in sidebar.rows() {
        let depth = (row.indent / INDENT_SIZE_PX) as usize + 1;
        let container_class = if row.highlighted {
            "field-item-container highlight"
        } else {
            "field-item-container"
        };
        nodes.push(node_tuple(
            depth,
            "div",
            container_class,
            false,
            &JsString::new(),
            vec![],
        ));
        let item_class = if row.generated {
            "field-item generated"
        } else {
            "field-item"
        };
        nodes.push(node_tuple(
            depth + 1,
            "div",
            item_class,
            false,
            &JsString::new(),
            vec![],
        ));
        for inner in ["polygon", "polygon", "rect"] {
            nodes.push(node_tuple(
                depth + 2,
                "svg",
                "field-item-icon",
                false,
                &JsString::new(),
                extra(
                    attrs,
                    |name| match name {
                        "height" | "width" => Some(cve_string(&js("12"))),
                        _ => None,
                    },
                    None,
                ),
            ));
            nodes.push(node_tuple(
                depth + 3,
                inner,
                "",
                false,
                &JsString::new(),
                vec![],
            ));
        }
        let mut label_class = "field-item-label".to_string();
        if row.known_label {
            label_class.push_str(" known");
        }
        if row.full_key.is_some() {
            label_class.push_str(" full-key");
        }
        if row.label_loading {
            label_class.push_str(" loading");
        }
        nodes.push(node_tuple(
            depth + 2,
            "div",
            &label_class,
            false,
            &JsString::new(),
            extra(
                attrs,
                |name| match name {
                    "title" => row.label_title.as_ref().map(cve_string),
                    _ => None,
                },
                None,
            ),
        ));
        nodes.push(node_tuple(depth + 3, "span", "", false, &row.title, vec![]));
        if row.full_key.as_ref().is_some_and(|k| !k.is_empty()) {
            nodes.push(node_tuple(
                depth + 3,
                "span",
                "",
                row.warning_hidden,
                &js(" \u{26A0}"),
                extra(
                    attrs,
                    |name| match name {
                        "title" => Some(cve_string(&row.warning_title)),
                        _ => None,
                    },
                    None,
                ),
            ));
        }
        if let Some(type_label) = &row.type_label {
            nodes.push(node_tuple(
                depth + 3,
                "span",
                "field-item-type-label",
                false,
                type_label,
                vec![],
            ));
        }
        let (value_children, value_text) = match &row.value {
            RowValue::None => (vec![], JsString::new()),
            RowValue::Bool {
                fill,
                line_hidden: _,
                tunable,
            } => {
                let circle_extra = extra(
                    attrs,
                    |name| match name {
                        "fill" => fill.as_ref().map(cve_string),
                        "cx" | "cy" | "r" => Some(cve_string(&js("4.5"))),
                        _ => None,
                    },
                    None,
                );
                (
                    vec![
                        node_tuple(
                            depth + 2,
                            "svg",
                            if *tunable { "tunable" } else { "" },
                            false,
                            &JsString::new(),
                            extra(
                                attrs,
                                |name| match name {
                                    "width" | "height" => Some(cve_string(&js("9"))),
                                    _ => None,
                                },
                                None,
                            ),
                        ),
                        node_tuple(
                            depth + 3,
                            "circle",
                            "",
                            false,
                            &JsString::new(),
                            circle_extra,
                        ),
                        node_tuple(depth + 3, "line", "", false, &JsString::new(), vec![]),
                    ],
                    JsString::new(),
                )
            }
            RowValue::Number {
                input_value,
                input_hidden,
                span_hidden,
                span_text,
                placeholder,
            } => {
                let input_extra = extra(
                    attrs,
                    |name| match name {
                        "title" => Some(cve_string(&js("Set value"))),
                        "placeholder" => placeholder.as_ref().map(cve_string),
                        "type" => Some(cve_string(&js("number"))),
                        _ => None,
                    },
                    Some(cve_string(input_value)),
                );
                (
                    vec![
                        node_tuple(
                            depth + 2,
                            "input",
                            "",
                            *input_hidden,
                            &JsString::new(),
                            input_extra,
                        ),
                        node_tuple(depth + 2, "span", "", *span_hidden, span_text, vec![]),
                    ],
                    JsString::new(),
                )
            }
            RowValue::Text { text } => (vec![], text.clone()),
        };
        nodes.push(node_tuple(
            depth + 1,
            "div",
            "field-value",
            row.value_hidden,
            &value_text,
            vec![],
        ));
        nodes.extend(value_children);
        if row.has_children {
            nodes.push(node_tuple(
                depth,
                "span",
                "",
                !row.expanded,
                &JsString::new(),
                vec![],
            ));
        }
    }
    nodes
}

fn project_search_results(sidebar: &Sidebar) -> Vec<Value> {
    let mut nodes = vec![node_tuple(
        0,
        "div",
        "search-results",
        sidebar.search_results_hidden(),
        &JsString::new(),
        vec![],
    )];
    for (key, hovered) in sidebar.search_items() {
        let class = if hovered {
            "search-results-item hovered"
        } else {
            "search-results-item"
        };
        nodes.push(node_tuple(1, "div", class, false, &key, vec![]));
    }
    nodes
}

fn project_drag_item(sidebar: &Sidebar) -> Vec<Value> {
    let mut nodes = vec![node_tuple(
        0,
        "span",
        "",
        sidebar.drag_item_hidden(),
        &JsString::new(),
        vec![],
    )];
    if let Some(item) = sidebar.drag_item() {
        nodes.push(node_tuple(
            1,
            "span",
            "field-text",
            false,
            &item.text,
            vec![],
        ));
    }
    nodes
}

fn project_title(sidebar: &Sidebar) -> Vec<Value> {
    vec![node_tuple(
        0,
        "div",
        "side-bar-title",
        false,
        sidebar.title_text(),
        vec![],
    )]
}

fn project_search_input(sidebar: &Sidebar, attrs: &[&str]) -> Vec<Value> {
    let input_extra = extra(
        attrs,
        |name| match name {
            "title" => Some(cve_string(&js("Search fields"))),
            "placeholder" => Some(cve_string(&js("Search"))),
            "type" => Some(cve_string(&js("search"))),
            _ => None,
        },
        Some(cve_string(sidebar.search_value())),
    );
    vec![node_tuple(
        0,
        "input",
        "side-bar-search",
        false,
        &JsString::new(),
        input_extra,
    )]
}

fn project_tuning_button(sidebar: &Sidebar, attrs: &[&str]) -> Vec<Value> {
    let class = if sidebar.tuning_button_active() {
        "side-bar-tuning-button active"
    } else {
        "side-bar-tuning-button"
    };
    vec![
        node_tuple(
            0,
            "button",
            class,
            sidebar.tuning_button_hidden(),
            &JsString::new(),
            extra(
                attrs,
                |name| match name {
                    "title" => Some(cve_string(&js("Toggle tuning mode"))),
                    _ => None,
                },
                None,
            ),
        ),
        node_tuple(1, "img", "", false, &JsString::new(), vec![]),
    ]
}

// --- Log steps ----------------------------------------------------------------

fn run_log_step(log: &Rc<RefCell<Log>>, calls: &[Value]) -> Result<(), String> {
    for call in calls {
        let parts = call
            .as_array()
            .ok_or_else(|| err("log step entries are [method, ...args]"))?;
        let (method, args) = parts
            .split_first()
            .ok_or_else(|| err("empty log step entry"))?;
        let method = method
            .as_str()
            .ok_or_else(|| err("log step method is a string"))?
            .to_string_lossy();
        let key = |i: usize| js_string(&args[i]);
        let ty = |i: usize| {
            if let Some(n) = args[i].as_f64() {
                return match n as u8 {
                    0 => Ok(LoggableType::Raw),
                    1 => Ok(LoggableType::Boolean),
                    2 => Ok(LoggableType::Number),
                    3 => Ok(LoggableType::String),
                    4 => Ok(LoggableType::BooleanArray),
                    5 => Ok(LoggableType::NumberArray),
                    6 => Ok(LoggableType::StringArray),
                    7 => Ok(LoggableType::Empty),
                    _ => Err(err(format!("bad LoggableType {n}"))),
                };
            }
            match args[i].as_str().map(|s| s.to_string_lossy()).as_deref() {
                Some("Boolean") => Ok(LoggableType::Boolean),
                Some("Number") => Ok(LoggableType::Number),
                Some("String") => Ok(LoggableType::String),
                Some("Raw") => Ok(LoggableType::Raw),
                Some("Empty") => Ok(LoggableType::Empty),
                Some("BooleanArray") => Ok(LoggableType::BooleanArray),
                Some("NumberArray") => Ok(LoggableType::NumberArray),
                Some("StringArray") => Ok(LoggableType::StringArray),
                other => Err(err(format!("bad LoggableType {other:?}"))),
            }
        };
        let array = |i: usize| args[i].as_array().ok_or_else(|| err("array argument"));
        let log = &mut *log.borrow_mut();
        match method.as_str() {
            "putBoolean" => log.put_boolean(&key(0)?, number(&args[1])?, truthy(&args[2])),
            "putNumber" => log.put_number(&key(0)?, number(&args[1])?, number(&args[2])?),
            "putString" => log.put_string(&key(0)?, number(&args[1])?, js_string(&args[2])?),
            "putBooleanArray" => {
                let values = array(2)?.iter().map(truthy).collect();
                log.put_boolean_array(&key(0)?, number(&args[1])?, values)
            }
            "putNumberArray" => {
                let values = array(2)?
                    .iter()
                    .map(number)
                    .collect::<Result<Vec<f64>, String>>()?;
                log.put_number_array(&key(0)?, number(&args[1])?, values)
            }
            "putStringArray" => {
                let values = array(2)?
                    .iter()
                    .map(js_string)
                    .collect::<Result<Vec<JsString>, String>>()?;
                log.put_string_array(&key(0)?, number(&args[1])?, values)
            }
            "putJSON" => log.put_json(&key(0)?, number(&args[1])?, js_string(&args[2])?),
            "putRaw" => {
                let bytes: Vec<u8> = match &args[2] {
                    Value::Bytes(b) => b.to_vec(),
                    Value::Array(items) => items
                        .iter()
                        .map(|v| number(v).map(|n| n as u8))
                        .collect::<Result<_, _>>()?,
                    other => return Err(err(format!("putRaw bytes, got {}", other.kind()))),
                };
                log.put_raw(&key(0)?, number(&args[1])?, RawValue::from(bytes))
                    .map_err(|e| format!("putRaw: {e:?}"))?;
            }
            "putUnknownStruct" => {
                let value = js_json::to_js(&args[2])?;
                log.put_unknown_struct(&key(0)?, number(&args[1])?, &value, false)
                    .map_err(|e| format!("putUnknownStruct: {e:?}"))?;
            }
            "putPose" => {
                let obj = js_json::to_js(&args[2])?;
                let JsValue::Object(map) = &obj else {
                    return Err(err("putPose value is an object"));
                };
                let Some(JsValue::Array(t)) = map.get("translation") else {
                    return Err(err("putPose needs translation"));
                };
                let Some(JsValue::Number(r)) = map.get("rotation") else {
                    return Err(err("putPose needs rotation"));
                };
                let (Some(JsValue::Number(tx)), Some(JsValue::Number(ty))) = (t.first(), t.get(1))
                else {
                    return Err(err("putPose translation is [x, y]"));
                };
                log.put_pose(&key(0)?, number(&args[1])?, [*tx, *ty], *r);
            }
            "setStructuredType" => {
                let value = match &args[1] {
                    Value::Null | Value::Undefined => None,
                    _ => Some(js_string(&args[1])?),
                };
                log.set_structured_type(&key(0)?, value);
            }
            "setWpilibType" => log.set_wpilib_type(&key(0)?, js_string(&args[1])?),
            "setMetadataString" => log.set_metadata_string(&key(0)?, js_string(&args[1])?),
            "setGeneratedParent" => log.set_generated_parent(&key(0)?),
            "createBlankField" => log.create_blank_field(&key(0)?, ty(1)?),
            "deleteField" => log.delete_field(&key(0)?),
            "clearBeforeTime" => log.clear_before_time(number(&args[0])?, truthy(&args[1])),
            "updateRangeWithTimestamp" => log.update_range_with_timestamp(number(&args[0])?),
            other => return Err(err(format!("unsupported log method {other}"))),
        }
    }
    Ok(())
}

// --- Replay -------------------------------------------------------------------

const SCROLL_ABORT_ERROR: &str = "TypeError: fieldElement.scrollIntoViewIfNeeded is not a function";

fn exception_emit(where_: &str) -> Emit {
    Emit {
        values: vec![
            ("exception", cve_string(&js(where_))),
            ("error", cve_string(&js(SCROLL_ABORT_ERROR))),
        ],
    }
}

/// Build a `SidebarEnv` inline so the `RefMut` temporaries never escape their
/// statement.
macro_rules! senv {
    ($ctx:expr) => {
        SidebarEnv {
            log: &mut *$ctx.log.borrow_mut(),
            selection: &*$ctx.selection.borrow(),
            tuner: $ctx.tuner.borrow().as_ref().map(|t| t as &dyn Tuner),
            dark_theme: $ctx.dark_theme,
            inner_height: $ctx.inner_height,
            can_scroll_into_view: false,
            loading_fields: &$ctx.loading_fields,
        }
    };
}

struct Ctx<'a> {
    log: &'a Rc<RefCell<Log>>,
    selection: &'a RefCell<ScriptedSelection>,
    tuner: &'a RefCell<Option<StubTuner>>,
    loading_fields: &'a HashSet<JsString>,
    range: &'a Rc<Cell<[f64; 2]>>,
    dark_theme: bool,
    inner_height: f64,
}

struct Runner<'a> {
    ctx: Ctx<'a>,
    sidebar: &'a mut Sidebar,
    layout: Layout,
    emitted: Vec<Emit>,
    now: f64,
    last_frame: f64,
    frame_period: f64,
    show_pending: bool,
}

impl Runner<'_> {
    fn row_rects(&self) -> Vec<RowRect> {
        let rows = self.sidebar.rows();
        let mut rects = Vec::with_capacity(rows.len());
        let mut collapsed: Vec<bool> = Vec::new();
        for row in &rows {
            let depth = (row.indent / INDENT_SIZE_PX) as usize;
            collapsed.truncate(depth);
            rects.push(self.layout.rect_of(&field_item_el(row, &collapsed)));
            collapsed.push(!row.expanded);
        }
        rects
    }

    fn sync_range(&self) {
        self.ctx
            .range
            .set(self.ctx.log.borrow().get_timestamp_range());
    }

    fn drain(&mut self) {
        self.sync_range();
        for payload in self.sidebar.take_drags() {
            self.emitted.push(Emit {
                values: vec![("drag", drag_payload(&payload))],
            });
        }
        for (name, data) in self.sidebar.take_main_messages() {
            self.emitted.push(Emit {
                values: vec![
                    ("main", cve_string(&name)),
                    ("data", js_json::from_js(&data)),
                ],
            });
        }
    }

    /// `env.ts::advance`: earliest timer first; a frame when due before any
    /// timer; timers due at the frame instant run first.
    fn advance(&mut self, ms: f64) {
        let end = self.now + ms;
        loop {
            let timer_due = self.sidebar.next_timer_due();
            let frame_at = self.last_frame + self.frame_period;
            if timer_due.is_some_and(|due| due <= end && due <= frame_at) {
                self.now = self.now.max(timer_due.unwrap_or(end));
                self.sidebar.fire_timers(self.now, &mut senv!(self.ctx));
            } else if frame_at <= end {
                self.now = frame_at;
                self.last_frame = frame_at;
                let rects = self.row_rects();
                self.sidebar.periodic(&mut senv!(self.ctx), &rects);
                if self.show_pending {
                    self.show_pending = false;
                    self.emitted.push(Emit {
                        values: vec![
                            ("main", cve_string(&js("show"))),
                            ("data", Value::Undefined),
                        ],
                    });
                }
            } else {
                self.now = end;
                break;
            }
            self.drain();
            if let Some(t) = self.ctx.tuner.borrow().as_ref() {
                self.emitted.extend(t.events.borrow_mut().drain(..));
            }
        }
    }

    fn outcome(&mut self, outcome: EventOutcome) {
        if outcome == EventOutcome::HighlightAborted {
            self.emitted.push(exception_emit("event"));
        }
    }

    fn nth_row_with(&self, index: usize, pred: impl Fn(&RowValue) -> bool) -> Option<usize> {
        self.sidebar
            .rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| pred(&r.value))
            .nth(index)
            .map(|(flat, _)| flat)
    }

    fn dispatch_event(&mut self, event: &Json) -> Result<(), String> {
        let target = event
            .get("target")
            .and_then(Json::as_str)
            .ok_or_else(|| err("event needs a target"))?
            .to_string_lossy();
        let etype = event
            .get("type")
            .and_then(Json::as_str)
            .ok_or_else(|| err("event needs a type"))?
            .to_string_lossy();
        let index = event.get("index").and_then(Json::as_f64).unwrap_or(0.0) as usize;
        let init = event.get("init");
        let iv = |name: &str| -> f64 {
            init.and_then(|i| i.get(name))
                .and_then(Json::as_f64)
                .unwrap_or(0.0)
        };
        let code = init
            .and_then(|i| i.get("code"))
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let key = init
            .and_then(|i| i.get("key"))
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let ctrl_or_meta = truthy(
            &init
                .and_then(|i| i.get("ctrlKey"))
                .map(json_to_cve)
                .unwrap_or(Value::Bool(false)),
        ) || truthy(
            &init
                .and_then(|i| i.get("metaKey"))
                .map(json_to_cve)
                .unwrap_or(Value::Bool(false)),
        );
        let props = event.get("props");
        let prop_str = |name: &str| -> Option<JsString> {
            props
                .and_then(|p| p.get(name))
                .and_then(|v| js_string(&json_to_cve(v)).ok())
        };
        let prop_num =
            |name: &str| -> Option<f64> { props.and_then(|p| p.get(name)).and_then(Json::as_f64) };
        let now = self.now;

        match (target.as_str(), etype.as_str()) {
            ("window", "mousemove") => {
                self.sidebar.window_mouse_move(iv("clientX"), iv("clientY"));
            }
            ("window", "mouseup") => self.sidebar.window_mouse_up(),
            ("window", "touchend") => self.sidebar.window_touch_end(),
            ("window", "touchmove") => self.sidebar.window_touch_move(),
            ("div.side-bar-handle", "mousedown") => self.sidebar.handle_mouse_down(),
            ("div.side-bar-handle", "click") => self.sidebar.handle_click(now),
            ("div.side-bar", "scroll") => {
                self.sidebar
                    .sidebar_scroll(prop_num("scrollTop").unwrap_or(0.0));
            }
            ("input.side-bar-search", "focus") => self.sidebar.search_focus(),
            ("input.side-bar-search", "blur") => self.sidebar.search_blur(),
            ("input.side-bar-search", "input") => {
                let value = prop_str("value").unwrap_or_default();
                self.sidebar.search_input(&mut senv!(self.ctx), value);
            }
            ("input.side-bar-search", "keydown") => {
                let outcome = self
                    .sidebar
                    .search_key_down(&mut senv!(self.ctx), &code, now);
                self.outcome(outcome);
            }
            (".search-results-item", "mousemove") => {
                self.sidebar.search_item_mouse_move(index);
            }
            (".search-results-item", "mousedown") | (".search-results-item", "touchdown") => {
                let outcome = self
                    .sidebar
                    .search_item_activate(&mut senv!(self.ctx), index, now);
                self.outcome(outcome);
            }
            ("#fieldList svg.field-item-icon", "click") => {
                let outcome =
                    self.sidebar
                        .icon_click(&mut senv!(self.ctx), index / 3, index % 3, now);
                self.outcome(outcome);
            }
            ("div.field-item-label", "mousedown") => {
                self.sidebar.label_mouse_down(
                    index,
                    iv("clientX"),
                    iv("clientY"),
                    iv("offsetX"),
                    iv("offsetY"),
                );
            }
            ("div.field-item-label", "mouseup") => {
                self.sidebar
                    .label_mouse_up(index, iv("clientX"), iv("clientY"), ctrl_or_meta);
            }
            ("div.field-item-label", "contextmenu") => {
                self.sidebar
                    .label_context_menu(index, iv("clientX"), iv("clientY"));
            }
            ("div.field-item-label", "touchstart") => {
                let rect = self.row_rects().get(index).copied().unwrap_or_default();
                self.sidebar
                    .label_touch_start(index, iv("clientX"), iv("clientY"), rect);
            }
            (".field-item-container input", "focus")
            | (".field-item-container input", "blur")
            | (".field-item-container input", "keydown")
            | (".field-item-container input", "input") => {
                let Some(flat) = self.nth_row_with(index, |v| matches!(v, RowValue::Number { .. }))
                else {
                    return Err(err(format!("no number input at index {index}")));
                };
                if let Some(value) = prop_str("value") {
                    self.sidebar.number_input_value(flat, value);
                }
                match etype.as_str() {
                    "focus" => self.sidebar.number_input_focus(flat, true),
                    "blur" => self.sidebar.number_input_focus(flat, false),
                    "keydown" => self.sidebar.number_input_key_down(flat, &key),
                    _ => {}
                }
            }
            (".side-bar-tuning-button", "click") => {
                self.sidebar.tuning_button_click(&senv!(self.ctx));
            }
            (".side-bar-tuning-button", "contextmenu") => {
                self.sidebar
                    .tuning_button_context_menu(iv("clientX"), iv("clientY"));
            }
            (".title-bar-menu button", "click") => {
                self.sidebar
                    .menu_button_click(index, menu_button_rect(&self.layout));
            }
            (".title-bar-menu button", "mouseleave") => {
                self.sidebar.menu_button_mouse_leave(index);
            }
            (".field-value svg", "click") => {
                let Some(flat) = self.nth_row_with(index, |v| matches!(v, RowValue::Bool { .. }))
                else {
                    return Err(err(format!("no boolean svg at index {index}")));
                };
                self.sidebar.svg_click(&mut senv!(self.ctx), flat);
            }
            ("div.field-item-label span", "click") => {
                // The type-warning span: index counts full-key rows' warning spans.
                let flat = self
                    .sidebar
                    .rows()
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.full_key.as_ref().is_some_and(|k| !k.is_empty()))
                    .nth(index)
                    .map(|(flat, _)| flat)
                    .ok_or_else(|| err(format!("no warning span at index {index}")))?;
                self.sidebar.warning_click(flat);
            }
            other => return Err(err(format!("unsupported event {other:?}"))),
        }
        Ok(())
    }
}

fn drag_payload(payload: &DragPayload) -> Value {
    Value::object([
        (
            "fields",
            Value::Array(payload.fields.iter().map(cve_string).collect()),
        ),
        (
            "children",
            Value::Array(payload.children.iter().map(cve_string).collect()),
        ),
    ])
}

const DEFAULT_LAYOUT_JSON: &str = include_str!("../../../oracle/src/dom/layout.json");

fn load_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    asrs_parity::json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn load_layout_rules(source: &Json) -> Vec<(String, [f64; 4])> {
    let mut rules = Vec::new();
    if let Some(list) = source.as_array() {
        for rule in list {
            let Some(selector) = rule.get("selector").and_then(Json::as_str) else {
                continue;
            };
            let Some(rect) = rule.get("rect").and_then(Json::as_array) else {
                continue;
            };
            if rect.len() != 4 {
                continue;
            }
            let n = |i: usize| rect[i].as_f64().unwrap_or(0.0);
            rules.push((selector.to_string_lossy(), [n(0), n(1), n(2), n(3)]));
        }
    }
    rules
}

fn replay(spec_path: &Path) -> Result<Vec<Emit>, String> {
    let spec = load_json(spec_path)?;
    let spec_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let fixture = spec
        .get("fixture")
        .ok_or_else(|| err("spec has no fixture"))?;
    let start = fixture
        .get("startTime")
        .and_then(Json::as_f64)
        .unwrap_or(1767225600000.0);
    let frame_period = fixture
        .get("window")
        .and_then(|w| w.get("framePeriod"))
        .and_then(Json::as_f64)
        .unwrap_or(16.0);
    let inner_height = fixture
        .get("window")
        .and_then(|w| w.get("height"))
        .and_then(Json::as_f64)
        .unwrap_or(650.0);
    let dark_theme = fixture
        .get("window")
        .and_then(|w| w.get("theme"))
        .and_then(Json::as_str)
        .is_some_and(|t| t.to_string_lossy() == "dark");

    let mut rules = Vec::new();
    if let Some(layout) = fixture.get("layout") {
        rules.extend(load_layout_rules(layout));
    }
    let defaults = asrs_parity::json::parse(DEFAULT_LAYOUT_JSON).map_err(|e| e.to_string())?;
    rules.extend(load_layout_rules(&defaults));
    let layout = Layout { rules };

    let log = Rc::new(RefCell::new(Log::new(true)));
    let now_cell = Rc::new(Cell::new(start));
    let range_cell = Rc::new(Cell::new(log.borrow().get_timestamp_range()));
    let selection = RefCell::new(ScriptedSelection {
        mode: SelectionMode::Idle,
        hovered_time: None,
        static_time: 0.0,
        live_connected: false,
        live_time: None,
        playback_start_log: 0.0,
        playback_start_real: 0.0,
        playback_speed: 1.0,
        playback_looping: false,
        now: now_cell.clone(),
        range: range_cell.clone(),
    });
    let tuner: RefCell<Option<StubTuner>> = RefCell::new(None);
    let tuner_events: Rc<RefCell<Vec<Emit>>> = Rc::new(RefCell::new(Vec::new()));
    let live_cell: Rc<RefCell<Option<f64>>> = Rc::new(RefCell::new(None));
    let filenames: Vec<JsString> = Vec::new();
    let mut sidebar = Sidebar::new(start, move || filenames.clone());
    let loading_fields: HashSet<JsString> = HashSet::new();

    let mut runner = Runner {
        ctx: Ctx {
            log: &log,
            selection: &selection,
            tuner: &tuner,
            loading_fields: &loading_fields,
            range: &range_cell,
            dark_theme,
            inner_height,
        },
        sidebar: &mut sidebar,
        layout,
        emitted: Vec::new(),
        now: start,
        last_frame: start,
        frame_period,
        show_pending: true,
    };

    // Boot: restore-state when the fixture carries one, then show-when-ready
    // queues the "show" emit on the first frame.
    if let Some(state) = fixture.get("state") {
        let data = js_json::to_js(&json_to_cve(state)).map_err(|e| format!("state: {e}"))?;
        if let Some(sidebar_state) = data.get("sidebar").and_then(SidebarState::from_js) {
            let outcome = runner
                .sidebar
                .restore_state(&mut senv!(runner.ctx), &sidebar_state);
            if outcome == EventOutcome::HighlightAborted {
                runner.emitted.push(exception_emit("main restore-state"));
            }
        }
    }
    runner.drain();

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .ok_or_else(|| err("fixture has no steps"))?;
    for (i, step) in steps.iter().enumerate() {
        let where_ = || format!("step {i}");
        if let Some(log_step) = step.get("log") {
            let calls: Vec<Value> = match log_step.as_str() {
                Some(path) => {
                    let path = spec_dir.join(path.to_string_lossy());
                    let ops = load_json(&path)?;
                    ops.as_array()
                        .ok_or_else(|| err(format!("{}: not an array", path.display())))?
                        .iter()
                        .map(json_to_cve)
                        .collect()
                }
                None => log_step
                    .as_array()
                    .ok_or_else(|| err("log step is an array or a path"))?
                    .iter()
                    .map(json_to_cve)
                    .collect(),
            };
            run_log_step(&log, &calls).map_err(|e| format!("{}: {e}", where_()))?;
            runner
                .sidebar
                .refresh(&mut senv!(runner.ctx), false)
                .map_err(|_| format!("{}: refresh aborted during log step", where_()))?;
        } else if let Some(sel_step) = step.get("selection") {
            let parts = sel_step
                .as_array()
                .ok_or_else(|| err("selection step is [method, ...args]"))?;
            let (method, args) = parts
                .split_first()
                .ok_or_else(|| err("empty selection step"))?;
            let method = method
                .as_str()
                .ok_or_else(|| err("selection method is a string"))?
                .to_string_lossy();
            let arg = |i: usize| -> Result<Value, String> {
                Ok(args.get(i).map(json_to_cve).unwrap_or(Value::Undefined))
            };
            let sel = &mut *selection.borrow_mut();
            match method.as_str() {
                "setHoveredTime" => sel.set_hovered_time(opt_number(&arg(0)?)?),
                "setSelectedTime" => sel.set_selected_time(number(&arg(0)?)?),
                "goIdle" => sel.go_idle(),
                "play" => sel.play(),
                "pause" => sel.pause(),
                "togglePlayback" => sel.toggle_playback(),
                "lock" => sel.lock(),
                "unlock" => sel.unlock(),
                "toggleLock" => sel.toggle_lock(),
                "stepCycle" => sel.step_cycle(truthy(&arg(0)?)),
                "setPlaybackSpeed" => sel.set_playback_speed(number(&arg(0)?)?),
                "setPlaybackLooping" => sel.set_playback_looping(truthy(&arg(0)?)),
                "setLiveDisconnected" => sel.set_live_disconnected(),
                other => return Err(format!("{}: unknown selection method {other}", where_())),
            }
        } else if let Some(live) = step.get("liveTime") {
            match opt_number(&json_to_cve(live))? {
                Some(t) => {
                    let already_connected = {
                        let s = selection.borrow();
                        s.live_connected
                    };
                    *live_cell.borrow_mut() = Some(t);
                    if !already_connected {
                        let cell = live_cell.clone();
                        selection
                            .borrow_mut()
                            .set_live_connected(Box::new(move || cell.borrow().unwrap_or(0.0)));
                    }
                }
                None => {
                    *live_cell.borrow_mut() = None;
                    selection.borrow_mut().set_live_disconnected();
                }
            }
        } else if let Some(tuner_step) = step.get("tuner") {
            match tuner_step {
                Json::Null => *tuner.borrow_mut() = None,
                _ => {
                    let keys = tuner_step
                        .as_array()
                        .ok_or_else(|| err("tuner step is a key list or null"))?
                        .iter()
                        .map(|k| js_string(&json_to_cve(k)))
                        .collect::<Result<HashSet<_>, _>>()?;
                    *tuner.borrow_mut() = Some(StubTuner {
                        keys,
                        events: tuner_events.clone(),
                    });
                }
            }
        } else if let Some(main) = step.get("main") {
            let name = main
                .as_str()
                .ok_or_else(|| err("main step name is a string"))?
                .to_string_lossy();
            let data = step
                .get("data")
                .map(json_to_cve)
                .or_else(|| step.get("cve1").map(json_to_cve))
                .unwrap_or(Value::Undefined);
            match name.as_str() {
                "toggle-sidebar" => runner.sidebar.toggle_visible(),
                "restore-state" => {
                    let data_js =
                        js_json::to_js(&data).map_err(|e| format!("{}: {e}", where_()))?;
                    if let Some(sidebar_state) =
                        data_js.get("sidebar").and_then(SidebarState::from_js)
                    {
                        let outcome = runner
                            .sidebar
                            .restore_state(&mut senv!(runner.ctx), &sidebar_state);
                        runner.outcome(outcome);
                    }
                }
                _ => {}
            }
        } else if let Some(ms) = step.get("advance") {
            now_cell.set({
                runner.advance(ms.as_f64().ok_or_else(|| err("advance is a number"))?);
                runner.now
            });
        } else if let Some(frames) = step.get("frames") {
            now_cell.set({
                runner.advance(
                    frames.as_f64().ok_or_else(|| err("frames is a number"))? * frame_period,
                );
                runner.now
            });
        } else if let Some(event) = step.get("event") {
            runner
                .dispatch_event(event)
                .map_err(|e| format!("{}: {e}", where_()))?;
        } else if let Some(capture) = step.get("capture") {
            let label = step
                .get("label")
                .map(json_to_cve)
                .unwrap_or_else(|| Value::number(i as f64));
            runner.emitted.push(Emit {
                values: vec![
                    ("capture", label),
                    ("time", Value::number(runner.now - start)),
                ],
            });
            let kinds: Vec<&Json> = match capture.as_array() {
                Some(list) => list.iter().collect(),
                None => vec![capture],
            };
            for spec in kinds {
                let attrs: Vec<String> = spec
                    .get("attributes")
                    .and_then(Json::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(|a| a.as_str().map(|s| s.to_string_lossy()))
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["title".to_string()]);
                let attr_refs: Vec<&str> = attrs.iter().map(|s| s.as_str()).collect();
                let kind = match spec {
                    Json::String(_) => spec
                        .as_str()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default(),
                    _ => spec
                        .get("kind")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default(),
                };
                match kind.as_str() {
                    "dom" => {
                        let selector = spec
                            .get("selector")
                            .and_then(Json::as_str)
                            .ok_or_else(|| err("dom capture needs a selector"))?
                            .to_string_lossy();
                        let nodes = match selector.as_str() {
                            "#fieldList" => project_field_list(runner.sidebar, &attr_refs),
                            "div.side-bar-title" => project_title(runner.sidebar),
                            ".search-results" => project_search_results(runner.sidebar),
                            "#dragItem" => project_drag_item(runner.sidebar),
                            "input.side-bar-search" => {
                                project_search_input(runner.sidebar, &attr_refs)
                            }
                            ".side-bar-tuning-button" => {
                                project_tuning_button(runner.sidebar, &attr_refs)
                            }
                            other => {
                                return Err(format!(
                                    "{}: unsupported dom selector {other}",
                                    where_()
                                ));
                            }
                        };
                        runner.emitted.push(dom_emit(&selector, nodes));
                    }
                    "eval" => {
                        let expr = spec
                            .get("expr")
                            .and_then(Json::as_str)
                            .ok_or_else(|| err("eval capture needs expr"))?
                            .to_string_lossy();
                        let value = match expr.as_str() {
                            "window.sidebar.saveState()" => {
                                js_json::from_js(&runner.sidebar.save_state().to_js())
                            }
                            "[...window.sidebar.getActiveFields()]" => {
                                let rects = runner.row_rects();
                                Value::Array(
                                    runner
                                        .sidebar
                                        .get_active_fields(&senv!(runner.ctx), &rects)
                                        .iter()
                                        .map(cve_string)
                                        .collect(),
                                )
                            }
                            other => return Err(format!("{}: unsupported eval {other}", where_())),
                        };
                        runner.emitted.push(Emit {
                            values: vec![("eval", cve_string(&js(&expr))), ("value", value)],
                        });
                    }
                    other => return Err(format!("{}: unsupported capture kind {other}", where_())),
                }
            }
        } else {
            return Err(format!("{}: unknown step", where_()));
        }
        runner.drain();
        if let Some(t) = runner.ctx.tuner.borrow().as_ref() {
            runner.emitted.extend(t.events.borrow_mut().drain(..));
        }
    }

    Ok(runner.emitted)
}

fn compare(spec: &Path, vector_id: &str) -> Result<(), String> {
    let emitted = replay(spec)?;
    let records = vectors::load(vector_id)?;
    if emitted.len() != records.len() {
        return Err(format!(
            "emitted {} records, vector has {}",
            emitted.len(),
            records.len()
        ));
    }
    for (emit, record) in emitted.iter().zip(records.iter()) {
        for (key, value) in &emit.values {
            let expected = record
                .value(key)
                .map_err(|e| format!("record {} missing {key}: {e}", record.line))?;
            if *value != expected {
                return Err(format!(
                    "line {} ({key}):\n  expected {}\n  actual   {}",
                    record.line,
                    expected.encode(),
                    value.encode()
                ));
            }
        }
    }
    Ok(())
}

fn specs() -> Vec<(PathBuf, String)> {
    let dir = vectors::workspace_root().join("parity/specs/dom/sidebar");
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with(".entry.json"))
        })
        .map(|p| {
            let name = p
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .trim_end_matches(".entry.json")
                .to_owned();
            (p, format!("dom/sidebar/{name}"))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn sidebar_dom_matches_upstream() {
    let specs = specs();
    assert!(!specs.is_empty(), "no sidebar specs");
    let mut failures = Vec::new();
    for (spec, id) in &specs {
        if let Err(e) = compare(spec, id) {
            failures.push(format!("{id}: {e}"));
        }
    }
    if !failures.is_empty() {
        panic!("{}", failures.join("\n\n"));
    }
}
