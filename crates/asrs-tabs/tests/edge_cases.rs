#![allow(clippy::disallowed_methods)]

//! Edge cases for the Table, Console and Metadata controller ports that the committed
//! `dom/tabs` vectors do not reach: empty logs, wrong-typed saved state, huge field lists
//! and unicode keys. Assertions are on the ported command structs and `to_js` output.

use std::cell::{Ref, RefCell};
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::{Log, LoggableType};
use asrs_model::preferences::Preferences;
use asrs_model::selection::Selection;
use asrs_model::selection_impl::SelectionImpl;
use asrs_model::tab_controller::TabController;
use asrs_tabs::console::controller::{ConsoleCommand, ConsoleController, ConsoleDom};
use asrs_tabs::cx::TabCx;
use asrs_tabs::metadata::MetadataController;
use asrs_tabs::table::{TableCommand, TableController, TableDom};

struct TestRng(u64);

impl Rng for TestRng {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / 9007199254740992.0
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> f64 {
        0.0
    }
}

struct MockCx {
    log: Rc<RefCell<Log>>,
    selection: SelectionImpl,
    preferences: Option<Preferences>,
    rng: TestRng,
}

impl TabCx for MockCx {
    fn tab_log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }

    fn tab_selection(&mut self) -> &mut dyn Selection {
        &mut self.selection
    }

    fn tab_preferences(&self) -> Option<&Preferences> {
        self.preferences.as_ref()
    }

    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }

    fn tab_renderer_top(&self) -> f64 {
        0.0
    }
}

fn mock_cx() -> MockCx {
    let log = Rc::new(RefCell::new(Log::new(true)));
    let selection = SelectionImpl::new(log.clone(), Rc::new(FixedClock));
    MockCx {
        log,
        selection,
        preferences: None,
        rng: TestRng(0x9e37_79b9),
    }
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn key(s: &str) -> JsValue {
    JsValue::String(js(s))
}

fn uuid() -> JsValue {
    key("tab-uuid")
}

fn table_controller(fields: &[&str]) -> TableController {
    let mut c = TableController::new(&js("uuid"), Rc::new(RefCell::new(TableDom::default())));
    let mut cx = mock_cx();
    c.restore_state(
        &mut cx,
        &JsValue::Array(fields.iter().map(|f| key(f)).collect()),
    )
    .unwrap();
    c
}

fn table_command(c: &mut TableController, cx: &mut MockCx) -> TableCommand {
    c.command(cx, &uuid())
        .unwrap()
        .as_any()
        .downcast_ref::<TableCommand>()
        .unwrap()
        .clone()
}

#[test]
fn table_empty_log() {
    let mut cx = mock_cx();
    let mut c = table_controller(&["/a", "/b"]);
    let cmd = table_command(&mut c, &mut cx);
    assert!(cmd.timestamps.is_empty());
    assert_eq!(cmd.fields.len(), 2);
    for field in &cmd.fields {
        assert!(!field.is_available);
        assert!(field.data.is_none());
        assert!(field.field_type.is_none());
    }
}

#[test]
fn table_restore_rejects_wrong_types() {
    let mut cx = mock_cx();
    let mut c = table_controller(&["/keep"]);

    // `checkArrayType(state, "string")` is false for each of these, so `fields` stays.
    for state in [
        JsValue::Null,
        JsValue::Undefined,
        JsValue::Number(1.0),
        JsValue::Bool(true),
        key("/not-an-array"),
        JsValue::Object(JsObject::new()),
        JsValue::Array(vec![key("/x"), JsValue::Number(2.0)]),
        JsValue::Array(vec![JsValue::Null]),
    ] {
        c.restore_state(&mut cx, &state).unwrap();
        assert_eq!(c.active_fields(&cx), vec![js("/keep")]);
    }
}

#[test]
fn table_restore_accepts_empty_array() {
    let mut cx = mock_cx();
    let mut c = table_controller(&["/gone"]);
    c.restore_state(&mut cx, &JsValue::Array(vec![])).unwrap();
    assert!(c.active_fields(&cx).is_empty());
    let cmd = table_command(&mut c, &mut cx);
    assert!(cmd.fields.is_empty());
}

#[test]
fn table_save_restore_round_trip() {
    let mut cx = mock_cx();
    let a = table_controller(&["/x", "/y", "/z"]);
    let saved = a.save_state(&cx);
    let mut b = TableController::new(&js("uuid2"), Rc::new(RefCell::new(TableDom::default())));
    b.restore_state(&mut cx, &saved).unwrap();
    assert_eq!(b.active_fields(&cx), vec![js("/x"), js("/y"), js("/z")],);
    assert_eq!(b.save_state(&cx), saved);
}

#[test]
fn table_wrong_field_types() {
    let mut cx = mock_cx();
    {
        let mut log = cx.log.borrow_mut();
        log.put_string(&js("/text"), 1.0, js("hello"));
        log.put_boolean(&js("/flag"), 1.0, true);
        log.put_number(&js("/num"), 1.0, 42.0);
    }
    let mut c = table_controller(&["/text", "/flag", "/num"]);
    let cmd = table_command(&mut c, &mut cx);
    let types: Vec<Option<LoggableType>> = cmd.fields.iter().map(|f| f.field_type).collect();
    assert_eq!(
        types,
        vec![
            Some(LoggableType::String),
            Some(LoggableType::Boolean),
            Some(LoggableType::Number),
        ]
    );
    for field in &cmd.fields {
        assert!(field.is_available);
        assert!(field.data.is_some());
    }
}

#[test]
fn table_huge_field_list() {
    let mut cx = mock_cx();
    let keys: Vec<String> = (0..512).map(|i| format!("/field/{i}")).collect();
    {
        let mut log = cx.log.borrow_mut();
        for key in &keys {
            log.put_number(&js(key), 0.0, 1.0);
            log.put_number(&js(key), 5.0, 2.0);
            log.put_number(&js(key), 10.0, 3.0);
        }
    }
    let mut c = table_controller(&keys.iter().map(String::as_str).collect::<Vec<_>>());
    c.add_renderer_range(&mut cx, &js("renderer"), Some([0.0, 100.0]))
        .unwrap();
    let cmd = table_command(&mut c, &mut cx);
    assert_eq!(cmd.fields.len(), 512);
    for (i, field) in cmd.fields.iter().enumerate() {
        assert!(field.is_available, "field {i} unavailable");
        assert_eq!(field.field_type, Some(LoggableType::Number));
        let data = field.data.as_ref().unwrap();
        assert_eq!(data.timestamps().len(), 3);
    }
}

#[test]
fn table_unicode_keys() {
    let mut cx = mock_cx();
    let keys = ["//", "/emoji/😀", "/données/été", "／／"];
    {
        let mut log = cx.log.borrow_mut();
        for (i, key) in keys.iter().enumerate() {
            log.put_number(&js(key), i as f64, i as f64 * 10.0);
        }
    }
    let mut c = table_controller(&keys);
    assert_eq!(
        c.active_fields(&cx),
        keys.iter().map(|k| js(k)).collect::<Vec<_>>()
    );
    let cmd = table_command(&mut c, &mut cx);
    for (i, field) in cmd.fields.iter().enumerate() {
        assert_eq!(field.key, key(keys[i]));
        assert!(field.is_available);
        assert_eq!(field.field_type, Some(LoggableType::Number));
    }
    let saved = c.save_state(&cx);
    let mut restored =
        TableController::new(&js("uuid"), Rc::new(RefCell::new(TableDom::default())));
    restored.restore_state(&mut cx, &saved).unwrap();
    assert_eq!(restored.save_state(&cx), saved);
}

#[test]
fn table_command_carries_selection() {
    let mut cx = mock_cx();
    cx.selection.set_selected_time(2.5);
    cx.selection.set_hovered_time(Some(6.0));
    let mut c = table_controller(&[]);
    let cmd = table_command(&mut c, &mut cx);
    assert_eq!(cmd.selected_time, Some(2.5));
    assert_eq!(cmd.hovered_time, Some(6.0));
}

fn console_controller() -> ConsoleController {
    ConsoleController::new(Rc::new(RefCell::new(ConsoleDom::default())))
}

fn console_command(c: &mut ConsoleController, cx: &mut MockCx) -> ConsoleCommand {
    c.command(cx, &uuid())
        .unwrap()
        .as_any()
        .downcast_ref::<ConsoleCommand>()
        .unwrap()
        .clone()
}

#[test]
fn console_empty_log() {
    let mut cx = mock_cx();
    let mut c = console_controller();
    let cmd = console_command(&mut c, &mut cx);
    assert_eq!(cmd.key, JsValue::Null);
    assert!(!cmd.key_available);
    assert!(cmd.lines.is_empty());
    assert_eq!(c.save_state(&cx), JsValue::Null);
    assert!(c.active_fields(&cx).is_empty());
}

#[test]
fn console_restore_wrong_types_ignored() {
    let mut cx = mock_cx();
    let mut c = console_controller();
    for state in [
        JsValue::Undefined,
        JsValue::Number(4.0),
        JsValue::Bool(false),
        JsValue::Array(vec![]),
        JsValue::Object(JsObject::new()),
    ] {
        c.restore_state(&mut cx, &state).unwrap();
        assert_eq!(c.save_state(&cx), JsValue::Null);
    }
    // Null is accepted: upstream checks `typeof state === "string" || state === null`.
    c.restore_state(&mut cx, &key("/RealOutputs/Console"))
        .unwrap();
    c.restore_state(&mut cx, &JsValue::Null).unwrap();
    assert_eq!(c.save_state(&cx), JsValue::Null);
}

#[test]
fn console_string_field_lines() {
    let mut cx = mock_cx();
    {
        let mut log = cx.log.borrow_mut();
        log.put_string(&js("/Console"), 1.0, js("Everything is fine"));
        log.put_string(&js("/Console"), 2.0, js("ERROR: motor stalled"));
        log.put_string(&js("/Console"), 3.0, js("Warning: brownout"));
    }
    let mut c = console_controller();
    c.restore_state(&mut cx, &key("/Console")).unwrap();
    let cmd = console_command(&mut c, &mut cx);
    assert!(cmd.key_available);
    assert_eq!(cmd.lines.len(), 3);
    assert!(!cmd.lines[0].is_error && !cmd.lines[0].is_warning);
    assert!(cmd.lines[1].is_error);
    assert!(cmd.lines[2].is_warning);
    assert_eq!(cmd.lines[1].timestamp, 2.0);
}

#[test]
fn console_selected_key_unavailable() {
    let mut cx = mock_cx();
    {
        let mut log = cx.log.borrow_mut();
        log.put_number(&js("/notConsole"), 1.0, 5.0);
    }
    let mut c = console_controller();
    c.restore_state(&mut cx, &key("/missing")).unwrap();
    let cmd = console_command(&mut c, &mut cx);
    assert!(!cmd.key_available);
    assert!(cmd.lines.is_empty());
    // A field that exists but is not a console type still reports available and no lines.
    c.restore_state(&mut cx, &key("/notConsole")).unwrap();
    let cmd = console_command(&mut c, &mut cx);
    assert!(cmd.key_available);
    assert!(cmd.lines.is_empty());
}

#[test]
fn console_structured_type_active_fields() {
    let mut cx = mock_cx();
    {
        let mut log = cx.log.borrow_mut();
        log.create_blank_field(&js("/RealOutputs/Console"), LoggableType::Empty);
        log.put_string(&js("/RealOutputs/Console/ConsoleLine"), 1.0, js("line"));
        log.set_structured_type(&js("/RealOutputs/Console"), Some(js("Console")));
    }
    let mut c = console_controller();
    c.restore_state(&mut cx, &key("/RealOutputs/Console"))
        .unwrap();
    assert_eq!(
        c.active_fields(&cx),
        vec![
            js("/RealOutputs/Console/ConsoleLine"),
            js("/RealOutputs/Console/ErrorInfo"),
            js("/RealOutputs/Console/ProgramCrashInfo"),
        ]
    );
}

#[test]
fn console_unicode_values() {
    let mut cx = mock_cx();
    {
        let mut log = cx.log.borrow_mut();
        log.put_string(&js("//Console"), 1.0, js("エラー: モーター"));
        log.put_string(&js("//Console"), 2.0, js(" ⚠️"));
    }
    let mut c = console_controller();
    c.restore_state(&mut cx, &key("//Console")).unwrap();
    let cmd = console_command(&mut c, &mut cx);
    assert!(cmd.key_available);
    assert_eq!(cmd.lines.len(), 2);
    assert_eq!(cmd.lines[0].value, js("エラー: モーター"));
    assert!(!cmd.lines[0].is_error); // lowercase search is ASCII "error"
}

#[test]
fn console_save_restore_round_trip() {
    let mut cx = mock_cx();
    let mut a = console_controller();
    a.restore_state(&mut cx, &key("/Console")).unwrap();
    let saved = a.save_state(&cx);
    assert_eq!(saved, key("/Console"));
    let mut b = console_controller();
    b.restore_state(&mut cx, &saved).unwrap();
    assert_eq!(b.save_state(&cx), saved);
}

#[test]
fn metadata_empty_log() {
    let mut cx = mock_cx();
    let mut c = MetadataController::new(&mut cx).unwrap();
    let cmd = c
        .command(&mut cx, &uuid())
        .unwrap()
        .as_any()
        .downcast_ref::<JsValue>()
        .unwrap()
        .clone();
    assert_eq!(cmd, JsValue::Object(JsObject::new()));
    assert_eq!(c.save_state(&cx), JsValue::Null);
    assert!(!c.active_fields(&cx).is_empty());
}

#[test]
fn metadata_fields_and_slots() {
    let mut cx = mock_cx();
    {
        let mut log = cx.log.borrow_mut();
        log.put_string(&js("/Metadata/Name"), 0.0, js("generic-bot"));
        log.put_string(&js("/RealMetadata/Name"), 0.0, js("real-bot"));
        log.put_string(&js("/ReplayMetadata/Name"), 0.0, js("replay-bot"));
        log.put_string(&js("/Metadata/Other"), 0.0, js("only-generic"));
        log.put_number(&js("/Metadata/Numeric"), 0.0, 3.0);
    }
    let mut c = MetadataController::new(&mut cx).unwrap();
    let cmd = c
        .command(&mut cx, &uuid())
        .unwrap()
        .as_any()
        .downcast_ref::<JsValue>()
        .unwrap()
        .clone();
    let JsValue::Object(map) = cmd else {
        panic!("command is an object")
    };
    let JsValue::Object(name) = map.get_key(&js("/Name")).unwrap().clone() else {
        panic!("entry is an object")
    };
    assert_eq!(name.get_key(&js("generic")), Some(&key("generic-bot")));
    assert_eq!(name.get_key(&js("real")), Some(&key("real-bot")));
    assert_eq!(name.get_key(&js("replay")), Some(&key("replay-bot")));
    let JsValue::Object(other) = map.get_key(&js("/Other")).unwrap().clone() else {
        panic!("entry is an object")
    };
    assert_eq!(other.get_key(&js("generic")), Some(&key("only-generic")));
    assert_eq!(other.get_key(&js("real")), Some(&JsValue::Null));
    // Non-string metadata values produce no slot write: `getString` returns undefined.
    let JsValue::Object(numeric) = map.get_key(&js("/Numeric")).unwrap().clone() else {
        panic!("entry is an object")
    };
    assert_eq!(numeric.get_key(&js("generic")), Some(&JsValue::Null));
}

#[test]
fn metadata_unicode_values() {
    let mut cx = mock_cx();
    {
        let mut log = cx.log.borrow_mut();
        log.put_string(&js("/Metadata/チーム"), 0.0, js("ロボット 🤖"));
    }
    let mut c = MetadataController::new(&mut cx).unwrap();
    let cmd = c
        .command(&mut cx, &uuid())
        .unwrap()
        .as_any()
        .downcast_ref::<JsValue>()
        .unwrap()
        .clone();
    let JsValue::Object(map) = cmd else {
        panic!("command is an object")
    };
    let JsValue::Object(entry) = map.get_key(&js("/チーム")).unwrap().clone() else {
        panic!("entry is an object")
    };
    assert_eq!(entry.get_key(&js("generic")), Some(&key("ロボット 🤖")));
}

#[test]
fn metadata_refresh_after_new_fields() {
    let mut cx = mock_cx();
    let mut c = MetadataController::new(&mut cx).unwrap();
    {
        let mut log = cx.log.borrow_mut();
        log.put_string(&js("/Metadata/Late"), 0.0, js("value"));
    }
    c.refresh(&mut cx).unwrap();
    let cmd = c
        .command(&mut cx, &uuid())
        .unwrap()
        .as_any()
        .downcast_ref::<JsValue>()
        .unwrap()
        .clone();
    let JsValue::Object(map) = cmd else {
        panic!("command is an object")
    };
    assert!(map.get_key(&js("/Late")).is_some());
}
