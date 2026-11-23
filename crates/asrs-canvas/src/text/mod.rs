pub mod cache;
pub mod coverage;
pub mod dom;
pub mod flow;
pub mod font;
pub mod inline;
pub mod layout;
pub mod line_break;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod lut;
#[cfg(target_os = "macos")]
mod mac;
pub mod metrics;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod shaping;

use std::sync::Arc;

pub use font::{Family, FontSpec};
pub use metrics::{FixedAdvance, TextMetrics, measure_text, text_state};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// `data` is a colour glyph's premultiplied RGBA, four bytes a pixel, not coverage.
    pub color: bool,
}

/// A glyph's coverage for an RGB-ordered LCD, one value per colour channel, `3 * width * height`
/// bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LcdMask {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// A glyph of one sized font instance. `face` is the backend's id for that instance: the primary
/// font of a [`FontSpec`] or a fallback font its shaper picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Glyph {
    pub face: u32,
    pub id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub glyph: Glyph,
    pub x: f32,
    /// UTF-16 offset of the first code unit of the character this glyph draws.
    pub cluster: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shaped {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
}

/// Vertical metrics of a font's primary face as Blink's `SimpleFontData` holds them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    /// Rounded ascent and descent in CSS px, after Blink's per-platform adjustments.
    pub ascent: f32,
    pub descent: f32,
    /// Rounded line gap in CSS px, the leading `line-height: normal` adds.
    pub line_gap: f32,
    /// OS/2 `sTypoAscender` and the negated `sTypoDescender` in font units, if the face has them.
    pub typo: Option<(f32, f32)>,
}

/// Ink bounds of a glyph in CSS px around its origin, y down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// A glyph source that mirrors the scaler Skia uses in Chromium on one platform.
pub trait TextBackend {
    /// Glyphs and advances of `text`, already space-normalized, with platform font fallback.
    /// Advances are in CSS px of `font` laid out at zoom `zoom`, as Blink lays DOM text out at
    /// the device scale; canvas text shapes at zoom 1.
    fn shape(&mut self, font: &FontSpec, text: &str, zoom: f32) -> Shaped;

    /// `text` as Blink's `HarfBuzzShaper` shapes a DOM text run: in one piece, so kerning applies
    /// across its spaces, where canvas text shapes every word apart.
    fn shape_run(&mut self, font: &FontSpec, text: &str, zoom: f32) -> Shaped {
        self.shape(font, text, zoom)
    }

    fn metrics(&mut self, font: &FontSpec) -> FontMetrics;

    /// The metrics at zoom `zoom` of the fallback font `glyph` comes from, or `None` for a glyph
    /// of a primary font.
    fn fallback_metrics(&mut self, glyph: Glyph, zoom: f32) -> Option<FontMetrics> {
        let _ = (glyph, zoom);
        None
    }

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds>;

    /// Raw coverage, before mask gamma, of `glyph` drawn at device scale `scale`, compressed
    /// horizontally by `x_scale`, with its origin `phase` quarter pixels right of the pixel grid.
    /// `left` and `top` are relative to that pixel. A colour glyph comes back as its RGBA image.
    fn raw_mask(&mut self, glyph: Glyph, scale: f64, x_scale: f64, phase: u8) -> Option<Arc<Mask>>;

    /// Raw coverage of `glyph` under any device matrix `[a, b, c, d]` (`x' = a x + c y`,
    /// `y' = b x + d y`, y down), its origin `phase` quarter pixels right of and below the pixel
    /// grid, as Skia rasterizes text under rotation or flips.
    fn raw_mask_transformed(
        &mut self,
        glyph: Glyph,
        matrix: [f64; 4],
        phase: (u8, u8),
    ) -> Option<Arc<Mask>>;

    /// `raw_mask` as three-channel LCD coverage, or `None` where the backend has no LCD text or
    /// the glyph is a colour glyph.
    fn raw_lcd_mask(
        &mut self,
        glyph: Glyph,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Arc<LcdMask>> {
        let _ = (glyph, scale, x_scale, phase);
        None
    }

    /// The display scale the app runs at, returning whether it changes what the backend answers.
    /// Chromium turns hinting off for every font, canvas fonts included, on a Linux display scaled
    /// above 1.
    fn set_device_scale(&mut self, scale: f32) -> bool {
        let _ = scale;
        false
    }
}

impl<T: TextBackend + ?Sized> TextBackend for Box<T> {
    fn shape(&mut self, font: &FontSpec, text: &str, zoom: f32) -> Shaped {
        (**self).shape(font, text, zoom)
    }

    fn shape_run(&mut self, font: &FontSpec, text: &str, zoom: f32) -> Shaped {
        (**self).shape_run(font, text, zoom)
    }

    fn metrics(&mut self, font: &FontSpec) -> FontMetrics {
        (**self).metrics(font)
    }

    fn fallback_metrics(&mut self, glyph: Glyph, zoom: f32) -> Option<FontMetrics> {
        (**self).fallback_metrics(glyph, zoom)
    }

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds> {
        (**self).glyph_bounds(glyph)
    }

    fn raw_mask(&mut self, glyph: Glyph, scale: f64, x_scale: f64, phase: u8) -> Option<Arc<Mask>> {
        (**self).raw_mask(glyph, scale, x_scale, phase)
    }

    fn raw_mask_transformed(
        &mut self,
        glyph: Glyph,
        matrix: [f64; 4],
        phase: (u8, u8),
    ) -> Option<Arc<Mask>> {
        (**self).raw_mask_transformed(glyph, matrix, phase)
    }

    fn raw_lcd_mask(
        &mut self,
        glyph: Glyph,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Arc<LcdMask>> {
        (**self).raw_lcd_mask(glyph, scale, x_scale, phase)
    }

    fn set_device_scale(&mut self, scale: f32) -> bool {
        (**self).set_device_scale(scale)
    }
}

/// The platform backend behind a glyph and shaping cache.
pub fn platform_backend() -> Option<Box<dyn TextBackend>> {
    #[cfg(target_os = "macos")]
    return Some(Box::new(cache::Cached::new(mac::CoreText::new())));
    #[cfg(target_os = "linux")]
    return linux::Fontations::new()
        .map(|b| Box::new(cache::Cached::new(b)) as Box<dyn TextBackend>);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return None;
}

/// Skia's `sk_relax`: a strike's device matrix entry rounded to 1/1024.
pub(crate) fn relax(v: f32) -> f32 {
    (v * 1024.0 + 0.5).floor() / 1024.0
}

/// Blink's `LayoutUnit::FromFloatRound`.
fn layout_round(v: f32) -> f32 {
    (v * 64.0).round() / 64.0
}

/// Blink's em-height ascent and descent for canvas text: the typo extents (or, without them, the
/// rounded ascent and descent) scaled to the font size, ascent rounded to 1/64.
pub fn normalized_typo(metrics: &FontMetrics, size: f32) -> (f32, f32) {
    let normalize = |ascent: f32, height: f32| {
        if height <= 0.0 || ascent < 0.0 || ascent > height {
            return None;
        }
        let ascent = layout_round(ascent * size / height);
        Some((ascent, layout_round(size) - ascent))
    };
    metrics
        .typo
        .and_then(|(a, d)| normalize(a, a + d))
        .or_else(|| normalize(metrics.ascent, metrics.ascent + metrics.descent))
        .unwrap_or((0.0, 0.0))
}

/// Blink's `GetFontBaseline`: how far below the alphabetic baseline the given `textBaseline`
/// lies.
pub fn baseline_offset(metrics: &FontMetrics, size: f32, baseline: &str) -> f32 {
    match baseline {
        "top" => normalized_typo(metrics, size).0,
        "hanging" => (80.0 * metrics.ascent) / 100.0,
        "ideographic" => -metrics.descent,
        "bottom" => -normalized_typo(metrics, size).1,
        "middle" => {
            let (ascent, descent) = normalized_typo(metrics, size);
            (ascent - descent) / 2.0
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baselines_follow_blink() {
        let sf = FontMetrics {
            ascent: 12.0,
            descent: 3.0,
            line_gap: 0.0,
            typo: Some((1950.0, 494.0)),
        };
        assert_eq!(normalized_typo(&sf, 12.0), (9.578125, 2.421875));
        assert_eq!(baseline_offset(&sf, 12.0, "hanging"), 9.6);
        assert_eq!(baseline_offset(&sf, 12.0, "ideographic"), -3.0);
        assert_eq!(baseline_offset(&sf, 12.0, "alphabetic"), 0.0);
        let no_typo = FontMetrics { typo: None, ..sf };
        assert_eq!(normalized_typo(&no_typo, 10.0), (8.0, 2.0));
    }
}
