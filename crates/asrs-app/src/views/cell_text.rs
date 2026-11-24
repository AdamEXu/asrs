//! Text with line breaks in a table cell, as `innerText` leaves it: a text node per line of the
//! string with a `<br>` after each but the last.

use std::ops::Range;

use asrs_canvas::text::TextBackend;
use asrs_canvas::text::dom::{self, Lu, WhiteSpace};
use asrs_ui::selection::geometry::{Block, Line, Piece};
use asrs_ui::text::TextStyle;

#[derive(Clone, Debug, PartialEq)]
pub struct CellLine {
    pub text: String,
    /// The text's bytes in [`CellText::text`].
    pub range: Range<usize>,
    /// The text node it is in.
    pub node: usize,
    /// Where the text starts, from the content box's left edge.
    pub left: Lu,
    pub top: Lu,
    pub height: Lu,
    pub width: Lu,
    /// The text box's top, from the content box's.
    pub box_top: Lu,
    pub box_height: Lu,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellText {
    pub text: String,
    pub lines: Vec<CellLine>,
    /// Lines with only a `<br>`: its byte in the text, and the line's top.
    blank: Vec<(usize, Lu)>,
    strut: Lu,
    pub width: Lu,
    pub height: Lu,
}

pub const fn px(whole: i32) -> Lu {
    whole * dom::LU_PER_PX
}

/// A CSS length in device `LayoutUnit`s.
pub fn length(css: f64, scale: f32) -> Lu {
    (css as f32 * scale * 64.0) as Lu
}

/// The used width of a `1px` border.
pub fn border(scale: f32) -> Lu {
    px(scale.floor().max(1.0) as i32)
}

pub fn css(lu: Lu, scale: f32) -> f64 {
    f64::from(lu) / 64.0 / f64::from(scale)
}

/// Where `piece` is in `text` from byte `from` on, past the spaces a line break takes.
pub fn following(text: &str, from: usize, piece: &str) -> Range<usize> {
    let spaces = text[from..].find(|c| c != ' ').unwrap_or(0);
    let start = if text[from..].starts_with(piece) {
        from
    } else {
        from + spaces
    };
    start..(start + piece.len()).min(text.len())
}

impl CellText {
    /// Lays `text` out in a content box `available` wide, or at its max-content width.
    pub fn new(
        backend: &mut dyn TextBackend,
        style: &TextStyle,
        text: &str,
        available: Option<Lu>,
        scale: f32,
    ) -> CellText {
        let mut paragraphs: Vec<&str> = text.split('\n').collect();
        let blank = |p: &&str| dom::collapse(p, WhiteSpace::Normal)[0].is_empty();
        if paragraphs.last().is_some_and(blank) {
            paragraphs.pop();
        }
        let (ascent, descent, strut) = dom::line_metrics(backend, style, scale);
        let mut out = CellText {
            strut,
            ..CellText::default()
        };
        for (node, paragraph) in paragraphs.into_iter().enumerate() {
            if node > 0 {
                out.text.push('\n');
            }
            let layout = dom::layout(backend, style, paragraph, available, scale);
            out.width = out.width.max(layout.width);
            let mut from = out.text.len();
            out.text
                .push_str(&dom::collapse(paragraph, WhiteSpace::Normal)[0]);
            if layout.lines.is_empty() {
                out.blank.push((from, out.height));
            }
            for line in &layout.lines {
                let text: String = line.fragments.iter().map(|f| f.text.as_str()).collect();
                let range = following(&out.text, from, &text);
                from = range.end;
                out.lines.push(CellLine {
                    text,
                    range,
                    node,
                    left: line.fragments.iter().map(|f| f.x).min().unwrap_or(0),
                    top: out.height + line.top,
                    height: line.height,
                    width: line.width,
                    box_top: out.height + line.baseline - ascent,
                    box_height: ascent + descent,
                });
            }
            out.height += if layout.lines.is_empty() {
                strut
            } else {
                layout.height
            };
        }
        out
    }

    /// The text's lines, for selecting.
    pub fn block(&self, style: &TextStyle) -> Block {
        let mut lines: Vec<Line> = self
            .lines
            .iter()
            .map(|line| Line {
                top: line.top,
                height: line.height,
                box_top: line.box_top,
                box_height: line.box_height,
                pieces: vec![Piece {
                    range: line.range.clone(),
                    node: line.node,
                    x: line.left,
                    width: line.width,
                    font: style.font,
                    tabs: false,
                }],
                newline: (self.text.as_bytes().get(line.range.end) == Some(&b'\n'))
                    .then_some(line.range.end),
                start: line.left,
            })
            .collect();
        for &(newline, top) in &self.blank {
            lines.push(Line {
                top,
                height: self.strut,
                box_top: top,
                box_height: self.strut,
                pieces: Vec::new(),
                newline: Some(newline),
                start: 0,
            });
        }
        lines.sort_by_key(|line| line.top);
        Block {
            lines,
            font: style.font,
        }
    }

    /// The width of the widest part of `text` that cannot be broken.
    pub fn min_content(
        backend: &mut dyn TextBackend,
        style: &TextStyle,
        text: &str,
        scale: f32,
    ) -> Lu {
        let mut style = *style;
        style.break_word = false;
        CellText::new(backend, &style, text, Some(0), scale).width
    }
}
