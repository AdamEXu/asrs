#![allow(clippy::disallowed_methods)]

//! Replays `prompts/flows` (oracle/src/genPrompts.ts: upstream's prompt pages in Blink, driven
//! with real input) through the prompt windows: each flow opens the page's [`PromptWindow`] in a
//! headless app, gets main's messages, and then the recorded user input as GPUI input events.
//! After the open and after every step the controls, the focused element, the buttons' class,
//! and the replies, alerts and errors since the previous record must equal the recording. Every
//! flow runs in an active and in an inactive window, whose blur events GPUI does not deliver.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use asrs_app::views::prompts::{self, PromptWindow};
use asrs_app::windows::WindowKind;
use asrs_js::string::JsString;
use asrs_model::app_identity::AppIdentity;
use asrs_parity::cve1::Value as Cve;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::prompts::{ControlValue, PromptEvent};
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, Bounds, ClipboardItem, Entity, HeadlessAppContext, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NoopTextSystem,
    PlatformInput, WindowBounds, WindowOptions, point, px, size,
};
use asrs_ui::platform::ScriptedNumpadEnter;

const WIDTH: f32 = 300.0;

fn kind(page: &str) -> (WindowKind, f32) {
    match page {
        "editRange" => (WindowKind::EditRange, 108.0),
        "editFov" => (WindowKind::EditFov, 81.0),
        "renameTab" => (WindowKind::RenameTab, 81.0),
        "unitConversion" => (WindowKind::UnitConversion, 162.0),
        "export" => (WindowKind::Export, 189.0),
        other => panic!("unknown page {other:?}"),
    }
}

fn utf16(s: &str) -> Utf16String {
    let mut out = Utf16String::new();
    out.push_str(s);
    out
}

fn text(s: &str) -> Cve {
    Cve::String(utf16(s))
}

fn js_text(s: &JsString) -> Cve {
    Cve::String(Utf16String::from_units(s.units().to_vec()))
}

fn obj(pairs: Vec<(&str, Cve)>) -> Cve {
    Cve::Object(pairs.into_iter().map(|(k, v)| (utf16(k), v)).collect())
}

fn get<'a>(value: &'a Cve, key: &str) -> Option<&'a Cve> {
    match value {
        Cve::Object(pairs) => pairs.iter().find(|(k, _)| k.eq_str(key)).map(|(_, v)| v),
        _ => None,
    }
}

fn string(value: Option<&Cve>, what: &str) -> Result<String, String> {
    match value {
        Some(Cve::String(s)) => Ok(s.to_string_lossy()),
        other => Err(format!("{what} is {other:?}, not a string")),
    }
}

fn field_names(record: &Record) -> Vec<String> {
    match &record.json {
        Json::Object(pairs) => pairs.iter().map(|(k, _)| k.to_string_lossy()).collect(),
        _ => Vec::new(),
    }
}

fn cve(record: &Record, key: &str) -> Result<Cve, String> {
    record
        .value(key)
        .map_err(|e| format!("line {}: {key}: {e}", record.line))
}

fn control(value: &ControlValue, disabled: bool) -> Cve {
    match value {
        ControlValue::Text(v) | ControlValue::Number(v) => obj(vec![
            ("value", js_text(v)),
            ("disabled", Cve::Bool(disabled)),
        ]),
        ControlValue::Select(select) => obj(vec![
            (
                "options",
                Cve::Array(
                    select
                        .options()
                        .map(|o| {
                            obj(vec![
                                ("text", js_text(&o.text())),
                                ("value", js_text(&o.value())),
                                ("disabled", Cve::Bool(o.is_disabled())),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("value", js_text(&select.value())),
            (
                "selectedIndex",
                Cve::Number(f64::from(select.selected_index()).to_bits()),
            ),
            ("disabled", Cve::Bool(disabled)),
        ]),
    }
}

struct Prompt<'a> {
    cx: &'a mut HeadlessAppContext,
    window: AnyWindowHandle,
    view: Entity<PromptWindow>,
    events: Rc<RefCell<Vec<PromptEvent>>>,
    height: f32,
}

impl Prompt<'_> {
    fn open<'a>(cx: &'a mut HeadlessAppContext, page: &str, active: bool) -> Prompt<'a> {
        let (kind, height) = kind(page);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(WIDTH), px(height)),
            ))),
            focus: false,
            show: false,
            ..Default::default()
        };
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        let (window, view) = cx
            .update(|cx| {
                asrs_ui::open_window(options, cx, |window, cx| {
                    let view = cx.new(|cx| PromptWindow::new(prompts::page(kind), window, cx));
                    cx.subscribe(&view, move |_, event: &PromptEvent, _| {
                        seen.borrow_mut().push(event.clone());
                    })
                    .detach();
                    if active {
                        window.activate_window();
                    }
                    view
                })
            })
            .expect("open a prompt window");
        cx.run_until_parked();
        Prompt {
            cx,
            window,
            view,
            events,
            height,
        }
    }

    fn update<R>(
        &mut self,
        f: impl FnOnce(
            &mut PromptWindow,
            &mut asrs_ui::gpui::Window,
            &mut asrs_ui::gpui::Context<'_, PromptWindow>,
        ) -> R,
    ) -> R {
        let view = self.view.clone();
        let result = self
            .cx
            .update_window(self.window, |_, window, cx| {
                view.update(cx, |view, cx| f(view, window, cx))
            })
            .expect("the window is open");
        self.settle();
        result
    }

    fn settle(&mut self) {
        self.cx
            .update_window(self.window, |_, window, _| window.refresh())
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    fn keystroke(&mut self, key: &str, key_char: Option<&str>, modifiers: Modifiers) {
        let keystroke = Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        };
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_keystroke(keystroke.clone(), cx);
                window.dispatch_event(
                    PlatformInput::KeyUp(asrs_ui::gpui::KeyUpEvent { keystroke }),
                    cx,
                );
            })
            .expect("the window is open");
        self.settle();
    }

    fn click(&mut self, x: f32, y: f32) {
        let position = point(px(x), px(y));
        let button = MouseButton::Left;
        let modifiers = Modifiers::default();
        let inputs = [
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }),
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers,
                click_count: 1,
            }),
        ];
        self.cx
            .update_window(self.window, |_, window, cx| {
                for input in inputs {
                    window.dispatch_event(input, cx);
                }
            })
            .expect("the window is open");
        self.settle();
    }

    fn apply(&mut self, step: &Cve) -> Result<(), String> {
        let op = string(get(step, "op"), "op")?;
        let id = get(step, "id");
        let value = get(step, "value");
        let fields = [id.is_some(), value.is_some()];
        let expected = match op.as_str() {
            "select" | "click" => [true, false],
            "text" | "insert" | "key" | "post" => [false, true],
            "pick" => [true, true],
            other => return Err(format!("unknown step op {other:?}")),
        };
        if fields != expected
            || !matches!(step, Cve::Object(p) if p.len() == 1 + fields.iter().filter(|f| **f).count())
        {
            return Err(format!("unexpected step fields: {}", step.encode()));
        }
        match op.as_str() {
            "select" => {
                let id = string(id, "id")?;
                self.update(|view, window, cx| view.select(&id, window, cx));
            }
            "text" => {
                for c in string(value, "value")?.chars() {
                    let typed = c.to_string();
                    let key = if c == ' ' { "space" } else { typed.as_str() };
                    self.keystroke(&key.to_lowercase(), Some(&typed), Modifiers::default());
                }
            }
            "insert" => {
                let pasted = string(value, "value")?;
                self.cx
                    .update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(pasted)));
                let command = self.cx.update(|cx| Modifiers::secondary_key(cx));
                self.keystroke("v", None, command);
            }
            "key" => {
                let name = string(value, "value")?;
                let (shift, name) = match name.strip_prefix("Shift+") {
                    Some(rest) => (true, rest),
                    None => (false, name.as_str()),
                };
                let numpad = name == "NumpadEnter";
                self.cx
                    .update(|cx| cx.set_global(ScriptedNumpadEnter(numpad)));
                let (key, key_char) = match name {
                    "Enter" | "NumpadEnter" => ("enter", Some("\n")),
                    "Tab" => ("tab", Some("\t")),
                    "Escape" => ("escape", None),
                    "Backspace" => ("backspace", None),
                    "ArrowLeft" => ("left", None),
                    "ArrowRight" => ("right", None),
                    " " => ("space", Some(" ")),
                    other => return Err(format!("no GPUI key for {other:?}")),
                };
                let modifiers = Modifiers {
                    shift,
                    ..Modifiers::default()
                };
                self.keystroke(key, key_char, modifiers);
            }
            "click" => {
                let id = string(id, "id")?;
                let (right, bottom) = match id.as_str() {
                    "exit" => (43.0 + 15.0, 8.0 + 15.0),
                    "confirm" => (8.0 + 15.0, 8.0 + 15.0),
                    "help" => (78.0 + 25.0, 8.0 + 15.0),
                    other => return Err(format!("no position for {other:?}")),
                };
                self.click(WIDTH - right, self.height - bottom);
            }
            "pick" => {
                let id = string(id, "id")?;
                let value = string(value, "value")?;
                let select = self
                    .update(|view, _, _| view.select_widget(&id).cloned())
                    .ok_or_else(|| format!("no select {id:?}"))?;
                let item = self
                    .cx
                    .update(|cx| {
                        select
                            .read(cx)
                            .model()
                            .select()
                            .options()
                            .position(|o| o.value().to_string_lossy() == value)
                    })
                    .ok_or_else(|| format!("no option {value:?} in {id:?}"))?;
                self.update(|view, window, cx| {
                    view.select(&id, window, cx);
                    select.update(cx, |select, cx| select.popup_closed(Some(item), cx));
                });
            }
            "post" => {
                let data = js_json::to_js(value.expect("checked above"))?;
                self.update(|view, window, cx| view.message(&data, window, cx));
            }
            _ => unreachable!("checked above"),
        }
        Ok(())
    }

    fn state(&mut self) -> Cve {
        let (controls, active, blurred) = self.update(|view, window, cx| {
            let controls: Vec<(Utf16String, Cve)> = view
                .page()
                .controls()
                .into_iter()
                .map(|c| {
                    let shown = view.shown(c.id, cx).expect("every control has a widget");
                    (utf16(c.id), control(&shown, view.disabled(c.id, cx)))
                })
                .collect();
            (controls, view.active(window, cx), view.page().blurred())
        });
        let mut replies = Vec::new();
        let mut alerts = Vec::new();
        let mut errors = Vec::new();
        for event in self.events.borrow_mut().drain(..) {
            match event {
                PromptEvent::Reply(value) => replies.push(js_json::from_js(&value)),
                PromptEvent::Alert(message) => alerts.push(js_text(&message)),
                PromptEvent::Error(e) => errors.push(obj(vec![
                    ("name", text(e.name)),
                    ("message", text(&e.message)),
                ])),
                PromptEvent::Select(id) => panic!("the view handles input.select() of {id}"),
            }
        }
        obj(vec![
            ("controls", Cve::Object(controls)),
            ("active", active.map_or(Cve::Null, text)),
            (
                "blurred",
                Cve::Array(vec![Cve::Bool(blurred), Cve::Bool(blurred)]),
            ),
            ("replies", Cve::Array(replies)),
            ("alerts", Cve::Array(alerts)),
            ("errors", Cve::Array(errors)),
        ])
    }
}

fn compare(what: &str, expected: &Cve, actual: &Cve) -> Result<(), String> {
    if vectors::values_match(expected, actual) {
        Ok(())
    } else {
        Err(format!(
            "{what}\n  expected {}\n       got {}",
            expected.encode(),
            actual.encode()
        ))
    }
}

fn replay(
    cx: &mut HeadlessAppContext,
    records: &[Record],
    active: bool,
    compared: &mut BTreeMap<String, usize>,
) -> Result<(), String> {
    let (open, rest) = records.split_first().expect("a flow has records");
    let (end, steps) = rest.split_last().ok_or("flow has no end record")?;
    let flow = open.string("flow").ok_or("open record without a flow")?;
    if field_names(open) != ["kind", "flow", "page", "posts", "state"] {
        return Err(format!("line {}: expected an open record", open.line));
    }
    let page = open.string("page").ok_or("open record without a page")?;
    let Cve::Array(posts) = cve(open, "posts")? else {
        return Err("posts is not an array".to_owned());
    };
    let mut prompt = Prompt::open(cx, &page, active);
    for post in &posts {
        let data = js_json::to_js(post)?;
        prompt.update(|view, window, cx| view.message(&data, window, cx));
    }
    let mut count = |record: &Record| {
        *compared
            .entry(record.string("kind").unwrap_or_default())
            .or_default() += 1;
    };
    compare("open", &cve(open, "state")?, &prompt.state())?;
    count(open);
    let mut replied = false;
    for record in steps {
        if record.string("kind").as_deref() != Some("step")
            || field_names(record) != ["kind", "flow", "step", "state"]
            || record.string("flow").as_deref() != Some(flow.as_str())
        {
            return Err(format!("line {}: unexpected record", record.line));
        }
        if replied {
            return Err(format!("line {}: the window closed before", record.line));
        }
        let step = cve(record, "step")?;
        prompt
            .apply(&step)
            .map_err(|e| format!("line {}: {e}", record.line))?;
        let state = prompt.state();
        if let Some(Cve::Array(replies)) = get(&state, "replies") {
            replied = replies
                .iter()
                .any(|r| page != "export" || !matches!(r, Cve::String(_)));
        }
        compare(
            &format!("line {}: step {}", record.line, step.encode()),
            &cve(record, "state")?,
            &state,
        )?;
        count(record);
    }
    if field_names(end) != ["kind", "flow", "replied"]
        || end.string("flow").as_deref() != Some(flow.as_str())
        || end.field("replied") != Some(&Json::Bool(replied))
    {
        return Err(format!(
            "line {}: bad end record (replied is {replied})",
            end.line
        ));
    }
    count(end);
    let window = prompt.window;
    prompt
        .cx
        .update_window(window, |_, window, _| window.remove_window())
        .map_err(|e| e.to_string())?;
    prompt.cx.run_until_parked();
    Ok(())
}

fn main() {
    assert_eq!(asrs_app::IDENTITY, AppIdentity::UPSTREAM);
    let id = "prompts/flows";
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut recorded: BTreeMap<String, usize> = BTreeMap::new();
    for record in &records {
        *recorded
            .entry(record.string("kind").unwrap_or_default())
            .or_default() += 1;
    }
    let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
    cx.update(asrs_ui::init);
    cx.run_until_parked();
    for active in [true, false] {
        let mut compared = BTreeMap::new();
        let mut failures = Vec::new();
        let mut flows = 0;
        let mut start = 0;
        while start < records.len() {
            let end = records[start..]
                .iter()
                .position(|r| r.string("kind").as_deref() == Some("end"))
                .map_or(records.len(), |i| start + i + 1);
            flows += 1;
            if let Err(e) = replay(&mut cx, &records[start..end], active, &mut compared) {
                let flow = records[start].string("flow").unwrap_or_default();
                failures.push(format!("flow {flow:?}: {e}"));
            }
            start = end;
        }
        assert!(
            failures.is_empty(),
            "{id}: active {active}: {} of {flows} flows differ\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
        assert!(flows > 0, "{id}: no flows");
        assert_eq!(
            recorded, compared,
            "{id}: compared counts differ from recorded counts"
        );
        println!("{id}: window active {active}: {flows} flows, {compared:?} records equal");
    }
}
