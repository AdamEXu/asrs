use std::collections::HashMap;

use core_foundation::attributed_string::CFMutableAttributedString;
use core_foundation::base::{CFRange, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::base::CGFloat;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::{CGContext, CGTextDrawingMode};
use core_graphics::font::CGGlyph;
use core_graphics::geometry::CGPoint;
use core_text::font::{CTFont, new_from_descriptor, new_ui_font_for_language};
use core_text::font_descriptor::{kCTFontOrientationHorizontal, kCTFontVariationAttribute};
use core_text::line::CTLine;
use core_text::string_attributes::kCTFontAttributeName;

use crate::text::{FontSpec, Mask, Shaped, ShapedGlyph, TextBackend, luminance_bucket};

const K_CT_FONT_UI_FONT_SYSTEM: u32 = 2;
const WGHT: i64 = 0x7767_6874;
const K_CG_IMAGE_ALPHA_ONLY: u32 = 7;

pub struct CoreText {
    fonts: HashMap<(u16, u64), CTFont>,
    cache: HashMap<(u16, u64, u32, u8, u32, u32), Option<Mask>>,
    pub lut: Option<Box<[[u8; 256]; 8]>>,
}

impl CoreText {
    pub fn new() -> Self {
        CoreText { fonts: HashMap::new(), cache: HashMap::new(), lut: None }
    }

    fn font(&mut self, spec: &FontSpec) -> CTFont {
        let key = (spec.weight, spec.size.to_bits());
        self.fonts
            .entry(key)
            .or_insert_with(|| {
                let base = new_ui_font_for_language(K_CT_FONT_UI_FONT_SYSTEM, spec.size, None);
                if spec.weight == 400 {
                    return base;
                }
                let vals = CFDictionary::from_CFType_pairs(&[(
                    CFNumber::from(WGHT),
                    CFNumber::from(spec.weight as f64),
                )]);
                let key = unsafe { CFString::wrap_under_get_rule(kCTFontVariationAttribute) };
                let attrs = CFDictionary::from_CFType_pairs(&[(key, vals)]);
                let desc = base.copy_descriptor().create_copy_with_attributes(attrs.to_untyped()).unwrap();
                new_from_descriptor(&desc, spec.size)
            })
            .clone()
    }

    pub fn width(&mut self, spec: &FontSpec, text: &str) -> f64 {
        self.line(spec, text).get_typographic_bounds().width
    }

    fn line(&mut self, spec: &FontSpec, text: &str) -> CTLine {
        let font = self.font(spec);
        let mut string = CFMutableAttributedString::new();
        string.replace_str(&CFString::new(text), CFRange::init(0, 0));
        let len = string.char_len();
        unsafe {
            string.set_attribute(
                CFRange::init(0, len),
                kCTFontAttributeName,
                &font,
            );
        }
        CTLine::new_with_attributed_string(string.as_concrete_TypeRef())
    }

    fn raw(&mut self, spec: &FontSpec, glyph: u32, scale: f32, sx: f32, sub: u8) -> Option<Mask> {
        let key = (spec.weight, spec.size.to_bits(), glyph, sub, scale.to_bits(), sx.to_bits());
        if let Some(m) = self.cache.get(&key) {
            return m.clone();
        }
        let font = self.font(spec);
        let rect = font.get_bounding_rects_for_glyphs(kCTFontOrientationHorizontal, &[glyph as CGGlyph]);
        let s = scale as f64;
        let sxs = s * sx as f64;
        let shift = sub as f64 / 4.0;
        let x0 = (rect.origin.x * sxs + shift).floor() as i32 - 2;
        let x1 = ((rect.origin.x + rect.size.width) * sxs + shift).ceil() as i32 + 2;
        let y0 = (rect.origin.y * s).floor() as i32 - 2;
        let y1 = ((rect.origin.y + rect.size.height) * s).ceil() as i32 + 2;
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mask = if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
            None
        } else {
            let mut bytes = vec![0u8; w * h];
            let cx = CGContext::create_bitmap_context(
                Some(bytes.as_mut_ptr() as *mut _),
                w,
                h,
                8,
                w,
                &CGColorSpace::create_device_gray(),
                K_CG_IMAGE_ALPHA_ONLY,
            );
            cx.translate(-x0 as CGFloat, -y0 as CGFloat);
            cx.scale(sxs, s);
            cx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
            cx.set_allows_antialiasing(true);
            cx.set_should_antialias(true);
            cx.set_allows_font_subpixel_positioning(true);
            cx.set_should_subpixel_position_fonts(true);
            cx.set_allows_font_subpixel_quantization(false);
            cx.set_should_subpixel_quantize_fonts(false);
            cx.set_gray_fill_color(0.0, 1.0);
            font.draw_glyphs(&[glyph as CGGlyph], &[CGPoint::new(shift / sxs, 0.0)], cx);
            Some(Mask { left: x0, top: -y1, width: w as u32, height: h as u32, data: bytes })
        };
        self.cache.insert(key, mask.clone());
        mask
    }

    fn apply_lut(&self, mask: Option<Mask>, color: [f32; 4]) -> Option<Mask> {
        let mut m = mask?;
        if let Some(lut) = &self.lut {
            let t = &lut[luminance_bucket(color)];
            for v in &mut m.data {
                *v = t[*v as usize];
            }
        }
        Some(m)
    }
}

impl TextBackend for CoreText {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped {
        let line = self.line(font, text);
        let mut glyphs = Vec::new();
        for run in line.glyph_runs().iter() {
            for (g, p) in run.glyphs().iter().zip(run.positions().iter()) {
                glyphs.push(ShapedGlyph { id: *g as u32, x: p.x });
            }
        }
        Shaped { glyphs, width: line.get_typographic_bounds().width }
    }

    fn rasterize(&mut self, font: &FontSpec, glyph: u32, scale: f32, sub: u8, color: [f32; 4]) -> Option<Mask> {
        let m = self.raw(font, glyph, scale, 1.0, sub);
        self.apply_lut(m, color)
    }

    fn rasterize_scaled(
        &mut self,
        font: &FontSpec,
        glyph: u32,
        scale: f32,
        sx: f32,
        sub: u8,
        color: [f32; 4],
    ) -> Option<Mask> {
        let m = self.raw(font, glyph, scale, sx, sub);
        self.apply_lut(m, color)
    }
}
