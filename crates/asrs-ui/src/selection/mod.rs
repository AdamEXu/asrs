// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Selecting and copying a page's text: Blink's `SelectionController` over the blocks of a
//! [`Document`], which a view implements for the text its page holds. [`geometry`] finds the
//! position at a point and the boxes a selection paints.

pub mod colors;
pub mod document;
pub mod geometry;
mod units;

use std::ops::Range;

use gpui_kit::{App, ClipboardItem, actions};

pub use document::{Document, Place};

actions!(text_selection, [Copy, SelectAll]);

/// Between two characters: `offset` bytes into block `block`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub block: usize,
    pub offset: usize,
}

/// A position with its affinity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub position: Position,
    /// At the end of a line that wraps here, rather than at the start of the next one.
    pub line_end: bool,
}

impl Caret {
    pub fn new(position: Position) -> Caret {
        Caret {
            position,
            line_end: false,
        }
    }
}

/// What is under a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The closest position, if the document has any.
    pub caret: Option<Caret>,
    /// The element can be selected.
    pub selectable: bool,
    /// The element is text that can be selected.
    pub text: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Granularity {
    #[default]
    Character,
    Word,
    Paragraph,
}

/// Blink's `EditingBehavior`, which differs per platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    Mac,
    Windows,
    Unix,
}

impl Behavior {
    pub const fn current() -> Behavior {
        if cfg!(target_os = "macos") {
            Behavior::Mac
        } else if cfg!(windows) {
            Behavior::Windows
        } else {
            Behavior::Unix
        }
    }

    fn directional(self) -> bool {
        self != Behavior::Mac
    }

    /// `kTextDragDelay` in ms.
    fn text_drag_delay(self) -> f64 {
        if self == Behavior::Mac { 150.0 } else { 0.0 }
    }

    fn selects_word_for_menu(self) -> bool {
        self == Behavior::Mac
    }

    fn selects_trailing_whitespace(self) -> bool {
        self == Behavior::Windows
    }

    /// `ShouldMoveCaretToHorizontalBoundaryWhenPastTopOrBottom`.
    pub fn moves_to_line_edge(self) -> bool {
        self != Behavior::Windows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// Writes selected text to the clipboard.
pub fn write_clipboard(text: &str, cx: &mut App) {
    let text = document::clipboard_text(text);
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub button: Button,
    pub click_count: usize,
    pub shift: bool,
}

/// An endpoint and the text of its block when it was set: a block whose text is replaced takes
/// the endpoint to its start, where Blink's goes when the text node it was in is removed.
#[derive(Clone, Debug)]
struct Endpoint {
    position: Position,
    text: String,
}

impl Endpoint {
    fn new(doc: &dyn Document, position: Position) -> Endpoint {
        let text = if position.block < doc.len() {
            doc.text(position.block).into_owned()
        } else {
            String::new()
        };
        Endpoint { position, text }
    }

    fn sync(&mut self, doc: &dyn Document) {
        if self.position.block >= doc.len() {
            return;
        }
        let text = doc.text(self.position.block);
        if self.text != text {
            self.position.offset = 0;
            self.text = text.into_owned();
        }
    }

    /// Where the endpoint shows: past the last block, at its end.
    fn visible(&self, doc: &dyn Document) -> Option<Position> {
        let end = document::end(doc)?;
        Some(self.position.min(end))
    }
}

/// `SelectionController::selection_state_`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    NotStarted,
    PlacedCaret,
    Extended,
}

/// What moving the pointer does after a press on the selection, which may drag its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextDrag {
    /// The press was not on the selection.
    No,
    Undecided,
    Started,
    TooSoon,
}

#[derive(Clone, Copy, Debug)]
struct Mouse {
    button: Button,
    hit: Hit,
    text_drag: TextDrag,
}

/// A document's selection and the mouse gesture that is changing it.
#[derive(Clone, Debug)]
pub struct TextSelection {
    behavior: Behavior,
    ends: Option<[Endpoint; 2]>,
    generation: u64,
    granularity: Granularity,
    state: State,
    mouse: Option<Mouse>,
}

impl Default for TextSelection {
    fn default() -> Self {
        TextSelection::new(Behavior::current())
    }
}

impl TextSelection {
    pub fn new(behavior: Behavior) -> TextSelection {
        TextSelection {
            behavior,
            ends: None,
            generation: 0,
            granularity: Granularity::Character,
            state: State::NotStarted,
            mouse: None,
        }
    }

    /// Whether there is a selection, be it a caret.
    pub fn placed(&self) -> bool {
        self.ends.is_some()
    }

    pub fn clear(&mut self) {
        self.ends = None;
        self.state = State::NotStarted;
    }

    /// Whether a press is one that selects while it is dragged.
    pub fn selecting(&self) -> bool {
        self.mouse.is_some_and(|m| {
            m.button == Button::Left && m.hit.selectable && m.text_drag == TextDrag::No
        })
    }

    /// Takes the endpoints along with what the page did to its text.
    pub fn sync(&mut self, doc: &dyn Document) {
        let generation = doc.generation();
        let Some(ends) = &mut self.ends else {
            self.generation = generation;
            return;
        };
        if generation != std::mem::replace(&mut self.generation, generation) {
            let start = Endpoint::new(doc, Position::default());
            *ends = [start.clone(), start];
            return;
        }
        for end in ends {
            end.sync(doc);
        }
    }

    /// The anchor and the focus.
    pub fn ends(&self, doc: &dyn Document) -> Option<[Position; 2]> {
        let [anchor, focus] = self.ends.as_ref()?;
        Some([anchor.visible(doc)?, focus.visible(doc)?])
    }

    /// The selected range, if it is not empty.
    pub fn range(&self, doc: &dyn Document) -> Option<Range<Position>> {
        let [anchor, focus] = self.ends(doc)?;
        (anchor != focus).then(|| anchor.min(focus)..anchor.max(focus))
    }

    /// The range that is painted. Blink's end at the start of a block is before its text, so the
    /// last text in the range is what comes before the block.
    pub fn painted(&self, doc: &dyn Document) -> Option<Range<Position>> {
        let mut range = self.range(doc)?;
        while range.end.offset == 0
            && range.end.block > range.start.block
            && !doc.place(range.end.block).after_other
        {
            let block = range.end.block - 1;
            let offset = doc.text(block).len();
            range.end = Position { block, offset };
        }
        (range.start < range.end).then_some(range)
    }

    /// The text a copy takes.
    pub fn text(&self, doc: &dyn Document) -> Option<String> {
        Some(document::extract(doc, &self.range(doc)?))
    }

    fn set(&mut self, doc: &dyn Document, anchor: Position, focus: Position) {
        self.generation = doc.generation();
        self.ends = Some([anchor, focus].map(|p| Endpoint::new(doc, p)));
        self.state = if anchor == focus {
            State::PlacedCaret
        } else {
            State::Extended
        };
    }

    pub fn select_all(&mut self, doc: &dyn Document) {
        let Some(end) = document::end(doc) else {
            return self.clear();
        };
        self.granularity = Granularity::Character;
        self.set(doc, Position::default(), end);
    }

    /// `ExtendSelectionAsDirectional`.
    fn directional(
        &self,
        doc: &dyn Document,
        caret: Caret,
        [anchor, focus]: [Position; 2],
    ) -> [Position; 2] {
        let granularity = self.granularity;
        let anchor_first = anchor <= focus;
        if caret.position < anchor {
            let start = units::start_of(doc, caret, granularity);
            let end = if anchor_first {
                units::end_of(doc, Caret::new(anchor), granularity)
            } else {
                anchor
            };
            return [end, start];
        }
        let start = if anchor_first {
            anchor
        } else {
            units::start_from_end(doc, anchor, granularity)
        };
        [start, units::end_of(doc, caret, granularity)]
    }

    /// `ExtendSelectionAsNonDirectional`.
    fn non_directional(
        &self,
        doc: &dyn Document,
        caret: Caret,
        [anchor, focus]: [Position; 2],
    ) -> [Position; 2] {
        let position = caret.position;
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        if start == end && position == start {
            return [anchor, focus];
        }
        let nearer_start = || {
            let to_start = document::distance(doc, start, position);
            to_start <= document::distance(doc, position, end)
        };
        if position < start || (position <= end && nearer_start()) {
            [end, units::start_of(doc, caret, self.granularity)]
        } else {
            [start, units::end_of(doc, caret, self.granularity)]
        }
    }

    /// `FrameSelection::Contains`.
    fn contains(&self, doc: &dyn Document, hit: &Hit) -> bool {
        match (self.range(doc), hit.caret) {
            (Some(range), Some(caret)) => {
                (range.start..=range.end).contains(&caret.position)
            }
            _ => false,
        }
    }

    /// `SelectClosestWordFromHitTestResult`.
    fn select_word(&mut self, doc: &dyn Document, caret: Caret, click_count: usize) {
        let start = units::start_of(doc, caret, Granularity::Word);
        let mut end = units::end_of(doc, caret, Granularity::Word);
        if click_count == 2 && self.behavior.selects_trailing_whitespace() && start != end {
            end = units::skip_whitespace(doc, end);
        }
        self.granularity = Granularity::Word;
        self.set(doc, start, end);
    }

    /// `HandleMousePressEvent`, and for the right button the context menu event that follows it.
    pub fn press(&mut self, doc: &dyn Document, hit: Hit, press: Press) {
        self.sync(doc);
        self.state = State::NotStarted;
        self.mouse = Some(Mouse {
            button: press.button,
            hit,
            text_drag: TextDrag::No,
        });
        match (hit.selectable.then_some(hit.caret).flatten(), press.button) {
            (None, _) => {}
            (Some(caret), _) if press.click_count <= 1 => self.single(doc, hit, caret, press.shift),
            (Some(caret), Button::Left) if press.click_count == 2 => {
                if self.range(doc).is_some() {
                    self.state = State::Extended;
                } else {
                    self.select_word(doc, caret, 2);
                }
            }
            (Some(caret), Button::Left) => {
                let start = units::start_of(doc, caret, Granularity::Paragraph);
                let end = units::end_of(doc, caret, Granularity::Paragraph);
                let end = units::within_selectable(doc, caret.position.block, end);
                self.granularity = Granularity::Paragraph;
                self.set(doc, start, end);
            }
            _ => {}
        }
        if press.button == Button::Right {
            self.context_menu(doc, hit);
        }
    }

    /// `HandleSingleClick`.
    fn single(&mut self, doc: &dyn Document, hit: Hit, caret: Caret, shift: bool) {
        if !shift && self.contains(doc, &hit) {
            if let Some(mouse) = &mut self.mouse {
                mouse.text_drag = TextDrag::Undecided;
            }
            return;
        }
        if let (true, Some(ends)) = (shift, self.ends(doc)) {
            let [anchor, focus] = if self.behavior.directional() {
                self.directional(doc, caret, ends)
            } else {
                self.non_directional(doc, caret, ends)
            };
            self.set(doc, anchor, focus);
            return;
        }
        self.granularity = Granularity::Character;
        self.set(doc, caret.position, caret.position);
    }

    /// `UpdateSelectionForContextMenuEvent`.
    fn context_menu(&mut self, doc: &dyn Document, hit: Hit) {
        if let Some(caret) = hit.caret.filter(|_| {
            hit.text && self.behavior.selects_word_for_menu() && !self.contains(doc, &hit)
        }) {
            self.select_word(doc, caret, 1);
        }
    }

    /// The pointer moved, `since_press` ms after the press, while the button is down:
    /// `HandleMouseDraggedEvent`.
    pub fn drag(&mut self, doc: &dyn Document, caret: Option<Caret>, since_press: f64) {
        self.sync(doc);
        let delay = self.behavior.text_drag_delay();
        let Some(mouse) = self
            .mouse
            .as_mut()
            .filter(|m| m.button == Button::Left && m.hit.selectable)
        else {
            return;
        };
        if mouse.text_drag == TextDrag::Undecided {
            mouse.text_drag = if since_press < delay {
                TextDrag::TooSoon
            } else {
                TextDrag::Started
            };
        }
        let (text_drag, down) = (mouse.text_drag, mouse.hit.caret);
        let (Some(caret), Some(ends)) = (caret, self.ends(doc)) else {
            return;
        };
        match text_drag {
            TextDrag::Started | TextDrag::Undecided => {}
            // Blink restarts the selection at the press and takes the caret that leaves for the
            // end of the gesture, so it only ever follows the pointer.
            TextDrag::TooSoon => self.set(doc, caret.position, caret.position),
            TextDrag::No => {
                let down = down.map(|down| [down.position; 2]);
                if self.state != State::Extended && down != Some(ends) {
                    self.set(doc, caret.position, caret.position);
                    return;
                }
                let [anchor, focus] = self.directional(doc, caret, ends);
                self.set(doc, anchor, focus);
                self.state = State::Extended;
            }
        }
    }

    /// `HandleMouseReleaseEvent`. `moved` is whether the pointer is elsewhere than at the press.
    pub fn release(&mut self, doc: &dyn Document, moved: bool) {
        self.sync(doc);
        let Some(mouse) = self.mouse.take() else {
            return;
        };
        if mouse.text_drag == TextDrag::Undecided
            && mouse.button != Button::Right
            && !moved
            && self.state != State::Extended
            && self.range(doc).is_some()
        {
            self.clear();
        }
    }
}

