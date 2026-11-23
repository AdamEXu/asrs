//! Replays `dom/tabs/table-real`, a Blink run with real geometry, against
//! `asrs_tabs::table::TableRenderer`. The fixture's recorder brackets every upstream
//! `render(command)` with `{"tr": "render"}` and `{"tr": "rendered"}`; the port renders the
//! same command with its layout reads answered, in order, by the `read` records inside the
//! bracket and its scroll writes checked against the `write` records, through
//! `asrs_parity::geometry::ScriptedReads`, and every read is also checked against
//! `asrs_tabs::table::layout::TableLayout`, the layout the app's view answers from. Geometry records outside a bracket belong to the
//! tab bar, sidebar and timeline, which other models replay.

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::selection::SelectionMode;
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::TabRenderer;
use asrs_parity::geometry::{GeometryRecord, ScriptedReads};
use asrs_parity::vectors::{Record, load};
use asrs_parity::{Json, Value};
use asrs_tabs::table::{TableCommand, TableCommandField, TableDom, TableGeometry, TableRenderer};
use support::table_geometry::{CONTAINER, SharedTable};
use support::{Host, field, number, opt};

const VECTOR: &str = "dom/tabs/table-real";
/// Two steps scroll the container behind the recorder's back, as a user does.
const USER_SCROLLS: usize = 2;

/// The recorded `TableRendererCommand`. The renderer reads each field's key and availability;
/// cell text is the view's.
fn command(value: &Value) -> TableCommand {
    let Value::Array(timestamps) = field(value, "timestamps") else {
        panic!("timestamps is not an array")
    };
    let Value::Array(fields) = field(value, "fields") else {
        panic!("fields is not an array")
    };
    let mode = number(field(value, "selectionMode"));
    TableCommand {
        timestamps: timestamps.iter().map(number).collect(),
        display_offset: number(field(value, "displayOffset")),
        fields: fields
            .iter()
            .map(|f| TableCommandField {
                key: match field(f, "key") {
                    Value::String(s) => JsValue::String(JsString::from_units(s.units().to_vec())),
                    other => panic!("field key {other:?}"),
                },
                is_available: matches!(field(f, "isAvailable"), Value::Bool(true)),
                data: None,
                field_type: None,
            })
            .collect(),
        selection_mode: SelectionMode::ALL
            .into_iter()
            .find(|m| m.number() == mode)
            .expect("a selection mode"),
        selected_time: opt(field(value, "selectedTime")),
        hovered_time: opt(field(value, "hoveredTime")),
    }
}

fn range_value(range: Option<[f64; 2]>) -> Value {
    range.map_or(Value::Null, |[a, b]| {
        Value::Array(vec![Value::Number(a.to_bits()), Value::Number(b.to_bits())])
    })
}

#[test]
fn table_real_geometry_replays_through_scripted_reads() {
    let records = load(VECTOR).unwrap_or_else(|e| panic!("{e}"));
    let dom = Rc::new(RefCell::new(TableDom::default()));
    let mut renderer = TableRenderer::new("uuid".into(), dom, true);
    let table = SharedTable::default();
    table.0.borrow_mut().layout.container = Rect {
        x: 300.0,
        y: 88.0,
        width: 800.0,
        height: 562.0,
    };
    renderer.set_geometry(Box::new(table.clone()));
    let mut user_scrolls = 0;
    let mut host = Host::default();

    let (mut renders, mut recorded_renders) = (0, 0);
    let (mut geometry_compared, mut geometry_recorded) = (0, 0);
    let mut open: Option<(usize, &Record)> = None;
    for (i, record) in records.iter().enumerate() {
        let tr = record.string("tr");
        match (tr.as_deref(), open) {
            (Some("render"), None) => {
                recorded_renders += 1;
                open = Some((i, record));
            }
            (Some("rendered"), Some((start, begin))) => {
                open = None;
                let span = &records[start + 1..i];
                let mut selection_calls = Vec::new();
                for r in span {
                    match r.string("tr").as_deref() {
                        Some("selection") => selection_calls.push(r.value("call").unwrap()),
                        Some(other) => panic!("line {}: {other} inside a render", r.line),
                        None if GeometryRecord::parse(r).unwrap().is_some() => {
                            geometry_recorded += 1
                        }
                        None => {
                            panic!("line {}: unexpected record in a render: {}", r.line, r.text)
                        }
                    }
                }
                {
                    let mut t = table.0.borrow_mut();
                    t.reads = ScriptedReads::from_records(span).unwrap();
                    let first = t.reads.peek().filter(|g| g.name == "scrollTop").unwrap();
                    assert!(first.is_on(CONTAINER));
                    let scroll_top = first.value.as_f64().unwrap();
                    if scroll_top != t.layout.scroll_top() {
                        t.layout.set_scroll_top(scroll_top);
                        user_scrolls += 1;
                    }
                }
                host.selection.calls.clear();
                let cmd = command(&begin.value("command").unwrap());
                let result = TabRenderer::<Host>::render(&mut renderer, &mut host, &cmd);
                let at = format!("render at line {}", begin.line);
                match record.field("threw") {
                    Some(_) => assert!(result.is_err(), "{at}: upstream threw, the port did not"),
                    None => {
                        result.unwrap_or_else(|e| panic!("{at}: {e:?}"));
                        assert_eq!(
                            range_value(TabRenderer::<Host>::timestamp_range(&renderer, &host)),
                            record.value("range").unwrap(),
                            "{at}: getTimestampRange()"
                        );
                    }
                }
                assert!(host.messages.is_empty(), "{at}: main messages");
                assert_eq!(
                    host.selection.calls, selection_calls,
                    "{at}: selection calls"
                );
                let taken = std::mem::take(&mut table.0.borrow_mut().reads);
                geometry_compared += taken.finish().unwrap_or_else(|e| panic!("{at}: {e}"));
                renders += 1;
            }
            (Some("mousemove"), None) => {
                renderer.body_mouse_move(number(&record.value("y").unwrap()));
            }
            (Some("mouseleave"), None) => renderer.body_mouse_leave(),
            (Some("selection"), Some(_)) => {}
            (Some(other), _) => panic!("line {}: tr {other:?} out of place", record.line),
            (None, None) => {
                let kind = match &record.json {
                    Json::Object(pairs) => pairs.first().map(|(k, _)| k.to_string_lossy()),
                    _ => None,
                };
                match kind.as_deref() {
                    Some("read" | "write" | "main" | "capture" | "selection") => {}
                    _ => panic!("line {}: unknown record {}", record.line, record.text),
                }
            }
            (None, Some(_)) => {}
        }
    }
    assert!(open.is_none(), "the vector ends inside a render");
    assert_eq!(renders, recorded_renders, "renders compared");
    assert_eq!(
        geometry_compared, geometry_recorded,
        "geometry records compared"
    );
    let t = table.0.borrow();
    assert!(t.mismatches.is_empty(), "{}", t.mismatches.join("\n"));
    assert!(t.checked > 1000, "{} reads checked", t.checked);
    assert_eq!(user_scrolls, USER_SCROLLS);
    assert!(
        renders > 100 && geometry_compared > 1000,
        "{renders} renders, {geometry_compared} reads"
    );
}
