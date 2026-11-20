#![allow(unsafe_code)]

use std::collections::HashMap;

use core_foundation::attributed_string::CFMutableAttributedString;
use core_foundation::base::{CFRange, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::{CGContext, CGTextDrawingMode};
use core_graphics::font::CGGlyph;
use core_graphics::geometry::{CGAffineTransform, CGPoint};
use core_text::font::{CTFont, new_from_descriptor, new_ui_font_for_language};
use core_text::font_descriptor::{kCTFontOrientationHorizontal, kCTFontVariationAttribute};
use core_text::line::CTLine;
use core_text::string_attributes::kCTFontAttributeName;

use super::{FontSpec, Mask, Shaped, ShapedGlyph, TextBackend};

const K_CT_FONT_UI_FONT_SYSTEM: u32 = 2;
const K_CG_IMAGE_ALPHA_ONLY: u32 = 7;
const WGHT: i64 = 0x7767_6874;
const OS2: u32 = 0x4f53_2f32;

type FontKey = (u16, u64);
type MaskKey = (FontKey, u32, u8, u64, u64);

pub(crate) struct CoreText {
    fonts: HashMap<FontKey, CTFont>,
    masks: HashMap<MaskKey, Option<Mask>>,
}

fn font_key(spec: &FontSpec) -> FontKey {
    (spec.weight, spec.size.to_bits())
}

impl CoreText {
    pub(crate) fn new() -> Self {
        CoreText {
            fonts: HashMap::new(),
            masks: HashMap::new(),
        }
    }

    fn font(&mut self, spec: &FontSpec) -> CTFont {
        self.fonts
            .entry(font_key(spec))
            .or_insert_with(|| {
                let base = new_ui_font_for_language(K_CT_FONT_UI_FONT_SYSTEM, spec.size, None);
                if spec.weight == 400 {
                    return base;
                }
                let axes = CFDictionary::from_CFType_pairs(&[(
                    CFNumber::from(WGHT),
                    CFNumber::from(f64::from(spec.weight)),
                )]);
                // SAFETY: a CoreText constant, valid for the life of the process.
                let key = unsafe { CFString::wrap_under_get_rule(kCTFontVariationAttribute) };
                let attributes = CFDictionary::from_CFType_pairs(&[(key, axes)]);
                let descriptor = base
                    .copy_descriptor()
                    .create_copy_with_attributes(attributes.to_untyped())
                    .expect("variation descriptor");
                new_from_descriptor(&descriptor, spec.size)
            })
            .clone()
    }

    fn line(&mut self, spec: &FontSpec, text: &str) -> CTLine {
        let font = self.font(spec);
        let mut string = CFMutableAttributedString::new();
        string.replace_str(&CFString::new(text), CFRange::init(0, 0));
        let len = string.char_len();
        // SAFETY: `kCTFontAttributeName` is a CoreText constant and `font` is a live CTFont.
        unsafe {
            string.set_attribute(CFRange::init(0, len), kCTFontAttributeName, &font);
        }
        CTLine::new_with_attributed_string(string.as_concrete_TypeRef())
    }

    fn render(
        &mut self,
        spec: &FontSpec,
        glyph: u32,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Mask> {
        let font = self.font(spec);
        let id = glyph as CGGlyph;
        let rect = font.get_bounding_rects_for_glyphs(kCTFontOrientationHorizontal, &[id]);
        if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
            return None;
        }
        let sx = scale * x_scale;
        let shift = f64::from(phase) / 4.0;
        let x0 = (rect.origin.x * sx + shift).floor() as i32 - 2;
        let x1 = ((rect.origin.x + rect.size.width) * sx + shift).ceil() as i32 + 2;
        let y0 = (rect.origin.y * scale).floor() as i32 - 2;
        let y1 = ((rect.origin.y + rect.size.height) * scale).ceil() as i32 + 2;
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut cx = CGContext::create_bitmap_context(
            None,
            w,
            h,
            8,
            w,
            &CGColorSpace::create_device_gray(),
            K_CG_IMAGE_ALPHA_ONLY,
        );
        cx.data().fill(0);
        cx.translate(-f64::from(x0), -f64::from(y0));
        cx.scale(sx, scale);
        cx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
        cx.set_allows_antialiasing(true);
        cx.set_should_antialias(true);
        cx.set_allows_font_subpixel_positioning(true);
        cx.set_should_subpixel_position_fonts(true);
        cx.set_allows_font_subpixel_quantization(false);
        cx.set_should_subpixel_quantize_fonts(false);
        cx.set_gray_fill_color(0.0, 1.0);
        font.draw_glyphs(&[id], &[CGPoint::new(shift / sx, 0.0)], cx.clone());
        let stride = cx.bytes_per_row();
        let data = cx.data();
        let mut bytes = Vec::with_capacity(w * h);
        for row in 0..h {
            bytes.extend_from_slice(&data[row * stride..row * stride + w]);
        }
        Some(Mask {
            left: x0,
            top: -y1,
            width: w as u32,
            height: h as u32,
            data: bytes,
        })
    }
}

impl TextBackend for CoreText {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped {
        let line = self.line(font, text);
        let mut glyphs = Vec::new();
        for run in line.glyph_runs().iter() {
            for (g, p) in run.glyphs().iter().zip(run.positions().iter()) {
                glyphs.push(ShapedGlyph {
                    id: u32::from(*g),
                    x: p.x,
                });
            }
        }
        Shaped {
            glyphs,
            width: line.get_typographic_bounds().width,
        }
    }

    fn typo_extents(&mut self, font: &FontSpec) -> (f64, f64) {
        let table = self
            .font(font)
            .get_font_table(OS2)
            .expect("the system UI font has an OS/2 table");
        let bytes = table.bytes();
        let ascender = i16::from_be_bytes([bytes[68], bytes[69]]);
        let descender = i16::from_be_bytes([bytes[70], bytes[71]]);
        (f64::from(ascender), -f64::from(descender))
    }

    fn raw_mask(
        &mut self,
        font: &FontSpec,
        glyph: u32,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Mask> {
        let key = (
            font_key(font),
            glyph,
            phase,
            scale.to_bits(),
            x_scale.to_bits(),
        );
        if let Some(mask) = self.masks.get(&key) {
            return mask.clone();
        }
        let mask = self.render(font, glyph, scale, x_scale, phase);
        self.masks.insert(key, mask.clone());
        mask
    }

    fn raw_mask_transformed(
        &mut self,
        spec: &FontSpec,
        glyph: u32,
        [a, b, c, d]: [f64; 4],
        (phase_x, phase_y): (u8, u8),
    ) -> Option<Mask> {
        let font = self.font(spec);
        let id = glyph as CGGlyph;
        let rect = font.get_bounding_rects_for_glyphs(kCTFontOrientationHorizontal, &[id]);
        if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
            return None;
        }
        // Font space is y-up and so is the bitmap context, while the canvas matrix is y-down.
        let m = CGAffineTransform::new(a, -b, -c, d, 0.0, 0.0);
        let (sx, sy) = (f64::from(phase_x) / 4.0, -f64::from(phase_y) / 4.0);
        let (l, t) = (rect.origin.x, rect.origin.y);
        let (r, u) = (l + rect.size.width, t + rect.size.height);
        let corners = [(l, t), (r, t), (l, u), (r, u)]
            .map(|(x, y)| (m.a * x + m.c * y + sx, m.b * x + m.d * y + sy));
        let min = |f: fn(&(f64, f64)) -> f64| corners.iter().map(f).fold(f64::INFINITY, f64::min);
        let max =
            |f: fn(&(f64, f64)) -> f64| corners.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
        let x0 = min(|p| p.0).floor() as i32 - 2;
        let x1 = max(|p| p.0).ceil() as i32 + 2;
        let y0 = min(|p| p.1).floor() as i32 - 2;
        let y1 = max(|p| p.1).ceil() as i32 + 2;
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut cx = CGContext::create_bitmap_context(
            None,
            w,
            h,
            8,
            w,
            &CGColorSpace::create_device_gray(),
            K_CG_IMAGE_ALPHA_ONLY,
        );
        cx.data().fill(0);
        cx.translate(sx - f64::from(x0), sy - f64::from(y0));
        cx.concat_ctm(m);
        cx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
        cx.set_allows_antialiasing(true);
        cx.set_should_antialias(true);
        cx.set_allows_font_subpixel_positioning(true);
        cx.set_should_subpixel_position_fonts(true);
        cx.set_allows_font_subpixel_quantization(false);
        cx.set_should_subpixel_quantize_fonts(false);
        cx.set_gray_fill_color(0.0, 1.0);
        font.draw_glyphs(&[id], &[CGPoint::new(0.0, 0.0)], cx.clone());
        let stride = cx.bytes_per_row();
        let data = cx.data();
        let mut bytes = Vec::with_capacity(w * h);
        for row in 0..h {
            bytes.extend_from_slice(&data[row * stride..row * stride + w]);
        }
        Some(Mask {
            left: x0,
            top: -y1,
            width: w as u32,
            height: h as u32,
            data: bytes,
        })
    }
}
