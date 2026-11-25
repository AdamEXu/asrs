#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/sidebar/<case>.entry.json` against the ported
//! `asrs_hub::sidebar::Sidebar` and compares the emitted records with the
//! committed `dom/sidebar/<case>` vector, line for line. The vectors are Blink runs
//! with `"geometry": "real"`: the sidebar's layout reads are answered from their
//! `read` records and its scroll writes checked against their `write` records, in
//! order, and the `/*sidebar:recorder*/` eval logs what Blink did on its own (native
//! `scroll` events, `blur` events, mouse offsets) as inputs. The runner only converts
//! fixture inputs to model calls and the resulting view state back to CVE-1 nodes;
//! all sidebar behavior lives in `asrs-hub`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_hub::drag::{Drag, DragPayload};
use asrs_hub::sidebar::{
    EventOutcome, INDENT_SIZE_PX, RowRect, RowRef, RowValue, Sidebar, SidebarEnv, SidebarGeometry,
    TUNING_INTERVAL_MS, Tuner, TunerWindow,
};
use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::time::{Clock, SetRng};
use asrs_js::value::JsValue;
use asrs_live::{Thrown, TunedValue};
use asrs_log::log::{Log, LoggableType, RawValue};
use asrs_log::realm::JsRealm;
use asrs_model::selection::Selection;
use asrs_model::selection_impl::SelectionImpl;
use asrs_parity::cve1::Value;
use asrs_parity::geometry::{GeometryRecord, ScriptedReads};
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};

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

struct ScriptedClock(Rc<Cell<f64>>);

impl Clock for ScriptedClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

struct StubTuner {
    keys: HashSet<JsString>,
    events: Rc<RefCell<Vec<Emit>>>,
}

impl Tuner for StubTuner {
    fn has_tunable_fields(&self, _log: &Log) -> bool {
        !self.keys.is_empty()
    }
    fn is_tunable(&self, _log: &Log, key: &JsString) -> bool {
        self.keys.contains(key)
    }
    fn publish(
        &self,
        _window: &mut TunerWindow<'_>,
        key: &JsString,
        value: TunedValue,
    ) -> Result<(), Thrown> {
        let value = match value {
            TunedValue::Number(n) => JsValue::Number(n),
            TunedValue::Boolean(b) => JsValue::Bool(b),
        };
        self.events.borrow_mut().push(Emit {
            values: vec![(
                "tuner",
                Value::Array(vec![
                    cve_string(&js("publish")),
                    cve_string(key),
                    js_json::from_js(&value),
                ]),
            )],
        });
        Ok(())
    }
    fn unpublish(&self, _window: &mut TunerWindow<'_>, key: &JsString) -> Result<(), Thrown> {
        self.events.borrow_mut().push(Emit {
            values: vec![(
                "tuner",
                Value::Array(vec![cve_string(&js("unpublish")), cve_string(key)]),
            )],
        });
        Ok(())
    }
}

/// `body`'s element children in `www/hub.html` that the sidebar reads or scrolls.
const SIDE_BAR: &str = "div.side-bar:18";
const SEARCH_RESULTS: &str = "div.search-results:17";
const SIDEBAR_ROOTS: [&str; 3] = [
    "div.side-bar:",
    "div.search-results:",
    "div.title-bar-menu:",
];

/// Answers the sidebar's layout reads from Blink's `read` records and checks its scroll writes
/// against the `write` records, in recorded order (oracle/README.md, "Blink backend").
struct Recorded {
    reads: ScriptedReads,
    inputs: Inputs,
}

impl Recorded {
    /// Whether Blink dispatched a `blur` to the search input next.
    fn take_blur(&mut self) -> bool {
        let blur = matches!(self.inputs.front(), Some((_, Input::Blur)));
        if blur {
            self.inputs.pop_front();
        }
        blur
    }

    fn row(row: RowRef<'_>, tail: &str) -> String {
        format!("{}>{tail}", row.container_path())
    }

    fn rect(&mut self, target: &str) -> RowRect {
        let [x, y, width, height] = self.reads.read_rect(target);
        RowRect {
            x,
            y,
            width,
            height,
        }
    }
}

impl SidebarGeometry for Recorded {
    fn field_item_rect(&mut self, row: RowRef<'_>) -> RowRect {
        self.rect(&Self::row(row, "div.field-item:0"))
    }

    fn value_client_width(&mut self, row: RowRef<'_>, _value: &RowValue, _hidden: bool) -> f64 {
        self.reads
            .read_f64("clientWidth", &Self::row(row, "div.field-value:1"))
    }

    fn label_rect(&mut self, row: RowRef<'_>) -> RowRect {
        self.rect(&Self::row(row, "div.field-item:0>div.field-item-label:3"))
    }

    fn scroll_into_view_if_needed(&mut self, row: RowRef<'_>) {
        self.reads.write(
            "scrollIntoViewIfNeeded",
            &Self::row(row, "div.field-item:0"),
            &Value::Array(Vec::new()),
        );
    }

    fn search_input_rect(&mut self) -> RowRect {
        self.rect(&format!(
            "{SIDE_BAR}>div.side-bar-search-container:1>input.side-bar-search:0"
        ))
    }

    fn scroll_search_item_into_view(&mut self, index: usize) {
        let options = Value::object([("block", cve_string(&js("nearest")))]);
        self.reads.write(
            "scrollIntoView",
            &format!("{SEARCH_RESULTS}>div.search-results-item:{index}"),
            &Value::Array(vec![options]),
        );
    }

    fn reset_search_results_scroll(&mut self) {
        self.reads
            .write("scrollTop", SEARCH_RESULTS, &Value::number(0.0));
    }

    fn scroll_top(&mut self) -> f64 {
        self.reads.read_f64("scrollTop", SIDE_BAR)
    }

    fn set_scroll_top(&mut self, value: f64) {
        self.reads
            .write("scrollTop", SIDE_BAR, &Value::number(value));
    }

    fn menu_button_rect(&mut self, index: usize) -> RowRect {
        self.rect(&format!(
            "div.title-bar-menu:1>table:0>tbody:0>tr:0>td:{index}>button:0"
        ))
    }

    fn blur_search_input(&mut self) -> bool {
        self.take_blur()
    }
}

/// Which records of a Blink run the sidebar replay consumes. The page also runs `Tabs` and
/// the default tab, whose geometry records (under `div.viewer`) belong to their own families;
/// any other geometry record is the sidebar's, including reads of rows a rebuild removed.
enum Kind {
    Geometry,
    Foreign,
    Input,
    Output,
}

fn kind(record: &Record) -> Result<Kind, String> {
    if let Some(g) = GeometryRecord::parse(record)? {
        let attached = g.el.starts_with("body");
        let ours = if attached {
            SIDEBAR_ROOTS.iter().any(|r| g.el.contains(r))
        } else {
            g.el.contains("div.field-item")
        };
        return Ok(if ours {
            Kind::Geometry
        } else if !attached || g.el.contains(">div.viewer:") {
            Kind::Foreign
        } else {
            return Err(format!("line {}: geometry record on {}", record.line, g.el));
        });
    }
    Ok(if record.field("sb").is_some() {
        Kind::Input
    } else {
        Kind::Output
    })
}

/// `{"sb": ...}` lines of the `/*sidebar:recorder*/` eval: what Blink did that the fixture
/// steps do not say, each at its position among the compared records.
/// Each input with the numbers of compared and geometry records before it.
type Inputs = VecDeque<((usize, usize), Input)>;

enum Input {
    Scroll,
    MouseDown(f64, f64),
    Blur,
}

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

fn display(shown: bool) -> Option<Value> {
    Some(cve_string(&js(if shown {
        "display: initial;"
    } else {
        "display: none;"
    })))
}

fn project_field_list(sidebar: &Sidebar, attrs: &[&str]) -> Vec<Value> {
    let style = |text: String| Some(cve_string(&js(&text)));
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
            extra(
                attrs,
                |name| match name {
                    "style" => style(format!(
                        "--value-width: {}px;",
                        number::to_string(row.value_width)
                    )),
                    _ => None,
                },
                None,
            ),
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
        let icons = [
            ("polygon", row.has_children && !row.expanded),
            ("polygon", row.expanded),
            ("rect", !row.has_children),
        ];
        for (inner, shown) in icons {
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
                        "style" => display(shown),
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
                    "style" => row.label_bold.and_then(|bold| {
                        style(format!(
                            "font-weight: {};",
                            if bold { "bolder" } else { "initial" }
                        ))
                    }),
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
                        "style" => style("cursor: help;".to_string()),
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
                line_hidden,
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
                let line_extra = extra(
                    attrs,
                    |name| match name {
                        "style" => line_hidden.and_then(|hidden| display(!hidden)),
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
                        node_tuple(depth + 3, "line", "", false, &JsString::new(), line_extra),
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
        if row.child_span {
            nodes.push(node_tuple(
                depth,
                "span",
                "",
                !row.expanded,
                &JsString::new(),
                extra(
                    attrs,
                    |name| match name {
                        "style" => style(format!(
                            "--indent: {}px;",
                            number::to_string(row.indent + INDENT_SIZE_PX)
                        )),
                        _ => None,
                    },
                    None,
                ),
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

fn project_drag_item(drag: &Drag, attrs: &[&str]) -> Vec<Value> {
    let style = drag.item_position().map(|(left, top)| {
        js(&format!(
            "left: {}px; top: {}px;",
            number::to_string(left),
            number::to_string(top)
        ))
    });
    let item_extra = extra(
        attrs,
        |name| match (name, &style) {
            ("style", Some(style)) => Some(cve_string(style)),
            _ => None,
        },
        None,
    );
    let mut nodes = vec![node_tuple(
        0,
        "span",
        "",
        drag.item_hidden(),
        &JsString::new(),
        item_extra,
    )];
    if let Some(item) = drag.item() {
        let weight = if item.bold { "bolder" } else { "initial" };
        let text_extra = extra(
            attrs,
            |name| (name == "style").then(|| cve_string(&js(&format!("font-weight: {weight};")))),
            None,
        );
        nodes.push(node_tuple(
            1,
            "span",
            "field-text",
            false,
            &item.text,
            text_extra,
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
            "putBoolean" => log
                .put_boolean(&key(0)?, number(&args[1])?, truthy(&args[2]))
                .unwrap(),
            "putNumber" => log
                .put_number(&key(0)?, number(&args[1])?, number(&args[2])?)
                .unwrap(),
            "putString" => log
                .put_string(&key(0)?, number(&args[1])?, js_string(&args[2])?)
                .unwrap(),
            "putBooleanArray" => {
                let values = array(2)?.iter().map(truthy).collect();
                log.put_boolean_array(&key(0)?, number(&args[1])?, values)
                    .unwrap()
            }
            "putNumberArray" => {
                let values = array(2)?
                    .iter()
                    .map(number)
                    .collect::<Result<Vec<f64>, String>>()?;
                log.put_number_array(&key(0)?, number(&args[1])?, values)
                    .unwrap()
            }
            "putStringArray" => {
                let values = array(2)?
                    .iter()
                    .map(js_string)
                    .collect::<Result<Vec<JsString>, String>>()?;
                log.put_string_array(&key(0)?, number(&args[1])?, values)
                    .unwrap()
            }
            "putJSON" => log
                .put_json(&key(0)?, number(&args[1])?, js_string(&args[2])?)
                .map_err(|e| format!("putJSON: {e:?}"))?,
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
                log.put_pose(&key(0)?, number(&args[1])?, [*tx, *ty], *r)
                    .unwrap();
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

fn exception_emit(where_: &str, outcome: EventOutcome) -> Option<Emit> {
    let EventOutcome::Threw(error) = outcome else {
        return None;
    };
    let error = format!("{}: {}", error.name, error.message);
    Some(Emit {
        values: vec![
            ("exception", cve_string(&js(where_))),
            ("error", cve_string(&js(&error))),
        ],
    })
}

/// Build a `SidebarEnv` inline so the `RefMut` temporaries never escape their
/// statement.
macro_rules! senv {
    ($runner:expr) => {
        SidebarEnv {
            log: $runner.ctx.log,
            selection: &mut *$runner.ctx.selection.borrow_mut(),
            tuner: $runner.ctx.tuner.borrow().as_ref().map(|t| t as &dyn Tuner),
            realm: &mut $runner.realm,
            clock: $runner.ctx.clock,
            dark_theme: $runner.ctx.dark_theme,
            inner_height: $runner.ctx.inner_height,
            loading_fields: &$runner.ctx.loading_fields,
            geometry: &mut $runner.geometry,
        }
    };
}

struct Ctx<'a> {
    log: &'a Rc<RefCell<Log>>,
    selection: &'a RefCell<SelectionImpl>,
    tuner: &'a RefCell<Option<StubTuner>>,
    clock: &'a dyn Clock,
    loading_fields: &'a HashSet<JsString>,
    dark_theme: bool,
    inner_height: f64,
}

struct Runner<'a> {
    ctx: Ctx<'a>,
    realm: JsRealm,
    sidebar: &'a mut Sidebar,
    drag: Drag,
    geometry: Recorded,
    emitted: Vec<Emit>,
    now: f64,
    tuning_due: f64,
    last_frame: f64,
    frame_period: f64,
    show_pending: bool,
}

impl Runner<'_> {
    /// The native `scroll` events Blink dispatched once the last step settled.
    fn native_scrolls(&mut self) {
        while let Some(((outputs, geometry), Input::Scroll)) = self.geometry.inputs.front()
            && *outputs <= self.emitted.len()
            && *geometry <= self.geometry.reads.consumed()
        {
            self.geometry.inputs.pop_front();
            self.sidebar.sidebar_scroll(&mut self.geometry);
        }
    }

    /// The `offsetX` and `offsetY` Blink gave a label `mousedown`.
    fn mouse_down_offset(&mut self) -> Result<(f64, f64), String> {
        match self.geometry.inputs.pop_front() {
            Some((_, Input::MouseDown(x, y))) => Ok((x, y)),
            _ => Err(err("a label mousedown with no recorded offsets")),
        }
    }

    fn drain(&mut self) {
        for update in self.drag.take_updates() {
            self.emitted.push(Emit {
                values: vec![
                    ("drag", drag_payload(&update.data)),
                    ("end", Value::Bool(update.end)),
                    ("x", Value::number(update.x)),
                    ("y", Value::number(update.y)),
                ],
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
            let timeout = self.sidebar.next_timeout().unwrap_or(f64::INFINITY);
            let timer_due = timeout.min(self.tuning_due);
            let frame_at = self.last_frame + self.frame_period;
            if timer_due <= end && timer_due <= frame_at {
                self.now = self.now.max(timer_due);
                if timeout <= self.tuning_due {
                    self.sidebar.fire_timeouts(self.now);
                } else {
                    self.sidebar.publish_tuning(&mut senv!(self));
                    self.tuning_due += TUNING_INTERVAL_MS;
                }
            } else if frame_at <= end {
                self.now = frame_at;
                self.last_frame = frame_at;
                self.sidebar.periodic(&mut senv!(self));
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
        self.emitted.extend(exception_emit("event", outcome));
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
        let now = self.now;

        match (target.as_str(), etype.as_str()) {
            ("window", "mousemove") => {
                let (x, y) = (iv("clientX"), iv("clientY"));
                self.sidebar.handle_mouse_move(x);
                self.drag.move_to(x, y);
                self.sidebar.fields_mouse_move(&mut self.drag, x, y);
            }
            ("window", "mouseup") => {
                self.sidebar.handle_mouse_up();
                self.drag.end();
            }
            ("window", "touchend") => self.drag.end(),
            ("div.side-bar-handle", "mousedown") => self.sidebar.handle_mouse_down(),
            ("div.side-bar-handle", "click") => self.sidebar.handle_click(now),
            ("input.side-bar-search", "focus") => self.sidebar.search_focus(),
            ("input.side-bar-search", "blur") => {
                if !self.geometry.take_blur() {
                    return Err(err("a blur event Blink did not record"));
                }
                self.sidebar.search_blur();
            }
            ("input.side-bar-search", "input") => {
                let value = prop_str("value").unwrap_or_default();
                self.sidebar.search_input(&mut senv!(self), value);
            }
            ("input.side-bar-search", "keydown") => {
                let outcome = self.sidebar.search_key_down(&mut senv!(self), &code, now);
                self.outcome(outcome);
            }
            (".search-results-item", "mousemove") => {
                self.sidebar.search_item_mouse_move(index);
            }
            (".search-results-item", "mousedown") | (".search-results-item", "touchdown") => {
                let outcome = self
                    .sidebar
                    .search_item_activate(&mut senv!(self), index, now);
                self.outcome(outcome);
            }
            ("#fieldList svg.field-item-icon", "click") => {
                let outcome = self
                    .sidebar
                    .icon_click(&mut senv!(self), index / 3, index % 3, now);
                self.outcome(outcome);
            }
            ("div.field-item-label", "mousedown") => {
                let (ox, oy) = self.mouse_down_offset()?;
                self.sidebar
                    .label_mouse_down(index, iv("clientX"), iv("clientY"), ox, oy);
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
                self.sidebar.label_touch_start(
                    &mut self.geometry,
                    &mut self.drag,
                    index,
                    iv("clientX"),
                    iv("clientY"),
                );
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
                self.sidebar.tuning_button_click(&mut senv!(self));
            }
            (".side-bar-tuning-button", "contextmenu") => {
                self.sidebar
                    .tuning_button_context_menu(iv("clientX"), iv("clientY"));
            }
            (".title-bar-menu button", "click") => {
                self.sidebar.menu_button_click(&mut self.geometry, index);
            }
            (".title-bar-menu button", "mouseleave") => {
                self.sidebar.menu_button_mouse_leave(index);
            }
            (".field-value svg", "click") => {
                let Some(flat) = self.nth_row_with(index, |v| matches!(v, RowValue::Bool { .. }))
                else {
                    return Err(err(format!("no boolean svg at index {index}")));
                };
                self.sidebar.svg_click(&mut senv!(self), flat);
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

fn load_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    asrs_parity::json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The run's compared records, its sidebar geometry records and its inputs.
fn split(records: &[Record]) -> Result<(Vec<&Record>, ScriptedReads, Inputs), String> {
    let mut outputs = Vec::new();
    let mut geometry = Vec::new();
    let mut inputs = VecDeque::new();
    for record in records {
        match kind(record)? {
            Kind::Geometry => geometry.push(record.clone()),
            Kind::Foreign => {}
            Kind::Output => outputs.push(record),
            Kind::Input => {
                let input = match record.string("sb").as_deref() {
                    Some("scroll") => Input::Scroll,
                    Some("blur") => Input::Blur,
                    Some("mousedown") => match record
                        .value("offset")
                        .map_err(|e| format!("line {}: {e}", record.line))?
                    {
                        Value::Array(xy) if xy.len() == 2 => {
                            Input::MouseDown(number(&xy[0])?, number(&xy[1])?)
                        }
                        other => return Err(format!("line {}: offset {other:?}", record.line)),
                    },
                    other => return Err(format!("line {}: unknown sb {other:?}", record.line)),
                };
                inputs.push_back(((outputs.len(), geometry.len()), input));
            }
        }
    }
    Ok((outputs, ScriptedReads::from_records(&geometry)?, inputs))
}

fn replay(
    spec_path: &Path,
    geometry: ScriptedReads,
    inputs: Inputs,
) -> Result<(Vec<Emit>, ScriptedReads), String> {
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
    if fixture
        .get("geometry")
        .and_then(Json::as_str)
        .map(|g| g.to_string_lossy())
        != Some("real".to_owned())
    {
        return Err(err("sidebar fixtures run with \"geometry\": \"real\""));
    }

    let log = Rc::new(RefCell::new(Log::new(true)));
    let now_cell = Rc::new(Cell::new(start));
    let selection = RefCell::new(SelectionImpl::new(
        log.clone(),
        Rc::new(ScriptedClock(now_cell.clone())),
    ));
    let tuner: RefCell<Option<StubTuner>> = RefCell::new(None);
    let tuner_events: Rc<RefCell<Vec<Emit>>> = Rc::new(RefCell::new(Vec::new()));
    let live_cell: Rc<RefCell<Option<f64>>> = Rc::new(RefCell::new(None));
    let filenames: Vec<JsString> = Vec::new();
    let mut sidebar = Sidebar::new(move || filenames.clone());
    let loading_fields: HashSet<JsString> = HashSet::new();
    let scripted = ScriptedClock(now_cell.clone());

    let mut runner = Runner {
        ctx: Ctx {
            log: &log,
            selection: &selection,
            tuner: &tuner,
            clock: &scripted,
            loading_fields: &loading_fields,
            dark_theme,
            inner_height,
        },
        realm: JsRealm::new(Box::new(SetRng::new(Vec::new()))),
        sidebar: &mut sidebar,
        drag: Drag::default(),
        geometry: Recorded {
            reads: geometry,
            inputs,
        },
        emitted: Vec::new(),
        now: start,
        tuning_due: start + TUNING_INTERVAL_MS,
        last_frame: start,
        frame_period,
        show_pending: true,
    };

    // Boot: restore-state when the fixture carries one, then show-when-ready
    // queues the "show" emit on the first frame.
    if let Some(state) = fixture.get("state") {
        let data = js_json::to_js(&json_to_cve(state)).map_err(|e| format!("state: {e}"))?;
        if let Some(sidebar_state) = data.get("sidebar") {
            let outcome = runner
                .sidebar
                .restore_state(&mut senv!(runner), sidebar_state)
                .map_err(|e| format!("{e:?}"))?;
            runner
                .emitted
                .extend(exception_emit("main restore-state", outcome));
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
            if let Err(outcome) = runner.sidebar.refresh(&mut senv!(runner), false) {
                runner.emitted.extend(exception_emit("log", outcome));
            }
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
                "setLiveConnected" => {
                    let cell = live_cell.clone();
                    sel.set_live_connected(Rc::new(move || cell.borrow().unwrap_or(f64::NAN)));
                }
                "setLiveDisconnected" => sel.set_live_disconnected(),
                other => return Err(format!("{}: unknown selection method {other}", where_())),
            }
        } else if let Some(live) = step.get("liveTime") {
            *live_cell.borrow_mut() = opt_number(&json_to_cve(live))?;
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
                    if let Some(sidebar_state) = data_js.get("sidebar") {
                        let outcome = runner
                            .sidebar
                            .restore_state(&mut senv!(runner), sidebar_state)
                            .map_err(|e| format!("{e:?}"))?;
                        runner.outcome(outcome);
                    }
                }
                other => return Err(format!("{}: main message {other} is not modeled", where_())),
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
        } else if let Some(expr) = step.get("eval").and_then(Json::as_str) {
            let expr = expr.to_string_lossy();
            if !expr.starts_with("/*sidebar:recorder*/") && !is_scroll_input(&expr) {
                return Err(format!("{}: unsupported eval {expr}", where_()));
            }
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
                            "#dragItem" => project_drag_item(&runner.drag, &attr_refs),
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
                                js_json::from_js(&runner.sidebar.save_state())
                            }
                            "[...window.sidebar.getActiveFields()]" => Value::Array(
                                runner
                                    .sidebar
                                    .get_active_fields(&mut senv!(runner))
                                    .iter()
                                    .map(cve_string)
                                    .collect(),
                            ),
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
        runner.native_scrolls();
    }

    if !runner.geometry.inputs.is_empty() {
        return Err(format!(
            "{} recorded inputs never replayed",
            runner.geometry.inputs.len()
        ));
    }
    Ok((runner.emitted, runner.geometry.reads))
}

/// A fixture eval that scrolls `.side-bar` inside `__oracleQuiet`: an input, whose `scroll`
/// event the recorder logs.
fn is_scroll_input(expr: &str) -> bool {
    expr.strip_prefix(
        "window.__oracleQuiet(() => { document.getElementsByClassName(\"side-bar\")[0].scrollTop = ",
    )
    .and_then(|rest| rest.strip_suffix("; })"))
    .is_some_and(|value| value.parse::<f64>().is_ok())
}

fn compare(spec: &Path, vector_id: &str) -> Result<(), String> {
    let all = vectors::load(vector_id)?;
    let (records, geometry, inputs) = split(&all)?;
    let (emitted, geometry) = replay(spec, geometry, inputs)?;
    geometry.finish()?;
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
