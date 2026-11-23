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
use crate::aaa;
use crate::blit::Spans;
use crate::hairline::P;
use crate::skpath::SkPath;
use crate::text::{LcdMask, Mask};

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

fn coverage(path: &SkPath, width: u32, height: u32) -> Vec<u8> {
    let mut spans = Spans::new(width, height);
    aaa::fill_path(path, width, height, false, &mut spans);
    let mut data = vec![0u8; (width * height) as usize];
    for (x, y, alpha) in spans.iter() {
        let start = y as usize * width as usize + x as usize;
        for (out, &a) in data[start..start + alpha.len()].iter_mut().zip(alpha) {
            if a != 0 {
                *out = a;
            }
        }
    }
    data
}

/// A device path's mask bounds, `[left, top, width, height]`.
pub fn mask_bounds(path: &SkPath) -> Option<[i32; 4]> {
    if path.is_empty() || !path.is_finite() {
        return None;
    }
    let [left, top, right, bottom] = path.bounds().round_out();
    (left < right && top < bottom).then_some([left, top, right - left, bottom - top])
}

/// `SkScalerContext::GenerateImageFromPath` for an A8 mask, before the preblend.
pub fn gray_mask(path: &SkPath) -> Option<Mask> {
    let [left, top, width, height] = mask_bounds(path)?;
    let (w, h) = (width as u32, height as u32);
    let local = path.transformed(&crate::path::Affine::translation(
        f64::from(-left),
        f64::from(-top),
    ));
    Some(Mask {
        left,
        top,
        width: w,
        height: h,
        data: coverage(&local, w, h),
        color: false,
    })
}

const LCD_FILTER: [[u32; 12]; 3] = [
    [
        0x03, 0x0b, 0x1c, 0x33, 0x40, 0x39, 0x24, 0x10, 0x05, 0x01, 0x00, 0x00,
    ],
    [
        0x00, 0x02, 0x08, 0x16, 0x2b, 0x3d, 0x3d, 0x2b, 0x16, 0x08, 0x02, 0x00,
    ],
    [
        0x00, 0x00, 0x01, 0x05, 0x10, 0x24, 0x39, 0x40, 0x33, 0x1c, 0x0b, 0x03,
    ],
];

/// `GenerateImageFromPath` and `pack4xHToMask` for a horizontal RGB LCD mask, before the preblend
/// and the RGB565 packing.
pub fn lcd_mask(path: &SkPath) -> Option<LcdMask> {
    let [left, top, inner, height] = mask_bounds(path)?;
    let (left, width) = (left - 1, inner + 2);
    let samples = inner as u32 * 4;
    let scaled = path.transformed(&crate::path::Affine {
        a: 4.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: f64::from(-(left + 1) * 4),
        f: f64::from(-top),
    });
    let source = coverage(&scaled, samples, height as u32);
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for row in source.chunks_exact(samples as usize) {
        for first in (-4..samples as i32 + 4).step_by(4) {
            let mut fir = [0u32; 3];
            for (k, at) in (first - 4..first + 8).enumerate() {
                let Some(&value) = usize::try_from(at).ok().and_then(|at| row.get(at)) else {
                    continue;
                };
                for (sum, filter) in fir.iter_mut().zip(&LCD_FILTER) {
                    *sum += filter[k] * u32::from(value);
                }
            }
            data.extend(fir.map(|v| (v / 256).min(255) as u8));
        }
    }
    Some(LcdMask {
        left,
        top,
        width: width as u32,
        height: height as u32,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcd_filter_spreads_one_pixel_to_each_side() {
        let mut path = SkPath::new();
        path.move_to(P { x: 2.0, y: 3.0 });
        for (x, y) in [(5.0, 3.0), (5.0, 5.0), (2.0, 5.0)] {
            path.line_to(P { x, y });
        }
        path.close();
        let gray = gray_mask(&path).expect("gray");
        assert_eq!((gray.left, gray.top, gray.width, gray.height), (2, 3, 3, 2));
        assert!(gray.data.iter().all(|&v| v == 255));
        let lcd = lcd_mask(&path).expect("lcd");
        assert_eq!((lcd.left, lcd.top, lcd.width, lcd.height), (1, 3, 5, 2));
        let middle = &lcd.data[6..9];
        assert_eq!(middle, [255; 3]);
        let sum = |f: &[u32]| f.iter().sum::<u32>() * 255 / 256;
        assert_eq!(u32::from(lcd.data[0]), sum(&LCD_FILTER[0][8..]));
        assert_eq!(u32::from(lcd.data[2]), sum(&LCD_FILTER[2][8..]));
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
