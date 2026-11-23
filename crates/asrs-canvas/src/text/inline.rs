// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Inline layout of styled text in a block with `white-space: break-spaces`,
//! `word-break: break-all` and `overflow-wrap: break-word`, as Blink's line breaker lays it out.
//! Units are those of [`super::dom`].

use std::collections::HashMap;
use std::ops::Range;

use super::dom::{Lu, boundaries, lu_ceil, utf16_len};
use super::font::FontKey;
use super::layout::normalize_space;
use super::{FontSpec, ShapedGlyph, TextBackend};

/// A run of one inline box's text, which may hold tabs and newlines, or a `<br>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item<'a> {
    Text { text: &'a str, font: FontSpec },
    Break,
}

/// One text fragment of a line: the bytes `range` of item `item`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub item: usize,
    pub range: Range<usize>,
    pub x: Lu,
    pub width: Lu,
    /// Tabs draw nothing.
    pub tabs: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub pieces: Vec<Piece>,
    pub width: Lu,
}

/// Blink's `LineBreakType::kBreakAll` opportunities between two characters of these classes, as
/// Chromium gives them for `word-break: break-all` with `white-space: break-spaces`. The classes
/// are those of [`class`].
const OPPORTUNITIES: [&[u8; 20]; 20] = [
    b"11111111111111111111",
    b"00011101010111111010",
    b"00000001000010000000",
    b"00011001010111101010",
    b"00000100000000010010",
    b"00011101010111111010",
    b"00000000000000000000",
    b"00000000000000000000",
    b"00011001010111111010",
    b"00011001010111110010",
    b"00010001010110111010",
    b"00110111010111111110",
    b"00011001010111101010",
    b"00000101000010010010",
    b"00011001010111111110",
    b"00011101010111111010",
    b"00011001010011001010",
    b"00000000000000000000",
    b"00011101010011011000",
    b"00011101010011011010",
];

/// Whether a tab may start a line after each class.
const TAB_AFTER: &[u8; 20] = b"11010100111010110000";

fn class(c: char) -> usize {
    match c {
        ' ' | '\t' => 0,
        '!' | '?' | '\u{3001}' | '\u{3002}' | '\u{ff01}' | '\u{ff0c}' | '\u{ff0e}' | '\u{ff1f}' => {
            1
        }
        '"' | '\u{ab}' | '\u{bb}' | '\u{2018}'..='\u{201f}' => 2,
        '$' | '\u{a3}' | '\u{a5}' | '\u{b1}' | '\u{20ac}' => 4,
        '%' => 5,
        '\'' => 6,
        '(' | '[' | '{' => 7,
        ')' | '/' | ']' | '}' => 8,
        '+' => 9,
        ',' | '.' | ':' | ';' => 10,
        '-' => 11,
        '<' => 12,
        '\\' => 13,
        '|' => 14,
        '\u{a2}' | '\u{b0}' | '\u{2030}'..='\u{2037}' | '\u{2103}' => 15,
        '\u{2e80}'..='\u{9fff}'
        | '\u{ac00}'..='\u{d7a3}'
        | '\u{f900}'..='\u{faff}'
        | '\u{1f000}'..='\u{3ffff}' => 16,
        '\u{a0}' | '\u{202f}' | '\u{2060}' => 17,
        '\u{2014}' => 18,
        '\u{2025}' | '\u{2026}' => 19,
        _ => 3,
    }
}

fn may_break(before: char, after: char) -> bool {
    let before = class(before);
    if after == '\t' {
        return TAB_AFTER[before] == b'1';
    }
    OPPORTUNITIES[before][class(after)] == b'1'
}

/// A stretch of an item that Blink makes one inline item: text without tabs or newlines, or tabs.
struct Segment {
    item: usize,
    font: FontSpec,
    tabs: bool,
    /// Each cluster's byte offset in the item, first character and advance in CSS px, which a
    /// tab only has once it is placed.
    clusters: Vec<(usize, char, f32)>,
    end: usize,
}

/// Line breaking state that outlives one block: the fonts known to give every printable ASCII
/// character the same advance, so their text needs no shaping to be measured.
#[derive(Default)]
pub struct Breaker {
    advances: HashMap<(FontKey, u32), Option<f32>>,
}

const ASCII: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

impl Breaker {
    /// The advance every printable ASCII character has in `font`, if they share one.
    fn ascii_advance(
        &mut self,
        backend: &mut dyn TextBackend,
        font: &FontSpec,
        scale: f32,
    ) -> Option<f32> {
        *self
            .advances
            .entry((font.key(), scale.to_bits()))
            .or_insert_with(|| {
                let at = boundaries(&backend.shape(font, ASCII, scale), ASCII.len());
                let advance = at[1] - at[0];
                let uniform = at
                    .windows(2)
                    .all(|w| ((w[1] - w[0]) - advance).abs() < 1e-3);
                uniform.then_some(advance)
            })
    }

    fn clusters(
        &mut self,
        backend: &mut dyn TextBackend,
        font: &FontSpec,
        text: &str,
        scale: f32,
    ) -> Vec<(usize, char, f32)> {
        if text.bytes().all(|b| (0x20..0x7f).contains(&b))
            && let Some(advance) = self.ascii_advance(backend, font, scale)
        {
            return text.char_indices().map(|(i, c)| (i, c, advance)).collect();
        }
        let shaped = backend.shape(font, &normalize_space(text), scale);
        let at = boundaries(&shaped, utf16_len(text));
        let mut starts = vec![false; at.len()];
        for glyph in &shaped.glyphs {
            if let Some(start) = starts.get_mut(glyph.cluster as usize) {
                *start = true;
            }
        }
        let mut out: Vec<(usize, char, f32)> = Vec::with_capacity(text.len());
        let mut from = 0.0;
        let mut unit = 0;
        for (byte, c) in text.char_indices() {
            if byte == 0 || starts[unit] {
                if let Some(last) = out.last_mut() {
                    last.2 = at[unit] - from;
                }
                from = at[unit];
                out.push((byte, c, 0.0));
            }
            unit += c.len_utf16();
        }
        if let Some(last) = out.last_mut() {
            last.2 = at[unit] - from;
        }
        out
    }

    /// Blink's `Font::TabWidth`: to the next stop, or the one after when that is closer than
    /// half a space. Device px.
    fn tab_width(
        &mut self,
        backend: &mut dyn TextBackend,
        font: &FontSpec,
        tab_size: u8,
        position: f32,
        scale: f32,
    ) -> f32 {
        let space = self.clusters(backend, font, " ", scale)[0].2 * scale;
        let base = space * f32::from(tab_size);
        let distance = base - position % base;
        if distance < space / 2.0 {
            distance + base
        } else {
            distance
        }
    }

    /// Lays `items` out in lines `available` wide at device scale `scale`, with `tab_size`
    /// spaces to a tab stop. Forced breaks end lines; the last one starts no empty line.
    pub fn layout(
        &mut self,
        backend: &mut dyn TextBackend,
        items: &[Item<'_>],
        available: Lu,
        tab_size: u8,
        scale: f32,
    ) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut paragraph: Vec<Segment> = Vec::new();
        let block = (available, tab_size, scale);
        for (item, content) in items.iter().enumerate() {
            let Item::Text { text, font } = *content else {
                self.paragraph(backend, &mut paragraph, block, true, &mut lines);
                continue;
            };
            let mut start = 0;
            while start < text.len() {
                let rest = &text[start..];
                let run =
                    |f: fn(u8) -> bool| rest.bytes().position(|b| !f(b)).unwrap_or(rest.len());
                let (len, tabs) = match rest.as_bytes()[0] {
                    b'\n' => {
                        self.paragraph(backend, &mut paragraph, block, true, &mut lines);
                        start += 1;
                        continue;
                    }
                    b'\t' => (run(|b| b == b'\t'), true),
                    // Blink's items builder cuts the spaces that begin the block or follow a
                    // forced break off as an item of their own.
                    b' ' if paragraph.is_empty() => (run(|b| b == b' '), false),
                    _ => (run(|b| b != b'\t' && b != b'\n'), false),
                };
                let content = &rest[..len];
                let mut clusters = if tabs {
                    content.char_indices().map(|(i, c)| (i, c, 0.0)).collect()
                } else {
                    self.clusters(backend, &font, content, scale)
                };
                for cluster in &mut clusters {
                    cluster.0 += start;
                }
                start += len;
                paragraph.push(Segment {
                    item,
                    font,
                    tabs,
                    clusters,
                    end: start,
                });
            }
        }
        self.paragraph(backend, &mut paragraph, block, false, &mut lines);
        lines
    }

    /// Breaks the segments before a forced break, or before the block's end, into lines.
    fn paragraph(
        &mut self,
        backend: &mut dyn TextBackend,
        segments: &mut Vec<Segment>,
        block: (Lu, u8, f32),
        forced: bool,
        lines: &mut Vec<Line>,
    ) {
        if segments.is_empty() && forced {
            lines.push(Line::default());
        }
        let mut start = (0, 0);
        while start.0 < segments.len() {
            let (line, next) = self.line(backend, segments, start, block);
            lines.push(line);
            start = next;
        }
        segments.clear();
    }

    /// The line that starts at cluster `start.1` of segment `start.0`, and where the next one
    /// does.
    fn line(
        &mut self,
        backend: &mut dyn TextBackend,
        segments: &mut [Segment],
        start: (usize, usize),
        (available, tab_size, scale): (Lu, u8, f32),
    ) -> (Line, (usize, usize)) {
        let room = available as f32 / 64.0;
        let mut origin: Lu = 0;
        let mut before = None;
        let mut opportunity = None;
        let mut overflow = None;
        'segments: for (s, segment) in segments.iter_mut().enumerate().skip(start.0) {
            let from = if s == start.0 { start.1 } else { 0 };
            let mut width = 0.0;
            for k in from..segment.clusters.len() {
                let (_, c, mut advance) = segment.clusters[k];
                let position = origin as f32 / 64.0;
                if segment.tabs {
                    let at = position + width * scale;
                    advance = self.tab_width(backend, &segment.font, tab_size, at, scale) / scale;
                    segment.clusters[k].2 = advance;
                }
                if before.is_some_and(|b| may_break(b, c)) {
                    opportunity = Some((s, k));
                }
                width += advance;
                if position + width * scale > room {
                    let anywhere = if before.is_none() { (s, k + 1) } else { (s, k) };
                    overflow = Some(opportunity.unwrap_or(anywhere));
                    break 'segments;
                }
                before = Some(c);
            }
            origin += lu_ceil(width * scale);
        }
        let mut end = overflow.unwrap_or((segments.len(), 0));
        if end.0 < segments.len() && end.1 == segments[end.0].clusters.len() {
            end = (end.0 + 1, 0);
        }
        let mut line = Line::default();
        for (s, segment) in segments.iter().enumerate().skip(start.0) {
            let from = if s == start.0 { start.1 } else { 0 };
            let to = match s.cmp(&end.0) {
                std::cmp::Ordering::Less => segment.clusters.len(),
                std::cmp::Ordering::Equal => end.1,
                std::cmp::Ordering::Greater => 0,
            };
            if to <= from {
                continue;
            }
            let advance: f32 = segment.clusters[from..to].iter().map(|c| c.2).sum();
            let width = lu_ceil(advance * scale);
            let byte = |k: usize| segment.clusters.get(k).map_or(segment.end, |c| c.0);
            line.pieces.push(Piece {
                item: segment.item,
                range: byte(from)..byte(to),
                x: line.width,
                width,
                tabs: segment.tabs,
            });
            line.width += width;
        }
        (line, end)
    }
}

/// The glyphs of `text` shaped alone, relative to its start.
pub fn glyphs(
    backend: &mut dyn TextBackend,
    font: &FontSpec,
    text: &str,
    scale: f32,
) -> Vec<ShapedGlyph> {
    backend.shape(font, &normalize_space(text), scale).glyphs
}
