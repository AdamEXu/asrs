use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;

use gpui_kit::{Hitbox, HitboxBehavior};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::PageOptions;
use super::wire::{FromHost, PageEvent, ToHost};
use crate::gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, Styled, Task, Window, canvas, point, px,
};

type Done = Box<dyn Fn(String) + Send>;

pub struct Page {
    host: Rc<Host>,
    _events: Task<()>,
}

struct Host {
    input: RefCell<ChildStdin>,
    /// What the host was told last.
    shown: Cell<bool>,
    placed: Cell<Option<([f64; 4], f64)>>,
    visible: Cell<bool>,
    dark: Cell<bool>,
    origin: Cell<Point<Pixels>>,
    queries: RefCell<HashMap<u64, Done>>,
    asked: Cell<u64>,
}

impl Page {
    pub fn open(
        options: PageOptions,
        window: &mut Window,
        cx: &mut App,
        on_event: impl Fn(PageEvent, &mut App) + 'static,
    ) -> Result<Page, String> {
        let parent = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Xcb(handle)) => u64::from(handle.window.get()),
            Ok(RawWindowHandle::Xlib(handle)) => handle.window,
            Ok(_) => return Err("a page can only be shown in an X11 window".to_owned()),
            Err(e) => return Err(e.to_string()),
        };
        let embeds = options.embeds.iter().map(OsString::from);
        let mut child = Command::new(&options.host)
            .arg(parent.to_string())
            .arg(if options.dark { "dark" } else { "light" })
            .arg(&options.root)
            .arg(options.index)
            .args(embeds)
            .env("GDK_BACKEND", "x11")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", options.host.display()))?;
        let input = child.stdin.take().expect("a piped stdin");
        let receiver = read(child);
        let host = Rc::new(Host {
            input: RefCell::new(input),
            shown: Cell::new(false),
            placed: Cell::new(None),
            visible: Cell::new(true),
            dark: Cell::new(options.dark),
            origin: Cell::default(),
            queries: RefCell::default(),
            asked: Cell::new(0),
        });
        let events = {
            let host = Rc::downgrade(&host);
            let window = Window::window_handle(window);
            cx.spawn(async move |cx| {
                while let Ok(message) = receiver.recv().await {
                    let Some(host) = host.upgrade() else {
                        return;
                    };
                    match message {
                        FromHost::Ready => {}
                        FromHost::Event(event) => cx.update(|cx| on_event(event, cx)),
                        FromHost::Result(id, json) => {
                            let done = host.queries.borrow_mut().remove(&id);
                            if let Some(done) = done {
                                done(json);
                            }
                        }
                        FromHost::Press(button, at) => {
                            cx.update(|cx| press(&host, window, button, at, cx));
                        }
                    }
                }
            })
        };
        Ok(Page {
            host,
            _events: events,
        })
    }

    pub fn element(&self) -> AnyElement {
        let host = self.host.clone();
        canvas(
            move |bounds, window, _| host.place(bounds, window),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    pub fn set_visible(&self, visible: bool, _: &mut App) {
        self.host.visible.set(visible);
        if !visible {
            self.host.show(false);
        }
    }

    pub fn set_dark(&self, dark: bool, _: &mut App) {
        if self.host.dark.replace(dark) != dark {
            self.host.send(&ToHost::Dark(dark));
        }
    }

    /// `location.hash = hash`, after which the page reports `id`. `hash` is a serialized URL
    /// fragment.
    pub fn assign_hash(&self, id: u64, hash: &str, cx: &mut App) {
        self.evaluate(&super::assignment(id, hash), cx);
    }

    pub fn evaluate(&self, script: &str, _: &mut App) {
        self.host.send(&ToHost::Evaluate(script.to_owned()));
    }

    /// Evaluates `script` and hands its result's JSON to `done`.
    pub fn query(&self, script: &str, done: impl Fn(String) + Send + 'static, _: &mut App) {
        let id = self.host.asked.get() + 1;
        self.host.asked.set(id);
        self.host.queries.borrow_mut().insert(id, Box::new(done));
        self.host.send(&ToHost::Query(id, script.to_owned()));
    }
}

impl Host {
    /// A host that is gone is reported by the thread that reads from it.
    fn send(&self, message: &ToHost) {
        let _ = self.input.borrow_mut().write_all(message.line().as_bytes());
    }

    fn show(&self, shown: bool) {
        if self.shown.replace(shown) != shown {
            self.send(&ToHost::Visible(shown));
        }
    }

    fn place(&self, bounds: Bounds<Pixels>, window: &mut Window) -> Option<Hitbox> {
        let seen = bounds.intersect(&window.content_mask().bounds);
        if !self.visible.get() || seen.is_empty() {
            self.show(false);
            return None;
        }
        let scale = f64::from(window.scale_factor());
        let device = |v: Pixels| f64::from(f32::from(v)) * scale;
        let rect = [
            device(bounds.origin.x),
            device(bounds.origin.y),
            device(bounds.size.width),
            device(bounds.size.height),
        ];
        self.origin.set(bounds.origin);
        if self.placed.replace(Some((rect, scale))) != Some((rect, scale)) {
            self.send(&ToHost::Bounds(rect, scale));
        }
        self.show(true);
        Some(window.insert_hitbox(seen, HitboxBehavior::BlockMouse))
    }
}

/// GPUI does not see presses on the page, which close menus and popovers.
fn press(host: &Host, window: AnyWindowHandle, button: u32, [x, y]: [f64; 2], cx: &mut App) {
    let button = match button {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        _ => return,
    };
    let modifiers = Modifiers::default();
    let _ = window.update(cx, |_, window, cx| {
        let scale = f64::from(window.scale_factor());
        let position = host.origin.get() + point(px((x / scale) as f32), px((y / scale) as f32));
        let down = MouseDownEvent {
            button,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        };
        let up = MouseUpEvent {
            button,
            position,
            modifiers,
            click_count: 1,
        };
        window.dispatch_event(PlatformInput::MouseDown(down), cx);
        window.dispatch_event(PlatformInput::MouseUp(up), cx);
    });
}

fn read(mut child: Child) -> async_channel::Receiver<FromHost> {
    let (sender, receiver) = async_channel::unbounded();
    let output = child.stdout.take().expect("a piped stdout");
    std::thread::spawn(move || {
        let mut failed = false;
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if let Some(message) = FromHost::parse(&line) {
                failed |= matches!(message, FromHost::Event(PageEvent::Failed(_)));
                let _ = sender.try_send(message);
            }
        }
        let status = child.wait();
        if !failed {
            let status = status.map_or_else(|e| e.to_string(), |s| s.to_string());
            let event = PageEvent::Failed(format!("asrs-webview ended ({status})"));
            let _ = sender.try_send(FromHost::Event(event));
        }
    });
    receiver
}
