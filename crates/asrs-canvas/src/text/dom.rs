//! Text in a CSS block box as Blink lays it out and paints it, for the subset of CSS upstream's
//! stylesheets use: one font per box, `line-height`, `text-align`, `direction`, `white-space`
//! (`normal`, `nowrap`, `pre`) and `text-overflow` (`clip`, `ellipsis`).
//!
//! Blink lays out zoomed: at device scale `s` every length is `s` times its CSS value and
//! `LayoutUnit`s are 1/64 of a device pixel. Glyph advances are the CSS-size advances times `s`,
//! while the vertical font metrics are those of the font at `s` times its size, rounded.

use std::sync::Arc;

use super::layout::normalize_space;
use super::lut::MaskGammaLut;
use super::{FontSpec, Mask, Shaped, ShapedGlyph, TextBackend};

/// One `LayoutUnit` of the zoomed layout, 1/64 of a device pixel.
pub type Lu = i32;

pub const LU_PER_PX: Lu = 64;

/// `LayoutUnit::FromFloatCeil`.
pub fn lu_ceil(v: f32) -> Lu {
    (v * 64.0).ceil() as Lu
}

/// `LayoutUnit::FromFloatRound`.
pub fn lu_round(v: f32) -> Lu {
    (v * 64.0).round() as Lu
}

/// `LayoutUnit::Floor`, as a `LayoutUnit`.
fn lu_floor_px(v: Lu) -> Lu {
    v.div_euclid(LU_PER_PX) * LU_PER_PX
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Start,
    End,
    Left,
    Right,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WhiteSpace {
    Normal,
    NoWrap,
    Pre,
}

/// `text-overflow` on a box with `overflow: hidden`; `Visible` is a box that does not clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Overflow {
    Visible,
    Clip,
    Ellipsis,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    Px(f32),
    Number(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// The font at its CSS size.
    pub font: FontSpec,
    pub line_height: LineHeight,
    pub align: Align,
    pub direction: Direction,
    pub white_space: WhiteSpace,
    pub overflow: Overflow,
}

impl Style {
    pub fn new(font: FontSpec) -> Style {
        Style {
            font,
            line_height: LineHeight::Normal,
            align: Align::Start,
            direction: Direction::Ltr,
            white_space: WhiteSpace::Normal,
            overflow: Overflow::Visible,
        }
    }
}

/// A run of glyphs painted from one origin: a line's text, the part of it an ellipsis leaves,
/// or the ellipsis.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    pub text: String,
    /// Origin from the content box's left edge.
    pub x: Lu,
    /// Snapped inline size.
    pub width: Lu,
    /// Glyphs at CSS-size positions relative to the origin.
    pub glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// From the content box's top edge.
    pub top: Lu,
    pub height: Lu,
    /// Alphabetic baseline from the content box's top edge.
    pub baseline: Lu,
    /// Inline size of the whole line before any ellipsis, as alignment sees it.
    pub width: Lu,
    pub fragments: Vec<Fragment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub scale: f32,
    pub lines: Vec<Line>,
    /// Widest line before alignment and truncation: the box's max-content width.
    pub width: Lu,
    pub height: Lu,
}

impl Layout {
    pub fn width_px(&self) -> f64 {
        f64::from(self.width) / 64.0 / f64::from(self.scale)
    }

    pub fn height_px(&self) -> f64 {
        f64::from(self.height) / 64.0 / f64::from(self.scale)
    }
}

/// Font metrics of the zoomed font in device pixels: ascent, descent and line height.
pub fn line_metrics(backend: &mut dyn TextBackend, style: &Style, scale: f32) -> (Lu, Lu, Lu) {
    let mut zoomed = style.font;
    zoomed.size *= scale;
    let m = backend.metrics(&zoomed);
    let (ascent, descent) = (m.ascent as Lu * LU_PER_PX, m.descent as Lu * LU_PER_PX);
    let height = match style.line_height {
        LineHeight::Normal => ascent + descent + m.line_gap as Lu * LU_PER_PX,
        LineHeight::Px(v) => lu_round(v * scale),
        LineHeight::Number(n) => lu_round(zoomed.size * n),
    };
    (ascent, descent, height)
}

fn is_collapsible(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// The text after white-space processing, one entry per line for `pre` and a single entry
/// otherwise. Collapsible runs become one space and spaces at the ends of the block go.
pub fn collapse(text: &str, white_space: WhiteSpace) -> Vec<String> {
    if white_space == WhiteSpace::Pre {
        return text.split('\n').map(str::to_owned).collect();
    }
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if is_collapsible(c) {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    vec![out]
}

/// Where a line may break in collapsed `white-space: normal` text, as `(end of the line's
/// content, start of the next line)` byte offsets: at each space, which neither line keeps, and
/// after a hyphen between two letters.
fn break_opportunities(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    for (i, &(byte, c)) in chars.iter().enumerate() {
        let next = chars.get(i + 1);
        if c == ' ' {
            out.push((byte, byte + 1));
        } else if c == '-'
            && i > 0
            && chars[i - 1].1.is_alphabetic()
            && next.is_some_and(|n| n.1.is_alphabetic())
        {
            out.push((byte + 1, byte + 1));
        }
    }
    out
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `LineOffsetForTextAlign`.
fn align_offset(align: Align, direction: Direction, space: Lu) -> Lu {
    let ltr = direction == Direction::Ltr;
    let align = match align {
        Align::Start if ltr => Align::Left,
        Align::Start => Align::Right,
        Align::End if ltr => Align::Right,
        Align::End => Align::Left,
        a => a,
    };
    match align {
        Align::Left if ltr => 0,
        Align::Left => space.min(0),
        Align::Right if !ltr => space,
        Align::Right => space.max(0),
        Align::Center if ltr || space > 0 => (space / 2).max(0),
        _ => space,
    }
}

/// Positions of the character boundaries of `shaped` in CSS px, by UTF-16 offset, for the
/// `len` code units it covers; offsets inside a cluster share their cluster's start.
fn boundaries(shaped: &Shaped, len: usize) -> Vec<f32> {
    let mut at = vec![f32::NAN; len + 1];
    for g in &shaped.glyphs {
        let c = g.cluster as usize;
        if c <= len && (at[c].is_nan() || g.x < at[c]) {
            at[c] = g.x;
        }
    }
    at[len] = shaped.width;
    let mut last = 0.0;
    for v in &mut at {
        if v.is_nan() {
            *v = last;
        } else {
            last = *v;
        }
    }
    at
}

fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn shift(glyphs: &[ShapedGlyph], dx: f32, from: u32) -> Vec<ShapedGlyph> {
    glyphs
        .iter()
        .map(|g| ShapedGlyph {
            x: g.x - dx,
            cluster: g.cluster - from,
            ..*g
        })
        .collect()
}

const ELLIPSIS: &str = "\u{2026}";

/// `NGLineTruncator::TruncateLine` for one text item that overflows `available`.
fn truncate(
    backend: &mut dyn TextBackend,
    style: &Style,
    text: &str,
    shaped: &Shaped,
    available: Lu,
    scale: f32,
) -> Vec<Fragment> {
    let ellipsis = backend.shape(&style.font, ELLIPSIS);
    let ellipsis_width = lu_ceil(ellipsis.width * scale);
    let room = (available - ellipsis_width) as f32 / 64.0;
    let len = utf16_len(text);
    let at = boundaries(shaped, len);
    let mut fragments = Vec::new();
    if style.direction == Direction::Ltr {
        let fit = (0..=len)
            .rev()
            .find(|&k| at[k] * scale <= room)
            .unwrap_or(0);
        let kept = lu_ceil(at[fit] * scale);
        if fit > 0 {
            let glyphs: Vec<ShapedGlyph> = shaped
                .glyphs
                .iter()
                .filter(|g| (g.cluster as usize) < fit)
                .copied()
                .collect();
            fragments.push(Fragment {
                text: text[..byte_offset(text, fit)].to_owned(),
                x: 0,
                width: kept,
                glyphs,
            });
        }
        fragments.push(Fragment {
            text: ELLIPSIS.to_owned(),
            x: if fit > 0 { kept } else { 0 },
            width: ellipsis_width,
            glyphs: ellipsis.glyphs,
        });
    } else {
        let fit = (0..=len)
            .find(|&k| (shaped.width - at[k]) * scale <= room)
            .unwrap_or(len);
        let kept = lu_ceil((shaped.width - at[fit]) * scale);
        let text_x = available - kept;
        fragments.push(Fragment {
            text: ELLIPSIS.to_owned(),
            x: text_x - ellipsis_width,
            width: ellipsis_width,
            glyphs: ellipsis.glyphs,
        });
        if fit < len {
            let glyphs: Vec<ShapedGlyph> = shaped
                .glyphs
                .iter()
                .filter(|g| (g.cluster as usize) >= fit)
                .copied()
                .collect();
            fragments.push(Fragment {
                text: text[byte_offset(text, fit)..].to_owned(),
                x: text_x,
                width: kept,
                glyphs: shift(&glyphs, at[fit], fit as u32),
            });
        }
    }
    fragments
}

/// Greedy line breaking of collapsed `white-space: normal` text into lines no wider than
/// `available`; a word wider than that gets a line of its own.
fn wrap(
    backend: &mut dyn TextBackend,
    style: &Style,
    text: &str,
    available: Lu,
    scale: f32,
) -> Vec<String> {
    let shaped = backend.shape(&style.font, &normalize_space(text));
    let at = boundaries(&shaped, utf16_len(text));
    let mut units = vec![0usize; text.len() + 1];
    let mut count = 0;
    for (byte, c) in text.char_indices() {
        units[byte] = count;
        count += c.len_utf16();
    }
    units[text.len()] = count;
    let fits =
        |from: usize, to: usize| lu_ceil((at[units[to]] - at[units[from]]) * scale) <= available;
    let mut breaks = break_opportunities(text);
    breaks.push((text.len(), text.len()));
    let mut lines = Vec::new();
    let mut start = 0;
    let mut candidate: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < breaks.len() {
        let (end, next) = breaks[i];
        if fits(start, end) {
            candidate = Some((end, next));
            i += 1;
        } else if let Some((end, next)) = candidate.take() {
            lines.push(text[start..end].to_owned());
            start = next;
        } else {
            lines.push(text[start..end].to_owned());
            start = next;
            i += 1;
        }
    }
    if let Some((end, _)) = candidate {
        lines.push(text[start..end].to_owned());
    }
    lines
}

/// Lays `text` out in a content box `available` wide, or at its max-content width when `None`.
pub fn layout(
    backend: &mut dyn TextBackend,
    style: &Style,
    text: &str,
    available: Option<Lu>,
    scale: f32,
) -> Layout {
    let (ascent, descent, line_height) = line_metrics(backend, style, scale);
    let leading = lu_floor_px((line_height - (ascent + descent)) / 2);
    let mut texts = Vec::new();
    for paragraph in collapse(text, style.white_space) {
        match available {
            Some(a) if style.white_space == WhiteSpace::Normal && !paragraph.is_empty() => {
                texts.extend(wrap(backend, style, &paragraph, a, scale));
            }
            _ => texts.push(paragraph),
        }
    }
    if texts.len() == 1 && texts[0].is_empty() && style.white_space != WhiteSpace::Pre {
        texts.clear();
    }
    let mut lines = Vec::with_capacity(texts.len());
    let mut max_width = 0;
    for (i, line_text) in texts.into_iter().enumerate() {
        let shaped = backend.shape(&style.font, &normalize_space(&line_text));
        let width = lu_ceil(shaped.width * scale);
        max_width = max_width.max(width);
        let room = available.unwrap_or(width);
        let fragments = if style.overflow == Overflow::Ellipsis && width > room {
            truncate(backend, style, &line_text, &shaped, room, scale)
        } else {
            vec![Fragment {
                text: line_text,
                x: align_offset(style.align, style.direction, room - width),
                width,
                glyphs: shaped.glyphs,
            }]
        };
        let top = i as Lu * line_height;
        lines.push(Line {
            top,
            height: line_height,
            baseline: top + leading + ascent,
            width,
            fragments,
        });
    }
    Layout {
        scale,
        height: lines.len() as Lu * line_height,
        lines,
        width: max_width,
    }
}

/// A fragment's coverage in device pixels after mask gamma, relative to the pixel its origin
/// falls in and to its baseline row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunImage {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

/// Paints `glyphs` from an origin `fraction` `LayoutUnit`s right of a device pixel boundary, as
/// Skia places DOM text: quarter-pixel x phases, whole-pixel baseline, each glyph's coverage
/// through the mask-gamma row of `color` and composited source-over.
pub fn rasterize(
    backend: &mut dyn TextBackend,
    lut: &MaskGammaLut,
    glyphs: &[ShapedGlyph],
    fraction: Lu,
    scale: f32,
    color: [u8; 3],
) -> Option<RunImage> {
    let origin = fraction as f32 / 64.0;
    let mut placed: Vec<(i32, Arc<Mask>)> = Vec::with_capacity(glyphs.len());
    for g in glyphs {
        let biased = origin + scale * g.x + 0.125;
        let ix = biased.floor();
        let phase = (((biased - ix) * 4.0).floor() as u8).min(3);
        if let Some(mask) = backend.raw_mask(g.glyph, f64::from(scale), 1.0, phase)
            && mask.width > 0
            && mask.height > 0
        {
            placed.push((ix as i32, mask));
        }
    }
    let left = placed.iter().map(|(x, m)| x + m.left).min()?;
    let right = placed
        .iter()
        .map(|(x, m)| x + m.left + m.width as i32)
        .max()?;
    let top = placed.iter().map(|(_, m)| m.top).min()?;
    let bottom = placed.iter().map(|(_, m)| m.top + m.height as i32).max()?;
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    let mut alpha = vec![0u8; width as usize * height as usize];
    for (x, mask) in &placed {
        for row in 0..mask.height as usize {
            let y = (mask.top - top) as usize + row;
            let src = &mask.data[row * mask.width as usize..(row + 1) * mask.width as usize];
            let dst_start = y * width as usize + (x + mask.left - left) as usize;
            for (d, &raw) in alpha[dst_start..dst_start + src.len()].iter_mut().zip(src) {
                if raw == 0 {
                    continue;
                }
                let a = u32::from(lut.apply(color, raw));
                let under = u32::from(*d);
                *d = (a + (under * (255 - a) + 127) / 255) as u8;
            }
        }
    }
    Some(RunImage {
        left,
        top,
        width,
        height,
        alpha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::FixedAdvance;

    fn style(css: &str) -> Style {
        Style::new(FontSpec::parse(css).unwrap())
    }

    #[test]
    fn collapses_white_space() {
        assert_eq!(collapse("  a \n\t b  ", WhiteSpace::NoWrap), ["a b"]);
        assert_eq!(collapse("a\n b", WhiteSpace::Pre), ["a", " b"]);
        assert_eq!(collapse("   ", WhiteSpace::Normal), [""]);
    }

    #[test]
    fn aligns_like_layout_ng() {
        assert_eq!(align_offset(Align::Center, Direction::Ltr, 65), 32);
        assert_eq!(align_offset(Align::Center, Direction::Ltr, -65), 0);
        assert_eq!(align_offset(Align::Center, Direction::Rtl, -65), -65);
        assert_eq!(align_offset(Align::Right, Direction::Ltr, -3), 0);
        assert_eq!(align_offset(Align::Right, Direction::Rtl, -3), -3);
        assert_eq!(align_offset(Align::Start, Direction::Rtl, 7), 7);
        assert_eq!(align_offset(Align::Left, Direction::Rtl, -7), -7);
    }

    #[test]
    fn fixed_advance_layout() {
        let mut backend = FixedAdvance;
        let s = style("10px sans-serif");
        let l = layout(&mut backend, &s, "abc de", None, 2.0);
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.width, lu_ceil(5.0 * 6.0 * 2.0 + 2.5 * 2.0));
        let (ascent, descent, height) = line_metrics(&mut backend, &s, 2.0);
        assert_eq!((ascent, descent, height), (16 * 64, 4 * 64, 20 * 64));
        assert_eq!(l.lines[0].baseline, 16 * 64);
        let wrapped = layout(&mut backend, &s, "abc de fgh", Some(lu_ceil(80.0)), 2.0);
        let texts: Vec<&str> = wrapped
            .lines
            .iter()
            .map(|l| l.fragments[0].text.as_str())
            .collect();
        assert_eq!(texts, ["abc de", "fgh"]);
        assert_eq!(wrapped.height, 2 * 20 * 64);
    }

    #[test]
    fn wraps_long_words_and_hyphens() {
        let s = style("10px sans-serif");
        let texts = |text: &str, px: f32| -> Vec<String> {
            layout(&mut FixedAdvance, &s, text, Some(lu_ceil(px)), 1.0)
                .lines
                .into_iter()
                .map(|l| l.fragments[0].text.clone())
                .collect()
        };
        assert_eq!(texts("abcdefgh ij", 30.0), ["abcdefgh", "ij"]);
        assert_eq!(texts("ab-cd ef", 40.0), ["ab-cd", "ef"]);
        assert_eq!(texts("ab-cd ef", 20.0), ["ab-", "cd", "ef"]);
        assert_eq!(texts("a b c", 1000.0), ["a b c"]);
        assert_eq!(texts("abc def ghi", 30.0), ["abc", "def", "ghi"]);
    }

    #[test]
    fn ellipsis_keeps_what_fits() {
        let mut backend = FixedAdvance;
        let mut s = style("10px sans-serif");
        s.overflow = Overflow::Ellipsis;
        s.white_space = WhiteSpace::NoWrap;
        let l = layout(&mut backend, &s, "abcdefgh", Some(lu_ceil(30.0)), 1.0);
        let f = &l.lines[0].fragments;
        assert_eq!(f[0].text, "abcd");
        assert_eq!((f[1].text.as_str(), f[1].x), (ELLIPSIS, lu_ceil(24.0)));
        s.direction = Direction::Rtl;
        s.align = Align::Right;
        let l = layout(&mut backend, &s, "abcdefgh", Some(lu_ceil(30.0)), 1.0);
        let f = &l.lines[0].fragments;
        assert_eq!(f[1].text, "efgh");
        assert_eq!(f[1].x, lu_ceil(30.0) - lu_ceil(24.0));
        assert_eq!(f[1].glyphs[0].x, 0.0);
        assert_eq!(f[0].x, f[1].x - lu_ceil(6.0));
    }
}
