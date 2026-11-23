// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

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
    /// `letter-spacing` in CSS px.
    pub letter_spacing: f32,
    /// `unicode-bidi: plaintext` on the text's inline box: its bidi paragraph takes the
    /// direction of its first strong character, while alignment and truncation keep the
    /// block's `direction`.
    pub bidi_plaintext: bool,
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
            letter_spacing: 0.0,
            bidi_plaintext: false,
        }
    }

    /// A style from CSS declarations: `font` (required), `line-height`, `text-align`,
    /// `direction`, `white-space`, `text-overflow` (which implies `overflow: hidden`) and
    /// `letter-spacing` and `unicode-bidi` (`normal` or `plaintext`).
    /// Unknown properties and values are `None`.
    pub fn from_css<'a>(
        declarations: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Option<Style> {
        let mut font = None;
        let mut style = Style::new(FontSpec::default());
        for (property, value) in declarations {
            let value = value.trim();
            match property {
                "font" => font = Some(FontSpec::parse(value)?),
                "line-height" => {
                    style.line_height = match value {
                        "normal" => LineHeight::Normal,
                        v => match v.strip_suffix("px") {
                            Some(px) => LineHeight::Px(px.parse().ok()?),
                            None => LineHeight::Number(v.parse().ok()?),
                        },
                    }
                }
                "text-align" => {
                    style.align = match value {
                        "start" => Align::Start,
                        "end" => Align::End,
                        "left" => Align::Left,
                        "right" => Align::Right,
                        "center" => Align::Center,
                        _ => return None,
                    }
                }
                "direction" => {
                    style.direction = match value {
                        "ltr" => Direction::Ltr,
                        "rtl" => Direction::Rtl,
                        _ => return None,
                    }
                }
                "white-space" => {
                    style.white_space = match value {
                        "normal" => WhiteSpace::Normal,
                        "nowrap" => WhiteSpace::NoWrap,
                        "pre" => WhiteSpace::Pre,
                        _ => return None,
                    }
                }
                "letter-spacing" => {
                    style.letter_spacing = match value {
                        "normal" => 0.0,
                        v => v.strip_suffix("px")?.parse().ok()?,
                    }
                }
                "unicode-bidi" => {
                    style.bidi_plaintext = match value {
                        "normal" => false,
                        "plaintext" => true,
                        _ => return None,
                    }
                }
                "text-overflow" => {
                    style.overflow = match value {
                        "visible" => Overflow::Visible,
                        "clip" => Overflow::Clip,
                        "ellipsis" => Overflow::Ellipsis,
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        style.font = font?;
        Some(style)
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
    /// The zoomed font's rounded ascent and descent, the extent of a line's text box.
    pub ascent: Lu,
    pub descent: Lu,
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

/// `text` shaped in `style`'s font, with `letter-spacing` added after every character as
/// Blink's `ShapeResultSpacing` does, the last one included.
fn shape(backend: &mut dyn TextBackend, style: &Style, text: &str, scale: f32) -> Shaped {
    let mut shaped = backend.shape_run(&style.font, text, scale);
    if style.letter_spacing != 0.0 {
        let mut chars_before = vec![0u32; utf16_len(text) + 1];
        let mut count = 0;
        let mut unit = 0;
        for c in text.chars() {
            for _ in 0..c.len_utf16() {
                chars_before[unit] = count;
                unit += 1;
            }
            count += 1;
        }
        chars_before[unit] = count;
        for g in &mut shaped.glyphs {
            let k = chars_before
                .get(g.cluster as usize)
                .copied()
                .unwrap_or(count);
            g.x += style.letter_spacing * k as f32;
        }
        shaped.width += style.letter_spacing * count as f32;
    }
    shaped
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

const ELLIPSIS: &str = "\u{2026}";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    L,
    R,
    En,
    Es,
    Et,
    Cs,
    Neutral,
}

/// Bidi classes for the scripts upstream's text uses: Latin, Greek and the like are strong
/// left-to-right, Hebrew and Arabic strong right-to-left.
fn class(c: char) -> Class {
    match c {
        '0'..='9' => Class::En,
        '+' | '-' => Class::Es,
        '#' | '$' | '%' | '\u{b0}' | '\u{b1}' | '\u{2030}' => Class::Et,
        ',' | '.' | '/' | ':' | '\u{a0}' => Class::Cs,
        '\u{590}'..='\u{8ff}' | '\u{fb1d}'..='\u{fdff}' | '\u{fe70}'..='\u{feff}' => Class::R,
        c if c.is_alphabetic() => Class::L,
        _ => Class::Neutral,
    }
}

/// UAX #9 for one paragraph at level 1 without explicit embeddings: rules W4 to W7, N1, N2, I1
/// and I2, reduced to the classes above. Returns the logical runs as `(byte range, rtl)`; a
/// right-to-left run is at level 1, a left-to-right one at level 2.
fn rtl_runs(text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut t: Vec<Class> = chars.iter().map(|&(_, c)| class(c)).collect();
    let n = t.len();
    for i in 1..n.saturating_sub(1) {
        if matches!(t[i], Class::Cs | Class::Es) && t[i - 1] == Class::En && t[i + 1] == Class::En {
            t[i] = Class::En;
        }
    }
    for i in 0..n {
        if t[i] == Class::En {
            let mut j = i;
            while j > 0 && t[j - 1] == Class::Et {
                t[j - 1] = Class::En;
                j -= 1;
            }
            let mut j = i + 1;
            while j < n && t[j] == Class::Et {
                t[j] = Class::En;
                j += 1;
            }
        }
    }
    for v in &mut t {
        if matches!(v, Class::Es | Class::Et | Class::Cs) {
            *v = Class::Neutral;
        }
    }
    let mut strong = Class::R;
    for v in &mut t {
        match *v {
            Class::L | Class::R => strong = *v,
            Class::En if strong == Class::L => *v = Class::L,
            _ => {}
        }
    }
    let mut i = 0;
    while i < n {
        if t[i] != Class::Neutral {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && t[i] == Class::Neutral {
            i += 1;
        }
        let before = if start == 0 { Class::R } else { t[start - 1] };
        let after = if i == n { Class::R } else { t[i] };
        let resolved = if before == Class::L && after == Class::L {
            Class::L
        } else {
            Class::R
        };
        for v in &mut t[start..i] {
            *v = resolved;
        }
    }
    let mut runs: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
    for (k, &(byte, c)) in chars.iter().enumerate() {
        let rtl = t[k] == Class::R;
        let end = byte + c.len_utf8();
        match runs.last_mut() {
            Some((range, r)) if *r == rtl => range.end = end,
            _ => runs.push((byte..end, rtl)),
        }
    }
    runs
}

fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '\u{ab}' => '\u{bb}',
        '\u{bb}' => '\u{ab}',
        c => c,
    }
}

/// The paragraph direction `unicode-bidi: plaintext` gives `text`: that of its first strong
/// character (rules P2 and P3), or `fallback` when it has none.
fn first_strong(text: &str, fallback: Direction) -> Direction {
    text.chars()
        .find_map(|c| match class(c) {
            Class::L => Some(Direction::Ltr),
            Class::R => Some(Direction::Rtl),
            _ => None,
        })
        .unwrap_or(fallback)
}

/// The line's bidi runs in visual order, each as the string it displays left to right.
fn visual_runs(text: &str, direction: Direction) -> Vec<String> {
    if direction == Direction::Ltr {
        return vec![text.to_owned()];
    }
    rtl_runs(text)
        .into_iter()
        .rev()
        .map(|(range, rtl)| {
            let run = &text[range];
            if rtl {
                run.chars().rev().map(mirror).collect()
            } else {
                run.to_owned()
            }
        })
        .collect()
}

/// The glyphs of `fragment` whose characters lie in `[from, to)`, moved so the first is at the
/// origin, as a fragment `width` wide at `x`.
fn sub_fragment(
    fragment: &Fragment,
    at: &[f32],
    from: usize,
    to: usize,
    x: Lu,
    width: Lu,
) -> Fragment {
    let glyphs = fragment
        .glyphs
        .iter()
        .filter(|g| (from..to).contains(&(g.cluster as usize)))
        .map(|g| ShapedGlyph {
            x: g.x - at[from],
            cluster: g.cluster - from as u32,
            ..*g
        })
        .collect();
    let text = &fragment.text;
    Fragment {
        text: text[byte_offset(text, from)..byte_offset(text, to)].to_owned(),
        x,
        width,
        glyphs,
    }
}

/// Where the first character's cluster ends: the line's first character is clipped, never
/// replaced by the ellipsis (`LineTruncator::TruncateChild`).
fn first_cluster_end(fragment: &Fragment, len: usize) -> usize {
    fragment
        .glyphs
        .iter()
        .map(|g| g.cluster as usize)
        .filter(|&c| c > 0)
        .min()
        .unwrap_or(len)
}

fn boundaries_of(fragment: &Fragment, width: f32, len: usize) -> Vec<f32> {
    boundaries(
        &Shaped {
            glyphs: fragment.glyphs.clone(),
            width,
        },
        len,
    )
}

/// `NGLineTruncator::TruncateLine`: drops what lies past the line end less the ellipsis, cuts the
/// fragment straddling that point at the last character that fits, and places the ellipsis
/// against what is left. The line end is the right edge for `ltr` and the left edge for `rtl`.
/// `fragments` are in visual order with their unsnapped widths.
fn truncate(
    backend: &mut dyn TextBackend,
    style: &Style,
    fragments: Vec<(Fragment, f32)>,
    available: Lu,
    scale: f32,
) -> Vec<Fragment> {
    let ellipsis = shape(backend, style, ELLIPSIS, scale);
    let ellipsis_width = lu_ceil(ellipsis.width * scale);
    let ellipsis_at = |x: Lu| Fragment {
        text: ELLIPSIS.to_owned(),
        x,
        width: ellipsis_width,
        glyphs: ellipsis.glyphs.clone(),
    };
    let mut out = Vec::new();
    if style.direction == Direction::Ltr {
        let limit = available - ellipsis_width;
        let mut end = 0;
        for (i, (f, width)) in fragments.into_iter().enumerate() {
            if f.x + f.width <= limit {
                end = f.x + f.width;
                out.push(f);
                continue;
            }
            let first = i == 0;
            if f.x < limit || (first && f.x < available) {
                let len = utf16_len(&f.text);
                let at = boundaries_of(&f, width, len);
                let room = (limit - f.x) as f32 / 64.0;
                let mut fit = (0..=len)
                    .rev()
                    .find(|&k| at[k] * scale <= room)
                    .unwrap_or(0);
                if fit == 0 && first {
                    fit = first_cluster_end(&f, len);
                }
                end = f.x;
                if fit > 0 {
                    let kept = lu_ceil(at[fit] * scale);
                    end += kept;
                    out.push(sub_fragment(&f, &at, 0, fit, f.x, kept));
                }
            }
            break;
        }
        out.push(ellipsis_at(end));
    } else {
        let limit = ellipsis_width;
        let mut start = available;
        for (i, (f, width)) in fragments.into_iter().rev().enumerate() {
            if f.x >= limit {
                start = f.x;
                out.push(f);
                continue;
            }
            let first = i == 0;
            let right = f.x + f.width;
            if right > limit || (first && right > 0) {
                let len = utf16_len(&f.text);
                let at = boundaries_of(&f, width, len);
                let room = (right - limit) as f32 / 64.0;
                let mut fit = (0..=len)
                    .find(|&k| (width - at[k]) * scale <= room)
                    .unwrap_or(len);
                if fit == len && first {
                    fit = f
                        .glyphs
                        .iter()
                        .map(|g| g.cluster as usize)
                        .max()
                        .unwrap_or(0);
                }
                start = right;
                if fit < len {
                    let kept = lu_ceil((width - at[fit]) * scale);
                    start -= kept;
                    out.push(sub_fragment(&f, &at, fit, len, start, kept));
                }
            }
            break;
        }
        out.push(ellipsis_at(start - ellipsis_width));
        out.reverse();
    }
    out
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
    let shaped = shape(backend, style, &normalize_space(text), scale);
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
    let strut = (leading + ascent, line_height - leading - ascent);
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
    let mut top = 0;
    for line_text in texts {
        let mut runs = Vec::new();
        let mut width = 0;
        let paragraph = if style.bidi_plaintext {
            first_strong(&line_text, style.direction)
        } else {
            style.direction
        };
        for run in visual_runs(&line_text, paragraph) {
            let shaped = shape(backend, style, &normalize_space(&run), scale);
            let run_width = lu_ceil(shaped.width * scale);
            runs.push((
                Fragment {
                    text: run,
                    x: width,
                    width: run_width,
                    glyphs: shaped.glyphs,
                },
                shaped.width,
            ));
            width += run_width;
        }
        max_width = max_width.max(width);
        let room = available.unwrap_or(width);
        let offset = align_offset(style.align, style.direction, room - width);
        for (f, _) in &mut runs {
            f.x += offset;
        }
        let fragments = if style.overflow == Overflow::Ellipsis && width > room {
            truncate(backend, style, runs, room, scale)
        } else {
            runs.into_iter().map(|(f, _)| f).collect()
        };
        let (above, below) = if style.line_height == LineHeight::Normal {
            used_fonts(backend, &fragments, scale, strut)
        } else {
            strut
        };
        lines.push(Line {
            top,
            height: above + below,
            baseline: top + above,
            width,
            fragments,
        });
        top += above + below;
    }
    Layout {
        scale,
        ascent,
        descent,
        height: top,
        lines,
        width: max_width,
    }
}

/// `InlineBoxState::AccumulateUsedFonts`: under `line-height: normal` a line box also holds the
/// ascent and descent, with half the line gap each side, of every fallback font its glyphs use.
fn used_fonts(
    backend: &mut dyn TextBackend,
    fragments: &[Fragment],
    scale: f32,
    (mut above, mut below): (Lu, Lu),
) -> (Lu, Lu) {
    for glyph in fragments.iter().flat_map(|f| &f.glyphs) {
        if let Some(m) = backend.fallback_metrics(glyph.glyph, scale) {
            let (a, d) = (m.ascent as Lu * LU_PER_PX, m.descent as Lu * LU_PER_PX);
            let gap = m.line_gap as Lu * LU_PER_PX;
            above = above.max(a + gap / 2);
            below = below.max(d + gap - gap / 2);
        }
    }
    (above, below)
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
    /// Straight RGBA of the whole run when it holds a colour glyph, which `alpha` then only
    /// approximates.
    pub rgba: Option<Vec<u8>>,
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
    if placed.iter().any(|(_, m)| m.color) {
        return Some(composite_color(
            lut,
            &placed,
            [left, top],
            [width, height],
            color,
        ));
    }
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
        rgba: None,
    })
}

/// A run with colour glyphs, composited source-over in premultiplied RGBA as Skia draws its
/// glyphs in order: masks in `color` through the mask-gamma row, colour glyphs as they are.
fn composite_color(
    lut: &MaskGammaLut,
    placed: &[(i32, Arc<Mask>)],
    [left, top]: [i32; 2],
    [width, height]: [u32; 2],
    color: [u8; 3],
) -> RunImage {
    let mut premul = vec![[0f32; 4]; width as usize * height as usize];
    for (x, mask) in placed {
        let bpp = if mask.color { 4 } else { 1 };
        for row in 0..mask.height as usize {
            let y = (mask.top - top) as usize + row;
            let dst_start = y * width as usize + (x + mask.left - left) as usize;
            for col in 0..mask.width as usize {
                let i = (row * mask.width as usize + col) * bpp;
                let src = if mask.color {
                    let p = &mask.data[i..i + 4];
                    [p[0], p[1], p[2], p[3]].map(f32::from)
                } else {
                    let raw = mask.data[i];
                    if raw == 0 {
                        continue;
                    }
                    let a = f32::from(lut.apply(color, raw));
                    [
                        f32::from(color[0]) * a / 255.0,
                        f32::from(color[1]) * a / 255.0,
                        f32::from(color[2]) * a / 255.0,
                        a,
                    ]
                };
                if src[3] == 0.0 {
                    continue;
                }
                let d = &mut premul[dst_start + col];
                let keep = 1.0 - src[3] / 255.0;
                for k in 0..4 {
                    d[k] = src[k] + d[k] * keep;
                }
            }
        }
    }
    let mut alpha = Vec::with_capacity(premul.len());
    let mut rgba = Vec::with_capacity(premul.len() * 4);
    for p in premul {
        let a = p[3].round().clamp(0.0, 255.0);
        alpha.push(a as u8);
        let straight = |v: f32| {
            if a == 0.0 {
                0
            } else {
                (v * 255.0 / p[3]).round().clamp(0.0, 255.0) as u8
            }
        };
        rgba.extend_from_slice(&[straight(p[0]), straight(p[1]), straight(p[2]), a as u8]);
    }
    RunImage {
        left,
        top,
        width,
        height,
        alpha,
        rgba: Some(rgba),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::FixedAdvance;

    fn style(css: &str) -> Style {
        Style::new(FontSpec::parse(css).unwrap())
    }

    #[test]
    fn parses_css_declarations() {
        let s = Style::from_css([
            ("font", "bold 14px system-ui"),
            ("line-height", "30px"),
            ("text-align", "right"),
            ("direction", "rtl"),
            ("white-space", "nowrap"),
            ("text-overflow", "ellipsis"),
        ])
        .unwrap();
        assert_eq!(s.font.weight, 700.0);
        assert_eq!(s.line_height, LineHeight::Px(30.0));
        assert_eq!(
            (s.align, s.direction, s.white_space, s.overflow),
            (
                Align::Right,
                Direction::Rtl,
                WhiteSpace::NoWrap,
                Overflow::Ellipsis
            )
        );
        let one = Style::from_css([("font", "11px system-ui"), ("line-height", "1")]).unwrap();
        assert_eq!(one.line_height, LineHeight::Number(1.0));
        assert_eq!(Style::from_css([("line-height", "1")]), None);
        assert_eq!(
            Style::from_css([("font", "12px serif"), ("color", "red")]),
            None
        );
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

    #[test]
    fn rtl_paragraphs_move_edge_neutrals() {
        let runs = |t: &str| visual_runs(t, Direction::Rtl);
        assert_eq!(runs("/Drive/Pose"), ["Drive/Pose", "/"]);
        assert_eq!(
            runs("/Module0/Speed (m/s)"),
            ["(", "Module0/Speed (m/s", "/"]
        );
        assert_eq!(runs("Alerts 3"), ["Alerts 3"]);
        assert_eq!(runs("12.5 m"), ["m", " ", "12.5"]);
        assert_eq!(visual_runs("/a", Direction::Ltr), ["/a"]);
    }

    #[test]
    fn rtl_ellipsis_keeps_the_trailing_run() {
        let mut s = style("10px sans-serif");
        s.overflow = Overflow::Ellipsis;
        s.white_space = WhiteSpace::NoWrap;
        s.direction = Direction::Rtl;
        s.align = Align::Right;
        let l = layout(&mut FixedAdvance, &s, "/abcdefgh", Some(lu_ceil(30.0)), 1.0);
        let texts: Vec<(&str, Lu)> = l.lines[0]
            .fragments
            .iter()
            .map(|f| (f.text.as_str(), f.x))
            .collect();
        let slash = lu_ceil(30.0) - lu_ceil(6.0);
        let kept = slash - lu_ceil(18.0);
        assert_eq!(
            texts,
            [(ELLIPSIS, kept - lu_ceil(6.0)), ("fgh", kept), ("/", slash)]
        );
    }
    #[test]
    fn letter_spacing_follows_every_character() {
        let mut backend = FixedAdvance;
        let plain = layout(&mut backend, &style("16px system-ui"), "ab", None, 1.0);
        let spaced_style =
            Style::from_css([("font", "16px system-ui"), ("letter-spacing", "0.5px")]).unwrap();
        let spaced = layout(&mut backend, &spaced_style, "ab", None, 1.0);
        assert!((spaced.width - plain.width - 64).abs() <= 1);
        let x = |l: &Layout| l.lines[0].fragments[0].glyphs[1].x;
        assert_eq!(x(&spaced) - x(&plain), 0.5);
    }

    #[test]
    fn plaintext_takes_the_first_strong_direction() {
        let mut s = Style::from_css([
            ("font", "14px system-ui"),
            ("direction", "rtl"),
            ("text-align", "right"),
            ("unicode-bidi", "plaintext"),
        ])
        .unwrap();
        let texts = |s: &Style| {
            layout(&mut FixedAdvance, s, "/Drive/Left", None, 1.0).lines[0]
                .fragments
                .iter()
                .map(|f| f.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(&s), ["/Drive/Left"]);
        s.bidi_plaintext = false;
        assert_eq!(texts(&s), ["Drive/Left", "/"]);
    }
}
