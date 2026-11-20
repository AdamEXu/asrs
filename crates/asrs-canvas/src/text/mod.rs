pub mod cache;
pub mod coverage;
pub mod font;
pub mod layout;
pub mod lut;
#[cfg(target_os = "macos")]
mod mac;
pub mod metrics;

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
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped;

    fn metrics(&mut self, font: &FontSpec) -> FontMetrics;

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds>;

    /// Raw coverage, before mask gamma, of `glyph` drawn at device scale `scale`, compressed
    /// horizontally by `x_scale`, with its origin `phase` quarter pixels right of the pixel grid.
    /// `left` and `top` are relative to that pixel.
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
}

/// The platform backend behind a glyph and shaping cache.
pub fn platform_backend() -> Option<Box<dyn TextBackend>> {
    #[cfg(target_os = "macos")]
    return Some(Box::new(cache::Cached::new(mac::CoreText::new())));
    #[cfg(not(target_os = "macos"))]
    return None;
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
