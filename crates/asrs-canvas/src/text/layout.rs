use std::borrow::Cow;
use std::sync::Arc;

use super::{FontSpec, Mask, Shaped, TextBackend, baseline_offset};

/// Blink's `TextRun::SetNormalizeSpace`: tabs, newlines and no-break spaces shape as spaces.
pub fn normalize_space(text: &str) -> Cow<'_, str> {
    if text.contains(['\t', '\n', '\u{a0}']) {
        Cow::Owned(text.replace(['\t', '\n', '\u{a0}'], " "))
    } else {
        Cow::Borrowed(text)
    }
}

/// `textAlign` resolved for left-to-right text: 0 for left, 1 for center, 2 for right.
fn align_kind(align: &str) -> u8 {
    match align {
        "center" => 1,
        "right" | "end" => 2,
        _ => 0,
    }
}

/// Shapes `text` the way canvas `fillText` and `measureText` see it.
pub fn shape(backend: &mut dyn TextBackend, font: &FontSpec, text: &str) -> Shaped {
    backend.shape(font, &normalize_space(text))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub x: i32,
    pub y: i32,
    pub mask: Arc<Mask>,
}

/// A float `SkMatrix` for a scale, skew and translate `[a, b, c, d, e, f]`.
type Matrix = [f32; 6];

fn pre_translate([a, b, c, d, e, f]: Matrix, x: f32, y: f32) -> Matrix {
    [a, b, c, d, a * x + c * y + e, b * x + d * y + f]
}

fn pre_scale_x([a, b, c, d, e, f]: Matrix, s: f32) -> Matrix {
    [a * s, b * s, c, d, e, f]
}

/// The glyph masks `fillText(text, x, y, maxWidth)` paints under the device matrix `ctm`, at
/// integer device positions: Blink's `DrawTextInternal` (float location, alignment, baseline,
/// `maxWidth` as a horizontal scale about the location) followed by Skia's glyph placement
/// (quarter-pixel x phase, rounded y when the matrix keeps axes, quarter phases on both axes
/// otherwise).
#[allow(clippy::too_many_arguments)]
pub fn place_text(
    backend: &mut dyn TextBackend,
    font: &FontSpec,
    align: &str,
    baseline: &str,
    text: &str,
    x: f64,
    y: f64,
    max_width: Option<f64>,
    ctm: [f64; 6],
) -> Vec<Placed> {
    if max_width.is_some_and(|m| !(m.is_finite() && m > 0.0)) {
        return Vec::new();
    }
    let shaped = shape(backend, font, text);
    let metrics = backend.metrics(font);
    let font_width = f64::from(shaped.width);
    let use_max_width = max_width.is_some_and(|m| m < font_width);
    let width = match max_width {
        Some(m) if use_max_width => m,
        _ => font_width,
    };
    let mut location = (
        x as f32,
        (y + f64::from(baseline_offset(&metrics, font.size, baseline))) as f32,
    );
    location.0 = match align_kind(align) {
        1 => (f64::from(location.0) - width / 2.0) as f32,
        2 => (f64::from(location.0) - width) as f32,
        _ => location.0,
    };
    let mut matrix = ctm.map(|v| v as f32);
    if use_max_width {
        matrix = pre_translate(matrix, location.0, location.1);
        let x_scale = if font_width > 0.0 {
            (width / font_width) as f32
        } else {
            0.0
        };
        matrix = pre_scale_x(matrix, x_scale);
        location = (0.0, 0.0);
    }
    let matrix = pre_translate(matrix, location.0, location.1);
    let [a, b, c, d, e, f] = matrix;
    let axis_aligned = b == 0.0 && c == 0.0 && a > 0.0 && d > 0.0;
    let mut placed = Vec::with_capacity(shaped.glyphs.len());
    for glyph in &shaped.glyphs {
        let device_x = a * glyph.x + e;
        let device_y = b * glyph.x + f;
        let (ix, iy, mask) = if axis_aligned {
            let (ix, iy, phase) = subpixel(device_x, device_y);
            let scale = f64::from(d);
            let mask = backend.raw_mask(glyph.glyph, scale, f64::from(a) / scale, phase);
            (ix, iy, mask)
        } else {
            let (ix, _, phase_x) = subpixel(device_x, 0.0);
            let (iy, _, phase_y) = subpixel(device_y, 0.0);
            let m = [a, b, c, d].map(f64::from);
            let mask = backend.raw_mask_transformed(glyph.glyph, m, (phase_x, phase_y));
            (ix, iy, mask)
        };
        if let Some(mask) = mask {
            placed.push(Placed { x: ix, y: iy, mask });
        }
    }
    placed
}

/// Skia's `SkGlyphPositionRoundingSpec` for four x samples: the integer pixel and the quarter
/// phase of `x + 1/8`, and `y` rounded.
fn subpixel(x: f32, y: f32) -> (i32, i32, u8) {
    let biased = x + 0.125;
    let ix = biased.floor();
    let phase = (((biased - ix) * 4.0).floor() as u8).min(3);
    (ix as i32, (y + 0.5).floor() as i32, phase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subpixel_matches_skia_rounding() {
        assert_eq!(subpixel(10.0, 4.49), (10, 4, 0));
        assert_eq!(subpixel(10.12, 4.5), (10, 5, 0));
        assert_eq!(subpixel(10.13, 0.0), (10, 0, 1));
        assert_eq!(subpixel(10.875, 0.0), (11, 0, 0));
        assert_eq!(subpixel(-0.3, -0.5), (-1, 0, 3));
    }

    #[test]
    fn normalizes_spaces() {
        assert_eq!(normalize_space("a\tb\nc\u{a0}d\re"), "a b c d\re");
        assert!(matches!(normalize_space("plain"), Cow::Borrowed(_)));
    }
}
