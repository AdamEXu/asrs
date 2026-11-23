// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Inline layout of the text nodes of one block, all in the block's font: lines break over the
//! nodes' joined text, and each node's part of a line is a piece of its own, as Blink makes it an
//! inline item of its own. `white-space` is `normal` (collapsed) or `break-spaces`, where every
//! space takes room and may end a line. Text is left to right.

use super::TextBackend;
use super::dom::{
    LineHeight, Lu, Style, align_offset, boundaries, extend_with_fallbacks, line_leading, line_metrics,
    lu_ceil, utf16_len,
};
use super::layout::normalize_space;
use super::line_break::breaks;

/// A text node's part of a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub item: usize,
    pub text: String,
    /// From the content box's left edge.
    pub x: Lu,
    pub width: Lu,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// From the content box's top edge.
    pub top: Lu,
    pub height: Lu,
    /// The top of the pieces' boxes, which are [`Flow::box_height`] tall.
    pub box_top: Lu,
    pub width: Lu,
    pub pieces: Vec<Piece>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flow {
    pub lines: Vec<Line>,
    /// The widest line.
    pub width: Lu,
    pub height: Lu,
    pub box_height: Lu,
}

/// The nodes' text joined, with where each node's part ends.
struct Joined {
    text: String,
    ends: Vec<usize>,
}

impl Joined {
    fn collapsed(items: &[&str]) -> Joined {
        let mut joined = Joined {
            text: String::new(),
            ends: Vec::new(),
        };
        for item in items {
            for c in item.chars() {
                let space = matches!(c, ' ' | '\t' | '\n' | '\r');
                if !space {
                    joined.text.push(c);
                } else if !joined.text.is_empty() && !joined.text.ends_with(' ') {
                    joined.text.push(' ');
                }
            }
            joined.ends.push(joined.text.len());
        }
        if joined.text.ends_with(' ') {
            joined.text.pop();
            let len = joined.text.len();
            for end in &mut joined.ends {
                *end = (*end).min(len);
            }
        }
        joined
    }

    fn preserved(items: &[&str]) -> Joined {
        let mut joined = Joined {
            text: String::new(),
            ends: Vec::new(),
        };
        for item in items {
            joined.text.push_str(item);
            joined.ends.push(joined.text.len());
        }
        joined
    }

    /// The nodes' parts of `from..to`.
    fn parts(&self, from: usize, to: usize) -> impl Iterator<Item = (usize, usize, usize)> {
        let first = self.ends.partition_point(|&end| end <= from);
        (first..self.ends.len())
            .map(move |item| {
                let start = if item == 0 { 0 } else { self.ends[item - 1] };
                (item, start.max(from), self.ends[item].min(to))
            })
            .take_while(move |&(_, start, _)| start < to)
            .filter(|(_, start, end)| start < end)
    }
}

/// `items` in a content box `available` wide, or on one line when `None`. `break_spaces` is
/// `white-space: break-spaces`, whose items hold no newlines or tabs.
pub fn layout(
    backend: &mut dyn TextBackend,
    style: &Style,
    items: &[&str],
    break_spaces: bool,
    available: Option<Lu>,
    scale: f32,
) -> Flow {
    let (ascent, descent, line_height) = line_metrics(backend, style, scale);
    let leading = line_leading(line_height, ascent, descent);
    let strut = (leading + ascent, line_height - leading - ascent);
    let joined = if break_spaces {
        Joined::preserved(items)
    } else {
        Joined::collapsed(items)
    };
    let mut flow = Flow {
        lines: Vec::new(),
        width: 0,
        height: 0,
        box_height: ascent + descent,
    };
    let content = |next: usize| {
        if break_spaces {
            next
        } else {
            joined.text[..next].trim_end_matches(' ').len()
        }
    };
    for range in line_ranges(backend, style, &joined, break_spaces, available, scale) {
        let mut line = Line {
            top: flow.height,
            height: 0,
            box_top: 0,
            width: 0,
            pieces: Vec::new(),
        };
        let (mut above, mut below) = strut;
        let end = content(range.end);
        let text = &joined.text[range.start..end];
        let shaped = backend.shape_run(&style.font, &normalize_space(text), scale);
        if style.line_height == LineHeight::Normal {
            (above, below) = extend_with_fallbacks(backend, &shaped.glyphs, scale, (above, below));
        }
        let at = boundaries(&shaped, utf16_len(text));
        for (item, from, to) in joined.parts(range.start, end) {
            let text = &joined.text[from..to];
            let first = utf16_len(&joined.text[range.start..from]);
            let width = lu_ceil((at[first + utf16_len(text)] - at[first]) * scale);
            line.pieces.push(Piece {
                item,
                text: text.to_owned(),
                x: line.width,
                width,
            });
            line.width += width;
        }
        let room = available.unwrap_or(line.width);
        let offset = align_offset(style.align, style.direction, room - line.width);
        for piece in &mut line.pieces {
            piece.x += offset;
        }
        line.height = above + below;
        line.box_top = line.top + above - ascent;
        flow.height += line.height;
        flow.width = flow.width.max(line.width);
        flow.lines.push(line);
    }
    flow
}

/// Greedy line breaking. A word wider than the line gets one of its own, or under
/// `overflow-wrap: break-word` as many as it takes.
fn line_ranges(
    backend: &mut dyn TextBackend,
    style: &Style,
    joined: &Joined,
    break_spaces: bool,
    available: Option<Lu>,
    scale: f32,
) -> Vec<std::ops::Range<usize>> {
    let text = joined.text.as_str();
    let len = text.len();
    let Some(available) = available.filter(|_| len > 0) else {
        return (len > 0).then_some(0..len).into_iter().collect();
    };
    let shaped = backend.shape_run(&style.font, &normalize_space(text), scale);
    let at = boundaries(&shaped, utf16_len(text));
    let mut units = vec![0usize; len + 1];
    let mut count = 0;
    for (byte, c) in text.char_indices() {
        units[byte] = count;
        count += c.len_utf16();
    }
    units[len] = count;
    let mut cluster = vec![false; count + 1];
    for glyph in &shaped.glyphs {
        if let Some(start) = cluster.get_mut(glyph.cluster as usize) {
            *start = true;
        }
    }
    let content = |next: usize| {
        if break_spaces {
            next
        } else {
            text[..next].trim_end_matches(' ').len()
        }
    };
    let fits = |from: usize, to: usize| {
        let widths = joined
            .parts(from, to)
            .map(|(_, a, b)| lu_ceil((at[units[b]] - at[units[a]]) * scale));
        widths.sum::<Lu>() <= available
    };
    let mut lines = Vec::new();
    let mut start = 0;
    while start < len {
        let mut end = None;
        let mut first = len;
        for (i, next) in breaks(&text[start..])
            .map(|next| start + next)
            .chain([len])
            .enumerate()
        {
            if i == 0 {
                first = next;
            }
            if !fits(start, content(next)) {
                break;
            }
            end = Some(next);
        }
        let end = end.unwrap_or_else(|| {
            let word = content(first);
            if !style.break_word {
                return first;
            }
            let mut offsets = text[start..word]
                .char_indices()
                .map(|(byte, _)| start + byte)
                .skip(1)
                .filter(|&byte| cluster[units[byte]]);
            let shortest = offsets.next().unwrap_or(word);
            let end = offsets.take_while(|&byte| fits(start, byte)).last();
            match end.unwrap_or(shortest) {
                end if end == word => first,
                end => end,
            }
        });
        lines.push(start..end);
        start = end;
    }
    lines
}
