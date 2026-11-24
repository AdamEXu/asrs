// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Blink's `HarfBuzzShaper` for the platforms whose scaler measures advances itself.

use std::collections::HashMap;

use rustybuzz::{BufferClusterLevel, UnicodeBuffer};

use super::shaping::{effective, emoji_clusters, hb_scale, script_runs, words};
use super::{FontSpec, Glyph, Shaped, ShapedGlyph};

/// The sized fonts a backend shapes with.
pub(crate) trait Faces {
    fn shaper(&self, face: u32) -> &rustybuzz::Face<'static>;

    fn upem(&self, face: u32) -> u16;

    /// `SkFont::getWidth` at `size`.
    fn advance(&mut self, face: u32, id: u32, size: f32) -> f32;

    fn primary(&mut self, spec: &FontSpec) -> Option<u32>;

    /// The standard font Blink tries after the families of every font list.
    fn standard(&mut self, spec: &FontSpec) -> Option<u32>;

    /// `FontCache::PlatformFallbackFontForCharacter`.
    fn substitute(&mut self, spec: &FontSpec, c: char, emoji: bool) -> Option<u32>;
}

struct ShapedRun {
    start: usize,
    face: u32,
    glyphs: Vec<RunGlyph>,
}

#[derive(Clone, Copy)]
struct RunGlyph {
    id: u32,
    cluster: usize,
    /// Font units the shaper moved the advance by, or `None` where it zeroed it.
    kern: Option<i32>,
    offset: i32,
}

const EMPTY: Shaped = Shaped {
    glyphs: Vec::new(),
    width: 0.0,
};

/// `TextBackend::shape`.
pub(crate) fn shape(faces: &mut dyn Faces, spec: &FontSpec, text: &str, zoom: f32) -> Shaped {
    let spec = &effective(spec, zoom);
    let Some(primary) = (spec.size > 0.0).then(|| faces.primary(spec)).flatten() else {
        return EMPTY;
    };
    let mut shaped = EMPTY;
    let mut start = 0;
    for word in words(text) {
        shape_word(faces, spec, primary, word, zoom, start, &mut shaped);
        start += word.encode_utf16().count() as u32;
    }
    shaped
}

/// `TextBackend::shape_run`.
pub(crate) fn shape_run(faces: &mut dyn Faces, spec: &FontSpec, text: &str, zoom: f32) -> Shaped {
    let spec = &effective(spec, zoom);
    let mut shaped = EMPTY;
    if spec.size > 0.0
        && !text.is_empty()
        && let Some(primary) = faces.primary(spec)
    {
        shape_word(faces, spec, primary, text, zoom, 0, &mut shaped);
    }
    shaped
}

fn shape_text(faces: &dyn Faces, face: u32, text: &str, base: usize) -> Vec<RunGlyph> {
    let shaper = faces.shaper(face);
    let mut buffer = UnicodeBuffer::new();
    buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let shaped = rustybuzz::shape(shaper, &[], buffer);
    shaped
        .glyph_infos()
        .iter()
        .zip(shaped.glyph_positions())
        .map(|(info, pos)| {
            let id = rustybuzz::ttf_parser::GlyphId(info.glyph_id as u16);
            let advance = i32::from(shaper.glyph_hor_advance(id).unwrap_or(0));
            RunGlyph {
                id: info.glyph_id,
                cluster: base + info.cluster as usize,
                kern: (pos.x_advance != 0 || advance == 0).then(|| pos.x_advance - advance),
                offset: pos.x_offset,
            }
        })
        .collect()
}

/// Shapes `word[from..to]` with `face`, keeping the runs it has glyphs for and returning the byte
/// ranges it has none for.
fn shape_range(
    faces: &dyn Faces,
    face: u32,
    word: &str,
    (from, to): (usize, usize),
    out: &mut Vec<ShapedRun>,
) -> Vec<(usize, usize)> {
    let glyphs = shape_text(faces, face, &word[from..to], from);
    let mut starts: Vec<usize> = glyphs.iter().map(|g| g.cluster).collect();
    starts.sort_unstable();
    starts.dedup();
    let end_of = |cluster: usize| starts.iter().copied().find(|&s| s > cluster).unwrap_or(to);
    let is_missing = |cluster: usize| glyphs.iter().any(|g| g.cluster == cluster && g.id == 0);
    let mut missing: Vec<(usize, usize)> = Vec::new();
    let mut run: Option<ShapedRun> = None;
    for &cluster in &starts {
        if is_missing(cluster) {
            out.extend(run.take());
            let end = end_of(cluster);
            match missing.last_mut() {
                Some(last) if last.1 == cluster => last.1 = end,
                _ => missing.push((cluster, end)),
            }
            continue;
        }
        let run = run.get_or_insert_with(|| ShapedRun {
            start: cluster,
            face,
            glyphs: Vec::new(),
        });
        run.glyphs
            .extend(glyphs.iter().filter(|g| g.cluster == cluster));
    }
    out.extend(run);
    missing
}

/// Blink's `HarfBuzzShaper` with its `FontFallbackIterator`: the emoji font first for emoji
/// presentation, then the font itself, the standard font, then a system fallback by character,
/// and the font's missing glyph for what no font has.
fn shape_segment(
    faces: &mut dyn Faces,
    spec: &FontSpec,
    primary: u32,
    word: &str,
    range: (usize, usize),
    emoji: bool,
    out: &mut Vec<ShapedRun>,
) {
    let mut tried = Vec::new();
    if emoji && let Some(c) = word[range.0..range.1].chars().next() {
        tried.extend(faces.substitute(spec, c, true));
    }
    tried.push(primary);
    let mut pending = vec![range];
    let shape_pending = |faces: &dyn Faces, face, pending: Vec<_>, out: &mut _| -> Vec<_> {
        pending
            .into_iter()
            .flat_map(|r| shape_range(faces, face, word, r, out))
            .collect()
    };
    for &face in &tried {
        pending = shape_pending(faces, face, pending, out);
    }
    if !pending.is_empty()
        && let Some(face) = faces.standard(spec)
        && !tried.contains(&face)
    {
        tried.push(face);
        pending = shape_pending(faces, face, pending, out);
    }
    while let Some(range) = pending.pop() {
        let Some(c) = word[range.0..range.1].chars().next() else {
            continue;
        };
        match faces.substitute(spec, c, false) {
            Some(face) if !tried.contains(&face) => {
                tried.push(face);
                pending.extend(shape_range(faces, face, word, range, out));
            }
            Some(face) if face != primary && !emoji => {
                let glyphs = shape_text(faces, face, &word[range.0..range.1], range.0);
                out.push(ShapedRun {
                    start: range.0,
                    face,
                    glyphs,
                });
            }
            _ => out.push(ShapedRun {
                start: range.0,
                face: primary,
                glyphs: shape_text(faces, primary, &word[range.0..range.1], range.0),
            }),
        }
    }
}

/// One word appended to `out`, shaped at zoom `zoom`: the pen advances by 16.16 advances from the
/// scaler plus the shaper's adjustments scaled to 16.16, accumulated in `f32`.
fn shape_word(
    faces: &mut dyn Faces,
    spec: &FontSpec,
    primary: u32,
    word: &str,
    zoom: f32,
    start: u32,
    out: &mut Shaped,
) {
    let units = word.encode_utf16().count() as isize;
    let mut bytes: HashMap<isize, usize> = HashMap::new();
    let mut unit = 0;
    for (at, c) in word.char_indices() {
        bytes.insert(unit, at);
        unit += c.len_utf16() as isize;
    }
    bytes.insert(units, word.len());
    let mut runs = Vec::new();
    let clusters = emoji_clusters(word, 0);
    for segment in clusters.chunk_by(|a, b| a.2 == b.2) {
        let (from, to) = (bytes[&segment[0].0], bytes[&segment[segment.len() - 1].1]);
        for (start, end) in script_runs(&word[from..to]) {
            let range = (from + start, from + end);
            shape_segment(faces, spec, primary, word, range, segment[0].2, &mut runs);
        }
    }
    runs.sort_by_key(|r| r.start);
    let size = spec.size * zoom;
    let mut pen = 0.0f32;
    for run in runs {
        let upem = u32::from(faces.upem(run.face));
        for g in run.glyphs {
            let offset = hb_scale(i64::from(g.offset), upem, size) as f32 / 65536.0;
            out.glyphs.push(ShapedGlyph {
                glyph: Glyph {
                    face: run.face,
                    id: g.id,
                },
                x: out.width + pen + offset / zoom,
                cluster: start + word[..g.cluster].encode_utf16().count() as u32,
            });
            let Some(kern) = g.kern else {
                continue;
            };
            let advance = (faces.advance(run.face, g.id, size) * 65536.0) as i32;
            let total = advance + hb_scale(i64::from(kern), upem, size);
            pen += total as f32 / 65536.0 / zoom;
        }
    }
    out.width += pen;
}
