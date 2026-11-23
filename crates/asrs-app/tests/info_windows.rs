#![allow(clippy::disallowed_methods)]

//! Replays the scripted flows of `info/beta-welcome` (oracle/src/genInfoWindows.ts: upstream's
//! page in Blink, driven with real input) through the GPUI window. After every step the
//! checkboxes, the button's state, the focused control and the replies must equal the recording.
//! The page is taller than the window with some platforms' fonts, so clicks scroll to their target
//! first, and the flows also run in a window that is too short with any.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use asrs_app::views::cell_text::{css, length};
use asrs_app::views::info::beta_welcome::Control;
use asrs_app::views::info::layout::{BetaLayout, LuRect, Typesetter};
use asrs_app::views::info::{Acknowledged, BetaWelcomePage};
use asrs_parity::cve1::Value;
use asrs_parity::vectors;
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, Bounds, Entity, HeadlessAppContext, KeyDownEvent, KeyUpEvent,
    Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NoopTextSystem, PlatformInput, ScrollDelta, ScrollEvent, WindowBounds, WindowOptions, point,
    px, size,
};

const WIDTH: f32 = 450.0;
/// The window's height in the recordings, and one no platform's fonts fit the page in.
const HEIGHTS: [f32; 2] = [490.0, 240.0];

fn items(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {}", other.kind()),
    }
}

fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    let Value::Object(pairs) = value else {
        panic!("not an object: {}", value.kind());
    };
    let found = pairs.iter().find(|(k, _)| k.eq_str(key));
    &found.unwrap_or_else(|| panic!("no {key}")).1
}

struct Beta {
    height: f32,
    typesetter: Typesetter,
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    page: Entity<BetaWelcomePage>,
    replies: Rc<Cell<usize>>,
}

impl Beta {
    fn open(data: &Value, height: f32) -> Beta {
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        cx.update(asrs_ui::init);
        let replies = Rc::new(Cell::new(0));
        let mut page = None;
        let window = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.0), px(0.0)),
                    size(px(WIDTH), px(height)),
                ))),
                ..Default::default()
            };
            let (window, entity) = asrs_ui::open_window(options, cx, |window, cx| {
                cx.new(|cx| BetaWelcomePage::new("AdvantageScope", window, cx))
            })
            .expect("a window");
            let count = replies.clone();
            cx.subscribe(&entity, move |_, _: &Acknowledged, _| {
                count.set(count.get() + 1);
            })
            .detach();
            for message in items(data) {
                let alpha = *message == Value::Bool(true);
                entity.update(cx, |page, cx| page.message(alpha, cx));
            }
            page = Some(entity);
            window
        });
        let mut beta = Beta {
            height,
            typesetter: Typesetter::default(),
            cx,
            window,
            page: page.expect("the page"),
            replies,
        };
        beta.settle();
        beta
    }

    fn settle(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .expect("open");
        self.cx.run_until_parked();
    }

    fn input(&mut self, input: PlatformInput) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(input, cx);
            })
            .expect("open");
        self.settle();
    }

    fn wheel(&mut self, by: f32) {
        self.input(PlatformInput::Scroll(ScrollEvent {
            position: point(px(1.0), px(1.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-by))),
            ..ScrollEvent::default()
        }));
    }

    /// Scrolls the window from the top just far enough to show `rect`, and clicks its centre.
    fn click(&mut self, rect: LuRect) {
        let [x, y, w, h] = rect.map(|v| css(v, 1.0) as f32);
        let scrolled = (y + h - self.height).ceil().max(0.0);
        self.wheel(-1e6);
        self.wheel(scrolled);
        let position = point(px(x + w / 2.0), px(y + h / 2.0 - scrolled));
        let modifiers = Modifiers::default();
        self.input(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        }));
        self.input(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }));
        self.input(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
        }));
    }

    fn key(&mut self, name: &str) {
        let (shift, key) = match name.strip_prefix("Shift+") {
            Some(key) => (true, key),
            None => (false, name),
        };
        let key = match key {
            "NumpadEnter" => "enter".to_owned(),
            other => other.to_lowercase(),
        };
        let keystroke = Keystroke {
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
            key,
            key_char: None,
        };
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_keystroke(keystroke.clone(), cx);
                window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
            })
            .expect("open");
        self.settle();
    }

    fn step(&mut self, step: &str) {
        let (op, what) = step.split_once(' ').expect("a step");
        if op == "key" {
            return self.key(what);
        }
        let (t, width) = (&mut self.typesetter, length(f64::from(WIDTH), 1.0));
        let layout = self.cx.update(|cx| {
            let page = self.page.read(cx).page();
            BetaLayout::new(t, page, "AdvantageScope", width, 1.0)
        });
        self.click(match what {
            "button" => layout.button,
            "text" => layout.intro.0,
            index => layout.rows[index.parse::<usize>().expect("a checkbox")].checkbox,
        });
    }

    fn check(&mut self, state: &Value, step: &str) {
        let page = self.page.clone();
        let (checked, enabled, active) = self
            .cx
            .update_window(self.window, |_, window, cx| {
                let view = page.read(cx);
                (
                    view.page().checked(),
                    view.page().enabled(),
                    view.active(window),
                )
            })
            .expect("open");
        let want: Vec<bool> = items(get(state, "checked"))
            .iter()
            .map(|c| *c == Value::Bool(true))
            .collect();
        assert_eq!(checked.as_slice(), want, "{step}");
        assert_eq!(get(state, "disabled"), &Value::Bool(!enabled), "{step}");
        let want = match get(state, "active") {
            Value::Null => None,
            Value::Number(bits) => Some(Control::Checkbox(f64::from_bits(*bits) as usize)),
            Value::String(_) => Some(Control::Button),
            other => panic!("{}", other.kind()),
        };
        assert_eq!(active, want, "{step}");
        assert_eq!(
            self.replies.take(),
            items(get(state, "replies")).len(),
            "{step}"
        );
    }
}

fn replay(height: f32) {
    let records = vectors::load("info/beta-welcome").expect("the vector");
    let (mut beta, mut flows, mut steps): (Option<Beta>, usize, usize) = (None, 0, 0);
    for record in &records {
        match record.string("kind").expect("a kind").as_str() {
            "dump" => {
                flows += 1;
                beta = Some(Beta::open(&record.value("data").expect("data"), height));
            }
            "step" => {
                steps += 1;
                let case = record.string("case").expect("a case");
                let step = record.string("step").expect("a step");
                let beta = beta.as_mut().expect("a page");
                beta.step(&step);
                beta.check(
                    &record.value("state").expect("state"),
                    &format!("{height} px, {case}: {step}"),
                );
            }
            other => assert_eq!(other, "end"),
        }
    }
    let recorded = records
        .iter()
        .filter(|r| r.string("kind").as_deref() == Some("step"));
    assert_eq!(steps, recorded.count());
    assert_eq!(flows, 26);
}

#[test]
fn beta_welcome_flows() {
    for height in HEIGHTS {
        replay(height);
    }
}

#[test]
fn a_held_space_clicks_once_at_its_release() {
    let records = vectors::load("info/beta-welcome").expect("the vector");
    let data = records[0].value("data").expect("data");
    let mut beta = Beta::open(&data, HEIGHTS[0]);
    beta.key("Tab");
    let keystroke = Keystroke {
        modifiers: Modifiers::default(),
        key: "space".to_owned(),
        key_char: None,
    };
    let checked = |beta: &mut Beta| {
        let page = beta.page.clone();
        beta.cx.update(|cx| page.read(cx).page().checked())
    };
    for is_held in [false, true, true] {
        beta.input(PlatformInput::KeyDown(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held,
            prefer_character_input: false,
        }));
        assert_eq!(checked(&mut beta), [false; 3]);
    }
    beta.input(PlatformInput::KeyUp(KeyUpEvent {
        keystroke: keystroke.clone(),
    }));
    assert_eq!(checked(&mut beta), [true, false, false]);

    beta.input(PlatformInput::KeyDown(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    }));
    beta.key("Tab");
    beta.input(PlatformInput::KeyUp(KeyUpEvent { keystroke }));
    assert_eq!(checked(&mut beta), [true, false, false]);
}
