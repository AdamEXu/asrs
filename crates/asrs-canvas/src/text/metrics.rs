use std::sync::Arc;

use super::layout::shape;
use super::{
    Bounds, FontMetrics, FontSpec, Glyph, Mask, Shaped, ShapedGlyph, TextBackend, baseline_offset,
};
use crate::display::{Call, Property};

/// The numeric fields of a canvas `TextMetrics`. `emHeightAscent` and `emHeightDescent` are
/// `undefined` in Chromium and are left out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMetrics {
    pub width: f64,
    pub actual_bounding_box_left: f64,
    pub actual_bounding_box_right: f64,
    pub actual_bounding_box_ascent: f64,
    pub actual_bounding_box_descent: f64,
    pub font_bounding_box_ascent: f64,
    pub font_bounding_box_descent: f64,
    pub hanging_baseline: f64,
    pub alphabetic_baseline: f64,
    pub ideographic_baseline: f64,
}

/// `gfx::RectF` as `[x, y, right, bottom]`, with its rule that empty rects never grow a union.
#[derive(Clone, Copy, Default)]
struct RectF([f32; 4]);

impl RectF {
    fn is_empty(&self) -> bool {
        self.0[2] - self.0[0] <= 0.0 || self.0[3] - self.0[1] <= 0.0
    }

    fn union(&mut self, other: RectF) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            *self = other;
            return;
        }
        let [x, y, r, b] = &mut self.0;
        *x = x.min(other.0[0]);
        *y = y.min(other.0[1]);
        *r = r.max(other.0[2]);
        *b = b.max(other.0[3]);
    }
}

/// Blink's `TextMetrics::Update` for left-to-right text in one font.
pub fn measure_text(
    backend: &mut dyn TextBackend,
    font: &FontSpec,
    align: &str,
    baseline: &str,
    text: &str,
) -> TextMetrics {
    let shaped = shape(backend, font, text);
    let mut ink = RectF::default();
    for glyph in &shaped.glyphs {
        if let Some(b) = backend.glyph_bounds(glyph.glyph) {
            ink.union(RectF([
                b.left + glyph.x,
                b.top,
                b.right + glyph.x,
                b.bottom,
            ]));
        }
    }
    let width = shaped.width;
    let anchor = match align {
        "center" => width / 2.0,
        "right" | "end" => width,
        _ => 0.0,
    };
    let metrics = backend.metrics(font);
    let baseline_y = baseline_offset(&metrics, font.size, baseline);
    let (ascent, descent) = (metrics.ascent, metrics.descent);
    let [x, y, right, bottom] = ink.0;
    TextMetrics {
        width: f64::from(width),
        actual_bounding_box_left: f64::from(-x + anchor),
        actual_bounding_box_right: f64::from(right - anchor),
        actual_bounding_box_ascent: f64::from(-y - baseline_y),
        actual_bounding_box_descent: f64::from(bottom + baseline_y),
        font_bounding_box_ascent: f64::from(ascent - baseline_y),
        font_bounding_box_descent: f64::from(descent + baseline_y),
        hanging_baseline: f64::from(ascent * 80.0 / 100.0 - baseline_y),
        alphabetic_baseline: f64::from(-baseline_y),
        ideographic_baseline: f64::from(-descent - baseline_y),
    }
}

/// The `font`, `textAlign` and `textBaseline` in effect after `calls`, following `save` and
/// `restore` and ignoring values a canvas rejects.
pub fn text_state(calls: &[Call]) -> (FontSpec, String, String) {
    let mut state = (
        FontSpec::default(),
        "start".to_owned(),
        "alphabetic".to_owned(),
    );
    let mut stack = Vec::new();
    for call in calls {
        match call {
            Call::Save => stack.push(state.clone()),
            Call::Restore => {
                if let Some(saved) = stack.pop() {
                    state = saved;
                }
            }
            Call::Set(Property::Font(css)) => {
                if let Some(font) = FontSpec::parse(css) {
                    state.0 = font;
                }
            }
            Call::Set(Property::TextAlign(align)) => {
                if ["start", "end", "left", "right", "center"].contains(&align.as_str()) {
                    state.1.clone_from(align);
                }
            }
            Call::Set(Property::TextBaseline(baseline)) => {
                let known = [
                    "top",
                    "hanging",
                    "middle",
                    "alphabetic",
                    "ideographic",
                    "bottom",
                ];
                if known.contains(&baseline.as_str()) {
                    state.2.clone_from(baseline);
                }
            }
            _ => {}
        }
    }
    state
}

/// A deterministic stand-in for a platform font, shared with the oracle's recording canvas so
/// painters that lay out by `measureText` produce identical traces on both sides. Advances per
/// UTF-16 code unit, in em: 0.25 for space, `.`, `,` and `:`, 0.55 for ASCII digits, 0.6 for
/// anything else. Every non-space code unit inks `[0, -0.7em, advance, 0.2em]`. Ascent is
/// `round(0.8em)`, descent `round(0.2em)`, with no typo metrics. It draws nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedAdvance;

impl FixedAdvance {
    fn advance(unit: u16) -> f32 {
        match unit {
            0x20 | 0x2c | 0x2e | 0x3a => 0.25,
            0x30..=0x39 => 0.55,
            _ => 0.6,
        }
    }
}

impl TextBackend for FixedAdvance {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped {
        let mut glyphs = Vec::new();
        let mut x = 0.0f32;
        for (i, unit) in text.encode_utf16().enumerate() {
            glyphs.push(ShapedGlyph {
                glyph: Glyph {
                    face: font.size.to_bits(),
                    id: u32::from(unit),
                },
                x,
                cluster: i as u32,
            });
            x += Self::advance(unit) * font.size;
        }
        Shaped { glyphs, width: x }
    }

    fn metrics(&mut self, font: &FontSpec) -> FontMetrics {
        FontMetrics {
            ascent: (font.size * 0.8 + 0.5).floor(),
            descent: (font.size * 0.2 + 0.5).floor(),
            line_gap: 0.0,
            typo: None,
        }
    }

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds> {
        let unit = glyph.id as u16;
        if unit == 0x20 {
            return None;
        }
        let size = f32::from_bits(glyph.face);
        Some(Bounds {
            left: 0.0,
            top: -0.7 * size,
            right: Self::advance(unit) * size,
            bottom: 0.2 * size,
        })
    }

    fn raw_mask(&mut self, _: Glyph, _: f64, _: f64, _: u8) -> Option<Arc<Mask>> {
        None
    }

    fn raw_mask_transformed(&mut self, _: Glyph, _: [f64; 4], _: (u8, u8)) -> Option<Arc<Mask>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_advance_metrics() {
        let font = FontSpec::parse("10px sans-serif").unwrap();
        let m = measure_text(&mut FixedAdvance, &font, "center", "alphabetic", "a 1");
        assert_eq!(m.width, 14.0);
        assert_eq!(m.actual_bounding_box_left, 7.0);
        assert_eq!(m.actual_bounding_box_right, 7.0);
        assert_eq!(m.actual_bounding_box_ascent, 7.0);
        assert_eq!(m.actual_bounding_box_descent, 2.0);
        assert_eq!(
            (m.font_bounding_box_ascent, m.font_bounding_box_descent),
            (8.0, 2.0)
        );
        assert_eq!(m.alphabetic_baseline.to_bits(), (-0.0f64).to_bits());
        let top = measure_text(&mut FixedAdvance, &font, "left", "top", "");
        assert_eq!(top.width, 0.0);
        assert_eq!(top.actual_bounding_box_ascent, -8.0);
        assert_eq!(top.ideographic_baseline, -10.0);
    }

    #[test]
    fn state_follows_save_and_restore() {
        let mut canvas = crate::Canvas2d::new(10, 10);
        canvas.set_font("bold 12px sans-serif");
        canvas.save();
        canvas.set_font("14px serif");
        canvas.set_text_align("center");
        canvas.set_text_baseline("nonsense");
        canvas.set_font("not a font");
        let (font, align, baseline) = text_state(&canvas.calls);
        assert_eq!(
            (font.size, align.as_str(), baseline.as_str()),
            (14.0, "center", "alphabetic")
        );
        canvas.restore();
        let (font, align, _) = text_state(&canvas.calls);
        assert_eq!(
            (font.size, font.weight, align.as_str()),
            (12.0, 700.0, "start")
        );
        let width = canvas.measure_text(&mut FixedAdvance, "ab").width;
        assert_eq!(width, f64::from(0.6f32 * 12.0 + 0.6 * 12.0));
    }
}
