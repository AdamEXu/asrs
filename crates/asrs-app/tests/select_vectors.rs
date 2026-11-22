#![allow(clippy::disallowed_methods)]

//! Replays Blink's `form/select-keys` vector through the real [`Select`] component over
//! [`FormSelect`] in a headless window: each key goes through the component's key routing and
//! its model, and after every op the select's `value`, `selectedIndex`, focus and the `input`
//! and `change` events it dispatched are compared with the record. Then every `form/select`
//! record is replayed through `asrs_tabs::form::Select` and the component's view of it (the
//! closed label and the popup's rows) is checked against the recorded options.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use asrs_app::views::select::FormSelect;
use asrs_js::string::JsString;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::form::{OptGroup, Select as FormModel, SelectItem as FormItem, SelectOption};
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    AppContext, Entity, Focusable, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    PlatformInput, WindowHandle, point, px, size,
};
use asrs_ui::input::{Select, SelectEvent, SelectModel, popup_menu};
use asrs_ui::platform::SelectMenuItemKind;

struct Tag {
    name: String,
    closing: bool,
    attributes: Vec<(String, String)>,
    text: String,
}

impl Tag {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The tags of the flat select markup the fixtures use: double-quoted attributes or bare
/// `disabled`, no entities.
fn tags(markup: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = markup;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').expect("unterminated tag");
        let inside = &rest[..end];
        rest = &rest[end + 1..];
        let text = rest[..rest.find('<').unwrap_or(rest.len())].to_owned();
        if let Some(name) = inside.strip_prefix('/') {
            out.push(Tag {
                name: name.to_owned(),
                closing: true,
                attributes: Vec::new(),
                text,
            });
            continue;
        }
        let (name, mut attrs) = inside.split_once(' ').unwrap_or((inside, ""));
        let mut attributes = Vec::new();
        loop {
            attrs = attrs.trim_start();
            if attrs.is_empty() {
                break;
            }
            if let Some(after) = attrs.strip_prefix("disabled") {
                attributes.push(("disabled".to_owned(), String::new()));
                attrs = after;
                continue;
            }
            let (attr, after) = attrs.split_once("=\"").expect("quoted attribute");
            let (value, after) = after.split_once('"').expect("terminated attribute");
            attributes.push((attr.to_owned(), value.to_owned()));
            attrs = after;
        }
        out.push(Tag {
            name: name.to_owned(),
            closing: false,
            attributes,
            text,
        });
    }
    out
}

/// The select of `markup` and whether it is disabled.
fn parse(markup: &str) -> (FormModel, bool) {
    let tags = tags(markup);
    let root = &tags[0];
    assert_eq!(root.name, "select", "{markup}");
    let mut items = Vec::new();
    let mut grouped = false;
    for t in &tags[1..] {
        match (t.name.as_str(), t.closing) {
            ("option", false) => items.push(FormItem::Option {
                option: SelectOption::new(t.attribute("value"), &t.text)
                    .disabled(t.attribute("disabled").is_some()),
                grouped,
            }),
            ("optgroup", false) => {
                items.push(FormItem::Group(
                    OptGroup::new(t.attribute("label").unwrap_or(""))
                        .disabled(t.attribute("disabled").is_some()),
                ));
                grouped = true;
            }
            ("optgroup", true) => grouped = false,
            ("option" | "select", true) => {}
            (other, _) => panic!("unexpected <{other}> in {markup}"),
        }
    }
    (
        FormModel::with_items(items),
        root.attribute("disabled").is_some(),
    )
}

/// The GPUI key name of each non-character key the fixtures press, and the character its
/// keypress carries.
fn named_key(name: &str) -> (&'static str, Option<&'static str>) {
    match name {
        "Enter" => ("enter", Some("\r")),
        "Escape" => ("escape", None),
        "Home" => ("home", None),
        "End" => ("end", None),
        "PageUp" => ("pageup", None),
        "PageDown" => ("pagedown", None),
        "ArrowLeft" => ("left", None),
        "ArrowRight" => ("right", None),
        other => panic!("no key mapping for {other}"),
    }
}

fn modifiers(op: &[Json]) -> Modifiers {
    let mut m = Modifiers::default();
    for name in op.get(3).and_then(Json::as_array).unwrap_or_default() {
        match name.as_str().map(|s| s.to_string_lossy()).as_deref() {
            Some("ctrl") => m.control = true,
            Some("alt") => m.alt = true,
            Some("meta") => m.platform = true,
            Some("shift") => m.shift = true,
            other => panic!("unknown modifier {other:?}"),
        }
    }
    m
}

fn text_arg(op: &[Json], i: usize) -> String {
    op.get(i)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| panic!("op argument {i} is not a string"))
}

fn time_arg(op: &[Json]) -> f64 {
    Value::from_json(op.get(2).expect("op time"))
        .expect("cve1 time")
        .as_f64()
        .expect("numeric time")
}

struct Step {
    window: WindowHandle<Select<FormSelect>>,
    entity: Entity<Select<FormSelect>>,
    events: Rc<RefCell<Vec<&'static str>>>,
}

fn open(headless: &mut HeadlessCapture, markup: &str, focus: bool) -> Step {
    let (model, disabled) = parse(markup);
    let window = headless
        .open_window(size(px(300.0), px(60.0)), 1.0, |window, cx| {
            cx.new(|cx| {
                let mut select = Select::new(FormSelect::new(model), window, cx);
                select.set_size(200.0, 19.0);
                select.set_disabled(disabled, cx);
                select
            })
        })
        .expect("open window");
    let events = Rc::new(RefCell::new(Vec::new()));
    let entity = headless
        .context()
        .update_window(window.into(), |root, window, cx| {
            let entity = root.downcast::<Select<FormSelect>>().expect("select root");
            let seen = events.clone();
            cx.subscribe(&entity, move |_, event: &SelectEvent, _| {
                seen.borrow_mut().push(match event {
                    SelectEvent::Input => "input",
                    SelectEvent::Change => "change",
                    SelectEvent::Focus => "focus",
                    SelectEvent::Blur => "blur",
                });
            })
            .detach();
            if focus && !disabled {
                let handle = entity.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            entity
        })
        .expect("window");
    headless.capture(window.into()).expect("first frame");
    Step {
        window,
        entity,
        events,
    }
}

fn apply(headless: &mut HeadlessCapture, step: &Step, op: &[Json]) {
    let kind = text_arg(op, 0);
    let entity = step.entity.clone();
    headless
        .context()
        .update_window(step.window.into(), |_, window, cx| match kind.as_str() {
            "char" | "key" => {
                let name = text_arg(op, 1);
                let (key, key_char) = if kind == "char" {
                    let key = if name == " " {
                        "space".to_owned()
                    } else {
                        name.to_lowercase()
                    };
                    (key, Some(name))
                } else {
                    let (key, ch) = named_key(&name);
                    (key.to_owned(), ch.map(str::to_owned))
                };
                let keystroke = Keystroke {
                    modifiers: modifiers(op),
                    key,
                    key_char,
                };
                let time = time_arg(op);
                if entity.read(cx).focus_handle(cx).is_focused(window) {
                    entity.update(cx, |select, cx| select.key(&keystroke, time, window, cx));
                }
            }
            "set" => {
                assert_eq!(
                    text_arg(op, 1),
                    "value",
                    "only value assignments are modelled"
                );
                let value = JsString::from(text_arg(op, 2).as_str());
                entity.update(cx, |select, cx| {
                    select.update_model(|m| m.select_mut().set_value(&value), cx)
                });
            }
            "blur" => {
                if entity.read(cx).focus_handle(cx).is_focused(window) {
                    window.blur(cx);
                }
            }
            "focus" => {
                if !entity.read(cx).is_disabled() {
                    let handle = entity.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            "press" => {
                assert!(
                    entity.read(cx).is_disabled(),
                    "a press on an enabled select opens the native popup"
                );
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: point(px(100.0), px(9.0)),
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
            }
            other => panic!("unknown op {other}"),
        })
        .expect("window");
    headless.capture(step.window.into()).expect("frame");
}

fn number(record: &Record, key: &str) -> f64 {
    record
        .field(key)
        .and_then(Json::as_f64)
        .unwrap_or_else(|| panic!("record has no {key}"))
}

fn compare(headless: &mut HeadlessCapture, step: &Step, record: &Record) -> Vec<String> {
    let (value, index, focused, popup) = headless
        .context()
        .update_window(step.window.into(), |_, window, cx| {
            let select = step.entity.read(cx);
            let model = select.model().select();
            (
                model.value().to_string_lossy(),
                model.selected_index(),
                select.focus_handle(cx).is_focused(window),
                select.popup_is_open(),
            )
        })
        .expect("window");
    let fired: Vec<&str> = step
        .events
        .borrow_mut()
        .drain(..)
        .filter(|e| matches!(*e, "input" | "change"))
        .collect();
    let expected_events: Vec<String> = record
        .field("events")
        .and_then(Json::as_array)
        .expect("events")
        .iter()
        .filter_map(|e| e.as_str().map(|s| s.to_string_lossy()))
        .filter(|e| matches!(e.as_str(), "input" | "change"))
        .collect();
    let mut failures = Vec::new();
    let expected_value = record.string("value").expect("value");
    if value != expected_value {
        failures.push(format!("value {value:?}, Blink {expected_value:?}"));
    }
    if f64::from(index) != number(record, "selectedIndex") {
        failures.push(format!(
            "selectedIndex {index}, Blink {}",
            number(record, "selectedIndex")
        ));
    }
    let expected_focus = matches!(record.field("focused"), Some(Json::Bool(true)));
    if focused != expected_focus {
        failures.push(format!("focused {focused}, Blink {expected_focus}"));
    }
    if fired != expected_events {
        failures.push(format!("events {fired:?}, Blink {expected_events:?}"));
    }
    if popup {
        failures.push("the popup opened".to_owned());
    }
    failures
}

fn select_keys(headless: &mut HeadlessCapture, specs: &Json) -> usize {
    let steps = specs
        .get("fixture")
        .and_then(|f| f.get("steps"))
        .and_then(Json::as_array)
        .expect("spec steps");
    let records = vectors::load("form/select-keys").expect("form/select-keys");
    let mut current: Option<Step> = None;
    let mut compared = 0;
    let mut failures = Vec::new();
    for record in &records {
        let index = number(record, "step") as usize;
        match record.field("op") {
            Some(Json::Null) => {
                let spec = &steps[index];
                let markup = spec
                    .get("markup")
                    .and_then(Json::as_str)
                    .expect("markup")
                    .to_string_lossy();
                let focus = !matches!(spec.get("focus"), Some(Json::Bool(false)));
                current = Some(open(headless, &markup, focus));
            }
            Some(op) => apply(
                headless,
                current.as_ref().expect("an op before its step"),
                op.as_array().expect("op array"),
            ),
            None => panic!("record without op"),
        }
        let step = current.as_ref().expect("step");
        for f in compare(headless, step, record) {
            failures.push(format!(
                "step {index} after {:?}: {f}",
                record.field("op").map(|j| j.to_compact())
            ));
        }
        compared += 1;
    }
    assert!(
        failures.is_empty(),
        "form/select-keys:\n{}",
        failures.join("\n")
    );
    assert_eq!(compared, records.len());
    compared
}

/// Each `form/select` record's options and selection, as the component shows them: the
/// closed label is the selected option's `text`, and the popup lists every option and
/// optgroup with the selected option's row marked.
fn select_view() -> usize {
    let records = vectors::load("form/select").expect("form/select");
    let mut checked = 0;
    for record in &records {
        let values = record
            .field("values")
            .and_then(Json::as_array)
            .expect("values");
        let read = |path: &str| -> Option<Value> {
            values
                .iter()
                .find(|v| {
                    v.get("path")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .as_deref()
                        == Some(path)
                })
                .and_then(|v| v.get("value"))
                .map(|v| Value::from_json(v).expect("cve1"))
        };
        let index = read("selectedIndex")
            .and_then(|v| v.as_f64())
            .expect("selectedIndex");
        let length = read("options.length")
            .and_then(|v| v.as_f64())
            .expect("length") as usize;
        let expected_label = if index >= 0.0 {
            match read(&format!("options.{index}.text")) {
                Some(Value::String(s)) => s.to_string_lossy(),
                _ => continue,
            }
        } else {
            String::new()
        };
        let markup = record.string("form").expect("form");
        if !markup.starts_with("<select") {
            continue;
        }
        let model = replayed(&records, record);
        let select = FormSelect::new(model);
        assert_eq!(select.shown_text(), expected_label, "{markup} label");
        let (menu, map) = popup_menu(&select.items(), select.selected_item());
        let options = menu
            .items
            .iter()
            .filter(|i| i.kind == SelectMenuItemKind::Option)
            .count();
        assert_eq!(options, length, "{markup} popup options");
        let selected_row = menu.selected.map(|r| map[r]);
        assert_eq!(
            selected_row,
            select.selected_item(),
            "{markup} popup selection"
        );
        let selected_option = selected_row.map(|item| {
            select.select().items()[..item]
                .iter()
                .filter(|i| matches!(i, FormItem::Option { .. }))
                .count() as f64
        });
        assert_eq!(selected_option.unwrap_or(-1.0), index, "{markup} popup row");
        checked += 1;
    }
    checked
}

/// The `form::Select` after every op up to `target`, from the record that parsed its markup.
fn replayed(records: &[Record], target: &Record) -> FormModel {
    let at = records
        .iter()
        .position(|r| std::ptr::eq(r, target))
        .expect("record");
    let start = records[..=at]
        .iter()
        .rposition(|r| matches!(r.field("op"), Some(Json::Null)))
        .expect("markup record");
    let (mut model, _) = parse(&records[start].string("form").expect("form"));
    for r in &records[start + 1..=at] {
        let op = r.field("op").and_then(Json::as_array).expect("op");
        let kind = text_arg(op, 0);
        let target = op
            .get(1)
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy());
        let num = |i: usize| {
            Value::from_json(&op[i])
                .expect("cve1")
                .as_f64()
                .expect("number")
        };
        let flag = |i: usize| matches!(op.get(i), Some(Json::Bool(true)));
        match (kind.as_str(), target.as_deref()) {
            ("set", Some("value")) => model.set_value(&JsString::from(text_arg(op, 2).as_str())),
            ("set", Some("selectedIndex")) => model.set_selected_index(num(2) as i32),
            ("remove", _) => model.remove(num(1) as usize),
            ("appendElement", Some("option")) => {
                let props = op[2].as_array().expect("props");
                let pair = props[0].as_array().expect("pair");
                model.append(SelectOption::from_inner_text(&JsString::from(
                    text_arg(pair, 1).as_str(),
                )));
            }
            ("set", Some(path)) if path.ends_with(".innerHTML") => {
                let group: usize = path["children.".len()..path.len() - ".innerHTML".len()]
                    .parse()
                    .expect("group index");
                model.clear_group(group);
                for t in tags(&text_arg(op, 2)) {
                    if t.name == "option" && !t.closing {
                        model.append_to_group(
                            group,
                            SelectOption::new(t.attribute("value"), &t.text)
                                .disabled(t.attribute("disabled").is_some()),
                        );
                    }
                }
            }
            ("set", Some(path)) if path.starts_with("children.") && path.ends_with(".disabled") => {
                let group: usize = path["children.".len()..path.len() - ".disabled".len()]
                    .parse()
                    .expect("group index");
                model.set_group_disabled(group, flag(2));
            }
            ("set", Some(path)) if path.starts_with("options.") && path.ends_with(".disabled") => {
                let index: usize = path["options.".len()..path.len() - ".disabled".len()]
                    .parse()
                    .expect("option index");
                model.set_option_disabled(index, flag(2));
            }
            other => panic!("unmodelled op {other:?}"),
        }
    }
    model
}

fn main() {
    let view = select_view();
    println!("select_vectors: form/select, {view} records shown and listed");
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("select_vectors: form/select-keys skipped, no headless renderer on this platform");
        return;
    }
    let spec_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../parity/specs/form/select-keys.entry.json"
    );
    let spec = asrs_parity::json::parse(&std::fs::read_to_string(spec_path).expect("spec"))
        .expect("spec json");
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    let keys = select_keys(&mut headless, &spec);
    println!("select_vectors: form/select-keys, {keys} records compared");
}
