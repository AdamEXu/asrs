//! Replays the R11 vectors written by `oracle/tools/r11Specs.mjs` and recorded in Blink:
//! `dom/{table,console,statistics,metadata}-view/*`.
//!
//! The recorder brackets every `render` and every driven event of a renderer between
//! `{"r11": "begin"}` and `{"r11": "end"}`. The port runs the same operation with its layout
//! reads answered, in order, by the bracket's `read` records and its scroll writes checked
//! against its `write` records; its selection calls, main messages, throw and whole view-model
//! must equal what the bracket recorded. Records outside a bracket belong to other models.

mod support;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::{LogValueSet, LogValueSetAny, LoggableType};
use asrs_model::preferences::{Preferences, Timestamps};
use asrs_model::selection::SelectionMode;
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::TabRenderer;
use asrs_parity::geometry::{GeometryRecord, ScriptedReads};
use asrs_parity::vectors::{Record, load, vectors_dir};
use asrs_parity::{Json, Utf16String, Value};
use asrs_tabs::console::ConsoleNode;
use asrs_tabs::console::controller::{
    ConsoleCommand, ConsoleDom, ConsoleGeometry, ConsoleLine, ConsoleRenderer,
};
use asrs_tabs::console::view::ConsoleRow;
use asrs_tabs::metadata::MetadataRenderer;
use asrs_tabs::statistics::values::{StatisticsRow, StatisticsTable};
use asrs_tabs::statistics::{StatisticsCommand, StatisticsField};
use asrs_tabs::table::{TableCommand, TableCommandField, TableDom, TableGeometry, TableRenderer};
use support::{Host, field, number, opt};

const TABLE_CONTAINER: &str = "div.data-table-container:2";
const TABLE_BODY: &str = "div.data-table-container:2>table.data-table:0>tbody:0";
const CONSOLE_CONTAINER: &str = "div.console-table-container:2";
const CONSOLE_BODY: &str = "div.console-table-container:2>table.console-table:0>tbody:0";

#[derive(Clone)]
struct Scripted(Rc<RefCell<ScriptedReads>>);

impl Scripted {
    fn rect(&self, target: &str) -> Rect {
        let [x, y, width, height] = self.0.borrow_mut().read_rect(target);
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl TableGeometry for Scripted {
    fn scroll_top(&mut self) -> f64 {
        self.0.borrow_mut().read_f64("scrollTop", TABLE_CONTAINER)
    }
    fn set_scroll_top(&mut self, value: f64) {
        let mut reads = self.0.borrow_mut();
        reads.write_f64("scrollTop", TABLE_CONTAINER, value);
    }
    fn container_client_height(&mut self) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("clientHeight", TABLE_CONTAINER)
    }
    fn header_client_height(&mut self) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("clientHeight", &format!("{TABLE_BODY}>tr:0"))
    }
    fn header_rect(&mut self) -> Rect {
        self.rect(&format!("{TABLE_BODY}>tr:0"))
    }
    fn data_row_rect(&mut self, index: usize, _hidden: bool) -> Rect {
        self.rect(&format!("{TABLE_BODY}>tr:{}", index + 2))
    }
    fn container_rect(&mut self) -> Rect {
        unreachable!("a controller read")
    }
    fn header_cell_rect(&mut self, _index: usize) -> Rect {
        unreachable!("a controller read")
    }
    fn header_key_client_width(&mut self, _index: usize) -> f64 {
        unreachable!("a controller read")
    }
    fn header_key_text_width(&mut self, _index: usize) -> f64 {
        unreachable!("a controller read")
    }
}

impl ConsoleGeometry for Scripted {
    fn container_rect(&mut self) -> Rect {
        self.rect(CONSOLE_CONTAINER)
    }
    fn scroll_top(&mut self, _: &[ConsoleRow]) -> f64 {
        self.0.borrow_mut().read_f64("scrollTop", CONSOLE_CONTAINER)
    }
    fn set_scroll_top(&mut self, _: &[ConsoleRow], value: f64) {
        let mut reads = self.0.borrow_mut();
        reads.write_f64("scrollTop", CONSOLE_CONTAINER, value);
    }
    fn scroll_height(&mut self, _: &[ConsoleRow]) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("scrollHeight", CONSOLE_CONTAINER)
    }
    fn container_client_height(&mut self) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("clientHeight", CONSOLE_CONTAINER)
    }
    fn lay_out(&mut self, _: &[ConsoleRow]) {}
    fn row_client_height(&mut self, index: usize) -> f64 {
        let mut reads = self.0.borrow_mut();
        reads.read_f64("clientHeight", &format!("{CONSOLE_BODY}>tr:{}", index + 1))
    }
}

fn js_string(s: &Utf16String) -> JsString {
    JsString::from_units(s.units().to_vec())
}

fn string(value: &Value) -> JsString {
    match value {
        Value::String(s) => js_string(s),
        other => panic!("not a string: {other:?}"),
    }
}

fn boolean(value: &Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| panic!("not a boolean: {value:?}"))
}

fn array(value: &Value) -> &[Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value:?}"))
}

fn numbers(value: &Value) -> Vec<f64> {
    array(value).iter().map(number).collect()
}

fn mode(value: &Value) -> SelectionMode {
    let n = number(value);
    SelectionMode::ALL
        .into_iter()
        .find(|m| m.number() == n)
        .expect("a selection mode")
}

fn to_js(value: &Value) -> JsValue {
    match value {
        Value::Undefined => JsValue::Undefined,
        Value::Null => JsValue::Null,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(bits) => JsValue::Number(f64::from_bits(*bits)),
        Value::String(s) => JsValue::String(js_string(s)),
        Value::Array(items) => JsValue::Array(items.iter().map(to_js).collect()),
        Value::Object(_) => JsValue::Object(to_js_object(value)),
        other => panic!("no JsValue for {other:?}"),
    }
}

fn to_js_object(value: &Value) -> JsObject {
    let Value::Object(pairs) = value else {
        panic!("not an object: {value:?}")
    };
    let mut object = JsObject::new();
    for (k, v) in pairs {
        object.insert(js_string(k), to_js(v));
    }
    object
}

fn s(text: &str) -> Value {
    Value::String(text.into())
}

fn js(text: &JsString) -> Value {
    Value::String(Utf16String::from_units(text.units().to_vec()))
}

fn strings(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|i| s(i)).collect())
}

fn num(n: f64) -> Value {
    Value::Number(n.to_bits())
}

fn hand(shown: bool) -> Value {
    if shown {
        strings(&["opacity 1s ease-in 1s", "0.15"])
    } else {
        strings(&["", "0"])
    }
}

fn row_classes(flags: &[(&str, bool)]) -> Value {
    let on: Vec<&str> = flags
        .iter()
        .filter(|(_, on)| *on)
        .map(|(n, _)| *n)
        .collect();
    strings(&on)
}

fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn value_set(value: &Value, field_type: LoggableType) -> LogValueSetAny {
    let timestamps = numbers(field(value, "timestamps"));
    let items = array(field(value, "values"));
    fn set<T>(timestamps: Vec<f64>, items: &[Value], read: impl Fn(&Value) -> T) -> LogValueSet<T> {
        assert_eq!(timestamps.len(), items.len());
        LogValueSet {
            timestamps,
            values: items.iter().map(read).collect(),
        }
    }
    let of = |read: fn(&Value) -> JsString| move |v: &Value| array(v).iter().map(read).collect();
    match field_type {
        LoggableType::Raw => LogValueSetAny::Raw(set(timestamps, items, |v| match v {
            Value::Bytes(b) => b.clone().into(),
            other => panic!("not bytes: {other:?}"),
        })),
        LoggableType::Boolean => LogValueSetAny::Boolean(set(timestamps, items, boolean)),
        LoggableType::Number => LogValueSetAny::Number(set(timestamps, items, number)),
        LoggableType::String => LogValueSetAny::String(set(timestamps, items, string)),
        LoggableType::BooleanArray => LogValueSetAny::BooleanArray(set(timestamps, items, |v| {
            array(v).iter().map(boolean).collect()
        })),
        LoggableType::NumberArray => LogValueSetAny::NumberArray(set(timestamps, items, numbers)),
        LoggableType::StringArray => {
            LogValueSetAny::StringArray(set(timestamps, items, of(string)))
        }
        LoggableType::Empty => LogValueSetAny::Empty(set(timestamps, items, |_| ())),
    }
}

fn loggable_type(value: &Value) -> Option<LoggableType> {
    const ALL: [LoggableType; 8] = [
        LoggableType::Raw,
        LoggableType::Boolean,
        LoggableType::Number,
        LoggableType::String,
        LoggableType::BooleanArray,
        LoggableType::NumberArray,
        LoggableType::StringArray,
        LoggableType::Empty,
    ];
    let n = opt(value)?;
    Some(
        ALL.into_iter()
            .find(|t| f64::from(*t as u8) == n)
            .expect("a loggable type"),
    )
}

fn table_command(value: &Value) -> TableCommand {
    TableCommand {
        timestamps: numbers(field(value, "timestamps")),
        display_offset: number(field(value, "displayOffset")),
        fields: array(field(value, "fields"))
            .iter()
            .map(|f| {
                let field_type = loggable_type(field(f, "type"));
                TableCommandField {
                    key: to_js(field(f, "key")),
                    is_available: boolean(field(f, "isAvailable")),
                    data: match field(f, "data") {
                        Value::Null => None,
                        data => Some(value_set(data, field_type.unwrap_or(LoggableType::Number))),
                    },
                    field_type,
                }
            })
            .collect(),
        selection_mode: mode(field(value, "selectionMode")),
        selected_time: opt(field(value, "selectedTime")),
        hovered_time: opt(field(value, "hoveredTime")),
    }
}

fn console_command(value: &Value) -> ConsoleCommand {
    ConsoleCommand {
        key: to_js(field(value, "key")),
        key_available: boolean(field(value, "keyAvailable")),
        lines: array(field(value, "lines"))
            .iter()
            .map(|line| ConsoleLine {
                timestamp: number(field(line, "timestamp")),
                value: string(field(line, "value")),
                is_error: boolean(field(line, "isError")),
                is_warning: boolean(field(line, "isWarning")),
            })
            .collect(),
        display_offset: number(field(value, "displayOffset")),
        selection_mode: mode(field(value, "selectionMode")),
        selected_time: opt(field(value, "selectedTime")),
        hovered_time: opt(field(value, "hoveredTime")),
    }
}

fn statistics_command(value: &Value) -> StatisticsCommand {
    StatisticsCommand {
        change_counter: number(field(value, "changeCounter")),
        bins: numbers(field(value, "bins")),
        step_size: number(field(value, "stepSize")),
        fields: array(field(value, "fields"))
            .iter()
            .map(|f| StatisticsField {
                title: string(field(f, "title")),
                color: to_js(field(f, "color")),
                histogram_counts: numbers(field(f, "histogramCounts")),
                stats: to_js_object(field(f, "stats")),
            })
            .collect(),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Run {
    Text {
        text: Vec<u16>,
        style: Option<String>,
        link: Option<JsString>,
        highlight: bool,
    },
    Break,
}

fn push_run(runs: &mut Vec<Run>, run: Run) {
    if let (
        Some(Run::Text {
            text,
            style,
            link,
            highlight,
        }),
        Run::Text {
            text: more,
            style: style2,
            link: link2,
            highlight: highlight2,
        },
    ) = (runs.last_mut(), &run)
        && (&*style, &*link, &*highlight) == (style2, link2, highlight2)
    {
        text.extend_from_slice(more);
        return;
    }
    runs.push(run);
}

fn recorded_runs(nodes: &[Value]) -> Vec<Run> {
    fn visit(
        node: &Value,
        style: Option<&str>,
        link: Option<&JsString>,
        highlight: bool,
        runs: &mut Vec<Run>,
    ) {
        let parts = match node {
            Value::String(text) => {
                push_run(
                    runs,
                    Run::Text {
                        text: text.units().to_vec(),
                        style: style.map(str::to_owned),
                        link: link.cloned(),
                        highlight,
                    },
                );
                return;
            }
            node => array(node),
        };
        let tag = string(&parts[0]).to_string_lossy();
        let attributes: Vec<(String, JsString)> = array(&parts[1])
            .iter()
            .map(|pair| {
                (
                    string(&array(pair)[0]).to_string_lossy(),
                    string(&array(pair)[1]),
                )
            })
            .collect();
        let children = array(&parts[2]);
        let only = match attributes.as_slice() {
            [] => None,
            [(name, value)] => Some((name.as_str(), value)),
            _ => panic!("unexpected attributes in {node:?}"),
        };
        let mut each = |style, link, highlight| {
            for child in children {
                visit(child, style, link, highlight, runs);
            }
        };
        match (tag.as_str(), only) {
            ("span", Some(("style", value))) => {
                each(Some(&value.to_string_lossy()), link, highlight)
            }
            ("span", Some(("class", value))) if *value == JsString::from("highlight") => {
                each(style, link, true)
            }
            ("a", Some(("href", value))) => each(style, Some(value), highlight),
            ("br", None) => runs.push(Run::Break),
            _ => panic!("unexpected element {node:?}"),
        }
    }
    let mut runs = Vec::new();
    for node in nodes {
        visit(node, None, None, false, &mut runs);
    }
    runs
}

fn port_runs(nodes: Vec<ConsoleNode>) -> Vec<Run> {
    let mut runs = Vec::new();
    for node in nodes {
        match node {
            ConsoleNode::LineBreak => runs.push(Run::Break),
            ConsoleNode::Text {
                text,
                style,
                link,
                highlight,
            } => push_run(
                &mut runs,
                Run::Text {
                    text: text.units().to_vec(),
                    style: style.css(),
                    link,
                    highlight,
                },
            ),
        }
    }
    runs
}

struct TableInstance {
    renderer: TableRenderer,
    reads: Rc<RefCell<ScriptedReads>>,
}

struct ConsoleInstance {
    renderer: ConsoleRenderer,
    reads: Rc<RefCell<ScriptedReads>>,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Nothing,
    Saved(JsValue),
    /// Whether the histogram's data was replaced.
    Histogram(bool),
}

enum Instance {
    Table(TableInstance),
    Console(ConsoleInstance),
    Statistics(StatisticsTable),
    Metadata(MetadataRenderer),
}

impl Instance {
    fn new(kind: &str, has_controller: bool) -> Instance {
        let reads = Rc::new(RefCell::new(ScriptedReads::default()));
        let scripted = Box::new(Scripted(reads.clone()));
        match kind {
            "table" => {
                let dom = Rc::new(RefCell::new(TableDom::default()));
                let mut renderer = TableRenderer::new("uuid".into(), dom, has_controller);
                renderer.set_geometry(scripted);
                Instance::Table(TableInstance { renderer, reads })
            }
            "console" => {
                let dom = Rc::new(RefCell::new(ConsoleDom::default()));
                let mut renderer = ConsoleRenderer::new(dom, has_controller);
                renderer.set_geometry(scripted);
                Instance::Console(ConsoleInstance { renderer, reads })
            }
            "statistics" => Instance::Statistics(StatisticsTable::new()),
            "metadata" => Instance::Metadata(MetadataRenderer::new()),
            other => panic!("unknown renderer kind {other:?}"),
        }
    }

    fn reads(&self) -> Option<&Rc<RefCell<ScriptedReads>>> {
        match self {
            Instance::Table(t) => Some(&t.reads),
            Instance::Console(c) => Some(&c.reads),
            Instance::Statistics(_) | Instance::Metadata(_) => None,
        }
    }

    /// Runs one operation, returning the members besides `threw`, `range` and `dom` its `end`
    /// record has.
    fn run(&mut self, host: &mut Host, op: &[Value], dark: bool) -> Result<Outcome, JsError> {
        let name = string(&op[0]).to_string_lossy();
        let args = &op[1..];
        match (self, name.as_str(), args) {
            (Instance::Table(t), "render", [command]) => {
                TabRenderer::<Host>::render(&mut t.renderer, host, &table_command(command))?
            }
            (Instance::Table(t), "mousemove", [y]) => t.renderer.body_mouse_move(number(y)),
            (Instance::Table(t), "mouseleave", []) => t.renderer.body_mouse_leave(),
            (Instance::Table(t), "row", [event, index]) => {
                match string(event).to_string_lossy().as_str() {
                    "click" => t.renderer.row_click(host, number(index) as usize),
                    "contextmenu" => t.renderer.row_context_menu(host),
                    other => panic!("unknown table row event {other:?}"),
                }
            }
            (Instance::Table(t), "jump", [value]) => {
                let dom = t.renderer.dom().clone();
                dom.borrow_mut().jump_input.set_value(string(value));
                t.renderer.jump(host);
            }
            (Instance::Console(c), "render", [command]) => {
                TabRenderer::<Host>::render(&mut c.renderer, host, &console_command(command))?
            }
            (Instance::Console(c), "filter", [value]) => {
                c.renderer.filter_input(host, &string(value))?
            }
            (Instance::Console(c), "highlight", []) => c.renderer.highlight_click(host)?,
            (Instance::Console(c), "restore", [state]) => {
                TabRenderer::<Host>::restore_state(&mut c.renderer, host, &to_js(state))?
            }
            (Instance::Console(c), "save", []) => {
                return Ok(Outcome::Saved(TabRenderer::<Host>::save_state(
                    &c.renderer,
                    host,
                )));
            }
            (Instance::Console(c), "row", [event, index]) => {
                let index = number(index) as usize;
                match string(event).to_string_lossy().as_str() {
                    "mouseenter" => c.renderer.row_mouse_enter(host, index),
                    "mouseleave" => c.renderer.row_mouse_leave(host),
                    "click" => c.renderer.row_click(host, index),
                    "contextmenu" => c.renderer.row_context_menu(host),
                    other => panic!("unknown console row event {other:?}"),
                }
            }
            (Instance::Console(c), "jump", [value]) => {
                let dom = c.renderer.dom().clone();
                dom.borrow_mut().jump_input.set_value(string(value));
                c.renderer.jump();
            }
            (Instance::Console(c), "export", []) => c.renderer.export_click(host),
            (Instance::Statistics(table), "render", [command]) => {
                let replaced = table.render(&statistics_command(command), dark)?;
                return Ok(Outcome::Histogram(replaced));
            }
            (Instance::Metadata(m), "render", [command]) => {
                TabRenderer::<Host>::render(m, host, &to_js(command))?
            }
            (_, other, _) => panic!("unknown operation {other:?}"),
        }
        Ok(Outcome::Nothing)
    }

    fn range(&self, host: &Host) -> Option<Value> {
        match self {
            Instance::Table(t) => Some(
                TabRenderer::<Host>::timestamp_range(&t.renderer, host)
                    .map_or(Value::Null, |[a, b]| Value::Array(vec![num(a), num(b)])),
            ),
            _ => None,
        }
    }

    fn check_dom(&self, recorded: &Value, at: &str) {
        match self {
            Instance::Table(t) => same(&table_dom(&t.renderer), recorded, at),
            Instance::Console(c) => {
                same(&console_dom(&c.renderer), &without(recorded, "rows"), at);
                let rows = array(field(recorded, "rows"));
                let view = &c.renderer.view().rows;
                assert_eq!(view.len(), rows.len(), "{at}: line count");
                for (i, (row, recorded)) in view.iter().zip(rows).enumerate() {
                    let [classes, time, value] = array(recorded) else {
                        panic!("{at}: line {i} has {recorded:?}")
                    };
                    let flags = [
                        ("error", row.error),
                        ("hovered", row.hovered),
                        ("selected", row.selected),
                        ("warning", row.warning),
                    ];
                    assert_eq!(&row_classes(&flags), classes, "{at}: line {i} classes");
                    assert_eq!(&s(&row.timestamp), time, "{at}: line {i} timestamp");
                    assert_eq!(
                        port_runs(row.nodes()),
                        recorded_runs(array(value)),
                        "{at}: line {i} value"
                    );
                }
            }
            Instance::Statistics(table) => same(&statistics_dom(table), recorded, at),
            Instance::Metadata(m) => same(&metadata_dom(m), recorded, at),
        }
    }
}

fn without(value: &Value, key: &str) -> Value {
    let Value::Object(pairs) = value else {
        panic!("not an object: {value:?}")
    };
    assert!(pairs.iter().any(|(k, _)| *k == key.into()), "no {key}");
    Value::Object(
        pairs
            .iter()
            .filter(|(k, _)| *k != key.into())
            .cloned()
            .collect(),
    )
}

/// Compares member by member so a failure names the one that differs.
fn same(port: &Value, recorded: &Value, at: &str) {
    let (Value::Object(port), Value::Object(recorded)) = (port, recorded) else {
        panic!("{at}: not objects")
    };
    let keys = |pairs: &[(Utf16String, Value)]| -> Vec<Utf16String> {
        pairs.iter().map(|(k, _)| k.clone()).collect()
    };
    assert_eq!(keys(port), keys(recorded), "{at}: members");
    for ((key, port), (_, recorded)) in port.iter().zip(recorded) {
        if let (Value::Array(a), Value::Array(b)) = (port, recorded) {
            assert_eq!(a.len(), b.len(), "{at}: {key:?} length");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                assert_eq!(a, b, "{at}: {key:?}[{i}] (port, upstream)");
            }
        } else {
            assert_eq!(port, recorded, "{at}: {key:?} (port, upstream)");
        }
    }
}

fn table_dom(renderer: &TableRenderer) -> Value {
    let view = renderer.view();
    let key_classes: &[&str] = if view.has_controller {
        &["data-table-key-container", "has-close-button"]
    } else {
        &["data-table-key-container"]
    };
    let headers = view
        .headers
        .iter()
        .map(|h| {
            Value::Array(vec![
                js(&h.title),
                js(&h.text),
                s(if h.available { "" } else { "line-through" }),
                strings(key_classes),
                s(""),
                Value::Bool(!view.has_controller),
            ])
        })
        .collect();
    let px = |n: f64| s(&format!("{}px", number::to_string(n)));
    let rows = view
        .rows
        .iter()
        .map(|row| {
            Value::Array(vec![
                row_classes(&[("hovered", row.hovered), ("selected", row.selected)]),
                Value::Bool(row.hidden),
                Value::Array(row.cells.iter().map(|c| s(c)).collect()),
            ])
        })
        .collect();
    object(vec![
        ("headers", Value::Array(headers)),
        (
            "fillers",
            Value::Array(vec![px(view.filler_upper_px), px(view.filler_lower_px)]),
        ),
        ("rows", Value::Array(rows)),
        ("hand", hand(view.show_hand)),
        ("placeholder", s(&view.jump_placeholder)),
    ])
}

fn console_dom(renderer: &ConsoleRenderer) -> Value {
    let view = renderer.view();
    let export = if view.has_controller {
        Value::Bool(false)
    } else {
        Value::Null
    };
    let decoration = if view.field_line_through {
        "line-through"
    } else {
        ""
    };
    object(vec![
        ("field", js(&view.field_text)),
        ("decoration", s(decoration)),
        ("deleteHidden", Value::Bool(view.field_delete_hidden)),
        ("exportButton", export),
        ("highlight", Value::Bool(view.highlight_active)),
        ("filter", js(renderer.filter())),
        ("hand", hand(view.show_hand)),
        ("placeholder", s(&view.jump_placeholder)),
    ])
}

fn statistics_dom(table: &StatisticsTable) -> Value {
    let cell = |text: &str, span: usize, color: Option<&String>| {
        Value::Array(vec![
            s(text),
            num(span as f64),
            color.map_or(Value::Null, |c| s(c)),
        ])
    };
    let rows = table
        .rows()
        .iter()
        .map(|row| match row {
            StatisticsRow::Section { title, col_span } => Value::Array(vec![
                strings(&["section"]),
                Value::Array(vec![cell(title, *col_span, None)]),
            ]),
            StatisticsRow::Values { title, cells } => {
                let mut out = vec![cell(title, 1, None)];
                out.extend(cells.iter().map(|c| cell(&c.text, 1, c.color.as_ref())));
                Value::Array(vec![strings(&["values"]), Value::Array(out)])
            }
        })
        .collect();
    object(vec![("rows", Value::Array(rows))])
}

fn metadata_dom(renderer: &MetadataRenderer) -> Value {
    let table = renderer.table();
    let mut headers = vec![s("")];
    headers.extend(table.columns.iter().map(|c| s(c.title())));
    let rows = table
        .rows
        .iter()
        .map(|row| {
            let mut cells = vec![Value::Array(vec![js(&row.key), strings(&[])])];
            cells.extend(row.cells.iter().map(|cell| {
                Value::Array(vec![
                    js(&cell.text),
                    row_classes(&[("no-data", cell.no_data)]),
                ])
            }));
            Value::Array(cells)
        })
        .collect();
    object(vec![
        ("alertHidden", Value::Bool(table.show_table)),
        ("tableHidden", Value::Bool(!table.show_table)),
        ("headers", Value::Array(headers)),
        ("rows", Value::Array(rows)),
    ])
}

fn case_ids(area: &str) -> Vec<String> {
    let dir = vectors_dir().join(area);
    let ids: BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            format!("{area}/{}", name.split('.').next().unwrap())
        })
        .collect();
    ids.into_iter().collect()
}

#[derive(Default, Debug)]
struct Tally {
    operations: BTreeMap<String, usize>,
    geometry: usize,
    changed: usize,
}

fn kind_of(record: &Record) -> String {
    match &record.json {
        Json::Object(pairs) => pairs.first().map(|(k, _)| k.to_string_lossy()),
        _ => None,
    }
    .unwrap_or_else(|| panic!("line {}: unknown record {}", record.line, record.text))
}

fn replay(id: &str, tally: &mut Tally) {
    let records = load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut host = Host::default();
    let mut instances: Vec<Instance> = Vec::new();
    let mut last_dom: Vec<Value> = Vec::new();
    let (mut begun, mut compared) = (0, 0);
    let mut open: Option<usize> = None;
    for (i, record) in records.iter().enumerate() {
        let r11 = record.string("r11");
        match (r11.as_deref(), open) {
            (Some("create"), None) => {
                let at = format!("{id} line {}", record.line);
                assert_eq!(
                    number(&record.value("id").unwrap()) as usize,
                    instances.len()
                );
                let has_controller = boolean(&record.value("hasController").unwrap());
                let instance = Instance::new(&record.string("kind").unwrap(), has_controller);
                let dom = record.value("dom").unwrap();
                instance.check_dom(&dom, &at);
                instances.push(instance);
                last_dom.push(dom);
            }
            (Some("begin"), None) => {
                begun += 1;
                open = Some(i);
            }
            (Some("end"), Some(start)) => {
                open = None;
                let begin = &records[start];
                let op = begin.value("op").unwrap();
                let op = array(&op);
                let name = string(&op[0]).to_string_lossy();
                let at = format!("{id} line {} {name}", begin.line);
                let index = number(&begin.value("id").unwrap()) as usize;
                assert_eq!(
                    begin.value("id").unwrap(),
                    record.value("id").unwrap(),
                    "{at}"
                );
                let instance = &mut instances[index];

                let span = &records[start + 1..i];
                let (mut calls, mut messages, mut geometry) = (Vec::new(), Vec::new(), 0);
                for r in span {
                    match kind_of(r).as_str() {
                        "r11" => {
                            assert_eq!(r.string("r11").as_deref(), Some("selection"), "{at}");
                            calls.push(r.value("call").unwrap());
                        }
                        "main" => messages
                            .push((r.string("main").unwrap(), to_js(&r.value("data").unwrap()))),
                        "read" | "write" => {
                            assert!(GeometryRecord::parse(r).unwrap().is_some(), "{at}");
                            geometry += 1;
                        }
                        _ => panic!("{at}: line {} inside a bracket: {}", r.line, r.text),
                    }
                }
                match instance.reads() {
                    Some(reads) => *reads.borrow_mut() = ScriptedReads::from_records(span).unwrap(),
                    None => assert_eq!(geometry, 0, "{at}: geometry of a tableless renderer"),
                }

                host.selection.calls.clear();
                host.messages.clear();
                host.preferences = Some(Preferences {
                    timestamps: match begin.string("timestamps").unwrap().as_str() {
                        "original" => Timestamps::Original,
                        "start-zero" => Timestamps::StartZero,
                        other => panic!("{at}: timestamps {other:?}"),
                    },
                    ..Preferences::default()
                });
                let dark = boolean(&begin.value("dark").unwrap());
                let result = instance.run(&mut host, op, dark);
                let threw = record
                    .field("threw")
                    .map(|_| string(&record.value("threw").unwrap()).to_string_lossy());
                let error = result
                    .as_ref()
                    .err()
                    .map(|e| format!("{}: {}", e.name, e.message));
                assert_eq!(error, threw, "{at}: throw");
                let member = |key| record.field(key).map(|_| record.value(key).unwrap());
                let recorded = match (member("result"), member("histogram")) {
                    (None, None) => Outcome::Nothing,
                    (Some(saved), None) => Outcome::Saved(to_js(&saved)),
                    (None, Some(replaced)) => Outcome::Histogram(boolean(&replaced)),
                    both => panic!("{at}: {both:?}"),
                };
                match result {
                    Ok(outcome) => assert_eq!(outcome, recorded, "{at}: outcome"),
                    Err(_) => assert!(
                        matches!(recorded, Outcome::Nothing | Outcome::Histogram(false)),
                        "{at}: a throw with {recorded:?}"
                    ),
                }
                assert_eq!(host.selection.calls, calls, "{at}: selection calls");
                let sent: Vec<(String, JsValue)> = host
                    .messages
                    .iter()
                    .map(|m| (m.name.name().to_owned(), m.data.clone()))
                    .collect();
                assert_eq!(sent, messages, "{at}: main messages");
                if let Some(reads) = instance.reads() {
                    let taken = std::mem::take(&mut *reads.borrow_mut());
                    let n = taken.finish().unwrap_or_else(|e| panic!("{at}: {e}"));
                    assert_eq!(n, geometry, "{at}: geometry records compared");
                    tally.geometry += n;
                }
                if let Some(range) = instance.range(&host) {
                    assert_eq!(
                        range,
                        record.value("range").unwrap(),
                        "{at}: getTimestampRange()"
                    );
                }
                let dom = record.value("dom").unwrap();
                instance.check_dom(&dom, &at);
                if dom != last_dom[index] {
                    tally.changed += 1;
                    last_dom[index] = dom;
                }
                *tally.operations.entry(name).or_default() += 1;
                compared += 1;
            }
            (Some("selection"), Some(_)) | (None, Some(_)) => {}
            (Some(other), _) => panic!("{id} line {}: r11 {other:?} out of place", record.line),
            (None, None) => match kind_of(record).as_str() {
                "read" | "write" | "main" => {}
                _ => panic!("{id} line {}: unknown record {}", record.line, record.text),
            },
        }
    }
    assert!(open.is_none(), "{id} ends inside a bracket");
    assert_eq!(compared, begun, "{id}: operations compared");
    assert!(compared > 0, "{id} records no operation");
}

fn replay_area(area: &str, cases: usize, operations: &[&str]) {
    let ids = case_ids(area);
    assert_eq!(ids.len(), cases, "{area} vectors");
    let mut tally = Tally::default();
    for id in &ids {
        replay(id, &mut tally);
    }
    println!("{area}: {tally:?}");
    let seen: Vec<&str> = tally.operations.keys().map(String::as_str).collect();
    assert_eq!(seen, operations, "{area}: operations replayed");
}

#[test]
fn table_views_equal_blink() {
    replay_area(
        "dom/table-view",
        6,
        &["jump", "mouseleave", "mousemove", "render", "row"],
    );
}

#[test]
fn console_views_equal_blink() {
    replay_area(
        "dom/console-view",
        6,
        &[
            "export",
            "filter",
            "highlight",
            "jump",
            "render",
            "restore",
            "row",
            "save",
        ],
    );
}

#[test]
fn statistics_values_equal_blink() {
    replay_area("dom/statistics-view", 3, &["render"]);
}

#[test]
fn metadata_tables_equal_blink() {
    replay_area("dom/metadata-view", 2, &["render"]);
}
