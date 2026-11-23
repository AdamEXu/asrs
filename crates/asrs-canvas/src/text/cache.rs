use std::collections::HashMap;
use std::sync::Arc;

use super::font::FontKey;
use super::{Bounds, FontMetrics, FontSpec, Glyph, LcdMask, Mask, Shaped, TextBackend};

const MAX_SHAPES: usize = 4096;
const MAX_MASKS: usize = 16384;

type MaskKey = (Glyph, [u64; 4], (u8, u8));

/// Memoizes a backend's shaping, metrics, bounds and glyph masks. Shapes and masks are dropped
/// wholesale when a table outgrows its cap, so memory stays bounded across long sessions.
pub struct Cached<B> {
    backend: B,
    shapes: HashMap<(FontKey, u32, String), Shaped>,
    runs: HashMap<(FontKey, u32, String), Shaped>,
    metrics: HashMap<FontKey, FontMetrics>,
    fallback_metrics: HashMap<(Glyph, u32), Option<FontMetrics>>,
    bounds: HashMap<Glyph, Option<Bounds>>,
    masks: HashMap<MaskKey, Option<Arc<Mask>>>,
    lcd_masks: HashMap<MaskKey, Option<Arc<LcdMask>>>,
    device_scale: f32,
}

impl<B: TextBackend> Cached<B> {
    pub fn new(backend: B) -> Self {
        Cached {
            backend,
            shapes: HashMap::new(),
            runs: HashMap::new(),
            metrics: HashMap::new(),
            fallback_metrics: HashMap::new(),
            bounds: HashMap::new(),
            masks: HashMap::new(),
            lcd_masks: HashMap::new(),
            device_scale: 1.0,
        }
    }

    fn mask(
        &mut self,
        key: MaskKey,
        render: impl FnOnce(&mut B) -> Option<Arc<Mask>>,
    ) -> Option<Arc<Mask>> {
        if let Some(mask) = self.masks.get(&key) {
            return mask.clone();
        }
        if self.masks.len() >= MAX_MASKS {
            self.masks.clear();
        }
        let mask = render(&mut self.backend);
        self.masks.insert(key, mask.clone());
        mask
    }
}

impl<B: TextBackend> TextBackend for Cached<B> {
    fn shape(&mut self, font: &FontSpec, text: &str, zoom: f32) -> Shaped {
        let key = (font.key(), zoom.to_bits(), text.to_owned());
        if let Some(shaped) = self.shapes.get(&key) {
            return shaped.clone();
        }
        if self.shapes.len() >= MAX_SHAPES {
            self.shapes.clear();
        }
        let shaped = self.backend.shape(font, text, zoom);
        self.shapes.insert(key, shaped.clone());
        shaped
    }

    fn shape_run(&mut self, font: &FontSpec, text: &str, zoom: f32) -> Shaped {
        let key = (font.key(), zoom.to_bits(), text.to_owned());
        if let Some(shaped) = self.runs.get(&key) {
            return shaped.clone();
        }
        if self.runs.len() >= MAX_SHAPES {
            self.runs.clear();
        }
        let shaped = self.backend.shape_run(font, text, zoom);
        self.runs.insert(key, shaped.clone());
        shaped
    }

    fn metrics(&mut self, font: &FontSpec) -> FontMetrics {
        *self
            .metrics
            .entry(font.key())
            .or_insert_with(|| self.backend.metrics(font))
    }

    fn fallback_metrics(&mut self, glyph: Glyph, zoom: f32) -> Option<FontMetrics> {
        *self
            .fallback_metrics
            .entry((glyph, zoom.to_bits()))
            .or_insert_with(|| self.backend.fallback_metrics(glyph, zoom))
    }

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds> {
        *self
            .bounds
            .entry(glyph)
            .or_insert_with(|| self.backend.glyph_bounds(glyph))
    }

    fn raw_mask(&mut self, glyph: Glyph, scale: f64, x_scale: f64, phase: u8) -> Option<Arc<Mask>> {
        let key = (
            glyph,
            [scale.to_bits(), x_scale.to_bits(), 0, 0],
            (phase, 0),
        );
        self.mask(key, |b| b.raw_mask(glyph, scale, x_scale, phase))
    }

    fn raw_mask_transformed(
        &mut self,
        glyph: Glyph,
        matrix: [f64; 4],
        phase: (u8, u8),
    ) -> Option<Arc<Mask>> {
        let key = (
            glyph,
            matrix.map(|v| v.to_bits()),
            (phase.0 | 0x80, phase.1),
        );
        self.mask(key, |b| b.raw_mask_transformed(glyph, matrix, phase))
    }

    fn raw_lcd_mask(
        &mut self,
        glyph: Glyph,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Arc<LcdMask>> {
        let key = (
            glyph,
            [scale.to_bits(), x_scale.to_bits(), 0, 0],
            (phase, 0),
        );
        if let Some(mask) = self.lcd_masks.get(&key) {
            return mask.clone();
        }
        if self.lcd_masks.len() >= MAX_MASKS {
            self.lcd_masks.clear();
        }
        let mask = self.backend.raw_lcd_mask(glyph, scale, x_scale, phase);
        self.lcd_masks.insert(key, mask.clone());
        mask
    }

    fn set_device_scale(&mut self, scale: f32) {
        if scale == self.device_scale {
            return;
        }
        self.device_scale = scale;
        self.backend.set_device_scale(scale);
        self.shapes.clear();
        self.runs.clear();
        self.metrics.clear();
        self.fallback_metrics.clear();
        self.bounds.clear();
        self.masks.clear();
        self.lcd_masks.clear();
    }
}
