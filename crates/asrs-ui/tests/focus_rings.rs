//! Replays the `focus/rings` vectors: which element of a page in Blink has the focus and takes the
//! focus ring after trusted input and script, in active and inactive windows.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;

use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_ui::focus;
use asrs_ui::selection::colors::ForcedActive;
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Capslock, Context, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PlatformInput,
    Render, Styled, TestAppContext, Window, WindowBounds, WindowHandle, WindowOptions, div, point,
    px, size,
};

const PLATFORMS: [&str; 2] = ["macos-aarch64", "linux-x86_64"];
/// The fixture's elements with `global.css`'s ring.
const RINGED: [&str; 6] = ["b", "s", "l", "t", "g", "w"];
/// Elements a press focuses, and the one only the keyboard or script does.
const PRESSED: [&str; 10] = ["b", "s", "l", "t", "n", "i", "x", "c", "r", "g"];
const OTHER: [&str; 1] = ["w"];
const SIDE: f32 = 20.0;

struct Page {
    elements: BTreeMap<&'static str, FocusHandle>,
    /// What the Tab being dispatched moves the focus to.
    tab: Option<FocusHandle>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let mut root = div().size_full().on_any_mouse_down(|_, window, cx| {
            if !window.default_prevented() {
                window.blur(cx);
            }
        });
        for (i, id) in PRESSED.iter().enumerate() {
            root = root.child(
                div()
                    .absolute()
                    .left(px(i as f32 * SIDE))
                    .top_0()
                    .size(px(SIDE))
                    .track_focus(&self.elements[id]),
            );
        }
        root
    }
}

fn centre(id: &str) -> (f32, f32) {
    let place = PRESSED.iter().position(|e| *e == id);
    place.map_or((5.0 * SIDE, 3.0 * SIDE), |i| {
        ((i as f32 + 0.5) * SIDE, 0.5 * SIDE)
    })
}

fn text(json: &Json) -> String {
    json.as_str().expect("a string").to_string_lossy()
}

fn held(json: Option<&Json>) -> Modifiers {
    let mut m = Modifiers::default();
    for name in json.and_then(Json::as_array).into_iter().flatten() {
        match text(name).as_str() {
            "shift" => m.shift = true,
            "ctrl" => m.control = true,
            "alt" => m.alt = true,
            "meta" => m.platform = true,
            other => panic!("modifier {other}"),
        }
    }
    m
}

fn key_name(name: &str) -> String {
    match name {
        " " => "space".to_owned(),
        _ => name.strip_prefix("Arrow").unwrap_or(name).to_lowercase(),
    }
}

struct Replay {
    cx: TestAppContext,
    window: WindowHandle<Page>,
    capslock: Capslock,
}

impl Replay {
    fn new(cx: TestAppContext) -> Replay {
        let window = cx.update(|cx| {
            let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(300.0), px(100.0)));
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            };
            cx.open_window(options, |_, cx| {
                cx.new(|cx: &mut Context<'_, Page>| {
                    let page = cx.weak_entity();
                    cx.intercept_keystrokes(move |event, window, cx| {
                        let tab = event.keystroke.key == "tab";
                        let next = tab.then(|| page.update(cx, |page, _| page.tab.take()));
                        if let Some(Ok(Some(next))) = next {
                            window.focus(&next, cx);
                        }
                    })
                    .detach();
                    let ids = PRESSED.iter().chain(&OTHER);
                    Page {
                        elements: ids.map(|id| (*id, cx.focus_handle())).collect(),
                        tab: None,
                    }
                })
            })
            .expect("a window")
        });
        Replay {
            cx,
            window,
            capslock: Capslock::default(),
        }
    }

    fn send(&mut self, event: PlatformInput) {
        let window: AnyWindowHandle = self.window.into();
        self.cx.run_until_parked();
        self.cx
            .update_window(window, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.dispatch_event(event, cx);
            })
            .expect("open");
    }

    fn key(&mut self, kind: &str, name: &str, modifiers: Modifiers) {
        let own = match name {
            "Shift" => Modifiers::shift(),
            "Control" => Modifiers::control(),
            "Alt" => Modifiers::alt(),
            "Meta" => Modifiers::command(),
            _ => Modifiers::none(),
        };
        let modifier = own != Modifiers::none() || name == "CapsLock";
        if kind != "up" {
            if name == "CapsLock" {
                self.capslock.on = !self.capslock.on;
            }
            self.send(if modifier {
                PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                    modifiers: modifiers | own,
                    capslock: self.capslock,
                })
            } else {
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke {
                        modifiers,
                        key: key_name(name),
                        key_char: None,
                    },
                    is_held: false,
                    prefer_character_input: false,
                })
            });
        }
        if kind != "down" {
            self.send(if modifier {
                PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                    modifiers,
                    capslock: self.capslock,
                })
            } else {
                PlatformInput::KeyUp(KeyUpEvent {
                    keystroke: Keystroke {
                        modifiers,
                        key: key_name(name),
                        key_char: None,
                    },
                })
            });
        }
    }

    fn press(&mut self, id: &str, button: MouseButton) {
        let (x, y) = centre(id);
        let position = point(px(x), px(y));
        let modifiers = Modifiers::none();
        self.send(PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }));
        self.send(PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers,
            click_count: 1,
        }));
    }

    fn perform(&mut self, op: &[Json], active: Option<&str>) {
        let name = || text(&op[1]);
        match text(&op[0]).as_str() {
            "active" => {
                let active = ForcedActive(op[1] == Json::Bool(true));
                self.cx.update(|cx| cx.set_global(active));
            }
            kind @ ("key" | "down" | "up") => {
                if name() == "Tab" {
                    self.page(|page, _, _| page.tab = active.map(|id| page.elements[id].clone()));
                }
                self.key(kind, &name(), held(op.get(2)));
            }
            "press" => {
                let button = match op.get(2).map(text).as_deref() {
                    None => MouseButton::Left,
                    Some("right") => MouseButton::Right,
                    Some("middle") => MouseButton::Middle,
                    Some(other) => panic!("button {other}"),
                };
                self.press(&name(), button);
            }
            "move" => {
                let (x, y) = centre(&name());
                self.send(PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(x), px(y)),
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                }));
            }
            "focus" => {
                let id = name();
                self.page(|page, window, cx| window.focus(&page.elements[id.as_str()], cx));
            }
            "blur" => self.page(|_, window, cx| window.blur(cx)),
            other => panic!("op {other}"),
        }
    }

    fn page<R>(
        &mut self,
        f: impl FnOnce(&mut Page, &mut Window, &mut Context<'_, Page>) -> R,
    ) -> R {
        self.window.update(&mut self.cx, f).expect("open")
    }
}

fn replay(platform: &str) -> usize {
    let id = format!("focus/rings/{platform}");
    let records = vectors::load(&id).unwrap_or_else(|e| panic!("{id}: {e}"));
    let mut replay = None;
    let mut sequence = None;
    let mut skipped = None;
    let mut compared = 0;
    let mut active = false;
    for record in &records {
        let Some(number) = record.field("sequence").and_then(Json::as_f64) else {
            continue;
        };
        if sequence != Some(number) {
            sequence = Some(number);
            let cx = TestAppContext::single();
            cx.update(|cx| cx.set_global(ForcedActive(false)));
            replay = Some(Replay::new(cx));
            active = false;
        }
        if skipped == sequence {
            continue;
        }
        let replay = replay.as_mut().expect("a sequence");
        let op = record.field("op").and_then(Json::as_array).expect("an op");
        let id = record.field("active").and_then(Json::as_str);
        let id = id.map(|id| id.to_string_lossy());
        if text(&op[0]) == "active" {
            active = op[1] == Json::Bool(true);
        }
        // A press activates the window before the page has it.
        if !active && text(&op[0]) == "press" && id.is_none() {
            skipped = sequence;
            continue;
        }
        replay.perform(op, id.as_deref());
        let bool_of = |key: &str| record.field(key) == Some(&Json::Bool(true));
        let at = format!("{platform} sequence {number}: {op:?} -> {id:?}");
        let Some(id) = id.as_deref() else {
            replay.page(|_, window, cx| assert!(window.focused(cx).is_none(), "{at}"));
            continue;
        };
        let (focused, has_focus, visible) = replay.page(|page, window, cx| {
            let handle = &page.elements[id];
            (
                handle.is_focused(window),
                focus::has_focus(handle, window, cx),
                focus::focus_visible(handle, window, cx),
            )
        });
        assert!(focused, "{at}: the focus is elsewhere");
        compared += 1;
        assert_eq!(has_focus, bool_of("focus"), "{at}: :focus");
        let ring = match id {
            "s" => has_focus,
            _ => visible && RINGED.contains(&id),
        };
        assert_eq!(ring, bool_of("ring"), "{at}: the ring");
    }
    compared
}

#[test]
fn rings_are_blinks() {
    for platform in PLATFORMS {
        let compared = replay(platform);
        assert!(compared > 2000, "{platform}: {compared} records");
    }
}
