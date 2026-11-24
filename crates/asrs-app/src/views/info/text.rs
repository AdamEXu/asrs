//! Selecting a page's text: the whole document is selectable (`body { user-select: text }`).

use std::ops::Range;

use asrs_canvas::text::dom::Lu;
use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::gpui::{App, Context, KeyDownEvent, MouseButton, MouseDownEvent, Window};
use asrs_ui::platform::EditCommand;
use asrs_ui::selection::colors::{SelectionColors, page_active};
use asrs_ui::selection::geometry::{Block, LuRect, Measure, part};
use asrs_ui::selection::{
    Behavior, Button, Caret, Document, Hit, Position, Press, TextSelection, write_clipboard,
};

use super::page::ScrolledPage;
use crate::views::page_text::{Selected, autoscroll_by, edit_command};

/// A document's text and where it is, in device `LayoutUnit`s from the page's top left corner.
pub trait DocumentText {
    fn document(&self) -> &dyn Document;

    fn hit(&self, at: [Lu; 2], backend: &mut dyn TextBackend, scale: f32) -> Hit;
}

/// The position in a block at a point of the document.
pub fn caret_in(
    (index, block): (usize, &Block),
    text: &str,
    [x, y]: [Lu; 2],
    origin: [Lu; 2],
    backend: &mut dyn TextBackend,
    scale: f32,
) -> Hit {
    let at = [x - origin[0], y - origin[1]];
    let mut measure = Measure {
        backend,
        text,
        scale,
    };
    Hit {
        caret: Some(block.caret_at(index, at, Behavior::current(), &mut measure)),
        selectable: true,
        text: block.on_text(at),
        link: None,
    }
}

pub fn unselectable(block: usize) -> Hit {
    Hit {
        caret: Some(Caret::new(Position { block, offset: 0 })),
        selectable: false,
        text: false,
        link: None,
    }
}

#[derive(Clone, Copy)]
struct Gesture {
    pressed: [f64; 2],
    at: [f64; 2],
    started: f64,
    autoscroll: bool,
}

pub struct PageText {
    state: TextSelection,
    gesture: Option<Gesture>,
    on_text: bool,
    backend: Box<dyn TextBackend>,
}

impl Default for PageText {
    fn default() -> Self {
        PageText {
            state: TextSelection::default(),
            gesture: None,
            on_text: false,
            backend: platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)),
        }
    }
}

impl PageText {
    pub fn cursor(&self) -> bool {
        self.state.selecting() || (self.on_text && self.gesture.is_none())
    }

    /// Whether the whole document is selected.
    pub fn whole(&self, doc: &dyn Document) -> bool {
        self.state.is_all(doc)
    }

    /// What the document's text is drawn selected with.
    pub(crate) fn selected(
        &mut self,
        doc: &dyn Document,
        window: &Window,
        cx: &App,
    ) -> Option<Selected> {
        let colors = SelectionColors::new(page_active(window, cx));
        self.state.sync(doc);
        Some(Selected {
            range: self.state.painted(doc)?,
            colors,
        })
    }

    /// The boxes a selection of `range` paints in a block whose origin is `origin`, each with its
    /// line.
    pub fn rects(
        &mut self,
        range: &Range<Position>,
        (index, block): (usize, &Block),
        text: &str,
        [x, y]: [Lu; 2],
        scale: f32,
    ) -> Vec<LuRect> {
        let Some((from, to)) = part(range, index) else {
            return Vec::new();
        };
        let mut measure = Measure {
            backend: self.backend.as_mut(),
            text,
            scale,
        };
        block
            .selection_rects(from, to, &mut measure)
            .into_iter()
            .map(|(_, _, [left, top, w, h])| [left + x, top + y, w, h])
            .collect()
    }
}

fn hit<V: ScrolledPage>(view: &mut V, [x, y]: [f64; 2], window: &Window) -> Option<Hit> {
    let scale = f64::from(window.scale_factor());
    let lu = |css: f64| (css * scale * 64.0).floor() as Lu;
    let scrolled = asrs_js::math::round(view.scroll().top() * scale) as Lu * 64;
    let page = view.document_text(window)?;
    let backend = view.text().backend.as_mut();
    Some(page.hit([lu(x), lu(y) + scrolled], backend, scale as f32))
}

fn now<V: ScrolledPage>(view: &mut V, cx: &App) -> f64 {
    view.scroll().now(cx)
}

pub fn mouse_down<V: ScrolledPage>(
    view: &mut V,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<'_, V>,
) {
    let at = [event.position.x, event.position.y].map(|v| mouse_client(f32::from(v)));
    let button = match event.button {
        MouseButton::Left => Button::Left,
        MouseButton::Middle => Button::Middle,
        MouseButton::Right => Button::Right,
        MouseButton::Navigate(_) => return,
    };
    let press = Press {
        button,
        click_count: event.click_count,
        shift: event.modifiers.shift,
    };
    let Some(hit) = hit(view, at, window) else {
        return;
    };
    let Some(page) = view.document_text(window) else {
        return;
    };
    let started = now(view, cx);
    let text = view.text();
    text.state.press(page.document(), hit, press);
    text.gesture = Some(Gesture {
        pressed: at,
        at,
        started,
        autoscroll: false,
    });
    cx.notify();
}

pub fn mouse_move<V: ScrolledPage>(
    view: &mut V,
    at: [f64; 2],
    pressed: bool,
    window: &mut Window,
    cx: &mut Context<'_, V>,
) {
    if !pressed && view.text().gesture.take().is_some() {
        view.text().state.release_lost();
    }
    if let Some(gesture) = &mut view.text().gesture {
        gesture.at = at;
        drag(view, window, cx);
        return;
    }
    let on_text = hit(view, at, window).is_some_and(|hit| hit.text);
    if std::mem::replace(&mut view.text().on_text, on_text) != on_text {
        cx.notify();
    }
}

fn drag<V: ScrolledPage>(view: &mut V, window: &mut Window, cx: &mut Context<'_, V>) {
    let Some(gesture) = view.text().gesture else {
        return;
    };
    let caret = hit(view, gesture.at, window).and_then(|hit| hit.caret);
    let Some(page) = view.document_text(window) else {
        return;
    };
    let since = now(view, cx) - gesture.started;
    let text = view.text();
    text.state.drag(page.document(), caret, since);
    let selecting = text.state.selecting() && text.state.range(page.document()).is_some();
    if let Some(gesture) = &mut text.gesture {
        gesture.autoscroll |= selecting;
    }
    cx.notify();
}

pub fn mouse_up<V: ScrolledPage>(
    view: &mut V,
    at: [f64; 2],
    window: &mut Window,
    cx: &mut Context<'_, V>,
) {
    let Some(gesture) = view.text().gesture.take() else {
        return;
    };
    if let Some(page) = view.document_text(window) {
        let text = view.text();
        text.state.release(page.document(), gesture.pressed != at);
        cx.notify();
    }
}

/// A frame of `AutoscrollController`'s animation.
pub fn autoscroll<V: ScrolledPage>(view: &mut V, window: &mut Window, cx: &mut Context<'_, V>) {
    let Some(gesture) = view.text().gesture.filter(|g| g.autoscroll) else {
        return;
    };
    window.request_animation_frame();
    let size = window.viewport_size();
    let viewport = [size.width, size.height].map(|v| f64::from(f32::from(v)));
    let scale = f64::from(window.scale_factor());
    let by = autoscroll_by([0.0, 0.0, viewport[0], viewport[1]], gesture.at, scale);
    if by[1] != 0.0 {
        view.scroll().scroll_by(by[1]);
        drag(view, window, cx);
    }
}

pub fn run<V: ScrolledPage>(
    view: &mut V,
    command: EditCommand,
    window: &mut Window,
    cx: &mut Context<'_, V>,
) {
    let Some(page) = view.document_text(window) else {
        return;
    };
    let text = view.text();
    match command {
        EditCommand::Copy => {
            text.state.sync(page.document());
            if let Some(copied) = text.state.text(page.document()) {
                write_clipboard(&copied, cx);
            }
        }
        EditCommand::SelectAll => text.state.select_all(page.document()),
    }
    cx.notify();
}

pub fn key_down<V: ScrolledPage>(
    view: &mut V,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<'_, V>,
) {
    if let Some(command) = edit_command(event) {
        run(view, command, window, cx);
    }
}
