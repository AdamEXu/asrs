//! What a page's selectable text is made of, and the text a range of it copies.

use std::borrow::Cow;
use std::ops::Range;

use super::Position;

/// Where a block stands in its document, as far as Blink's `TextIterator` shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Place {
    /// A table cell.
    pub cell: bool,
    /// A cell other than its table's first one.
    pub tab: bool,
    /// The block, or an element that starts with it, is selectable and not inline.
    pub newline_before: bool,
    /// The block, or an element that ends with it, is not inline.
    pub newline_after: bool,
    /// The way to the next block passes an element that cannot be selected.
    pub closed: bool,
    /// Between the last block and this one there is something else: text that cannot be selected
    /// or an image.
    pub after_other: bool,
    /// The characters between the last block and this one, those of text that cannot be selected
    /// included.
    pub gap: usize,
}

/// The selectable text of a page: its blocks of inline text in document order. A block's text is
/// what it renders, with `\n` for each `<br>`.
pub trait Document {
    fn len(&self) -> usize;

    fn text(&self, block: usize) -> Cow<'_, str>;

    fn place(&self, block: usize) -> Place;

    /// Changes when the page rebuilds its elements, which leaves no text node of the last ones.
    fn generation(&self) -> u64 {
        0
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The end of the document's last block.
pub fn end(doc: &dyn Document) -> Option<Position> {
    let block = doc.len().checked_sub(1)?;
    Some(Position {
        block,
        offset: doc.text(block).len(),
    })
}

/// `FrameSelection::SelectedTextForClipboard` for `start..end`.
pub fn extract(doc: &dyn Document, range: &Range<Position>) -> String {
    let (start, end) = (range.start, range.end);
    let mut out = String::new();
    for block in start.block..=end.block {
        let place = doc.place(block);
        if block != start.block && !out.ends_with('\n') {
            if place.tab {
                out.push('\t');
            } else if place.newline_before {
                out.push('\n');
            }
        }
        let text = doc.text(block);
        let from = if block == start.block { start.offset } else { 0 };
        let to = if block == end.block {
            end.offset
        } else {
            text.len()
        };
        out.push_str(&text[from..to]);
        if block != end.block && place.newline_after && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// What `SystemClipboard` writes of the selected text.
pub fn clipboard_text(text: &str) -> String {
    let text = text.replace('\u{a0}', " ").replace('\0', "");
    if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text
    }
}

/// `TextIterator::RangeLength` over every visible position.
pub fn distance(doc: &dyn Document, from: Position, to: Position) -> usize {
    let units = |text: &str| text.encode_utf16().count();
    if from.block == to.block {
        return units(&doc.text(from.block)[from.offset..to.offset]);
    }
    let first = units(&doc.text(from.block)[from.offset..]);
    let last = units(&doc.text(to.block)[..to.offset]);
    let gaps: usize = (from.block + 1..=to.block).map(|b| doc.place(b).gap).sum();
    let between: usize = (from.block + 1..to.block)
        .map(|b| units(&doc.text(b)))
        .sum();
    first + gaps + between + last
}
