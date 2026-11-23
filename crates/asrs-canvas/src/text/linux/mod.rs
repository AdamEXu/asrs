// Ported from Skia and Blink, Chromium m142 (BSD-3-Clause, Copyright Google Inc. and The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Linux text as Chromium 142 draws it. Its system fonts are Skia's Fontations typefaces, not
//! FreeType ones: skrifa's port of FreeType's light autohinter makes the outlines and Skia's own
//! analytic antialiasing fills them.

mod bitmap;
pub mod fontconfig;
mod matrix;
mod scaler;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use rustybuzz::{BufferClusterLevel, UnicodeBuffer};

use self::fontconfig::{Fallback, Hinting, Match, RenderStyle};
use self::matrix::M2;
use self::scaler::Typeface;
use super::font::FontKey;
use super::shaping::{emoji_clusters, hb_scale, round, words};
use super::{
    Bounds, Family, FontMetrics, FontSpec, Glyph, LcdMask, Mask, Shaped, ShapedGlyph, TextBackend,
    relax,
};
use crate::path::Affine;
use crate::skpath::SkPath;

const EMOJI_LOCALE: &str = "und-Zsye";
const LOCALE: &str = "en-US";
/// The character whose fallback font Blink takes as the colour emoji font.
const EMOJI: char = '\u{1f46a}';
const BOLD: f32 = 600.0;
/// The size Skia measures linear-metrics fonts at.
const CANONICAL_SIZE: f32 = 64.0;
const LCD_MAX_SIZE: f32 = 48.0;

struct Face {
    typeface: usize,
    size: f32,
    /// Blink's synthetic italic.
    skew: bool,
    style: RenderStyle,
    fallback: bool,
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

/// The device matrix of text at `scale`, compressed horizontally by `x_scale`.
fn upright(scale: f64, x_scale: f64) -> M2 {
    M2 {
        sx: relax((scale * x_scale) as f32),
        sy: relax(scale as f32),
        ..M2::IDENTITY
    }
}

/// `FontDescription::EffectiveFontSize`: Blink floors a font's zoomed size to 1/100 px.
fn effective(spec: &FontSpec, zoom: f32) -> FontSpec {
    let zoomed = spec.size * zoom;
    let floored = (zoomed * 100.0).floor() / 100.0;
    if floored == zoomed {
        return *spec;
    }
    FontSpec {
        size: floored / zoom,
        ..*spec
    }
}

/// Skia's Fontations scaler over fontconfig, configured as Chromium configures it on Linux.
pub struct Fontations {
    typefaces: Vec<Typeface>,
    files: HashMap<(PathBuf, u32), Option<usize>>,
    faces: Vec<Face>,
    primary: HashMap<FontKey, u32>,
    substitutes: HashMap<(FontKey, usize), u32>,
    fallback: Fallback,
    system_family: String,
    device_scale: f32,
}

impl Fontations {
    /// `None` without a single font fontconfig can match.
    pub fn new() -> Option<Fontations> {
        let mut backend = Fontations {
            typefaces: Vec::new(),
            files: HashMap::new(),
            faces: Vec::new(),
            primary: HashMap::new(),
            substitutes: HashMap::new(),
            fallback: Fallback::default(),
            system_family: fontconfig::resolve_family("system-ui").unwrap_or_default(),
            device_scale: 1.0,
        };
        let spec = FontSpec::default();
        backend.resolve(&spec)?;
        Some(backend)
    }

    /// The file and face index behind a font, for checks against the pinned fonts.
    pub fn font_file(&mut self, spec: &FontSpec) -> Option<(PathBuf, u32)> {
        let found = self.resolve(spec)?.1;
        Some((found.path, found.index))
    }

    fn typeface(&mut self, found: &Match) -> Option<usize> {
        let key = (found.path.clone(), found.index);
        if let Some(&known) = self.files.get(&key) {
            return known;
        }
        let loaded = Typeface::load(found).map(|typeface| {
            self.typefaces.push(typeface);
            self.typefaces.len() - 1
        });
        self.files.insert(key, loaded);
        loaded
    }

    /// The family and face of a font: Chromium's default family for a generic name or the named
    /// one, Blink's alternate name for it, then `GetLastResortFallbackFont`.
    fn resolve(&mut self, spec: &FontSpec) -> Option<(String, Match, usize)> {
        let system = self.system_family.clone();
        let families = match spec.family {
            Family::System => [system.as_str(), ""],
            Family::SansSerif | Family::Arial | Family::HelveticaNeue => ["Arial", "Helvetica"],
            Family::Serif => ["Times New Roman", "Times"],
            Family::Monospace => ["Monospace", ""],
            Family::Courier => ["Courier", "Courier New"],
        };
        let named = families.into_iter().filter(|f| !f.is_empty());
        for family in named.chain(["Sans", "Serif", ""]) {
            if let Some(found) = fontconfig::match_family(family, spec.weight, spec.italic)
                && let Some(typeface) = self.typeface(&found)
            {
                return Some((family.to_owned(), found, typeface));
            }
        }
        None
    }

    fn primary(&mut self, spec: &FontSpec) -> Option<u32> {
        if let Some(&face) = self.primary.get(&spec.key()) {
            return Some(face);
        }
        let (family, _, typeface) = self.resolve(spec)?;
        let bold = self.typefaces[typeface].weight >= BOLD;
        let italic = self.typefaces[typeface].italic;
        let style = fontconfig::render_style(&family, spec.size, bold, italic);
        self.faces.push(Face {
            typeface,
            size: spec.size,
            skew: spec.italic && !italic,
            style,
            fallback: false,
        });
        let face = (self.faces.len() - 1) as u32;
        self.primary.insert(spec.key(), face);
        Some(face)
    }

    /// `FontCache::PlatformFallbackFontForCharacter`.
    fn substitute(&mut self, spec: &FontSpec, c: char, emoji: bool) -> Option<u32> {
        if !emoji && (spec.italic || spec.weight >= BOLD) {
            let regular = FontSpec {
                weight: 400.0,
                italic: false,
                ..*spec
            };
            if let Some((_, _, typeface)) = self.resolve(&regular)
                && self.typefaces[typeface].has_char(c)
            {
                return Some(self.substitute_face(spec, typeface));
            }
        }
        let (c, locale) = if emoji {
            (EMOJI, EMOJI_LOCALE)
        } else {
            (c, LOCALE)
        };
        let found = self.fallback.font_for(c, locale)?;
        let typeface = self.typeface(&found)?;
        Some(self.substitute_face(spec, typeface))
    }

    fn substitute_face(&mut self, spec: &FontSpec, typeface: usize) -> u32 {
        if let Some(&face) = self.substitutes.get(&(spec.key(), typeface)) {
            return face;
        }
        self.faces.push(Face {
            typeface,
            size: spec.size,
            skew: spec.italic && !self.typefaces[typeface].italic,
            style: fontconfig::render_style("", 0.0, false, false),
            fallback: true,
        });
        let face = (self.faces.len() - 1) as u32;
        self.substitutes.insert((spec.key(), typeface), face);
        face
    }

    /// Chromium's `GetFontRenderParams`: subpixel positioning, which linear metrics come with,
    /// is for displays scaled above 1 and turns hinting off.
    fn linear(&self) -> bool {
        self.device_scale > 1.0
    }

    fn hinting(&self, face: &Face) -> Hinting {
        if self.linear() {
            Hinting::None
        } else {
            face.style.hinting
        }
    }

    /// `SkFont::getWidth` at `size`.
    fn advance(&mut self, face: u32, id: u32, size: f32) -> f32 {
        if self.linear() {
            return self.strike_advance(face, id, CANONICAL_SIZE) * (size / CANONICAL_SIZE);
        }
        self.strike_advance(face, id, size)
    }

    fn strike_advance(&mut self, face: u32, id: u32, size: f32) -> f32 {
        let face = &self.faces[face as usize];
        let (index, skew) = (face.typeface, face.skew);
        let (hinting, autohint) = (self.hinting(face), face.style.autohint);
        let typeface = &self.typefaces[index];
        if typeface.color
            && let Some(strike) = bitmap::strike(&typeface.font, id, size)
        {
            return strike.advance * (size / strike.ppem);
        }
        let linear = typeface.advance(id, size);
        if matches!(hinting, Hinting::None | Hinting::Slight) || skew {
            return linear;
        }
        match self.typefaces[index].outline(id, size, hinting, autohint) {
            Some((_, Some(hinted))) if hinted.round() != 0.0 => hinted.round(),
            _ => linear,
        }
    }

    /// The glyph outline under `post`, as `generatePath` and `internalGetPath` make it.
    fn device_path(
        &mut self,
        glyph: Glyph,
        size: f32,
        post: &M2,
        phase: (f32, f32),
        hinted: bool,
    ) -> Option<SkPath> {
        let face = self.faces.get(glyph.face as usize)?;
        let split = matrix::split(size, face.skew, post)?;
        let hinting = if hinted && matrix::is_axis_aligned(face.skew, post) {
            self.hinting(face)
        } else {
            Hinting::None
        };
        let (index, autohint) = (face.typeface, face.style.autohint);
        let (mut path, _) =
            self.typefaces[index].outline(glyph.id, split.size, hinting, autohint)?;
        let m = split.remaining;
        if m != M2::IDENTITY {
            path = path.transformed(&Affine {
                a: f64::from(m.sx),
                b: f64::from(m.ky),
                c: f64::from(m.kx),
                d: f64::from(m.sy),
                e: 0.0,
                f: 0.0,
            });
        }
        if phase != (0.0, 0.0) {
            path = path.transformed(&Affine::translation(f64::from(phase.0), f64::from(phase.1)));
        }
        Some(path)
    }

    fn mask(&mut self, glyph: Glyph, post: &M2, phase: (u8, u8)) -> Option<Arc<Mask>> {
        let face = self.faces.get(glyph.face as usize)?;
        let size = face.size;
        let phase = (f32::from(phase.0) / 4.0, f32::from(phase.1) / 4.0);
        if self.typefaces[face.typeface].color
            && let Some(mask) = self.color_mask(glyph, size, post, phase)
        {
            return Some(Arc::new(mask));
        }
        let path = self.device_path(glyph, size, post, phase, true)?;
        scaler::gray_mask(&path).map(Arc::new)
    }

    fn color_mask(&self, glyph: Glyph, size: f32, post: &M2, phase: (f32, f32)) -> Option<Mask> {
        let face = &self.faces[glyph.face as usize];
        let split = matrix::split(size, face.skew, post)?;
        let font = &self.typefaces[face.typeface].font;
        let strike = bitmap::strike(font, glyph.id, split.size)?;
        let (width, height) = strike.dimensions()?;
        let (matrix, (x, y)) = strike.placement(split.size, &split.remaining);
        let moved = (x + phase.0, y + phase.1);
        let [l, t, r, b] = bitmap::bounds(width, height, &matrix, moved);
        let (left, top) = (l.floor() as i32, t.floor() as i32);
        let (w, h) = (
            (r.ceil() as i32 - left) as u32,
            (b.ceil() as i32 - top) as u32,
        );
        // `generatePngImage` moves the glyph by its phase before the remaining matrix, where
        // `generateMetrics` moves its bounds after it.
        let (px, py) = split.remaining.map(phase.0, phase.1);
        let local = (x + px - left as f32, y + py - top as f32);
        Some(Mask {
            left,
            top,
            width: w,
            height: h,
            data: bitmap::draw(strike.decode()?, &matrix, local, w, h)?,
            color: true,
        })
    }

    /// The integer ink box `SkFont::getBounds` measures at `size`, `[left, top, right, bottom]`.
    fn strike_bounds(&mut self, glyph: Glyph, size: f32, hinted: bool) -> Option<[f32; 4]> {
        let face = self.faces.get(glyph.face as usize)?;
        let typeface = &self.typefaces[face.typeface];
        if typeface.color {
            let split = matrix::split(size, face.skew, &M2::IDENTITY)?;
            if let Some(strike) = bitmap::strike(&typeface.font, glyph.id, split.size) {
                let (width, height) = strike.dimensions()?;
                let (matrix, translate) = strike.placement(split.size, &split.remaining);
                let [l, t, r, b] = bitmap::bounds(width, height, &matrix, translate);
                return Some([l.floor(), t.floor(), r.ceil(), b.ceil()]);
            }
        }
        let path = self.device_path(glyph, size, &M2::IDENTITY, (0.0, 0.0), hinted)?;
        let [left, top, width, height] = scaler::mask_bounds(&path)?;
        Some([
            left as f32,
            top as f32,
            (left + width) as f32,
            (top + height) as f32,
        ])
    }

    fn vertical_metrics(&self, face: u32, size: f32) -> FontMetrics {
        let typeface = &self.typefaces[self.faces[face as usize].typeface];
        let metrics = typeface.metrics(size);
        let mut ascent = round(metrics.ascent);
        let mut descent = round(-metrics.descent);
        if descent < -metrics.descent && self.linear() && ascent >= 1.0 {
            descent += 1.0;
            ascent -= 1.0;
        }
        FontMetrics {
            ascent,
            descent,
            line_gap: round(metrics.leading),
            typo: typeface.typo,
        }
    }

    fn shape_text(&self, face: u32, text: &str, base: usize) -> Vec<RunGlyph> {
        let typeface = &self.typefaces[self.faces[face as usize].typeface];
        let shaper = &typeface.shaper;
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

    /// Shapes `word[from..to]` with `face`, keeping the runs it has glyphs for and returning the
    /// byte ranges it has none for.
    fn shape_range(
        &self,
        face: u32,
        word: &str,
        (from, to): (usize, usize),
        out: &mut Vec<ShapedRun>,
    ) -> Vec<(usize, usize)> {
        let glyphs = self.shape_text(face, &word[from..to], from);
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
    /// presentation, then the font itself, then a system fallback by character, and the font's
    /// missing glyph for what no font has.
    fn shape_segment(
        &mut self,
        spec: &FontSpec,
        primary: u32,
        word: &str,
        range: (usize, usize),
        emoji: bool,
        out: &mut Vec<ShapedRun>,
    ) {
        let mut tried = Vec::new();
        if emoji {
            tried.extend(self.substitute(spec, EMOJI, true));
        }
        tried.push(primary);
        let mut pending = vec![range];
        for &face in &tried {
            pending = pending
                .into_iter()
                .flat_map(|r| self.shape_range(face, word, r, out))
                .collect();
        }
        while let Some(range) = pending.pop() {
            let Some(c) = word[range.0..range.1].chars().next() else {
                continue;
            };
            match self.substitute(spec, c, false) {
                Some(face) if !tried.contains(&face) => {
                    tried.push(face);
                    pending.extend(self.shape_range(face, word, range, out));
                }
                Some(face) if face != primary && !emoji => {
                    let glyphs = self.shape_text(face, &word[range.0..range.1], range.0);
                    out.push(ShapedRun {
                        start: range.0,
                        face,
                        glyphs,
                    });
                }
                _ => out.push(ShapedRun {
                    start: range.0,
                    face: primary,
                    glyphs: self.shape_text(primary, &word[range.0..range.1], range.0),
                }),
            }
        }
    }

    /// One word shaped at zoom `zoom`: the pen advances by 16.16 advances from the scaler plus
    /// the shaper's adjustments scaled to 16.16, accumulated in `f32`.
    #[allow(clippy::too_many_arguments)]
    fn shape_word(
        &mut self,
        spec: &FontSpec,
        primary: u32,
        word: &str,
        zoom: f32,
        origin: f32,
        start: u32,
        out: &mut Vec<ShapedGlyph>,
    ) -> f32 {
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
            let range = (bytes[&segment[0].0], bytes[&segment[segment.len() - 1].1]);
            self.shape_segment(spec, primary, word, range, segment[0].2, &mut runs);
        }
        runs.sort_by_key(|r| r.start);
        let size = spec.size * zoom;
        let mut pen = 0.0f32;
        for run in runs {
            let upem = u32::from(self.typefaces[self.faces[run.face as usize].typeface].upem);
            for g in run.glyphs {
                let offset = hb_scale(i64::from(g.offset), upem, size) as f32 / 65536.0;
                out.push(ShapedGlyph {
                    glyph: Glyph {
                        face: run.face,
                        id: g.id,
                    },
                    x: origin + pen + offset / zoom,
                    cluster: start + word[..g.cluster].encode_utf16().count() as u32,
                });
                let Some(kern) = g.kern else {
                    continue;
                };
                let advance = (self.advance(run.face, g.id, size) * 65536.0) as i32;
                let total = advance + hb_scale(i64::from(kern), upem, size);
                pen += total as f32 / 65536.0 / zoom;
            }
        }
        pen
    }
}

impl TextBackend for Fontations {
    fn shape(&mut self, spec: &FontSpec, text: &str, zoom: f32) -> Shaped {
        let spec = &effective(spec, zoom);
        let mut glyphs = Vec::new();
        let primary = (spec.size > 0.0).then(|| self.primary(spec)).flatten();
        let Some(primary) = primary else {
            return Shaped { glyphs, width: 0.0 };
        };
        let mut width = 0.0f32;
        let mut start = 0;
        for word in words(text) {
            width += self.shape_word(spec, primary, word, zoom, width, start, &mut glyphs);
            start += word.encode_utf16().count() as u32;
        }
        Shaped { glyphs, width }
    }

    fn shape_run(&mut self, spec: &FontSpec, text: &str, zoom: f32) -> Shaped {
        let spec = &effective(spec, zoom);
        let mut glyphs = Vec::new();
        let primary = (spec.size > 0.0 && !text.is_empty())
            .then(|| self.primary(spec))
            .flatten();
        let Some(primary) = primary else {
            return Shaped { glyphs, width: 0.0 };
        };
        let width = self.shape_word(spec, primary, text, zoom, 0.0, 0, &mut glyphs);
        Shaped { glyphs, width }
    }

    fn metrics(&mut self, spec: &FontSpec) -> FontMetrics {
        let spec = &effective(spec, 1.0);
        let primary = (spec.size > 0.0).then(|| self.primary(spec)).flatten();
        match primary {
            Some(face) => self.vertical_metrics(face, spec.size),
            None => FontMetrics {
                ascent: 0.0,
                descent: 0.0,
                line_gap: 0.0,
                typo: None,
            },
        }
    }

    fn fallback_metrics(&mut self, glyph: Glyph, zoom: f32) -> Option<FontMetrics> {
        let face = self.faces.get(glyph.face as usize)?;
        face.fallback
            .then(|| self.vertical_metrics(glyph.face, face.size * zoom))
    }

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds> {
        let size = self.faces.get(glyph.face as usize)?.size;
        let bounds = if self.linear() {
            let scale = size / CANONICAL_SIZE;
            self.strike_bounds(glyph, CANONICAL_SIZE, false)
                .map(|b| b.map(|v| v * scale))
        } else {
            self.strike_bounds(glyph, size, true)
        };
        let [left, top, right, bottom] = bounds.unwrap_or_default();
        Some(Bounds {
            left,
            top,
            right,
            bottom,
        })
    }

    fn raw_mask(&mut self, glyph: Glyph, scale: f64, x_scale: f64, phase: u8) -> Option<Arc<Mask>> {
        self.mask(glyph, &upright(scale, x_scale), (phase, 0))
    }

    fn raw_mask_transformed(
        &mut self,
        glyph: Glyph,
        [a, b, c, d]: [f64; 4],
        phase: (u8, u8),
    ) -> Option<Arc<Mask>> {
        let post = M2 {
            sx: relax(a as f32),
            kx: relax(c as f32),
            ky: relax(b as f32),
            sy: relax(d as f32),
        };
        self.mask(glyph, &post, phase)
    }

    fn raw_lcd_mask(
        &mut self,
        glyph: Glyph,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Arc<LcdMask>> {
        let face = self.faces.get(glyph.face as usize)?;
        let post = upright(scale, x_scale);
        let device_size = face.size * post.sy;
        if !face.style.lcd || device_size > LCD_MAX_SIZE || self.typefaces[face.typeface].color {
            return None;
        }
        let phase = (f32::from(phase) / 4.0, 0.0);
        let path = self.device_path(glyph, face.size, &post, phase, true)?;
        scaler::lcd_mask(&path).map(Arc::new)
    }

    fn set_device_scale(&mut self, scale: f32) -> bool {
        let changed = (scale > 1.0) != self.linear();
        self.device_scale = scale;
        changed
    }
}
