// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Words and paragraphs around a position (`visible_units*.cc`, `selection_adjuster.cc`).

use asrs_canvas::text::caret::word_at;

use super::document::{Document, end};
use super::{Caret, Granularity, Position};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Previous,
    Next,
}

fn at(block: usize, offset: usize) -> Position {
    Position { block, offset }
}

fn next_boundary(text: &str, offset: usize) -> usize {
    let mut chars = text[offset..].chars();
    chars.next().map_or(offset, |c| offset + c.len_utf8())
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    let last = text[..offset].chars().next_back();
    last.map_or(offset, |c| offset - c.len_utf8())
}

/// `NextPositionOf`.
fn next(doc: &dyn Document, position: Position) -> Option<Position> {
    let text = doc.text(position.block);
    if position.offset < text.len() {
        return Some(at(position.block, next_boundary(&text, position.offset)));
    }
    (position.block + 1 < doc.len()).then(|| at(position.block + 1, 0))
}

/// `PreviousPositionOf`.
fn previous(doc: &dyn Document, position: Position) -> Option<Position> {
    if position.offset > 0 {
        let text = doc.text(position.block);
        return Some(at(position.block, previous_boundary(&text, position.offset)));
    }
    let block = position.block.checked_sub(1)?;
    Some(at(block, doc.text(block).len()))
}

fn paragraph_start(doc: &dyn Document, position: Position) -> Position {
    let text = doc.text(position.block);
    let start = text[..position.offset].rfind('\n').map_or(0, |n| n + 1);
    at(position.block, start)
}

fn paragraph_end(doc: &dyn Document, position: Position) -> Position {
    let text = doc.text(position.block);
    let rest = text[position.offset..].find('\n');
    at(
        position.block,
        rest.map_or(text.len(), |n| position.offset + n),
    )
}

fn is_paragraph_end(doc: &dyn Document, position: Position) -> bool {
    paragraph_end(doc, position) == position
}

/// `ChooseWordSide`.
fn word_side(doc: &dyn Document, caret: Caret) -> Side {
    if end(doc) == Some(caret.position) || caret.line_end {
        Side::Previous
    } else {
        Side::Next
    }
}

fn word_start(doc: &dyn Document, caret: Caret) -> Position {
    let Position { block, offset } = caret.position;
    let text = doc.text(block);
    let character = match word_side(doc, caret) {
        Side::Next if offset == text.len() => return caret.position,
        Side::Next => offset,
        Side::Previous if offset == 0 => return caret.position,
        Side::Previous => previous_boundary(&text, offset),
    };
    at(block, word_at(&text, character).start)
}

fn word_end(doc: &dyn Document, caret: Caret) -> Position {
    let Position { block, offset } = caret.position;
    let text = doc.text(block);
    let character = match word_side(doc, caret) {
        Side::Next if offset == text.len() => return caret.position,
        Side::Next => offset,
        Side::Previous if offset == 0 => return caret.position,
        Side::Previous => previous_boundary(&text, offset),
    };
    at(block, word_at(&text, character).end)
}

/// `ComputeStartRespectingGranularity`.
pub fn start_of(doc: &dyn Document, caret: Caret, granularity: Granularity) -> Position {
    let position = caret.position;
    match granularity {
        Granularity::Character => position,
        Granularity::Word => word_start(doc, caret),
        Granularity::Paragraph => {
            let start = paragraph_start(doc, position);
            match previous(doc, position) {
                Some(before) if start == position && end(doc) == Some(position) => {
                    paragraph_start(doc, before)
                }
                _ => start,
            }
        }
    }
}

/// `ComputeEndRespectingGranularity`.
pub fn end_of(doc: &dyn Document, caret: Caret, granularity: Granularity) -> Position {
    let position = caret.position;
    match granularity {
        Granularity::Character => position,
        Granularity::Word => {
            if !is_paragraph_end(doc, position) {
                return word_end(doc, caret);
            }
            if doc.place(position.block).cell {
                return position;
            }
            next(doc, position).unwrap_or(position)
        }
        Granularity::Paragraph => {
            let end = paragraph_end(doc, position);
            next(doc, end).unwrap_or(end)
        }
    }
}

/// `ComputeStartFromEndForExtendForward`.
pub fn start_from_end(doc: &dyn Document, end: Position, granularity: Granularity) -> Position {
    if granularity == Granularity::Character {
        return end;
    }
    let before = previous(doc, end).unwrap_or(end);
    start_of(doc, Caret::new(before), granularity)
}

/// `SkipWhitespace`, which keeps to the block.
pub fn skip_whitespace(doc: &dyn Document, position: Position) -> Position {
    let text = doc.text(position.block);
    let rest = &text[position.offset..];
    let end = rest
        .find(|c: char| c == '\n' || !c.is_whitespace())
        .unwrap_or(rest.len());
    at(position.block, position.offset + end)
}

/// What `AdjustSelectionByUserSelect` leaves of a paragraph that starts in block `block`: it does
/// not reach over an element that cannot be selected.
pub fn within_selectable(doc: &dyn Document, block: usize, end: Position) -> Position {
    if end.block > block && doc.place(block).closed {
        at(block, doc.text(block).len())
    } else {
        end
    }
}
