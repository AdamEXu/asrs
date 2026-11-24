// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2006-2023 Google Inc.). See THIRD_PARTY_NOTICES.md.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use skrifa::instance::{Location, LocationRef, Size};
use skrifa::metrics::{GlyphMetrics, Metrics};
use skrifa::outline::{
    DrawSettings, Engine, GlyphStyles, HintingInstance, HintingMode, HintingOptions,
    OutlineGlyphCollection, OutlineGlyphFormat, OutlinePen, SmoothMode, Target,
};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};

use super::fontconfig::{Hinting, Match};
use crate::hairline::P;
use crate::skpath::SkPath;

/// A font file's bytes, read once per process: faces borrow them forever.
fn font_bytes(path: &Path) -> Option<&'static [u8]> {
    static FILES: Mutex<Option<HashMap<PathBuf, &'static [u8]>>> = Mutex::new(None);
    let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
    let files = files.get_or_insert_default();
    if let Some(data) = files.get(path) {
        return Some(data);
    }
    let data: &'static [u8] = Vec::leak(std::fs::read(path).ok()?);
    files.insert(path.to_owned(), data);
    Some(data)
}

/// One face of a font file, as `SkTypeface_Fontations` holds it.
pub struct Typeface {
    pub font: FontRef<'static>,
    location: Location,
    pub shaper: rustybuzz::Face<'static>,
    outlines: OutlineGlyphCollection<'static>,
    styles: OnceLock<GlyphStyles>,
    hinting: HashMap<(u32, Hinting, bool), Option<HintingInstance>>,
    pub weight: f32,
    pub italic: bool,
    pub color: bool,
    pub upem: u16,
    /// OS/2 `sTypoAscender` and the negated `sTypoDescender`.
    pub typo: Option<(f32, f32)>,
}

#[derive(Default)]
struct Pen {
    path: SkPath,
    started: bool,
    current: P,
}

impl Pen {
    fn point(x: f32, y: f32) -> P {
        P { x, y: -y }
    }

    fn going_to(&mut self, p: P) {
        if !self.started {
            self.started = true;
            self.path.move_to(self.current);
        }
        self.current = p;
    }

    fn is_current(&self, p: P) -> bool {
        self.current.x == p.x && self.current.y == p.y
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.started {
            self.close();
            self.started = false;
        }
        self.current = Pen::point(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = Pen::point(x, y);
        if !self.is_current(p) {
            self.going_to(p);
            self.path.line_to(p);
        }
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (c, p) = (Pen::point(cx, cy), Pen::point(x, y));
        if !(self.is_current(c) && self.is_current(p)) {
            self.going_to(p);
            self.path.quad_to(c, p);
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (Pen::point(cx0, cy0), Pen::point(cx1, cy1), Pen::point(x, y));
        if !(self.is_current(c0) && self.is_current(c1) && self.is_current(p)) {
            self.going_to(p);
            self.path.cubic_to(c0, c1, p);
        }
    }

    fn close(&mut self) {
        self.path.close();
    }
}

impl Typeface {
    pub fn load(found: &Match) -> Option<Typeface> {
        Typeface::new(font_bytes(&found.path)?, found)
    }

    fn new(data: &'static [u8], found: &Match) -> Option<Typeface> {
        let face_index = found.index & 0xffff;
        let font = FontRef::from_index(data, face_index).ok()?;
        let instance = (found.index >> 16) as usize;
        let mut location = Location::default();
        let mut shaper = rustybuzz::Face::from_slice(data, face_index)?;
        if instance > 0
            && let Some(named) = font.named_instances().get(instance - 1)
        {
            location = named.location();
            let variations: Vec<rustybuzz::Variation> = font
                .axes()
                .iter()
                .zip(named.user_coords())
                .map(|(axis, value)| rustybuzz::Variation {
                    tag: rustybuzz::ttf_parser::Tag::from_bytes(&axis.tag().to_be_bytes()),
                    value,
                })
                .collect();
            shaper.set_variations(&variations);
        }
        let typo = font.os2().ok().map(|os2| {
            (
                f32::from(os2.s_typo_ascender()),
                -f32::from(os2.s_typo_descender()),
            )
        });
        Some(Typeface {
            shaper,
            outlines: font.outline_glyphs(),
            styles: OnceLock::new(),
            hinting: HashMap::new(),
            weight: found.weight,
            italic: found.italic,
            color: ["CBDT", "COLR", "sbix", "SVG "].iter().any(|tag| {
                let tag = skrifa::Tag::new(tag.as_bytes().try_into().expect("four bytes"));
                font.table_data(tag).is_some()
            }),
            upem: font.head().map(|h| h.units_per_em()).unwrap_or(0),
            typo,
            location,
            font,
        })
    }

    pub fn has_char(&self, c: char) -> bool {
        self.font.charmap().map(c).is_some_and(|g| g.to_u32() != 0)
    }

    pub fn metrics(&self, size: f32) -> Metrics {
        Metrics::new(&self.font, Size::new(size), &self.location)
    }

    pub fn advance(&self, glyph: u32, size: f32) -> f32 {
        GlyphMetrics::new(&self.font, Size::new(size), &self.location)
            .advance_width(GlyphId::new(glyph))
            .unwrap_or(0.0)
    }

    /// `make_hinting_instance` for the hinting Skia picks, or an unhinted outline.
    fn ensure_hinting(&mut self, size: f32, hinting: Hinting, autohint: bool) {
        let key = (size.to_bits(), hinting, autohint);
        if self.hinting.contains_key(&key) {
            return;
        }
        let outlines = &self.outlines;
        let coords = LocationRef::from(&self.location);
        let instance = if outlines.require_interpreter() {
            HintingInstance::new(outlines, Size::new(size), coords, HintingMode::Strong).ok()
        } else if hinting == Hinting::None {
            None
        } else {
            let force = hinting == Hinting::Slight
                && outlines.format() == Some(OutlineGlyphFormat::Glyf)
                || autohint;
            let engine = if force {
                let styles = self.styles.get_or_init(|| GlyphStyles::new(outlines));
                Engine::Auto(Some(styles.clone()))
            } else {
                Engine::AutoFallback
            };
            let mode = if hinting == Hinting::Slight {
                SmoothMode::Light
            } else {
                SmoothMode::Normal
            };
            let options = HintingOptions {
                engine,
                target: Target::Smooth {
                    mode,
                    symmetric_rendering: true,
                    preserve_linear_metrics: false,
                },
            };
            HintingInstance::new(outlines, Size::new(size), coords, options).ok()
        };
        self.hinting.insert(key, instance);
    }

    /// `generatePathForGlyphId`: the outline at `size`, y down.
    pub fn outline(
        &mut self,
        glyph: u32,
        size: f32,
        hinting: Hinting,
        autohint: bool,
    ) -> Option<(SkPath, Option<f32>)> {
        self.ensure_hinting(size, hinting, autohint);
        let outline = self.outlines.get(GlyphId::new(glyph))?;
        let settings = match &self.hinting[&(size.to_bits(), hinting, autohint)] {
            Some(instance) => DrawSettings::hinted(instance, false),
            None => DrawSettings::unhinted(Size::new(size), &self.location),
        };
        let mut pen = Pen::default();
        let adjusted = outline.draw(settings, &mut pen).ok()?;
        Some((pen.path, adjusted.advance_width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_font_file_is_read_once() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let [a, b] = [(); 2].map(|()| font_bytes(&path).expect("a file"));
        assert!(std::ptr::eq(a, b));
    }
}
