#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use core_foundation::attributed_string::CFMutableAttributedString;
use core_foundation::base::{CFRange, CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::{CGContext, CGTextDrawingMode};
use core_graphics::font::CGGlyph;
use core_graphics::geometry::{CGAffineTransform, CGPoint, CGRect, CGSize};
use core_text::font::{CTFont, new_from_descriptor, new_from_name, new_ui_font_for_language};
use core_text::font_descriptor::{
    kCTFontBoldTrait, kCTFontColorGlyphsTrait, kCTFontItalicTrait, kCTFontOrientationHorizontal,
    kCTFontVariationAttribute, new_from_attributes,
};
use core_text::line::CTLine;
use core_text::run::CTRun;
use core_text::string_attributes::kCTFontAttributeName;

use super::font::FontKey;
use super::{
    Bounds, Family, FontMetrics, FontSpec, Glyph, Mask, Shaped, ShapedGlyph, TextBackend, relax,
};

const K_CT_FONT_UI_FONT_SYSTEM: u32 = 2;
const K_CG_IMAGE_ALPHA_ONLY: u32 = 7;
const K_CG_IMAGE_ALPHA_PREMULTIPLIED_FIRST: u32 = 2;
const K_CG_BITMAP_BYTE_ORDER_32_HOST: u32 = 2 << 12;
const WGHT: i64 = 0x7767_6874;
const OS2: u32 = 0x4f53_2f32;
const EMOJI: [&str; 2] = [".AppleColorEmojiUI", "AppleColorEmoji"];

/// Direct CoreText, configured like Skia's `SkScalerContext_Mac` in Chromium.
pub(crate) struct CoreText {
    faces: Vec<Face>,
    primary: HashMap<FontKey, u32>,
    fallback: HashMap<(FontKey, String), u32>,
    device: HashMap<(u32, u32), CTFont>,
}

fn variation(base: &CTFont, weight: f32, size: f64) -> CTFont {
    let axes = CFDictionary::from_CFType_pairs(&[(
        CFNumber::from(WGHT),
        CFNumber::from(f64::from(weight)),
    )]);
    // SAFETY: a CoreText constant, valid for the life of the process.
    let key = unsafe { CFString::wrap_under_get_rule(kCTFontVariationAttribute) };
    let attributes = CFDictionary::from_CFType_pairs(&[(key, axes)]);
    let descriptor = base
        .copy_descriptor()
        .create_copy_with_attributes(attributes.to_untyped())
        .expect("variation descriptor");
    new_from_descriptor(&descriptor, size)
}

struct Face {
    font: CTFont,
    /// A `wght` instance of a variable font. Chromium takes its advances from HarfBuzz, which
    /// rounds HVAR deltas to whole font units, and its ink bounds from the varied outlines, where
    /// CoreText's bounding rects describe the default instance.
    varied: bool,
    /// Skia draws the glyphs of a font with colour glyphs as ARGB images, never as masks.
    color: bool,
    /// Blink's synthetic italic: an upright fallback for an italic font, drawn under Skia's
    /// fake-italic skew.
    skew: bool,
}

unsafe extern "C" {
    fn CTFontCreatePathForGlyph(
        font: core_text::font::CTFontRef,
        glyph: CGGlyph,
        matrix: *const CGAffineTransform,
    ) -> *const std::ffi::c_void;
    fn CGPathGetBoundingBox(path: *const std::ffi::c_void) -> CGRect;
    fn CGPathRelease(path: *const std::ffi::c_void);
    fn CTFontCreateCopyWithAttributes(
        font: core_text::font::CTFontRef,
        size: f64,
        matrix: *const CGAffineTransform,
        attributes: core_text::font_descriptor::CTFontDescriptorRef,
    ) -> core_text::font::CTFontRef;
    fn CTFontCopyAttribute(
        font: core_text::font::CTFontRef,
        attribute: core_foundation::string::CFStringRef,
    ) -> core_foundation::base::CFTypeRef;
}

/// Skia's `SkCTFontCreateExactCopy`: `font` at `size`, keeping the optical size of `font` and
/// with tracking off.
fn exact_copy(font: &CTFont, size: f64) -> CTFont {
    let opsz_key = CFString::from_static_string("NSCTFontOpticalSizeAttribute");
    // SAFETY: `font` is a live CTFont and the attribute, if any, is returned under the create rule.
    let opsz = unsafe {
        let value = CTFontCopyAttribute(font.as_concrete_TypeRef(), opsz_key.as_concrete_TypeRef());
        (!value.is_null()).then(|| CFType::wrap_under_create_rule(value))
    }
    .and_then(|v| v.downcast::<CFNumber>())
    .and_then(|n| n.to_f64())
    .filter(|&v| v > 0.0)
    .unwrap_or_else(|| font.pt_size());
    let attributes = CFDictionary::from_CFType_pairs(&[
        (opsz_key, CFNumber::from(opsz).as_CFType()),
        (
            CFString::from_static_string("NSCTFontUnscaledTrackingAttribute"),
            CFNumber::from(0i32).as_CFType(),
        ),
    ]);
    let descriptor = new_from_attributes(&attributes);
    // SAFETY: both arguments are live CoreText objects; the copy is returned under the create rule.
    unsafe {
        CTFont::wrap_under_create_rule(CTFontCreateCopyWithAttributes(
            font.as_concrete_TypeRef(),
            size,
            std::ptr::null(),
            descriptor.as_concrete_TypeRef(),
        ))
    }
}

fn path_bounds(font: &CTFont, glyph: CGGlyph) -> Option<CGRect> {
    // SAFETY: `font` is a live CTFont; the returned path is owned here and released once read.
    unsafe {
        let path = CTFontCreatePathForGlyph(font.as_concrete_TypeRef(), glyph, std::ptr::null());
        if path.is_null() {
            return None;
        }
        let rect = CGPathGetBoundingBox(path);
        CGPathRelease(path);
        Some(rect)
    }
}

/// CoreText reads a size of 0 as its default of 12 pt, so a zero-size spec never reaches it.
fn create_font(spec: &FontSpec) -> Face {
    let size = f64::from(spec.size);
    let mut traits = 0;
    if spec.italic {
        traits |= kCTFontItalicTrait;
    }
    let varied = spec.family == Family::System && spec.weight != 400.0;
    let font = match spec.family {
        Family::System => {
            let base = new_ui_font_for_language(K_CT_FONT_UI_FONT_SYSTEM, size, None);
            if varied {
                variation(&base, spec.weight, size)
            } else {
                base
            }
        }
        family => {
            let name = match family {
                Family::SansSerif => "Helvetica",
                Family::Monospace => "Courier",
                Family::Arial => "Arial",
                _ => "Times",
            };
            if spec.weight >= 600.0 {
                traits |= kCTFontBoldTrait;
            }
            new_from_name(name, size).expect("a standard macOS font")
        }
    };
    let font = if traits == 0 {
        font
    } else {
        font.clone_with_symbolic_traits(traits, traits)
            .unwrap_or(font)
    };
    Face {
        font,
        varied,
        color: false,
        skew: false,
    }
}

/// Skia's fake-italic skew, `SkFont::setSkewX(-1/4)`, in CoreGraphics' y-up space.
const FAKE_ITALIC: CGAffineTransform = CGAffineTransform {
    a: 1.0,
    b: 0.0,
    c: 0.25,
    d: 1.0,
    tx: 0.0,
    ty: 0.0,
};

impl Face {
    fn skew_matrix(&self) -> CGAffineTransform {
        if self.skew {
            FAKE_ITALIC
        } else {
            CGAffineTransform::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
        }
    }
}

/// `CGRectApplyAffineTransform` as `[left, bottom, right, top]`.
fn transformed_rect(r: CGRect, m: CGAffineTransform) -> [f64; 4] {
    let corners = [
        (r.origin.x, r.origin.y),
        (r.origin.x + r.size.width, r.origin.y),
        (r.origin.x, r.origin.y + r.size.height),
        (r.origin.x + r.size.width, r.origin.y + r.size.height),
    ]
    .map(|(x, y)| (m.a * x + m.c * y + m.tx, m.b * x + m.d * y + m.ty));
    let fold = |f: fn(&(f64, f64)) -> f64, init: f64, pick: fn(f64, f64) -> f64| {
        corners.iter().map(f).fold(init, pick)
    };
    [
        fold(|p| p.0, f64::INFINITY, f64::min),
        fold(|p| p.1, f64::INFINITY, f64::min),
        fold(|p| p.0, f64::NEG_INFINITY, f64::max),
        fold(|p| p.1, f64::NEG_INFINITY, f64::max),
    ]
}

fn has_color_glyphs(font: &CTFont) -> bool {
    font.symbolic_traits() & kCTFontColorGlyphsTrait != 0
}

fn run_font(run: &CTRun) -> Option<CTFont> {
    // SAFETY: a CoreText constant, valid for the life of the process.
    let key = unsafe { CFString::wrap_under_get_rule(kCTFontAttributeName) };
    run.attributes()?.find(&key)?.clone().downcast::<CTFont>()
}

/// Blink's `CachingWordShapeIterator` for space-normalized text: every space is a word of its
/// own and shapes apart from its neighbours.
fn words(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = if rest.starts_with(' ') {
            1
        } else {
            rest.find(' ').unwrap_or(rest.len())
        };
        let (word, tail) = rest.split_at(end);
        rest = tail;
        Some(word)
    })
}

/// HarfBuzz's `em_mult` scaling of whole font units to 16.16 at Blink's font scale, the size
/// truncated to 16.16.
fn hb_scale(units: i64, upem: u32, size: f32) -> i32 {
    let x_scale = (size * 65536.0) as i64;
    let x_mult = (x_scale << 16) / i64::from(upem);
    ((units * x_mult + 32768) >> 16) as i32
}

/// Skia's `SkScalarRoundToScalar`.
fn round(v: f32) -> f32 {
    (v + 0.5).floor()
}

fn bitmap_context(width: usize, height: usize) -> CGContext {
    let mut cx = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width,
        &CGColorSpace::create_device_gray(),
        K_CG_IMAGE_ALPHA_ONLY,
    );
    cx.data().fill(0);
    cx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
    cx.set_allows_antialiasing(true);
    cx.set_should_antialias(true);
    cx.set_allows_font_subpixel_positioning(true);
    cx.set_should_subpixel_position_fonts(true);
    cx.set_allows_font_subpixel_quantization(false);
    cx.set_should_subpixel_quantize_fonts(false);
    cx.set_gray_fill_color(0.0, 1.0);
    cx
}

fn read_mask(mut cx: CGContext, x0: i32, y1: i32, w: usize, h: usize) -> Arc<Mask> {
    let stride = cx.bytes_per_row();
    let data = cx.data();
    let mut bytes = Vec::with_capacity(w * h);
    for row in 0..h {
        bytes.extend_from_slice(&data[row * stride..row * stride + w]);
    }
    Arc::new(Mask {
        left: x0,
        top: -y1,
        width: w as u32,
        height: h as u32,
        data: bytes,
        color: false,
    })
}

/// `SkScalerContext_Mac`'s offscreen for colour glyphs: premultiplied ARGB in host order on
/// a device RGB context, cleared to transparent, with CoreGraphics left to pick and scale the
/// bitmap strike.
fn color_context(width: usize, height: usize) -> CGContext {
    let mut cx = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width * 4,
        &CGColorSpace::create_device_rgb(),
        K_CG_IMAGE_ALPHA_PREMULTIPLIED_FIRST | K_CG_BITMAP_BYTE_ORDER_32_HOST,
    );
    cx.data().fill(0);
    cx.set_allows_font_subpixel_quantization(false);
    cx.set_should_subpixel_quantize_fonts(false);
    cx.set_allows_font_subpixel_positioning(true);
    cx.set_should_subpixel_position_fonts(true);
    cx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
    cx.set_rgb_fill_color(0.0, 0.0, 0.0, 1.0);
    cx.set_should_antialias(true);
    cx.set_should_smooth_fonts(false);
    cx
}

fn read_color(mut cx: CGContext, left: i32, top: i32, w: usize, h: usize) -> Arc<Mask> {
    let stride = cx.bytes_per_row();
    let data = cx.data();
    let mut bytes = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        for px in data[row * stride..row * stride + w * 4].chunks_exact(4) {
            let argb = u32::from_ne_bytes([px[0], px[1], px[2], px[3]]);
            bytes.extend_from_slice(&[
                (argb >> 16) as u8,
                (argb >> 8) as u8,
                argb as u8,
                (argb >> 24) as u8,
            ]);
        }
    }
    Arc::new(Mask {
        left,
        top,
        width: w as u32,
        height: h as u32,
        data: bytes,
        color: true,
    })
}

impl CoreText {
    pub(crate) fn new() -> Self {
        CoreText {
            faces: Vec::new(),
            primary: HashMap::new(),
            fallback: HashMap::new(),
            device: HashMap::new(),
        }
    }

    /// The face at the device text size, as `SkScalerContext_Mac` sizes its CTFont.
    fn device_font(&mut self, face: u32, font: &CTFont, size: f32) -> CTFont {
        self.device
            .entry((face, size.to_bits()))
            .or_insert_with(|| exact_copy(font, f64::from(size)))
            .clone()
    }

    fn primary(&mut self, spec: &FontSpec) -> u32 {
        if let Some(&face) = self.primary.get(&spec.key()) {
            return face;
        }
        let face = self.faces.len() as u32;
        self.faces.push(create_font(spec));
        self.primary.insert(spec.key(), face);
        face
    }

    fn run_face(&mut self, spec: &FontSpec, primary: u32, font: CTFont) -> u32 {
        let name = font.postscript_name();
        if name == self.faces[primary as usize].font.postscript_name() {
            return primary;
        }
        *self.fallback.entry((spec.key(), name)).or_insert_with(|| {
            let color = has_color_glyphs(&font);
            let skew = spec.italic && font.symbolic_traits() & kCTFontItalicTrait == 0;
            self.faces.push(Face {
                font,
                varied: false,
                color,
                skew,
            });
            (self.faces.len() - 1) as u32
        })
    }

    fn line(font: &CTFont, text: &str, overrides: &[(CFRange, CTFont)]) -> CTLine {
        let mut string = CFMutableAttributedString::new();
        string.replace_str(&CFString::new(text), CFRange::init(0, 0));
        let len = string.char_len();
        // SAFETY: `kCTFontAttributeName` is a CoreText constant and the fonts are live CTFonts.
        unsafe {
            string.set_attribute(CFRange::init(0, len), kCTFontAttributeName, font);
            for (range, font) in overrides {
                string.set_attribute(*range, kCTFontAttributeName, font);
            }
        }
        CTLine::new_with_attributed_string(string.as_concrete_TypeRef())
    }

    /// Chromium picks `Apple Color Emoji` for emoji, where CoreText falls back to the UI variant
    /// of it next to the system font, which has other advances. Skia sizes it with tracking off,
    /// and the font's `trak` table would otherwise widen small sizes by up to a pixel.
    fn emoji_overrides(line: &CTLine, len: isize, size: f64) -> Vec<(CFRange, CTFont)> {
        let runs: Vec<(isize, bool)> = line
            .glyph_runs()
            .iter()
            .map(|run| {
                let start = run.string_indices().first().copied().unwrap_or(len);
                let emoji =
                    run_font(&run).is_some_and(|f| EMOJI.contains(&f.postscript_name().as_str()));
                (start, emoji)
            })
            .collect();
        let mut overrides = Vec::new();
        for (k, &(start, emoji)) in runs.iter().enumerate() {
            if emoji {
                let end = runs.get(k + 1).map_or(len, |r| r.0);
                let font = new_from_name("AppleColorEmoji", size).expect("Apple Color Emoji");
                let font = exact_copy(&font, size);
                overrides.push((CFRange::init(start, end - start), font));
            }
        }
        overrides
    }

    /// One word as HarfBuzz shapes it in Chromium: CoreText's glyphs and kerning, with each
    /// advance taken from the scaler and truncated to 16.16, accumulated in `f32`.
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
        let font = self.faces[primary as usize].font.clone();
        let mut line = Self::line(&font, word, &[]);
        let len = word.encode_utf16().count() as isize;
        let overrides = Self::emoji_overrides(&line, len, f64::from(spec.size));
        if !overrides.is_empty() {
            line = Self::line(&font, word, &overrides);
        }
        let mut pen = 0.0f32;
        for run in line.glyph_runs().iter() {
            let run_font = run_font(&run).unwrap_or_else(|| font.clone());
            let ids = run.glyphs();
            let positions = run.positions();
            let indices = run.string_indices();
            let Some(first) = positions.first() else {
                continue;
            };
            let run_end = first.x + run.get_typographic_bounds().width;
            let mut advances = vec![CGSize::new(0.0, 0.0); ids.len()];
            // SAFETY: `ids` and `advances` both hold `ids.len()` elements.
            unsafe {
                run_font.get_advances_for_glyphs(
                    kCTFontOrientationHorizontal,
                    ids.as_ptr(),
                    advances.as_mut_ptr(),
                    ids.len() as isize,
                );
            }
            let face = self.run_face(spec, primary, run_font.clone());
            let varied = self.faces[face as usize].varied;
            let zoomed = (self.faces[face as usize].color && zoom != 1.0).then(|| {
                let device = self.device_font(face, &run_font, spec.size * zoom);
                let mut zoomed = vec![CGSize::new(0.0, 0.0); ids.len()];
                // SAFETY: `ids` and `zoomed` both hold `ids.len()` elements.
                unsafe {
                    device.get_advances_for_glyphs(
                        kCTFontOrientationHorizontal,
                        ids.as_ptr(),
                        zoomed.as_mut_ptr(),
                        ids.len() as isize,
                    );
                }
                zoomed
            });
            let upem = self.faces[face as usize].font.units_per_em();
            let units_per_px = f64::from(upem) / f64::from(spec.size);
            for (i, (&id, p)) in ids.iter().zip(positions.iter()).enumerate() {
                out.push(ShapedGlyph {
                    glyph: Glyph {
                        face,
                        id: u32::from(id),
                    },
                    x: origin + pen,
                    cluster: start + indices.get(i).map_or(0, |&k| k as u32),
                });
                let next = positions.get(i + 1).map_or(run_end, |q| q.x);
                let raw = advances[i].width;
                let kern = ((next - p.x - raw) * 65536.0).round() as i32;
                let advance = if let Some(zoomed) = &zoomed {
                    let device = ((zoomed[i].width as f32) * 65536.0) as i32;
                    (device as f32 / zoom) as i32
                } else if varied {
                    hb_scale((raw * units_per_px).round() as i64, upem, spec.size)
                } else {
                    ((raw as f32) * 65536.0) as i32
                };
                pen += (advance + kern) as f32 / 65536.0;
            }
        }
        pen
    }

    /// `SkScalerContext_Mac::generateMetrics` and `generateImage` for a colour glyph: the face
    /// at the device text size, the rest of the device matrix as the text matrix, bounds rounded
    /// out with a pixel of margin, and the glyph drawn from the inverse-mapped corner.
    fn color_image(
        &mut self,
        glyph: Glyph,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Arc<Mask>> {
        let face = &self.faces[glyph.face as usize];
        let font = face.font.clone();
        let skew = face.skew;
        let id = glyph.id as CGGlyph;
        let size = font.pt_size() as f32;
        let text_size = size * relax(scale as f32);
        let a = f64::from(size * relax((scale * x_scale) as f32) / text_size);
        let c = if skew { 0.25 * a } else { 0.0 };
        let m = CGAffineTransform::new(a, 0.0, c, 1.0, 0.0, 0.0);
        let device = self.device_font(glyph.face, &font, text_size);
        let advance = {
            let mut advance = CGSize::new(0.0, 0.0);
            // SAFETY: one glyph in, one advance out.
            unsafe {
                device.get_advances_for_glyphs(kCTFontOrientationHorizontal, &id, &mut advance, 1)
            };
            advance
        };
        let rect = device.get_bounding_rects_for_glyphs(kCTFontOrientationHorizontal, &[id]);
        if (advance.width == 0.0 && advance.height == 0.0)
            || rect.size.width <= 0.0
            || rect.size.height <= 0.0
        {
            return None;
        }
        let [l, b, r, t] = transformed_rect(rect, m).map(|v| v as f32);
        let sub_x = f32::from(phase) / 4.0;
        let left = l.floor() as i32 - 1;
        let top = (-t).floor() as i32 - 1;
        let right = (r + sub_x).ceil() as i32 + 1;
        let bottom = (-b).ceil() as i32 + 1;
        let (w, h) = ((right - left) as usize, (bottom - top) as usize);
        let cx = color_context(w, h);
        cx.set_text_matrix(&m);
        let point = CGPoint::new(f64::from(sub_x) - f64::from(left), f64::from(bottom));
        let point = CGPoint::new((point.x - c * point.y) / a, point.y);
        device.draw_glyphs(&[id], &[point], cx.clone());
        Some(read_color(cx, left, top, w, h))
    }

    fn glyph_rect(&self, glyph: Glyph) -> Option<(CTFont, CGGlyph, [f64; 4])> {
        let face = self.faces.get(glyph.face as usize)?;
        let font = face.font.clone();
        let id = glyph.id as CGGlyph;
        let r = if face.varied {
            path_bounds(&font, id)?
        } else {
            font.get_bounding_rects_for_glyphs(kCTFontOrientationHorizontal, &[id])
        };
        if r.size.width <= 0.0 || r.size.height <= 0.0 {
            return None;
        }
        if face.color {
            let [l, b, r, t] = transformed_rect(r, face.skew_matrix());
            let rect = [
                l.floor() - 1.0,
                b.floor() - 1.0,
                r.ceil() + 1.0,
                t.ceil() + 1.0,
            ];
            return Some((font, id, rect));
        }
        let rect = [
            r.origin.x,
            r.origin.y,
            r.origin.x + r.size.width,
            r.origin.y + r.size.height,
        ];
        Some((font, id, rect))
    }
}

impl TextBackend for CoreText {
    fn shape(&mut self, spec: &FontSpec, text: &str, zoom: f32) -> Shaped {
        if spec.size <= 0.0 {
            return Shaped {
                glyphs: Vec::new(),
                width: 0.0,
            };
        }
        let primary = self.primary(spec);
        let mut glyphs = Vec::new();
        let mut width = 0.0f32;
        let mut start = 0;
        for word in words(text) {
            width += self.shape_word(spec, primary, word, zoom, width, start, &mut glyphs);
            start += word.encode_utf16().count() as u32;
        }
        Shaped { glyphs, width }
    }

    fn metrics(&mut self, spec: &FontSpec) -> FontMetrics {
        if spec.size <= 0.0 {
            return FontMetrics {
                ascent: 0.0,
                descent: 0.0,
                line_gap: 0.0,
                typo: None,
            };
        }
        let primary = self.primary(spec);
        let font = &self.faces[primary as usize].font;
        let mut ascent = round(font.ascent() as f32);
        let descent = round(font.descent() as f32);
        if matches!(
            font.family_name().as_str(),
            "Times" | "Helvetica" | "Courier"
        ) {
            ascent += ((ascent + descent) * 0.15 + 0.5).floor();
        }
        let typo = font.get_font_table(OS2).and_then(|table| {
            let bytes = table.bytes();
            let ascender = i16::from_be_bytes([*bytes.get(68)?, *bytes.get(69)?]);
            let descender = i16::from_be_bytes([*bytes.get(70)?, *bytes.get(71)?]);
            Some((f32::from(ascender), -f32::from(descender)))
        });
        FontMetrics {
            ascent,
            descent,
            line_gap: round(font.leading() as f32),
            typo,
        }
    }

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds> {
        let (_, _, [l, b, r, t]) = self.glyph_rect(glyph)?;
        Some(Bounds {
            left: l as f32,
            top: -t as f32,
            right: r as f32,
            bottom: -b as f32,
        })
    }

    fn raw_mask(&mut self, glyph: Glyph, scale: f64, x_scale: f64, phase: u8) -> Option<Arc<Mask>> {
        if self.faces.get(glyph.face as usize)?.color {
            return self.color_image(glyph, scale, x_scale, phase);
        }
        let (font, id, [l, b, r, t]) = self.glyph_rect(glyph)?;
        let size = font.pt_size() as f32;
        let text_size = size * relax(scale as f32);
        let residual = f64::from(size * relax((scale * x_scale) as f32) / text_size);
        let sx = scale * x_scale;
        let shift = f64::from(phase) / 4.0;
        let x0 = (l * sx + shift).floor() as i32 - 2;
        let x1 = (r * sx + shift).ceil() as i32 + 2;
        let y0 = (b * scale).floor() as i32 - 2;
        let y1 = (t * scale).ceil() as i32 + 2;
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let device = self.device_font(glyph.face, &font, text_size);
        let cx = bitmap_context(w, h);
        cx.set_text_matrix(&CGAffineTransform::new(residual, 0.0, 0.0, 1.0, 0.0, 0.0));
        let origin = CGPoint::new((shift - f64::from(x0)) * (1.0 / residual), -f64::from(y0));
        device.draw_glyphs(&[id], &[origin], cx.clone());
        Some(read_mask(cx, x0, y1, w, h))
    }

    fn raw_mask_transformed(
        &mut self,
        glyph: Glyph,
        [a, b, c, d]: [f64; 4],
        (phase_x, phase_y): (u8, u8),
    ) -> Option<Arc<Mask>> {
        let (font, id, [l, bottom, r, top]) = self.glyph_rect(glyph)?;
        let [a, b, c, d] = [a, b, c, d].map(|v| f64::from(relax(v as f32)));
        // Font space is y-up and so is the bitmap context, while the canvas matrix is y-down.
        let m = CGAffineTransform::new(a, -b, -c, d, 0.0, 0.0);
        let (sx, sy) = (f64::from(phase_x) / 4.0, -f64::from(phase_y) / 4.0);
        let corners = [(l, bottom), (r, bottom), (l, top), (r, top)]
            .map(|(x, y)| (m.a * x + m.c * y + sx, m.b * x + m.d * y + sy));
        let min = |f: fn(&(f64, f64)) -> f64| corners.iter().map(f).fold(f64::INFINITY, f64::min);
        let max =
            |f: fn(&(f64, f64)) -> f64| corners.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
        let x0 = min(|p| p.0).floor() as i32 - 2;
        let x1 = max(|p| p.0).ceil() as i32 + 2;
        let y0 = min(|p| p.1).floor() as i32 - 2;
        let y1 = max(|p| p.1).ceil() as i32 + 2;
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let cx = bitmap_context(w, h);
        cx.translate(sx - f64::from(x0), sy - f64::from(y0));
        cx.concat_ctm(m);
        font.draw_glyphs(&[id], &[CGPoint::new(0.0, 0.0)], cx.clone());
        Some(read_mask(cx, x0, y1, w, h))
    }
}
