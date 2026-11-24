// Ported from Skia and Blink, Chromium m142 (BSD-3-Clause, Copyright Google Inc. and The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Windows text as Chromium 142 draws it: DirectWrite masks configured as `SkScalerContext_DW`
//! configures them, Blink's font matching and fallback, HarfBuzz shaping with design advances.

#![allow(unsafe_code)]

mod color;
mod embolden;
mod fallback;
mod outline;
pub mod settings;
mod shader;
mod typeface;

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::sync::Arc;

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_GLYPH_METRICS,
    DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN, DWRITE_GRID_FIT_MODE_ENABLED, DWRITE_MATRIX,
    DWRITE_MEASURING_MODE, DWRITE_MEASURING_MODE_GDI_CLASSIC, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_RENDERING_MODE,
    DWRITE_RENDERING_MODE_ALIASED, DWRITE_RENDERING_MODE_GDI_CLASSIC,
    DWRITE_RENDERING_MODE_NATURAL, DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, DWRITE_TEXTURE_ALIASED_1x1, DWRITE_TEXTURE_CLEARTYPE_3x1,
    DWRITE_TEXTURE_TYPE, DWriteCreateFactory, IDWriteFactory, IDWriteFactory2, IDWriteFont,
    IDWriteFontCollection, IDWriteFontFallback, IDWriteGlyphRunAnalysis, IDWriteNumberSubstitution,
    IDWriteTextAnalysisSource, IDWriteTextAnalysisSource_Impl,
};
use windows::core::{HSTRING, OutRef, implement};

use self::fallback::Priority;
use self::settings::{Edging, TextSettings};
use self::typeface::{GaspRange, Typeface};
use super::font::FontKey;
use super::harfbuzz::{self, Faces};
use super::matrix::{self, M2, Split, upright};
use super::shaping::{effective, round};
use super::{
    Bounds, Family, FontMetrics, FontSpec, Glyph, LcdMask, Mask, Shaped, TextBackend, path_mask,
};
use crate::path::Affine;
use crate::skpath::SkPath;

/// `kBoldWeightValue`.
const BOLD: f32 = 700.0;
/// `kBoldThreshold`.
const SYNTHETIC_BOLD: f32 = 600.0;
/// `SK_MAX_SIZE_FOR_LCDTEXT`.
const LCD_MAX_SIZE: f32 = 48.0;

struct Face {
    typeface: usize,
    size: f32,
    /// Blink's synthetic italic.
    skew: bool,
    /// Blink's synthetic bold.
    embolden: bool,
    fallback: bool,
}

/// What `SkScalerContext_DW` derives from a strike's size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Strike {
    /// `fTextSizeRender`.
    size: f32,
    rendering: DWRITE_RENDERING_MODE,
    texture: DWRITE_TEXTURE_TYPE,
    measure_size: f32,
    measuring: DWRITE_MEASURING_MODE,
    /// DirectWrite's grayscale antialiasing rather than ClearType's.
    grayscale: bool,
}

/// A glyph's bitmap in device pixels, with one or three coverage values each.
struct Bitmap {
    bounds: [i32; 4],
    channels: usize,
    data: Vec<u8>,
}

/// DirectWrite, configured as Chromium's Skia and Blink configure it.
pub struct DirectWrite {
    factory: IDWriteFactory2,
    collection: IDWriteFontCollection,
    fallback: Option<IDWriteFontFallback>,
    settings: TextSettings,
    system_family: String,
    typefaces: Vec<Typeface>,
    files: HashMap<(PathBuf, u32), usize>,
    matched: HashMap<(String, u32, bool), Option<usize>>,
    faces: Vec<Face>,
    primary: HashMap<FontKey, u32>,
    substitutes: HashMap<(FontKey, usize), u32>,
}

impl DirectWrite {
    /// `None` without DirectWrite or a single font.
    pub fn new() -> Option<DirectWrite> {
        DirectWrite::with_settings(None)
    }

    /// With the user's text settings, or `settings` in their place.
    pub fn with_settings(settings: Option<TextSettings>) -> Option<DirectWrite> {
        // SAFETY: plain COM calls on live objects.
        let (factory, collection, fallback) = unsafe {
            let factory: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let mut collection = None;
            IDWriteFactory::GetSystemFontCollection(&factory, &mut collection, false).ok()?;
            let fallback = factory.GetSystemFontFallback().ok();
            (factory, collection?, fallback)
        };
        let settings = settings.unwrap_or_else(TextSettings::system);
        let mut backend = DirectWrite {
            settings,
            factory,
            collection,
            fallback,
            system_family: settings::menu_font_family().unwrap_or_default(),
            typefaces: Vec::new(),
            files: HashMap::new(),
            matched: HashMap::new(),
            faces: Vec::new(),
            primary: HashMap::new(),
            substitutes: HashMap::new(),
        };
        backend.resolve(&FontSpec::default())?;
        Some(backend)
    }

    /// The file and face index behind a font, for checks against the pinned fonts.
    pub fn font_file(&mut self, spec: &FontSpec) -> Option<(PathBuf, u32)> {
        let typeface = self.resolve(spec)?;
        let typeface = &self.typefaces[typeface];
        // SAFETY: a plain COM call on a live face.
        let index = unsafe { typeface.face.GetIndex() };
        Some((typeface.path.clone()?, index))
    }

    /// The file of the font that draws `c` where `spec`'s own does not.
    pub fn fallback_file(&mut self, spec: &FontSpec, c: char, emoji: bool) -> Option<PathBuf> {
        let face = self.substitute(spec, c, emoji)?;
        self.typefaces[self.faces[face as usize].typeface]
            .path
            .clone()
    }

    fn typeface(&mut self, font: &IDWriteFont) -> Option<usize> {
        let typeface = Typeface::new(font)?;
        // SAFETY: a plain COM call on a live face.
        let index = unsafe { typeface.face.GetIndex() };
        // SAFETY: as above.
        let simulated = unsafe { typeface.face.GetSimulations() }.0 != 0;
        let key = typeface
            .path
            .clone()
            .filter(|_| !simulated)
            .map(|path| (path, index));
        if let Some(&known) = key.as_ref().and_then(|key| self.files.get(key)) {
            return Some(known);
        }
        self.typefaces.push(typeface);
        let added = self.typefaces.len() - 1;
        if let Some(key) = key {
            self.files.insert(key, added);
        }
        Some(added)
    }

    /// `SkFontMgr::matchFamilyStyle`.
    fn match_family(&mut self, family: &str, weight: f32, italic: bool) -> Option<usize> {
        let key = (family.to_owned(), weight.to_bits(), italic);
        if let Some(&known) = self.matched.get(&key) {
            return known;
        }
        // SAFETY: plain COM calls on a live collection.
        let found = unsafe {
            let (mut index, mut exists) = (0, false.into());
            self.collection
                .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
                .ok()
                .filter(|()| exists.as_bool())
                .and_then(|()| self.collection.GetFontFamily(index).ok())
        };
        let typeface = found
            .and_then(|family| typeface::match_style(&family, weight, italic))
            .and_then(|font| self.typeface(&font));
        self.matched.insert(key, typeface);
        typeface
    }

    fn is_present(&self, family: &str) -> bool {
        let (mut index, mut exists) = (0, false.into());
        // SAFETY: a plain COM call on a live collection.
        let found = unsafe {
            self.collection
                .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
        };
        found.is_ok() && exists.as_bool()
    }

    /// The typeface of a font: Chromium's default family for a generic name or the named one,
    /// then `GetLastResortFallbackFont`.
    fn resolve(&mut self, spec: &FontSpec) -> Option<usize> {
        let family = match spec.family {
            Family::System => self.system_family.clone(),
            Family::SansSerif | Family::Arial | Family::HelveticaNeue => "Arial".to_owned(),
            Family::Serif => "Times New Roman".to_owned(),
            Family::Monospace => "Consolas".to_owned(),
            Family::Courier => "Courier New".to_owned(),
        };
        let last_resort = [
            "Arial",
            "MS UI Gothic",
            "Microsoft Sans Serif",
            "Segoe UI",
            "Calibri",
            "Times New Roman",
            "Courier New",
        ];
        std::iter::once(family.as_str())
            .chain(last_resort)
            .find_map(|family| self.match_family(family, spec.weight, spec.italic))
    }

    fn fallback_typeface(&mut self, spec: &FontSpec, c: char, emoji: bool) -> Option<usize> {
        let priority = Priority::of(c, emoji);
        let families = fallback::families(c, priority, spec.family == Family::Monospace);
        let first = families
            .candidates
            .iter()
            .find(|family| self.is_present(family))
            .copied()
            .or(families.last_resort);
        for family in first
            .into_iter()
            .chain(families.wide_coverage.iter().copied())
        {
            if let Some(typeface) = self.match_family(family, spec.weight, spec.italic)
                && self.typefaces[typeface].has_char(c)
            {
                return Some(typeface);
            }
        }
        let font = self.map_character(spec, c, fallback::locale(c, priority))?;
        let typeface = self.typeface(&font)?;
        self.typefaces[typeface].has_char(c).then_some(typeface)
    }

    /// `SkFontMgr_DirectWrite::fallback`.
    fn map_character(&self, spec: &FontSpec, c: char, locale: &str) -> Option<IDWriteFont> {
        let text: Vec<u16> = c.encode_utf16(&mut [0; 2]).to_vec();
        let length = text.len() as u32;
        let source: IDWriteTextAnalysisSource = CharacterSource {
            text,
            locale: HSTRING::from(locale),
        }
        .into();
        let style = if spec.italic {
            DWRITE_FONT_STYLE_ITALIC
        } else {
            DWRITE_FONT_STYLE_NORMAL
        };
        let family = match spec.family {
            Family::Courier => "Courier",
            Family::Arial => "Arial",
            _ => "",
        };
        let (mut mapped, mut font, mut scale) = (0, None, 0.0);
        // SAFETY: the out parameters are live for the call, the source outlives it.
        unsafe {
            self.fallback.as_ref()?.MapCharacters(
                &source,
                0,
                length,
                None,
                &HSTRING::from(family),
                DWRITE_FONT_WEIGHT(spec.weight as i32),
                style,
                DWRITE_FONT_STRETCH_NORMAL,
                &mut mapped,
                &mut font,
                &mut scale,
            )
        }
        .ok()?;
        font
    }

    fn face(&self, spec: &FontSpec, typeface: usize, fallback: bool) -> Face {
        let loaded = &self.typefaces[typeface];
        Face {
            typeface,
            size: spec.size,
            skew: spec.italic && !loaded.italic,
            embolden: spec.weight >= SYNTHETIC_BOLD && !loaded.bold,
            fallback,
        }
    }

    fn substitute_face(&mut self, spec: &FontSpec, typeface: usize) -> u32 {
        if let Some(&face) = self.substitutes.get(&(spec.key(), typeface)) {
            return face;
        }
        self.faces.push(self.face(spec, typeface, true));
        let face = (self.faces.len() - 1) as u32;
        self.substitutes.insert((spec.key(), typeface), face);
        face
    }

    /// `SkScalerContext_DW`'s constructor for text of `size` device px.
    fn strike(&self, typeface: &Typeface, size: f32, axis_aligned: bool) -> Strike {
        let mut gdi = round(size * 64.0) / 64.0;
        if gdi == 0.0 {
            gdi = 1.0;
        }
        let bitmap = !typeface.avoids_bitmaps() && typeface.has_bitmap_strike(gdi as i32);
        let strike = |size, rendering, measure_size, measuring| Strike {
            size,
            rendering,
            texture: DWRITE_TEXTURE_CLEARTYPE_3x1,
            measure_size,
            measuring,
            grayscale: false,
        };
        let natural = |size_to_render, symmetric| {
            let rendering = if symmetric {
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC
            } else {
                DWRITE_RENDERING_MODE_NATURAL
            };
            strike(
                size_to_render,
                rendering,
                size,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
        let gdi_classic =
            |rendering| strike(gdi, rendering, gdi, DWRITE_MEASURING_MODE_GDI_CLASSIC);
        let gasp = typeface
            .gasp_range(round(gdi) as i32)
            .filter(|range| range.version >= 1);
        let mut strike = if self.settings.edging == Edging::Alias {
            Strike {
                texture: DWRITE_TEXTURE_ALIASED_1x1,
                ..gdi_classic(DWRITE_RENDERING_MODE_ALIASED)
            }
        } else if bitmap && axis_aligned {
            gdi_classic(DWRITE_RENDERING_MODE_GDI_CLASSIC)
        } else if bitmap {
            gdi_classic(DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC)
        } else if let Some(range) = gasp {
            natural(size, range.flags & GaspRange::SYMMETRIC_SMOOTHING != 0)
        } else if size > 20.0 || !typeface.hinted {
            natural(size, true)
        } else {
            natural(gdi, false)
        };
        if self.settings.edging == Edging::AntiAlias {
            strike.texture = DWRITE_TEXTURE_ALIASED_1x1;
            strike.grayscale = true;
        }
        if typeface.cbdt {
            strike.measuring = DWRITE_MEASURING_MODE_NATURAL;
        }
        strike
    }

    fn split(&self, glyph: Glyph, post: &M2) -> Option<(&Face, Split, Strike)> {
        let face = self.faces.get(glyph.face as usize)?;
        let split = matrix::split(face.size, face.skew, post)?;
        let aligned = matrix::is_axis_aligned(face.skew, post);
        let strike = self.strike(&self.typefaces[face.typeface], split.size, aligned);
        Some((face, split, strike))
    }

    fn analyze(
        &self,
        typeface: &Typeface,
        strike: &Strike,
        rendering: DWRITE_RENDERING_MODE,
        glyph: u16,
        transform: &DWRITE_MATRIX,
    ) -> Option<IDWriteGlyphRunAnalysis> {
        let advance = 0.0f32;
        let offset = DWRITE_GLYPH_OFFSET::default();
        let run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(typeface.face.clone())),
            fontEmSize: strike.size,
            glyphCount: 1,
            glyphIndices: &glyph,
            glyphAdvances: &advance,
            glyphOffsets: &offset,
            isSideways: false.into(),
            bidiLevel: 0,
        };
        // SAFETY: the run points at live values for the length of the call, and its face is
        // released once.
        unsafe {
            let analysis = if strike.grayscale {
                self.factory.CreateGlyphRunAnalysis(
                    &run,
                    Some(transform),
                    rendering,
                    strike.measuring,
                    DWRITE_GRID_FIT_MODE_ENABLED,
                    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                    0.0,
                    0.0,
                )
            } else {
                IDWriteFactory::CreateGlyphRunAnalysis(
                    &self.factory,
                    &run,
                    1.0,
                    Some(transform),
                    rendering,
                    strike.measuring,
                    0.0,
                    0.0,
                )
            };
            drop(ManuallyDrop::into_inner(run.fontFace));
            analysis.ok()
        }
    }

    fn bitmap(
        &self,
        typeface: &Typeface,
        strike: &Strike,
        glyph: u16,
        transform: &DWRITE_MATRIX,
    ) -> Option<Bitmap> {
        let mut modes = vec![(strike.rendering, strike.texture)];
        if strike.texture != DWRITE_TEXTURE_ALIASED_1x1 || strike.grayscale {
            modes.push((DWRITE_RENDERING_MODE_ALIASED, DWRITE_TEXTURE_ALIASED_1x1));
        }
        modes.into_iter().find_map(|(rendering, texture)| {
            let analysis = self.analyze(typeface, strike, rendering, glyph, transform)?;
            // SAFETY: `data` holds the texture the bounds describe.
            unsafe {
                let bounds = analysis.GetAlphaTextureBounds(texture).ok()?;
                if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
                    return None;
                }
                let channels = if texture == DWRITE_TEXTURE_CLEARTYPE_3x1 {
                    3
                } else {
                    1
                };
                let area = (bounds.right - bounds.left) * (bounds.bottom - bounds.top);
                let mut data = vec![0u8; area as usize * channels];
                analysis
                    .CreateAlphaTexture(texture, &bounds, &mut data)
                    .ok()?;
                Some(Bitmap {
                    bounds: [bounds.left, bounds.top, bounds.right, bounds.bottom],
                    channels,
                    data,
                })
            }
        })
    }

    /// The glyph outline under `split`'s remaining matrix, for glyphs Skia fills itself.
    fn device_path(
        &self,
        typeface: &Typeface,
        strike: &Strike,
        glyph: u16,
        transform: &DWRITE_MATRIX,
    ) -> Option<SkPath> {
        let path = outline::outline(&typeface.face, strike.size, &[glyph], None, None)?;
        Some(path.transformed(&Affine {
            a: f64::from(transform.m11),
            b: f64::from(transform.m12),
            c: f64::from(transform.m21),
            d: f64::from(transform.m22),
            e: f64::from(transform.dx),
            f: f64::from(transform.dy),
        }))
    }

    /// The glyph's coverage under `post`, in one channel or, `lcd`, in three.
    fn coverage(&self, glyph: Glyph, post: &M2, phase: (u8, u8), lcd: bool) -> Option<Bitmap> {
        let (face, split, strike) = self.split(glyph, post)?;
        let typeface = &self.typefaces[face.typeface];
        let id = u16::try_from(glyph.id)
            .ok()
            .filter(|&id| id < typeface.glyph_count)?;
        let transform = transform(&split.remaining, phase);
        let drawn = (!face.embolden)
            .then(|| self.bitmap(typeface, &strike, id, &transform))
            .flatten();
        if let Some(bitmap) = drawn {
            if bitmap.channels == 1 || lcd {
                return Some(bitmap);
            }
            return Some(Bitmap {
                data: averaged(&bitmap.data),
                channels: 1,
                ..bitmap
            });
        }
        let mut path = self.device_path(typeface, &strike, id, &transform)?;
        if face.embolden {
            let post = device_matrix(post);
            let local = path.transformed(&post.inverse()?);
            let width = embolden::extra_width(face.size);
            path = embolden::embolden(&local, width).transformed(&post);
        }
        if self.settings.edging != Edging::SubpixelAntiAlias {
            let mask = path_mask::gray(&path)?;
            return Some(Bitmap {
                bounds: rect(mask.left, mask.top, mask.width, mask.height),
                channels: 1,
                data: mask.data,
            });
        }
        let mask = path_mask::lcd(&path)?;
        Some(Bitmap {
            bounds: rect(mask.left, mask.top, mask.width, mask.height),
            channels: if lcd { 3 } else { 1 },
            data: if lcd { mask.data } else { averaged(&mask.data) },
        })
    }

    fn mask(&self, glyph: Glyph, post: &M2, phase: (u8, u8)) -> Option<Arc<Mask>> {
        if let Some(mask) = self.color_mask(glyph, post, phase) {
            return Some(Arc::new(mask));
        }
        let Bitmap { bounds, data, .. } = self.coverage(glyph, post, phase, false)?;
        Some(Arc::new(Mask {
            left: bounds[0],
            top: bounds[1],
            width: (bounds[2] - bounds[0]) as u32,
            height: (bounds[3] - bounds[1]) as u32,
            data,
            color: false,
        }))
    }

    fn color_glyph(&self, glyph: Glyph, post: &M2, phase: (u8, u8)) -> Option<color::Glyph<'_>> {
        let (face, split, strike) = self.split(glyph, post)?;
        let typeface = &self.typefaces[face.typeface];
        if !typeface.color {
            return None;
        }
        color::Glyph::new(
            &self.factory,
            typeface,
            u16::try_from(glyph.id).ok()?,
            strike.size,
            strike.measuring,
            transform(&split.remaining, phase),
        )
    }

    fn color_mask(&self, glyph: Glyph, post: &M2, phase: (u8, u8)) -> Option<Mask> {
        self.color_glyph(glyph, post, phase)?.draw()
    }

    fn vertical_metrics(&self, face: u32, size: f32) -> FontMetrics {
        let typeface = &self.typefaces[self.faces[face as usize].typeface];
        let strike = self.strike(typeface, size, true);
        let mut metrics = typeface.metrics;
        if strike.measuring != DWRITE_MEASURING_MODE_NATURAL {
            let mut compatible = DWRITE_FONT_METRICS::default();
            // SAFETY: a COM call that writes the struct it is given.
            let read = unsafe {
                typeface
                    .face
                    .GetGdiCompatibleMetrics(strike.size, 1.0, None, &mut compatible)
            };
            if read.is_ok() {
                metrics = compatible;
            }
        }
        let upem = f32::from(metrics.designUnitsPerEm);
        let ascent = strike.size * f32::from(metrics.ascent) / upem;
        let descent = strike.size * f32::from(metrics.descent) / upem;
        let line_gap = strike.size * f32::from(metrics.lineGap) / upem;
        FontMetrics {
            ascent: round(ascent),
            descent: round(descent),
            line_gap: round(line_gap),
            typo: typeface.typo,
        }
    }
}

fn rect(left: i32, top: i32, width: u32, height: u32) -> [i32; 4] {
    [left, top, left + width as i32, top + height as i32]
}

fn transform(m: &M2, phase: (u8, u8)) -> DWRITE_MATRIX {
    DWRITE_MATRIX {
        m11: m.sx,
        m12: m.ky,
        m21: m.kx,
        m22: m.sy,
        dx: f32::from(phase.0) / 4.0,
        dy: f32::from(phase.1) / 4.0,
    }
}

fn device_matrix(m: &M2) -> Affine {
    Affine {
        a: f64::from(m.sx),
        b: f64::from(m.ky),
        c: f64::from(m.kx),
        d: f64::from(m.sy),
        e: 0.0,
        f: 0.0,
    }
}

/// `RGBToA8`.
fn averaged(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|p| (p.iter().map(|&v| u32::from(v)).sum::<u32>() / 3) as u8)
        .collect()
}

/// The single character DirectWrite maps to a font.
#[implement(IDWriteTextAnalysisSource)]
struct CharacterSource {
    text: Vec<u16>,
    locale: HSTRING,
}

impl IDWriteTextAnalysisSource_Impl for CharacterSource_Impl {
    fn GetTextAtPosition(
        &self,
        position: u32,
        text: *mut *mut u16,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        let rest = self.text.get(position as usize..).unwrap_or_default();
        // SAFETY: both out pointers are DirectWrite's, and the text outlives the call.
        unsafe {
            *text = if rest.is_empty() {
                std::ptr::null_mut()
            } else {
                rest.as_ptr().cast_mut()
            };
            *length = rest.len() as u32;
        }
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        position: u32,
        text: *mut *mut u16,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        let before = &self.text[..self.text.len().min(position as usize)];
        // SAFETY: both out pointers are DirectWrite's, and the text outlives the call.
        unsafe {
            *text = if before.is_empty() {
                std::ptr::null_mut()
            } else {
                before.as_ptr().cast_mut()
            };
            *length = before.len() as u32;
        }
        Ok(())
    }

    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }

    fn GetLocaleName(
        &self,
        _: u32,
        length: *mut u32,
        locale: *mut *mut u16,
    ) -> windows::core::Result<()> {
        // SAFETY: both out pointers are DirectWrite's, and the locale outlives the call.
        unsafe {
            *length = self.text.len() as u32;
            *locale = self.locale.as_ptr().cast_mut();
        }
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        _: u32,
        length: *mut u32,
        substitution: OutRef<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        // SAFETY: the out pointer is DirectWrite's.
        unsafe { *length = self.text.len() as u32 };
        substitution.write(None)
    }
}

impl Faces for DirectWrite {
    fn shaper(&self, face: u32) -> &rustybuzz::Face<'static> {
        &self.typefaces[self.faces[face as usize].typeface].shaper
    }

    fn upem(&self, face: u32) -> u16 {
        self.typefaces[self.faces[face as usize].typeface]
            .metrics
            .designUnitsPerEm
    }

    /// `SkScalerContext_DW::setAdvance`, rounded where text is not positioned by subpixels.
    fn advance(&mut self, face: u32, id: u32, size: f32) -> f32 {
        let typeface = &self.typefaces[self.faces[face as usize].typeface];
        let Some(id) = u16::try_from(id)
            .ok()
            .filter(|&id| id < typeface.glyph_count)
        else {
            return 0.0;
        };
        let strike = self.strike(typeface, size, true);
        let gdi = strike.measuring != DWRITE_MEASURING_MODE_NATURAL;
        let mut metrics = DWRITE_GLYPH_METRICS::default();
        // SAFETY: one glyph in, one metrics struct out.
        let read = unsafe {
            if gdi {
                typeface.face.GetGdiCompatibleGlyphMetrics(
                    strike.measure_size,
                    1.0,
                    None,
                    false,
                    &id,
                    1,
                    &mut metrics,
                    false,
                )
            } else {
                typeface
                    .face
                    .GetDesignGlyphMetrics(&id, 1, &mut metrics, false)
            }
        };
        if read.is_err() {
            return 0.0;
        }
        let upem = f32::from(typeface.metrics.designUnitsPerEm);
        let advance = strike.measure_size * metrics.advanceWidth as f32 / upem;
        if gdi || self.settings.edging == Edging::Alias {
            round(advance)
        } else {
            advance
        }
    }

    fn primary(&mut self, spec: &FontSpec) -> Option<u32> {
        if let Some(&face) = self.primary.get(&spec.key()) {
            return Some(face);
        }
        let typeface = self.resolve(spec)?;
        self.faces.push(self.face(spec, typeface, false));
        let face = (self.faces.len() - 1) as u32;
        self.primary.insert(spec.key(), face);
        Some(face)
    }

    fn standard(&mut self, spec: &FontSpec) -> Option<u32> {
        let serif = FontSpec {
            family: Family::Serif,
            ..*spec
        };
        let typeface = self.resolve(&serif)?;
        if self.resolve(spec) == Some(typeface) {
            return self.primary(spec);
        }
        Some(self.substitute_face(spec, typeface))
    }

    fn substitute(&mut self, spec: &FontSpec, c: char, emoji: bool) -> Option<u32> {
        if is_private_use(c) || is_noncharacter(c) {
            return None;
        }
        // Blink looks the list's first family up by name, which a generic one does not have.
        let named = matches!(spec.family, Family::Courier | Family::Arial);
        if !emoji && named && (spec.italic || spec.weight >= BOLD) {
            let regular = FontSpec {
                weight: 400.0,
                italic: false,
                ..*spec
            };
            if let Some(typeface) = self.resolve(&regular)
                && self.typefaces[typeface].has_char(c)
            {
                return Some(self.substitute_face(spec, typeface));
            }
        }
        let typeface = self.fallback_typeface(spec, c, emoji)?;
        Some(self.substitute_face(spec, typeface))
    }
}

fn is_private_use(c: char) -> bool {
    matches!(u32::from(c), 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x10_0000..=0x10_fffd)
}

fn is_noncharacter(c: char) -> bool {
    let c = u32::from(c);
    matches!(c, 0xfdd0..=0xfdef) || c & 0xfffe == 0xfffe
}

impl TextBackend for DirectWrite {
    fn shape(&mut self, spec: &FontSpec, text: &str, zoom: f32) -> Shaped {
        harfbuzz::shape(self, spec, text, zoom)
    }

    fn shape_run(&mut self, spec: &FontSpec, text: &str, zoom: f32) -> Shaped {
        harfbuzz::shape_run(self, spec, text, zoom)
    }

    fn metrics(&mut self, spec: &FontSpec) -> FontMetrics {
        let spec = &effective(spec, 1.0);
        match (spec.size > 0.0).then(|| self.primary(spec)).flatten() {
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
        self.faces.get(glyph.face as usize)?;
        let post = M2::IDENTITY;
        let [left, top, right, bottom] = match self.color_glyph(glyph, &post, (0, 0)) {
            Some(color) => color.bounds(),
            None => self
                .coverage(glyph, &post, (0, 0), false)
                .map(|bitmap| bitmap.bounds)
                .unwrap_or_default(),
        }
        .map(|v| v as f32);
        Some(Bounds {
            left,
            top,
            right,
            bottom,
        })
    }

    fn subpixel_positioning(&self) -> bool {
        self.settings.edging != Edging::Alias
    }

    fn raw_mask(&mut self, glyph: Glyph, scale: f64, x_scale: f64, phase: u8) -> Option<Arc<Mask>> {
        self.mask(glyph, &upright(scale, x_scale), (phase, 0))
    }

    fn raw_mask_transformed(
        &mut self,
        glyph: Glyph,
        m: [f64; 4],
        phase: (u8, u8),
    ) -> Option<Arc<Mask>> {
        self.mask(glyph, &matrix::relaxed(m), phase)
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
        let too_big = post.sx * post.sy * (face.size * face.size) > LCD_MAX_SIZE * LCD_MAX_SIZE;
        if self.settings.edging != Edging::SubpixelAntiAlias
            || too_big
            || self.color_glyph(glyph, &post, (phase, 0)).is_some()
        {
            return None;
        }
        let Bitmap {
            bounds,
            channels,
            mut data,
        } = self.coverage(glyph, &post, (phase, 0), true)?;
        if channels == 1 {
            data = data.into_iter().flat_map(|v| [v; 3]).collect();
        }
        if self.settings.bgr {
            data.as_chunks_mut::<3>()
                .0
                .iter_mut()
                .for_each(|p| p.swap(0, 2));
        }
        Some(Arc::new(LcdMask {
            left: bounds[0],
            top: bounds[1],
            width: (bounds[2] - bounds[0]) as u32,
            height: (bounds[3] - bounds[1]) as u32,
            data,
        }))
    }
}
