//! The text of the hub's tabs that can be selected (`user-select: text` in `renderers.css`): each
//! tab's blocks and where they are, for [`TextSelection`].

use std::cell::RefCell;
use std::ops::Range;

use asrs_canvas::text::dom::{self, LU_PER_PX, Lu};
use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::gpui::{
    App, Context, Div, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Styled, Window,
    div, px,
};
use asrs_ui::input;
use asrs_ui::selection::all;
use asrs_ui::selection::colors::{SelectionColors, page_active};
use asrs_ui::selection::document::end;
use asrs_ui::selection::geometry::{Block, LuRect, Measure, part};
use asrs_ui::selection::{
    Behavior, Button, Caret, Document, Hit, Position, Press, TextSelection, write_clipboard,
};

use super::frame::{FrameHost, HubFrame};
use super::large_table::Grid;

/// The page whose document a selection is in.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Owner {
    Table(asrs_js::string::JsString),
    Console(usize),
    Statistics(usize),
    Metadata,
}

/// A tab's text and where it is in the window.
pub(super) trait PageText {
    fn document(&self) -> &dyn Document;

    /// What is at a client position in the tab's renderer element, or past the window's edges.
    fn hit(&self, at: [f64; 2], backend: &mut dyn TextBackend) -> Hit;

    /// The client box of what scrolls.
    fn scroller(&self) -> [f64; 4];

    /// The renderer element's client box.
    fn renderer(&self) -> [f64; 4] {
        self.scroller()
    }
}

/// A length in device `LayoutUnit`s.
pub(super) fn device(css: f64, scale: f64) -> Lu {
    dom::lu_round((css * scale) as f32)
}

pub(super) fn floor_device(css: f64, scale: f64) -> Lu {
    (css * scale * f64::from(LU_PER_PX)).floor() as Lu
}

/// An element that cannot be selected, before block `block` in the document.
pub(super) fn unselectable(doc: &dyn Document, block: usize) -> Hit {
    let next = (block < doc.len()).then_some(Position { block, offset: 0 });
    Hit {
        caret: next.or_else(|| end(doc)).map(Caret::new),
        selectable: false,
        text: false,
        link: None,
    }
}

/// `PositionForPointInTable` for a point outside the table: the closer end of it.
pub(super) fn beside_table(doc: &dyn Document, x: f64, [left, width]: [f64; 2]) -> Hit {
    let block = if x - left <= width / 2.0 {
        0
    } else {
        doc.len()
    };
    unselectable(doc, block)
}

/// A block of selectable text whose content box is at `origin` in device `LayoutUnit`s.
pub(super) struct PlacedBlock<'a> {
    pub index: usize,
    pub block: &'a Block,
    pub text: &'a str,
    pub origin: [Lu; 2],
    pub scale: f64,
}

impl PlacedBlock<'_> {
    fn measure<'b>(&'b self, backend: &'b mut dyn TextBackend) -> Measure<'b> {
        Measure {
            backend,
            text: self.text,
            scale: self.scale as f32,
        }
    }

    fn in_block(&self, [x, y]: [f64; 2]) -> [Lu; 2] {
        [
            floor_device(x, self.scale) - self.origin[0],
            floor_device(y, self.scale) - self.origin[1],
        ]
    }

    /// The text node at a client position.
    pub fn node_at(&self, at: [f64; 2]) -> Option<usize> {
        self.block.node_at(self.in_block(at))
    }

    pub fn hit(&self, at: [f64; 2], backend: &mut dyn TextBackend) -> Hit {
        let at = self.in_block(at);
        let behavior = Behavior::current();
        let mut measure = self.measure(backend);
        Hit {
            caret: Some(self.block.caret_at(self.index, at, behavior, &mut measure)),
            selectable: true,
            text: self.block.on_text(at),
            link: None,
        }
    }

    /// The client boxes a selection of `range` paints behind the block's text.
    pub fn rects(&self, range: &Range<Position>, backend: &mut dyn TextBackend) -> Vec<[f64; 4]> {
        self.pieces(range, backend)
            .into_iter()
            .map(|(_, _, rect)| rect)
            .collect()
    }

    /// The same with each box's line and piece.
    pub fn pieces(
        &self,
        range: &Range<Position>,
        backend: &mut dyn TextBackend,
    ) -> Vec<(usize, Option<usize>, [f64; 4])> {
        let Some((from, to)) = part(range, self.index) else {
            return Vec::new();
        };
        let mut measure = self.measure(backend);
        let rects = self.block.selection_rects(from, to, &mut measure);
        let css = |lu: Lu| f64::from(lu) / f64::from(LU_PER_PX) / self.scale;
        let place = |[x, y, w, h]: LuRect| [x + self.origin[0], y + self.origin[1], w, h].map(css);
        rects
            .into_iter()
            .map(|(line, piece, rect)| (line, piece, place(rect)))
            .collect()
    }
}

/// What a tab's text is drawn selected with.
pub(crate) struct Selected {
    pub range: Range<Position>,
    pub colors: SelectionColors,
}

impl Selected {
    pub(super) fn paint(
        &self,
        grid: Grid,
        block: &PlacedBlock<'_>,
        backend: &mut dyn TextBackend,
        layer: Div,
    ) -> Div {
        block
            .rects(&self.range, backend)
            .into_iter()
            .fold(layer, |layer, rect| {
                layer.child(grid.place(rect).bg(self.colors.rgba()))
            })
    }
}

impl Selected {
    /// The part of `text`, what is drawn in the layer, that is in a box of the selection, in the
    /// colour selected text has.
    pub(super) fn recoloured(
        &self,
        grid: Grid,
        [x, y, w, h]: [f64; 4],
        text: impl FnOnce(u32) -> Vec<Div>,
    ) -> Option<Div> {
        let color = self.colors.text?;
        let left = px((grid.origin[0] - grid.snap(x)) as f32);
        let top = px((grid.origin[1] - grid.snap(y)) as f32);
        let layer = div().absolute().left(left).top(top).children(text(color));
        Some(grid.place([x, y, w, h]).overflow_hidden().child(layer))
    }
}

/// A press on the page and where the pointer is since.
#[derive(Clone, Copy, Debug)]
struct Gesture {
    pressed: [f64; 2],
    at: [f64; 2],
    started: f64,
    autoscroll: bool,
}

/// The hub page's selection.
pub struct PageSelection {
    state: TextSelection,
    owner: Option<Owner>,
    /// The selection is the whole document, whichever tab shows.
    all: bool,
    gesture: Option<Gesture>,
    /// Whether the pointer is over text that can be selected.
    on_text: bool,
    backend: RefCell<Box<dyn TextBackend>>,
}

impl Default for PageSelection {
    fn default() -> Self {
        PageSelection {
            state: TextSelection::default(),
            owner: None,
            all: false,
            gesture: None,
            on_text: false,
            backend: RefCell::new(platform_backend().unwrap_or_else(|| Box::new(FixedAdvance))),
        }
    }
}

impl PageSelection {
    pub(super) fn backend(&self) -> std::cell::RefMut<'_, Box<dyn TextBackend>> {
        self.backend.borrow_mut()
    }

    pub(super) fn backend_cell(&self) -> &RefCell<Box<dyn TextBackend>> {
        &self.backend
    }

    /// `cursor: auto`: an I-beam over text and while selecting.
    pub(super) fn text_cursor(&self) -> bool {
        self.state.selecting() || (self.on_text && self.gesture.is_none())
    }

    /// The selection of `owner`'s document, for drawing it.
    pub(super) fn selected(
        &mut self,
        owner: &Owner,
        doc: &dyn Document,
        window: &Window,
        cx: &App,
    ) -> Option<Selected> {
        if self.all {
            self.state.keep_all(doc);
            self.owner = Some(owner.clone());
        } else if self.owner.as_ref() != Some(owner) {
            return None;
        }
        self.state.sync(doc);
        Some(Selected {
            range: self.state.painted(doc)?,
            colors: SelectionColors::new(page_active(window, cx)),
        })
    }
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed tab's selectable text.
    fn page_text<'a>(&'a self, cx: &'a App) -> Option<(Owner, Box<dyn PageText + 'a>)> {
        self.table_text(cx)
            .or_else(|| self.console_text(cx))
            .or_else(|| self.statistics_text(cx))
            .or_else(|| self.metadata_text(cx))
    }

    /// What is at a client position.
    fn text_hit(&self, page: &dyn PageText, at @ [x, y]: [f64; 2], window: &Window) -> Hit {
        let doc = page.document();
        let inside =
            |[left, top, w, h]: [f64; 4]| x >= left && x < left + w && y >= top && y < top + h;
        let size = window.viewport_size();
        let window = [
            0.0,
            0.0,
            f64::from(f32::from(size.width)),
            f64::from(f32::from(size.height)),
        ];
        let renderer = page.renderer();
        let timeline = self.timeline_rect().is_some_and(inside);
        if !timeline && (inside(renderer) || !inside(window)) {
            return page.hit(at, &mut **self.text.backend());
        }
        // The document's order: the side bar and the tab bar, the tab, its controls, the timeline.
        let below = timeline || (x >= renderer[0] && y >= renderer[1] + renderer[3]);
        unselectable(doc, if below { doc.len() } else { 0 })
    }

    /// Runs `f` on the selection and the displayed tab's text.
    fn with_text<R>(
        &mut self,
        cx: &mut App,
        f: impl FnOnce(&Self, &mut TextSelection, &dyn PageText) -> R,
    ) -> Option<R> {
        let mut state = std::mem::take(&mut self.text.state);
        let mut other = TextSelection::default();
        let found = self.page_text(cx).map(|(owner, page)| {
            let shown = self.text.owner.as_ref() == Some(&owner);
            let target = if shown { &mut state } else { &mut other };
            if self.text.all {
                target.keep_all(page.document());
            }
            let result = f(self, target, page.as_ref());
            let all = self.text.all && target.is_all(page.document());
            (result, owner, all)
        });
        self.text.state = state;
        let (result, owner, all) = found?;
        self.text.all = all;
        // A selection in a hidden tab stays until another takes its place.
        if other.placed() {
            self.text.state = other;
            self.text.owner = Some(owner);
        }
        Some(result)
    }

    pub(super) fn text_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
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
        let pressed = self.with_text(cx, |frame, state, page| {
            let hit = frame.text_hit(page, at, window);
            state.press(page.document(), hit, press);
        });
        if pressed.is_some() {
            self.text.gesture = Some(Gesture {
                pressed: at,
                at,
                started: self.host().frame_time(cx),
                autoscroll: false,
            });
            cx.notify();
        }
    }

    pub(super) fn text_mouse_move(
        &mut self,
        at: [f64; 2],
        pressed: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !pressed && self.text.gesture.take().is_some() {
            self.text.state.release_lost();
        }
        if self.text.gesture.is_none() {
            let on_text = self
                .page_text(cx)
                .is_some_and(|(_, page)| self.text_hit(page.as_ref(), at, window).text);
            if std::mem::replace(&mut self.text.on_text, on_text) != on_text {
                cx.notify();
            }
            return;
        }
        if let Some(gesture) = &mut self.text.gesture {
            gesture.at = at;
        }
        self.text_drag(window, cx);
    }

    fn text_drag(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(gesture) = self.text.gesture else {
            return;
        };
        let since = self.host().frame_time(cx) - gesture.started;
        let autoscroll = self.with_text(cx, |frame, state, page| {
            let hit = frame.text_hit(page, gesture.at, window);
            state.drag(page.document(), hit.caret, since);
            let [left, top, w, h] = page.scroller();
            let [x, y] = gesture.at;
            let over = x >= left && x < left + w && y >= top && y < top + h;
            state.selecting()
                && (gesture.autoscroll || over && state.range(page.document()).is_some())
        });
        if let (Some(gesture), Some(autoscroll)) = (&mut self.text.gesture, autoscroll) {
            gesture.autoscroll = autoscroll;
        }
        cx.notify();
    }

    pub(super) fn text_mouse_up(
        &mut self,
        button: MouseButton,
        at: [f64; 2],
        cx: &mut Context<'_, Self>,
    ) {
        if !matches!(
            button,
            MouseButton::Left | MouseButton::Middle | MouseButton::Right
        ) {
            return;
        }
        if let Some(gesture) = self.text.gesture.take() {
            self.with_text(cx, |_, state, page| {
                state.release(page.document(), gesture.pressed != at)
            });
            cx.notify();
        }
    }

    /// A frame of `AutoscrollController`'s animation: the pointer's position is selected to, then
    /// scrolled into view.
    fn text_autoscroll(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(gesture) = self.text.gesture.filter(|g| g.autoscroll) else {
            return;
        };
        window.request_animation_frame();
        let Some((owner, scroller)) = self.page_text(cx).map(|(o, p)| (o, p.scroller())) else {
            return;
        };
        if self.text.owner.as_ref() != Some(&owner) {
            return;
        }
        let by = autoscroll_by(scroller, gesture.at, f64::from(window.scale_factor()));
        if by == [0.0, 0.0] {
            return;
        }
        match owner {
            Owner::Table(_) => self.table_scroll_by(by, cx),
            Owner::Console(_) => self.console_scroll_by(by),
            Owner::Statistics(_) => self.statistics_scroll_by(by),
            Owner::Metadata => {}
        }
        self.text_drag(window, cx);
    }

    /// Cmd+C and Cmd+A, or Ctrl+C, Ctrl+Insert and Ctrl+A.
    pub(super) fn text_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        match edit_command(event) {
            Some(EditCommand::Copy) => self.copy_text(cx),
            Some(EditCommand::SelectAll) => self.select_all_text(cx),
            _ => {}
        }
    }

    pub(super) fn copy_text(&mut self, cx: &mut Context<'_, Self>) {
        let mut copied = None;
        self.with_text(cx, |_, state, page| {
            state.sync(page.document());
            copied = state.text(page.document());
        });
        if let Some(text) = copied {
            write_clipboard(&text, cx);
        }
    }

    pub(super) fn select_all_text(&mut self, cx: &mut Context<'_, Self>) {
        self.text.all = true;
        cx.notify();
    }

    /// Forgets the selection of a tab that is gone, whose key another may get.
    pub(super) fn text_retain(&mut self, live: impl FnOnce(&Owner) -> bool) {
        if self.text.owner.as_ref().is_some_and(|owner| !live(owner)) {
            self.text.state.clear();
            self.text.owner = None;
        }
    }

    pub(super) fn text_frame(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let frame = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            // An input that has the focus has the page's selection.
            if input::focused(window) {
                let _ = frame.update(cx, |frame, cx| {
                    if frame.text.state.placed() || frame.text.all {
                        frame.text.state.clear();
                        frame.text.all = false;
                        cx.notify();
                    }
                });
            }
        });
        all::set(self.text.all, window, cx);
        self.text_autoscroll(window, cx);
    }
}

pub(crate) use asrs_ui::platform::EditCommand;

/// The editing command a page gives a keystroke.
pub(crate) fn edit_command(event: &KeyDownEvent) -> Option<EditCommand> {
    let m = event.keystroke.modifiers;
    let (command, other) = if cfg!(target_os = "macos") {
        (m.platform, m.control)
    } else {
        (m.control, m.platform)
    };
    if !command || other || m.shift || m.alt {
        return None;
    }
    match event.keystroke.key.as_str() {
        "c" => Some(EditCommand::Copy),
        "insert" if !cfg!(target_os = "macos") => Some(EditCommand::Copy),
        "a" => Some(EditCommand::SelectAll),
        _ => None,
    }
}

/// `kAutoscrollBeltSize` in device px.
const AUTOSCROLL_BELT: f64 = 20.0;

/// How far a frame of `AutoscrollController`'s animation scrolls a box: a pointer less than a
/// belt from its edge counts as a belt further out, and its pixel is scrolled into view.
pub(crate) fn autoscroll_by([left, top, w, h]: [f64; 4], [x, y]: [f64; 2], scale: f64) -> [f64; 2] {
    let (pixel, belt) = (1.0 / scale, AUTOSCROLL_BELT / scale);
    [(x, left, left + w), (y, top, top + h)].map(|(at, from, to)| {
        let at = if at < from + belt {
            at - belt
        } else if at > to - belt {
            at + belt
        } else {
            at
        };
        if at < from {
            at - from
        } else if at + pixel > to {
            at + pixel - to
        } else {
            0.0
        }
    })
}
