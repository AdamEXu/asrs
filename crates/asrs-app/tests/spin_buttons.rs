#![allow(clippy::disallowed_methods)]

//! Replays `form/spin-buttons`: mouse input to a number input's spin buttons in Blink, on a
//! [`TextField`] over the form model the views use, in a headless window. The value, the focus,
//! the selection and the events after every step must equal the recording, and a held button must
//! repeat after the recorded delays.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use asrs_app::views::number::FormNumber;
use asrs_js::string::JsString;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use asrs_tabs::form::NumberInput;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    App, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PlatformInput,
    Render, Styled, Window, WindowHandle, div, point, px, size,
};
use asrs_ui::input::{FieldEvent, FieldKind, FieldStyle, TextField};

const LEFT: f32 = 20.0;
const TOP: f32 = 20.0;
const WIDTH: f32 = 208.0;
const HEIGHT: f32 = 21.0;
const MS: Duration = Duration::from_millis(1);

type Events = Rc<RefCell<Vec<(&'static str, String)>>>;

struct View(Entity<TextField<FormNumber>>);

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("field")
                .absolute()
                .left(px(LEFT))
                .top(px(TOP))
                .child(self.0.clone()),
        )
    }
}

fn text(json: &Json) -> String {
    json.as_str().expect("a string").to_string_lossy()
}

fn attribute(markup: &str, name: &str) -> Option<String> {
    let rest = markup.split_once(&format!(" {name}=\""))?.1;
    Some(rest.split_once('"')?.0.to_owned())
}

fn place(name: &str) -> (f32, f32) {
    let (right, middle) = (LEFT + WIDTH - 4.0, TOP + 3.0 + 7.0);
    let (up, down) = (middle - 3.5, middle + 3.5);
    match name {
        "up" => (right - 7.0, up),
        "down" => (right - 7.0, down),
        "text" => (LEFT + 10.0, TOP + HEIGHT / 2.0),
        "beside-up" => (right - 40.0, up),
        "beside-down" => (right - 40.0, down),
        "above" => (right - 7.0, TOP - 10.0),
        "outside" => (5.0, 150.0),
        other => panic!("no place {other}"),
    }
}

struct Page<'a> {
    headless: &'a mut HeadlessCapture,
    window: WindowHandle<View>,
    field: Entity<TextField<FormNumber>>,
    events: Events,
    at: (f32, f32),
    pressed: bool,
}

impl<'a> Page<'a> {
    fn open(headless: &'a mut HeadlessCapture, markup: &str) -> Page<'a> {
        let mut input = NumberInput::default();
        if let Some(step) = attribute(markup, "step") {
            input.set_step(JsString::from(step));
        }
        if let Some(value) = attribute(markup, "value") {
            input.set_value(JsString::from(value));
        }
        let events = Events::default();
        let heard = events.clone();
        let window = headless
            .open_window(size(px(400.0), px(200.0)), 1.0, |window, cx| {
                let field = cx.new(|cx| {
                    let style = FieldStyle::native();
                    let model = FormNumber(input);
                    let mut field = TextField::new(model, FieldKind::Number, style, window, cx);
                    field.set_size(WIDTH, HEIGHT);
                    field
                });
                cx.subscribe(&field, move |field, event: &FieldEvent, cx| {
                    let name = match event {
                        FieldEvent::Input => "input",
                        FieldEvent::Change => "change",
                        FieldEvent::Enter | FieldEvent::Focus | FieldEvent::Blur => return,
                    };
                    let value = field.read(cx).value();
                    heard.borrow_mut().push((name, value));
                })
                .detach();
                cx.new(|_| View(field))
            })
            .expect("window");
        let field = headless
            .context()
            .update(|cx| window.read_with(cx, |view, _| view.0.clone()))
            .expect("view");
        let mut page = Page {
            headless,
            window,
            field,
            events,
            at: place("outside"),
            pressed: false,
        };
        page.draw();
        page
    }

    fn draw(&mut self) {
        self.headless.capture(self.window.into()).expect("draw");
    }

    fn update(&mut self, f: impl FnOnce(&Entity<TextField<FormNumber>>, &mut Window, &mut App)) {
        let field = self.field.clone();
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, cx| f(&field, window, cx))
            .expect("window");
        self.headless.context().run_until_parked();
        self.draw();
    }

    fn dispatch(&mut self, event: PlatformInput) {
        self.update(|_, window, cx| {
            window.dispatch_event(event, cx);
        });
    }

    fn apply(&mut self, op: &[Json]) {
        let position = |(x, y): (f32, f32)| point(px(x), px(y));
        match text(&op[0]).as_str() {
            "move" => {
                self.at = place(&text(&op[1]));
                let moved = PlatformInput::MouseMove(MouseMoveEvent {
                    position: position(self.at),
                    pressed_button: self.pressed.then_some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                });
                self.dispatch(moved);
            }
            "put" => self.at = place(&text(&op[1])),
            "press" => {
                self.pressed = true;
                self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: position(self.at),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }));
            }
            "release" => {
                self.pressed = false;
                self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: position(self.at),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }));
            }
            "type" => {
                assert_eq!(text(&op[1]), "7");
                self.key("7");
            }
            "key" if text(&op[1]) == "ArrowDown" => self.key("down"),
            "key" => self.key("up"),
            "blur" => self.update(|field, window, cx| {
                field.update(cx, |field, cx| field.blur(window, cx));
            }),
            other => panic!("unknown op {other}"),
        }
    }

    fn key(&mut self, key: &'static str) {
        let key = asrs_ui::gpui::Keystroke::parse(key).expect("a key");
        self.update(|_, window, cx| {
            window.dispatch_keystroke(key, cx);
        });
    }

    /// Advances to just before and then to each repeat, and returns what the repeats left.
    fn hold(&mut self, initial: Duration, period: Duration, repeats: usize) -> Vec<String> {
        let mut values = Vec::new();
        for i in 0..repeats {
            let delay = if i == 0 { initial } else { period };
            for (advance, count) in [(delay - MS, i), (MS, i + 1)] {
                self.headless.context().advance_clock(advance);
                self.headless.context().run_until_parked();
                let heard = self.events.borrow().len();
                assert_eq!(heard, count, "{repeats} repeats, {i}");
            }
            values.push(self.events.borrow()[i].1.clone());
        }
        values
    }

    fn close(self) {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, _| window.remove_window())
            .expect("window");
        self.headless.context().run_until_parked();
    }
}

fn main() {
    if asrs_parity::ci::skip_in_ci("spin buttons", "a GPU") {
        return;
    }
    if cfg!(target_os = "windows") {
        println!("spin buttons: skipped, no headless renderer on this platform");
        return;
    }
    let spec = vectors::workspace_root().join("parity/specs/form/spin-buttons.entry.json");
    let spec = json::parse(&std::fs::read_to_string(spec).expect("the spec")).expect("JSON");
    let steps = spec.get("fixture").and_then(|f| f.get("steps"));
    let steps = steps.and_then(Json::as_array).expect("steps");
    let host = if cfg!(target_os = "macos") {
        "macos-aarch64"
    } else {
        "linux-x86_64"
    };
    let records = vectors::load(&format!("form/spin-buttons/{host}")).expect("vectors");
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless");
    let mut records = records.iter();
    let (mut compared, mut held) = (0, 0);
    for step in steps {
        let name = text(step.get("name").expect("a name"));
        let mut page = Page::open(&mut headless, &text(step.get("markup").expect("markup")));
        let ops = step.get("ops").and_then(Json::as_array).expect("ops");
        for op in std::iter::once(&Json::Null).chain(ops) {
            let record = records.next().expect("a record");
            let at = format!("{name} {}", op.to_compact());
            assert_eq!(record.field("op").expect("op"), op, "{at}");
            let want = record.field("events").expect("events");
            let op = op.as_array();
            if let Some(initial) = want.get("initial").and_then(Json::as_f64) {
                let period = want.get("period").and_then(Json::as_f64).expect("period");
                let [initial, period] =
                    [initial, period].map(|ms| Duration::from_millis(ms as u64));
                let values = want.get("values").and_then(Json::as_array).expect("values");
                let want: Vec<String> = values.iter().map(text).collect();
                let got = page.hold(initial, period, want.len());
                assert_eq!(got, want, "{at}");
                held += 1;
            } else {
                match op {
                    Some(op) if text(&op[0]) == "hold" => {
                        let ms = op[1].as_f64().expect("ms") as u64;
                        page.headless
                            .context()
                            .advance_clock(Duration::from_millis(ms));
                        page.headless.context().run_until_parked();
                    }
                    Some(op) => page.apply(op),
                    None => {}
                }
                let want: Vec<(String, Option<String>)> = want
                    .as_array()
                    .expect("events")
                    .iter()
                    .map(|event| {
                        let event = event.as_array().expect("an event");
                        (text(&event[0]), event.get(1).map(text))
                    })
                    .collect();
                let got = page.events.borrow().clone();
                assert_eq!(got.len(), want.len(), "{at}: {got:?}, upstream {want:?}");
                for ((got, value), (want, at_event)) in got.iter().zip(&want) {
                    assert_eq!(got, want, "{at}");
                    if let Some(at_event) = at_event {
                        assert_eq!(value, at_event, "{at}");
                    }
                }
            }
            page.events.borrow_mut().clear();
            let field = page.field.clone();
            let window = page.window;
            let (value, focused, selected) = page
                .headless
                .context()
                .update_window(window.into(), |_, window, cx| {
                    let handle = field.focus_handle(cx);
                    let field = field.read(cx);
                    (
                        field.value(),
                        handle.is_focused(window),
                        field.selected_text(cx),
                    )
                })
                .expect("window");
            if let Some(want) = record.field("value") {
                assert_eq!(value, text(want), "{at}: value");
            }
            let want = record.field("focused").expect("focused");
            assert_eq!(&Json::Bool(focused), want, "{at}: focused");
            let want = text(record.field("selected").expect("selected"));
            assert_eq!(selected, want, "{at}: selected");
            compared += 1;
        }
        page.close();
    }
    assert!(records.next().is_none(), "a record no step has");
    assert!(held > 0, "nothing was held");
    println!("spin buttons: {compared} records, {held} holds");
}
