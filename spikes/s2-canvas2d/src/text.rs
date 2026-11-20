use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use gpui::{
    DevicePixels, Font, FontFeatures, FontId, FontRun, FontStyle, FontWeight, GlyphId,
    PlatformTextSystem, RenderGlyphParams, point, px,
};
use swash::scale::{Render, ScaleContext, Source};
use swash::shape::ShapeContext;
use swash::zeno::{Format, Transform, Vector};
use swash::{FontRef, tag_from_bytes};

pub const SYSTEM_FONT_FILE: &str = "/System/Library/Fonts/SFNS.ttf";

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FontSpec {
    pub size: f64,
    pub weight: u16,
}

pub struct ShapedGlyph {
    pub id: u32,
    pub x: f64,
}

pub struct Shaped {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f64,
}

#[derive(Clone)]
pub struct Mask {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub trait TextBackend {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped;
    fn rasterize(&mut self, font: &FontSpec, glyph: u32, scale: f32, sub: u8, color: [f32; 4]) -> Option<Mask>;
    fn rasterize_scaled(
        &mut self,
        font: &FontSpec,
        glyph: u32,
        scale: f32,
        sx: f32,
        sub: u8,
        color: [f32; 4],
    ) -> Option<Mask>;
}

fn system_font_data() -> &'static [u8] {
    static DATA: OnceLock<Vec<u8>> = OnceLock::new();
    DATA.get_or_init(|| std::fs::read(SYSTEM_FONT_FILE).expect("system font"))
}

fn typo_metrics() -> (f64, f64) {
    let font = FontRef::from_index(system_font_data(), 0).unwrap();
    let os2 = font.table(tag_from_bytes(b"OS/2")).unwrap();
    let asc = i16::from_be_bytes([os2[68], os2[69]]) as f64;
    let desc = i16::from_be_bytes([os2[70], os2[71]]) as f64;
    (asc, -desc)
}

fn layout_round(v: f64) -> f64 {
    (v * 64.0).round() / 64.0
}

pub fn normalized_typo(size: f64) -> (f64, f64) {
    let (asc, desc) = typo_metrics();
    let ascent = layout_round(asc * size / (asc + desc));
    let descent = layout_round(size) - ascent;
    (ascent, descent)
}

pub fn baseline_offset(font: &FontSpec, baseline: &str) -> f64 {
    let (ascent, descent) = normalized_typo(font.size);
    match baseline {
        "top" => ascent,
        "bottom" => -descent,
        "middle" => (ascent - descent) / 2.0,
        _ => 0.0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dilation {
    Off,
    ByColor,
}

pub struct GpuiText {
    ts: Arc<dyn PlatformTextSystem>,
    ids: HashMap<u16, FontId>,
    dilation: Dilation,
    cache: HashMap<(u16, u32, u32, u8, u8, u32), Option<Mask>>,
    fallback: SwashText,
    pub lut: Option<Box<[[u8; 256]; 8]>>,
}

pub fn luminance_bucket(color: [f32; 4]) -> usize {
    let lum = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
    ((lum * 255.0).round().clamp(0.0, 255.0) as u8 >> 5) as usize
}

impl GpuiText {
    pub fn new(dilation: Dilation) -> Self {
        let platform = gpui_platform::current_platform(true);
        GpuiText {
            ts: platform.text_system(),
            ids: HashMap::new(),
            dilation,
            cache: HashMap::new(),
            fallback: SwashText::new(),
            lut: None,
        }
    }

    fn font_id(&mut self, weight: u16) -> FontId {
        let ts = self.ts.clone();
        *self.ids.entry(weight).or_insert_with(|| {
            ts.font_id(&Font {
                family: ".SystemUIFont".into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight(weight as f32),
                style: FontStyle::Normal,
            })
            .unwrap()
        })
    }

    pub fn width(&mut self, font: &FontSpec, text: &str) -> f64 {
        let id = self.font_id(font.weight);
        let layout = self.ts.layout_line(text, px(font.size as f32), &[FontRun { len: text.len(), font_id: id }]);
        f32::from(layout.width) as f64
    }

    fn dilation_for(&self, color: [f32; 4]) -> u8 {
        match self.dilation {
            Dilation::Off => 0,
            Dilation::ByColor => {
                let lum = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
                ((4.0 * lum + 0.5).floor() as i32).clamp(0, 4) as u8
            }
        }
    }
}

impl TextBackend for GpuiText {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped {
        let id = self.font_id(font.weight);
        let layout = self.ts.layout_line(text, px(font.size as f32), &[FontRun { len: text.len(), font_id: id }]);
        let mut glyphs = Vec::new();
        for run in &layout.runs {
            for g in &run.glyphs {
                glyphs.push(ShapedGlyph { id: g.id.0, x: f32::from(g.position.x) as f64 });
            }
        }
        Shaped { glyphs, width: f32::from(layout.width) as f64 }
    }

    fn rasterize(&mut self, font: &FontSpec, glyph: u32, scale: f32, sub: u8, color: [f32; 4]) -> Option<Mask> {
        let dilation = if self.lut.is_some() { 0 } else { self.dilation_for(color) };
        let mask = self.raw_mask(font, glyph, scale, sub, dilation);
        match (&self.lut, mask) {
            (Some(lut), Some(mut m)) => {
                let table = &lut[luminance_bucket(color)];
                for v in &mut m.data {
                    *v = table[*v as usize];
                }
                Some(m)
            }
            (_, m) => m,
        }
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
        self.fallback.rasterize_scaled(font, glyph, scale, sx, sub, color)
    }
}

impl GpuiText {
    fn raw_mask(&mut self, font: &FontSpec, glyph: u32, scale: f32, sub: u8, dilation: u8) -> Option<Mask> {
        let key = (font.weight, (font.size * 64.0) as u32, glyph, sub, dilation, scale.to_bits());
        if let Some(m) = self.cache.get(&key) {
            return m.clone();
        }
        let params = RenderGlyphParams {
            font_id: self.font_id(font.weight),
            glyph_id: GlyphId(glyph),
            font_size: px(font.size as f32),
            subpixel_variant: point(sub, 0),
            scale_factor: scale,
            is_emoji: false,
            subpixel_rendering: false,
            dilation,
        };
        let mask = (|| {
            let bounds = self.ts.glyph_raster_bounds(&params).ok()?;
            if bounds.size.width == DevicePixels(0) || bounds.size.height == DevicePixels(0) {
                return None;
            }
            let (size, bytes) = self.ts.rasterize_glyph(&params, bounds).ok()?;
            Some(Mask {
                left: bounds.origin.x.0,
                top: bounds.origin.y.0,
                width: size.width.0 as u32,
                height: size.height.0 as u32,
                data: bytes,
            })
        })();
        self.cache.insert(key, mask.clone());
        mask
    }
}

pub struct SwashText {
    shape_cx: ShapeContext,
    scale_cx: ScaleContext,
    pub opsz: bool,
}

impl SwashText {
    pub fn new() -> Self {
        SwashText { shape_cx: ShapeContext::new(), scale_cx: ScaleContext::new(), opsz: true }
    }

    fn variations(&self, font: &FontSpec) -> Vec<(&'static str, f32)> {
        let mut v = vec![("wght", font.weight as f32)];
        if self.opsz {
            v.push(("opsz", font.size as f32));
        }
        v
    }

    fn render(&mut self, font: &FontSpec, glyph: u32, scale: f32, sx: f32, sub: u8) -> Option<Mask> {
        let vars = self.variations(font);
        let fref = FontRef::from_index(system_font_data(), 0)?;
        let mut scaler = self
            .scale_cx
            .builder(fref)
            .size(font.size as f32 * scale)
            .hint(false)
            .variations(vars)
            .build();
        let mut render = Render::new(&[Source::Outline]);
        render.format(Format::Alpha).offset(Vector::new(sub as f32 / 4.0, 0.0));
        if sx != 1.0 {
            render.transform(Some(Transform::scale(sx, 1.0)));
        }
        let img = render.render(&mut scaler, glyph as u16)?;
        if img.placement.width == 0 {
            return None;
        }
        Some(Mask {
            left: img.placement.left,
            top: -img.placement.top,
            width: img.placement.width,
            height: img.placement.height,
            data: img.data,
        })
    }
}

impl TextBackend for SwashText {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped {
        let vars = self.variations(font);
        let fref = FontRef::from_index(system_font_data(), 0).unwrap();
        let mut shaper = self.shape_cx.builder(fref).size(font.size as f32).variations(vars).build();
        shaper.add_str(text);
        let mut glyphs = Vec::new();
        let mut x = 0.0f64;
        shaper.shape_with(|cluster| {
            for g in cluster.glyphs {
                glyphs.push(ShapedGlyph { id: g.id as u32, x: x + g.x as f64 });
                x += g.advance as f64;
            }
        });
        Shaped { glyphs, width: x }
    }

    fn rasterize(&mut self, font: &FontSpec, glyph: u32, scale: f32, sub: u8, _color: [f32; 4]) -> Option<Mask> {
        self.render(font, glyph, scale, 1.0, sub)
    }

    fn rasterize_scaled(
        &mut self,
        font: &FontSpec,
        glyph: u32,
        scale: f32,
        sx: f32,
        sub: u8,
        _color: [f32; 4],
    ) -> Option<Mask> {
        self.render(font, glyph, scale, sx, sub)
    }
}
