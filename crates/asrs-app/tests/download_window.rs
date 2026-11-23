#![allow(clippy::disallowed_methods)]

//! Replays `download/page` (oracle/src/genDownloadPage.ts: upstream's download page in Blink,
//! driven with real input) through the download window in a headless app: each flow gets main's
//! messages and the recorded user input as GPUI input events. After the open and after every step
//! the rows, the alert, the progress bar and its text, the scroll position, the focused element, the
//! buttons' class, and the messages, alerts and errors since the previous record must equal the
//! recording. Every flow runs in an active and in an inactive window. The loading animation's
//! phases and the place of its bars are compared too.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use asrs_app::views::download::{DownloadWindow, loading};
use asrs_js::value::JsValue;
use asrs_model::preferences::Preferences;
use asrs_parity::cve1::Value as Cve;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_services::download::downloader::{ListItem, Progress, WindowMessage};
use asrs_services::download::page::{PageEvent, Row, alert_runs};
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, Bounds, Entity, HeadlessAppContext, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NoopTextSystem, PlatformInput,
    WindowBounds, WindowOptions, point, px, size,
};
use asrs_ui::platform::ScriptedNumpadEnter;

const WIDTH: f32 = 500.0;
const HEIGHT: f32 = 472.0;

fn utf16(s: &str) -> Utf16String {
    let mut out = Utf16String::new();
    out.push_str(s);
    out
}

fn text(s: &str) -> Cve {
    Cve::String(utf16(s))
}

fn number(v: f64) -> Cve {
    Cve::Number(v.to_bits())
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

fn float(value: Option<&Cve>, what: &str) -> Result<f64, String> {
    match value {
        Some(Cve::Number(bits)) => Ok(f64::from_bits(*bits)),
        other => Err(format!("{what} is {other:?}, not a number")),
    }
}

fn floats(value: &Cve) -> Result<Vec<f64>, String> {
    match value {
        Cve::Array(items) => items.iter().map(|v| float(Some(v), "an item")).collect(),
        other => Err(format!("{other:?} is not an array")),
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

fn message(name: &str, data: &JsValue) -> Result<WindowMessage, String> {
    let text = || {
        let text = data.as_str().ok_or("the message is not a string")?;
        Ok::<_, String>(text.to_string_lossy())
    };
    Ok(match name {
        "show-error" => WindowMessage::ShowError(text()?),
        "show-alert" => WindowMessage::ShowAlert(text()?),
        "set-progress" => WindowMessage::SetProgress(match data.as_f64() {
            Some(0.0) => Progress::Preparing,
            Some(1.0) => Progress::Finished,
            Some(other) => return Err(format!("progress {other}")),
            None => {
                let field = |key: &str| data.get(key).and_then(JsValue::as_f64);
                Progress::Transferring {
                    current: field("current").unwrap_or(f64::NAN),
                    total: field("total").unwrap_or(f64::NAN),
                }
            }
        }),
        "set-list" => WindowMessage::SetList(
            data.as_array()
                .ok_or("the list is not an array")?
                .iter()
                .map(|file| {
                    Ok(ListItem {
                        name: file
                            .get("name")
                            .and_then(JsValue::as_str)
                            .ok_or("a file without a name")?
                            .to_string_lossy(),
                        size: file.get("size").and_then(JsValue::as_f64),
                        is_folder: file
                            .get("isFolder")
                            .and_then(JsValue::as_bool)
                            .ok_or("a file without isFolder")?,
                    })
                })
                .collect::<Result<_, &str>>()?,
        ),
        other => return Err(format!("unknown message {other:?}")),
    })
}

fn row(row: &Row) -> Cve {
    let mut pairs = vec![("selected", Cve::Bool(row.selected))];
    if let Some(item) = &row.item {
        let src = item.icon.file.as_deref().map_or(Cve::Null, text);
        pairs.extend([
            ("name", text(&item.name)),
            ("size", text(&item.size)),
            ("src", src),
            ("imgClass", text(item.icon.class.unwrap_or_default())),
        ]);
    }
    obj(pairs)
}

struct Page<'a> {
    cx: &'a mut HeadlessAppContext,
    window: AnyWindowHandle,
    view: Entity<DownloadWindow>,
    events: Rc<RefCell<Vec<PageEvent>>>,
    height: f32,
    now: f64,
}

impl Page<'_> {
    fn open<'a>(cx: &'a mut HeadlessAppContext, platform: &str, active: bool) -> Page<'a> {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(WIDTH), px(HEIGHT)),
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
                    let view = cx.new(|cx| DownloadWindow::new(platform, window, cx));
                    cx.subscribe(&view, move |_, event: &PageEvent, _| {
                        seen.borrow_mut().push(event.clone());
                    })
                    .detach();
                    if active {
                        window.activate_window();
                    }
                    view
                })
            })
            .expect("open the download window");
        cx.run_until_parked();
        Page {
            cx,
            window,
            view,
            events,
            height: HEIGHT,
            now: 0.0,
        }
    }

    fn update<R>(
        &mut self,
        f: impl FnOnce(
            &mut DownloadWindow,
            &mut asrs_ui::gpui::Window,
            &mut asrs_ui::gpui::Context<'_, DownloadWindow>,
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

    fn resize(&mut self, height: f32) {
        self.height = height;
        self.cx
            .update_window(self.window, |_, window, _| {
                window.resize(size(px(WIDTH), px(height)));
            })
            .expect("the window is open");
        self.cx.simulate_scale_factor_change(self.window, 1.0);
        self.settle();
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

    fn click(&mut self, x: f32, y: f32, modifiers: Modifiers) {
        let position = point(px(x), px(y));
        let button = MouseButton::Left;
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

    fn key(&mut self, name: &str) -> Result<(), String> {
        let mut modifiers = Modifiers::default();
        let mut name = name;
        while let Some((modifier, rest)) = name.split_once('+') {
            match modifier {
                "Shift" => modifiers.shift = true,
                "Control" => modifiers.control = true,
                "Alt" => modifiers.alt = true,
                "Meta" => modifiers.platform = true,
                other => return Err(format!("unknown modifier {other:?}")),
            }
            name = rest;
        }
        self.cx
            .update(|cx| cx.set_global(ScriptedNumpadEnter(name == "NumpadEnter")));
        let (key, key_char) = match name {
            "Enter" | "NumpadEnter" => ("enter", Some("\n")),
            "Tab" => ("tab", Some("\t")),
            "Escape" => ("escape", None),
            " " => ("space", Some(" ")),
            "a" => ("a", Some("a")),
            "A" => ("a", Some("A")),
            "b" => ("b", Some("b")),
            other => return Err(format!("no GPUI key for {other:?}")),
        };
        self.keystroke(key, key_char, modifiers);
        Ok(())
    }

    fn post(&mut self, name: &str, data: &Cve) -> Result<(), String> {
        let data = js_json::to_js(data)?;
        match name {
            "set-preferences" => {
                let preferences = Preferences::from_js(&data);
                self.update(|view, _, cx| view.set_preferences(&preferences, cx));
            }
            "set-focused" => {
                let focused = data.as_bool().ok_or("set-focused is not a boolean")?;
                self.update(|view, _, cx| view.set_focused(focused, cx));
            }
            name => {
                let message = message(name, &data)?;
                let now = self.now;
                self.update(|view, window, cx| view.message_at(&message, now, window, cx));
            }
        }
        Ok(())
    }

    fn apply(&mut self, step: &Cve) -> Result<(), String> {
        let op = string(get(step, "op"), "op")?;
        let expected: &[&str] = match op.as_str() {
            "post" => &["op", "name", "data"],
            "row" => &["op", "index", "shift"],
            "click" => &["op", "name"],
            "key" | "resize" | "clock" | "scroll" => &["op", "value"],
            other => return Err(format!("unknown step op {other:?}")),
        };
        let names: Vec<String> = match step {
            Cve::Object(pairs) => pairs.iter().map(|(k, _)| k.to_string_lossy()).collect(),
            _ => Vec::new(),
        };
        if names != expected {
            return Err(format!("unexpected step fields: {}", step.encode()));
        }
        let value = get(step, "value");
        match op.as_str() {
            "post" => {
                let name = string(get(step, "name"), "name")?;
                self.post(&name, get(step, "data").expect("checked above"))?;
            }
            "row" => {
                let index = float(get(step, "index"), "index")? as f32;
                let shift = match get(step, "shift") {
                    Some(Cve::Bool(shift)) => *shift,
                    other => return Err(format!("shift is {other:?}")),
                };
                let scrolled = self.update(|view, _, _| view.scroll_top()) as f32;
                let modifiers = Modifiers {
                    shift,
                    ..Modifiers::default()
                };
                let y = 3.0 + 25.0 * index + 12.5 - scrolled;
                self.click(WIDTH / 2.0, y, modifiers);
            }
            "click" => {
                let right = match string(get(step, "name"), "name")?.as_str() {
                    "exit" => 43.0 + 15.0,
                    "download" => 8.0 + 15.0,
                    other => return Err(format!("no position for {other:?}")),
                };
                let (x, y) = (WIDTH - right, self.height - 8.0 - 15.0);
                self.click(x, y, Modifiers::default());
            }
            "key" => self.key(&string(value, "value")?)?,
            "resize" => self.resize(float(value, "value")? as f32),
            "clock" => self.now = float(value, "value")?,
            "scroll" => {
                let top = float(value, "value")?;
                self.update(|view, _, cx| view.set_scroll_top(top, cx));
            }
            _ => unreachable!("checked above"),
        }
        Ok(())
    }

    fn state(&mut self) -> Cve {
        let mut sent = Vec::new();
        let mut alerts = Vec::new();
        let mut errors = Vec::new();
        let posted = |name, data| obj(vec![("name", text(name)), ("data", data)]);
        for event in self.events.borrow_mut().drain(..) {
            match event {
                PageEvent::Start { address, path } => {
                    let data = obj(vec![
                        ("address", text(&address.to_string_lossy())),
                        ("path", text(&path.to_string_lossy())),
                    ]);
                    sent.push(posted("start", data));
                }
                PageEvent::Close => sent.push(posted("close", Cve::Undefined)),
                PageEvent::Save(files) => {
                    let files = files.iter().map(|file| match file {
                        Some(file) => text(file),
                        None => Cve::Undefined,
                    });
                    sent.push(posted("save", Cve::Array(files.collect())));
                }
                PageEvent::Alert(message) => alerts.push(text(message)),
                PageEvent::Error(e) => errors.push(obj(vec![
                    ("name", text(e.name)),
                    ("message", text(&e.message)),
                ])),
            }
        }
        self.update(|view, window, _| {
            let page = view.page();
            let alert = page.alert().map_or(Cve::Null, |html| {
                let runs = alert_runs(html);
                let shown: String = runs.iter().map(|run| run.text.as_str()).collect();
                let underlined = runs.iter().filter(|run| run.underline);
                let underlined = underlined.map(|run| text(&run.text)).collect();
                obj(vec![
                    ("text", text(&shown)),
                    ("underlined", Cve::Array(underlined)),
                ])
            });
            let progress = page.progress().map_or(Cve::Null, |(value, details)| {
                obj(vec![("value", number(value)), ("details", text(details))])
            });
            let blurred = Cve::Bool(page.blurred());
            obj(vec![
                ("rows", Cve::Array(page.rows().iter().map(row).collect())),
                ("loading", Cve::Bool(page.loading())),
                (
                    "overflow",
                    text(if page.scrolls() { "auto" } else { "hidden" }),
                ),
                ("scrollTop", number(view.scroll_top())),
                ("alert", alert),
                ("progress", progress),
                ("active", view.active(window).map_or(Cve::Null, text)),
                ("blurred", Cve::Array(vec![blurred.clone(), blurred])),
                ("sent", Cve::Array(sent)),
                ("alerts", Cve::Array(alerts)),
                ("errors", Cve::Array(errors)),
            ])
        })
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
    let (open, steps) = records.split_first().expect("a flow has records");
    if field_names(open) != ["kind", "flow", "platform", "steps", "state"]
        || open.string("kind").as_deref() != Some("open")
    {
        return Err(format!("line {}: expected an open record", open.line));
    }
    let declared = match open.field("steps") {
        Some(Json::Number(n)) => n.to_string(),
        other => return Err(format!("steps is {other:?}")),
    };
    if declared != steps.len().to_string() {
        return Err(format!("line {}: {} steps", open.line, steps.len()));
    }
    let flow = open.string("flow");
    let platform = open.string("platform").ok_or("a flow without a platform")?;
    let mut count = |record: &Record| {
        let kind = record.string("kind").unwrap_or_default();
        *compared.entry(kind).or_default() += 1;
    };
    let mut page = Page::open(cx, &platform, active);
    compare("open", &cve(open, "state")?, &page.state())?;
    count(open);
    for record in steps {
        if field_names(record) != ["kind", "flow", "step", "state"]
            || record.string("kind").as_deref() != Some("step")
            || record.string("flow") != flow
        {
            return Err(format!("line {}: unexpected record", record.line));
        }
        let step = cve(record, "step")?;
        page.apply(&step)
            .map_err(|e| format!("line {}: {e}", record.line))?;
        let what = format!("line {}: step {}", record.line, step.encode());
        compare(&what, &cve(record, "state")?, &page.state())?;
        count(record);
    }
    let window = page.window;
    page.cx
        .update_window(window, |_, window, _| window.remove_window())
        .map_err(|e| e.to_string())?;
    page.cx.run_until_parked();
    Ok(())
}

/// `getComputedStyle` prints six digits.
const SCALE_TOLERANCE: f64 = 1e-5;

fn loading_record(record: &Record) -> Result<(), String> {
    if field_names(record) != ["kind", "time", "scales"] {
        return Err(format!("line {}: unexpected record", record.line));
    }
    let time = match record.field("time") {
        Some(Json::Number(n)) => n.to_string().parse::<f64>().map_err(|e| e.to_string())?,
        other => return Err(format!("time is {other:?}")),
    };
    let expected = floats(&cve(record, "scales")?)?;
    let actual = loading::scales(time);
    let close = |(a, b): (&f64, &f64)| (a - b).abs() <= SCALE_TOLERANCE;
    if expected.len() == actual.len() && expected.iter().zip(&actual).all(close) {
        Ok(())
    } else {
        Err(format!("{time} ms: {expected:?} vs {actual:?}"))
    }
}

/// The bars where Blink on macOS puts them; the space between them is the body font's.
fn bars_record(record: &Record) -> Result<(), String> {
    if field_names(record) != ["kind", "container", "bars"] {
        return Err(format!("line {}: unexpected record", record.line));
    }
    let (width, height) = (f64::from(WIDTH), f64::from(HEIGHT) - 46.0);
    let expected = floats(&cve(record, "container")?)?;
    let actual = loading::container(width, height);
    if expected != actual {
        return Err(format!("container: {expected:?} vs {actual:?}"));
    }
    let Cve::Array(bars) = cve(record, "bars")? else {
        return Err("bars is not an array".to_owned());
    };
    let expected: Vec<Vec<f64>> = bars.iter().map(floats).collect::<Result<_, _>>()?;
    let actual = loading::bars(width, height);
    if expected == actual.map(|bar| bar.to_vec()) {
        Ok(())
    } else {
        Err(format!("{expected:?} vs {actual:?}"))
    }
}

fn main() {
    let records = vectors::load("download/page").expect("download/page");
    let mut failures = Vec::new();
    let mut phases = 0;
    let mut bars = 0;
    let mut flows: Vec<Vec<Record>> = Vec::new();
    let mut recorded = BTreeMap::<String, usize>::new();
    for record in records {
        let kind = record.string("kind").unwrap_or_default();
        let result = match kind.as_str() {
            "loading" => {
                phases += 1;
                loading_record(&record)
            }
            "bars" => {
                bars += 1;
                bars_record(&record)
            }
            "open" => {
                *recorded.entry(kind).or_default() += 1;
                flows.push(vec![record]);
                Ok(())
            }
            "step" => {
                *recorded.entry(kind).or_default() += 1;
                let flow = flows.last_mut().expect("a step follows its flow's open");
                flow.push(record);
                Ok(())
            }
            other => panic!("unknown record kind {other:?}"),
        };
        failures.extend(result.err());
    }
    for active in [false, true] {
        let mut compared = BTreeMap::new();
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        cx.update(asrs_ui::init);
        cx.run_until_parked();
        for flow in &flows {
            if let Err(e) = replay(&mut cx, flow, active, &mut compared) {
                let name = flow[0].string("flow").unwrap_or_default();
                failures.push(format!("{name} (active: {active}): {e}"));
            }
        }
        if failures.is_empty() {
            assert_eq!(compared, recorded, "every record is compared");
        }
        println!("{} flows, active: {active}: {compared:?}", flows.len());
    }
    for failure in failures.iter().take(8) {
        eprintln!("{failure}\n");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
    assert_eq!((phases, bars), (358, 1), "loading records");
}
