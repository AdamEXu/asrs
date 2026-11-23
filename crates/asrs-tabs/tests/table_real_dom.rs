//! Replays `dom/tabs/table-real`, a Blink run with real geometry, against
//! `asrs_tabs::table::TableRenderer`. The fixture's recorder brackets every upstream
//! `render(command)` with `{"tr": "render"}` and `{"tr": "rendered"}`; the port renders the
//! same command with its layout reads answered, in order, by the `read` records inside the
//! bracket and its scroll writes checked against the `write` records, through
//! `asrs_parity::geometry::ScriptedReads`. Geometry records outside a bracket belong to the
//! tab bar, sidebar and timeline, which other models replay.

use std::cell::{Ref, RefCell};
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::Rng;
use asrs_js::value::JsValue;
use asrs_log::log::Log;
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::{Selection, SelectionMode, TimeSupplier};
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::TabRenderer;
use asrs_parity::geometry::{GeometryRecord, ScriptedReads};
use asrs_parity::vectors::{Record, load};
use asrs_parity::{Json, Value};
use asrs_tabs::cx::TabCx;
use asrs_tabs::table::{TableCommand, TableCommandField, TableDom, TableGeometry, TableRenderer};

const VECTOR: &str = "dom/tabs/table-real";
const CONTAINER: &str = "div.data-table-container:2";
const HEADER: &str = "div.data-table-container:2>table.data-table:0>tbody:0>tr:0";

fn data_row(index: usize) -> String {
    format!(
        "div.data-table-container:2>table.data-table:0>tbody:0>tr:{}",
        index + 2
    )
}

struct ScriptedTable(Rc<RefCell<ScriptedReads>>);

fn rect([x, y, width, height]: [f64; 4]) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

impl TableGeometry for ScriptedTable {
    fn scroll_top(&mut self) -> f64 {
        self.0.borrow_mut().read_f64("scrollTop", CONTAINER)
    }
    fn set_scroll_top(&mut self, value: f64) {
        self.0.borrow_mut().write_f64("scrollTop", CONTAINER, value);
    }
    fn container_client_height(&mut self) -> f64 {
        self.0.borrow_mut().read_f64("clientHeight", CONTAINER)
    }
    fn header_client_height(&mut self) -> f64 {
        self.0.borrow_mut().read_f64("clientHeight", HEADER)
    }
    fn header_rect(&mut self) -> Rect {
        rect(self.0.borrow_mut().read_rect(HEADER))
    }
    fn data_row_rect(&mut self, index: usize, _hidden: bool) -> Rect {
        rect(self.0.borrow_mut().read_rect(&data_row(index)))
    }
}

#[derive(Default)]
struct RecordingSelection {
    calls: Vec<Value>,
}

impl RecordingSelection {
    fn call(&mut self, name: &str, args: Vec<Value>) {
        let mut call = vec![Value::String(name.into())];
        call.extend(args);
        self.calls.push(Value::Array(call));
    }

    fn unexpected(&self, name: &str) -> ! {
        panic!("the table renderer called selection.{name}")
    }
}

fn opt_number(value: Option<f64>) -> Value {
    value.map_or(Value::Null, |n| Value::Number(n.to_bits()))
}

impl Selection for RecordingSelection {
    fn get_mode(&self) -> SelectionMode {
        self.unexpected("getMode")
    }
    fn get_hovered_time(&self) -> Option<f64> {
        self.unexpected("getHoveredTime")
    }
    fn set_hovered_time(&mut self, value: Option<f64>) {
        self.call("setHoveredTime", vec![opt_number(value)]);
    }
    fn get_selected_time(&mut self) -> Option<f64> {
        self.unexpected("getSelectedTime")
    }
    fn set_selected_time(&mut self, time: f64) {
        self.call("setSelectedTime", vec![Value::Number(time.to_bits())]);
    }
    fn go_idle(&mut self) {
        self.call("goIdle", Vec::new());
    }
    fn play(&mut self) {
        self.call("play", Vec::new());
    }
    fn pause(&mut self) {
        self.call("pause", Vec::new());
    }
    fn toggle_playback(&mut self) {
        self.unexpected("togglePlayback")
    }
    fn lock(&mut self) {
        self.call("lock", Vec::new());
    }
    fn unlock(&mut self) {
        self.call("unlock", Vec::new());
    }
    fn toggle_lock(&mut self) {
        self.unexpected("toggleLock")
    }
    fn step_cycle(&mut self, _: bool) {
        self.unexpected("stepCycle")
    }
    fn set_live_connected(&mut self, _: TimeSupplier) {
        self.unexpected("setLiveConnected")
    }
    fn set_live_disconnected(&mut self) {
        self.unexpected("setLiveDisconnected")
    }
    fn get_current_live_time(&self) -> Option<f64> {
        self.unexpected("getCurrentLiveTime")
    }
    fn get_render_time(&mut self) -> Option<f64> {
        self.unexpected("getRenderTime")
    }
    fn set_playback_speed(&mut self, _: f64) {
        self.unexpected("setPlaybackSpeed")
    }
    fn set_playback_looping(&mut self, _: bool) {
        self.unexpected("setPlaybackLooping")
    }
    fn set_grab_zoom_range(&mut self, _: Option<[f64; 2]>) {
        self.unexpected("setGrabZoomRange")
    }
    fn get_grab_zoom_range(&self) -> Option<[f64; 2]> {
        self.unexpected("getGrabZoomRange")
    }
    fn finish_grab_zoom(&mut self) {
        self.unexpected("finishGrabZoom")
    }
    fn get_timeline_range(&mut self) -> [f64; 2] {
        self.unexpected("getTimelineRange")
    }
    fn get_timeline_is_max_zoom(&self) -> bool {
        self.unexpected("getTimelineIsMaxZoom")
    }
    fn set_timeline_range(&mut self, _: [f64; 2], _: bool) {
        self.unexpected("setTimelineRange")
    }
    fn apply_timeline_scroll(&mut self, _: f64, _: f64, _: f64) {
        self.unexpected("applyTimelineScroll")
    }
}

struct NoRandom;

impl Rng for NoRandom {
    fn next_f64(&mut self) -> f64 {
        panic!("the table renderer draws no Math.random values")
    }
}

struct Host {
    log: RefCell<Log>,
    selection: RecordingSelection,
    rng: NoRandom,
}

impl TabCx for Host {
    fn tab_log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }
    fn tab_selection(&mut self) -> &mut dyn Selection {
        &mut self.selection
    }
    fn tab_preferences(&self) -> Option<&Preferences> {
        None
    }
    fn tab_preferences_mut(&mut self) -> Option<&mut Preferences> {
        None
    }
    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        None
    }
    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }
    fn send_main_message(&mut self, _: NamedMessage<HubToMain>) {
        panic!("the table renderer sends no main messages")
    }
    fn send_open_xr(&mut self, _: JsValue) {
        panic!("the table renderer sends no main messages")
    }
}

fn field<'a>(object: &'a Value, key: &str) -> &'a Value {
    match object {
        Value::Object(pairs) => pairs
            .iter()
            .find(|(k, _)| *k == key.into())
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("command has no {key}")),
        other => panic!("not an object: {other:?}"),
    }
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value:?}"))
}

fn opt(value: &Value) -> Option<f64> {
    match value {
        Value::Null => None,
        v => Some(number(v)),
    }
}

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
    let reads = Rc::new(RefCell::new(ScriptedReads::default()));
    renderer.set_geometry(Box::new(ScriptedTable(reads.clone())));
    let mut host = Host {
        log: RefCell::new(Log::new(true)),
        selection: RecordingSelection::default(),
        rng: NoRandom,
    };

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
                *reads.borrow_mut() = ScriptedReads::from_records(span).unwrap();
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
                assert_eq!(
                    host.selection.calls, selection_calls,
                    "{at}: selection calls"
                );
                let taken = std::mem::take(&mut *reads.borrow_mut());
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
    assert!(
        renders > 100 && geometry_compared > 1000,
        "{renders} renders, {geometry_compared} reads"
    );
}
