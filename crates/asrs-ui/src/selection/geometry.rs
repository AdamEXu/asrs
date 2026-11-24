// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Where a block's text is: the position at a point and the boxes a selection of it paints.

use std::ops::Range;

use asrs_canvas::text::caret::{carets, space_width};
use asrs_canvas::text::dom::{LU_PER_PX, Lu};
use asrs_canvas::text::{FontSpec, TextBackend};

use super::{Behavior, Caret, Position};

/// One text fragment of a line. Lengths are device `LayoutUnit`s from the block's origin.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    /// The bytes of the block's text.
    pub range: Range<usize>,
    /// The text node it is in.
    pub node: usize,
    pub x: Lu,
    pub width: Lu,
    pub font: FontSpec,
    pub tabs: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub top: Lu,
    pub height: Lu,
    /// The pieces' text boxes.
    pub box_top: Lu,
    pub box_height: Lu,
    pub pieces: Vec<Piece>,
    /// The byte of the `\n` that ends the line.
    pub newline: Option<usize>,
    /// Where an empty line's `<br>` is.
    pub start: Lu,
}

/// A block's lines, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub lines: Vec<Line>,
    /// The font of the block's `<br>`s.
    pub font: FontSpec,
    /// `text-overflow: ellipsis` (`ShouldTruncateOverflowingText`).
    pub truncates: bool,
}

/// `[x, y, width, height]` in device `LayoutUnit`s.
pub type LuRect = [Lu; 4];

/// What [`Block`]s are measured with.
pub struct Measure<'a> {
    pub backend: &'a mut dyn TextBackend,
    pub text: &'a str,
    pub scale: f32,
}

impl Measure<'_> {
    /// The caret positions of a piece: bytes of the block's text with their distance from the
    /// piece's left edge.
    fn carets(&mut self, piece: &Piece) -> Vec<(usize, f32)> {
        let text = &self.text[piece.range.clone()];
        if piece.tabs {
            let tab = 4.0 * space_width(self.backend, &piece.font, self.scale);
            let width = piece.width as f32 / LU_PER_PX as f32;
            let count = text.len();
            return (0..=count)
                .map(|i| {
                    let x = if i == 0 {
                        0.0
                    } else {
                        width - tab * (count - i) as f32
                    };
                    (piece.range.start + i, x)
                })
                .collect();
        }
        carets(self.backend, &piece.font, text, self.scale)
            .into_iter()
            .map(|(byte, x)| (piece.range.start + byte, x))
            .collect()
    }

    fn caret_x(&mut self, piece: &Piece, offset: usize) -> f32 {
        let carets = self.carets(piece);
        let at = carets.partition_point(|&(byte, _)| byte < offset);
        carets.get(at).or(carets.last()).map_or(0.0, |&(_, x)| x)
    }
}

impl Line {
    fn end(&self) -> usize {
        self.pieces
            .last()
            .map_or_else(|| self.newline.unwrap_or(0), |piece| piece.range.end)
    }

    /// `PositionForPointInInlineBox`.
    fn offset_at(&self, x: Lu, measure: &mut Measure<'_>) -> usize {
        let inside = self.pieces.iter().find(|p| p.x <= x && x < p.x + p.width);
        let after = || self.pieces.iter().find(|p| x < p.x);
        let Some(piece) = inside.or_else(after).or(self.pieces.last()) else {
            return self.end();
        };
        let x = (x - piece.x) as f32 / LU_PER_PX as f32;
        let carets = measure.carets(piece);
        let next = carets.windows(2).find(|w| x < (w[0].1 + w[1].1) / 2.0);
        next.map_or(piece.range.end, |w| w[0].0)
    }

    /// Whether the line ends with a wrap at `offset` rather than a forced break or the block.
    fn wraps_at(&self, offset: usize, last: bool) -> bool {
        !last && self.newline.is_none() && offset == self.end()
    }
}

impl Block {
    /// `PositionForPointInInlineFormattingContext` for a point from the block's origin.
    pub fn caret_at(
        &self,
        block: usize,
        [x, y]: [Lu; 2],
        behavior: Behavior,
        measure: &mut Measure<'_>,
    ) -> Caret {
        let caret = |offset| Position { block, offset };
        let below = self.lines.iter().position(|l| y < l.top + l.height);
        let Some(index) = below.or(self.lines.len().checked_sub(1)) else {
            return Caret::new(caret(0));
        };
        let line = &self.lines[index];
        let outside = y < line.top || y >= line.top + line.height;
        let offset = if !outside || !behavior.moves_to_line_edge() {
            line.offset_at(x, measure)
        } else if y < line.top {
            line.pieces.first().map_or(line.end(), |p| p.range.start)
        } else {
            line.end()
        };
        Caret {
            position: caret(offset),
            line_end: line.wraps_at(offset, index + 1 == self.lines.len()),
        }
    }

    /// Whether a point from the block's origin is on its text.
    pub fn on_text(&self, [x, y]: [Lu; 2]) -> bool {
        self.lines.iter().any(|line| {
            (line.box_top..line.box_top + line.box_height).contains(&y)
                && line
                    .pieces
                    .iter()
                    .any(|p| (p.x..p.x + p.width).contains(&x))
        })
    }

    /// The boxes `LayoutSelection` paints for a selection of the block's text from `start`,
    /// `None` when it starts before the block, to `end`, `None` when it ends after it. Each has
    /// the index of the line and, unless it is a `<br>`'s, of the piece it is behind.
    pub fn selection_rects(
        &self,
        start: Option<usize>,
        end: Option<usize>,
        measure: &mut Measure<'_>,
    ) -> Vec<(usize, Option<usize>, LuRect)> {
        let nodes = |offset: usize, downstream: bool| {
            let mut pieces = self.lines.iter().flat_map(|line| &line.pieces);
            let holds = |p: &&Piece| {
                if downstream {
                    p.range.start <= offset && offset < p.range.end
                } else {
                    p.range.start < offset && offset <= p.range.end
                }
            };
            pieces
                .clone()
                .find(holds)
                .or_else(|| pieces.find(|p| (p.range.start..=p.range.end).contains(&offset)))
        };
        let start_node = start.and_then(|s| Some(nodes(s, true)?.node));
        let end_node = end.and_then(|e| Some(nodes(e, false)?.node));
        let from = start.unwrap_or(0);
        let to = end.unwrap_or(usize::MAX);
        let mut out = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            for (p, piece) in line.pieces.iter().enumerate() {
                let Range { start: s, end: e } = piece.range;
                let after_start = start_node.is_none_or(|n| piece.node >= n);
                let before_end = end_node.is_none_or(|n| piece.node <= n);
                if !after_start || !before_end {
                    continue;
                }
                let in_start = start_node == Some(piece.node);
                let in_end = end_node == Some(piece.node);
                let continues = (!in_start || from <= e) && (!in_end || e < to);
                let last = p + 1 == line.pieces.len() && line.newline.is_none();
                let wrap = continues && last && !self.truncates;
                let (a, b) = (from.clamp(s, e), to.clamp(s, e));
                if a >= b {
                    continue;
                }
                let (left, right) = if (a, b) == (s, e) {
                    (piece.x, piece.x + piece.width)
                } else {
                    let left = (measure.caret_x(piece, a) * LU_PER_PX as f32).floor() as Lu;
                    let right = (measure.caret_x(piece, b) * LU_PER_PX as f32).ceil() as Lu;
                    (piece.x + left, piece.x + right)
                };
                let space = if wrap {
                    self.space(&piece.font, measure)
                } else {
                    0
                };
                out.push((index, Some(p), line.rect(left, right - left + space)));
            }
            if let Some(newline) = line.newline
                && from <= newline
                && newline < to
            {
                let left = line.pieces.last().map_or(line.start, |p| p.x + p.width);
                let space = self.space(&self.font, measure);
                out.push((index, None, line.rect(left, space)));
            }
        }
        out
    }

    fn space(&self, font: &FontSpec, measure: &mut Measure<'_>) -> Lu {
        (space_width(measure.backend, font, measure.scale) * LU_PER_PX as f32) as Lu
    }
}

impl Line {
    fn rect(&self, x: Lu, width: Lu) -> LuRect {
        let top = self.top.min(self.box_top);
        let bottom = (self.top + self.height).max(self.box_top + self.box_height);
        [x, top, width, bottom - top]
    }
}

/// What part of block `block` the range covers, as [`Block::selection_rects`] takes it; `None`
/// when it covers none.
pub fn part(range: &Range<Position>, block: usize) -> Option<(Option<usize>, Option<usize>)> {
    if block < range.start.block || block > range.end.block {
        return None;
    }
    Some((
        (block == range.start.block).then_some(range.start.offset),
        (block == range.end.block).then_some(range.end.offset),
    ))
}
