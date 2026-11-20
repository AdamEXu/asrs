pub mod coverage;
pub mod lut;
#[cfg(target_os = "macos")]
mod mac;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSpec {
    pub size: f64,
    pub weight: u16,
}

impl FontSpec {
    /// The size and numeric weight of a CSS `font` shorthand; family, style and line height are
    /// not resolved yet.
    pub fn parse(css: &str) -> FontSpec {
        let mut font = FontSpec {
            size: 10.0,
            weight: 400,
        };
        for token in css.split_whitespace() {
            if let Some(px) = token.strip_suffix("px") {
                if let Ok(size) = px.parse() {
                    font.size = size;
                }
                break;
            }
            match token {
                "bold" => font.weight = 700,
                "normal" => font.weight = 400,
                _ => {
                    if let Ok(weight) = token.parse() {
                        font.weight = weight;
                    }
                }
            }
        }
        font
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub id: u32,
    pub x: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shaped {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f64,
}

/// A glyph source that mirrors the scaler Skia uses in Chromium on one platform.
pub trait TextBackend {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped;

    /// OS/2 `sTypoAscender` and the negated `sTypoDescender`, in font units.
    fn typo_extents(&mut self, font: &FontSpec) -> (f64, f64);

    /// Raw coverage, before mask gamma, of `glyph` drawn at device scale `scale`, compressed
    /// horizontally by `x_scale`, with its origin `phase` quarter pixels right of the pixel grid.
    /// `left` and `top` are relative to that pixel.
    fn raw_mask(
        &mut self,
        font: &FontSpec,
        glyph: u32,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Mask>;
}

pub fn platform_backend() -> Option<Box<dyn TextBackend>> {
    #[cfg(target_os = "macos")]
    return Some(Box::new(mac::CoreText::new()));
    #[cfg(not(target_os = "macos"))]
    return None;
}

fn layout_round(v: f64) -> f64 {
    (v * 64.0).round() / 64.0
}

/// Blink's ascent and descent for canvas text: the typo extents normalized to the em size and
/// rounded to 1/64.
pub fn normalized_typo(backend: &mut dyn TextBackend, font: &FontSpec) -> (f64, f64) {
    let (asc, desc) = backend.typo_extents(font);
    let ascent = layout_round(asc * font.size / (asc + desc));
    (ascent, layout_round(font.size) - ascent)
}

pub fn baseline_offset(backend: &mut dyn TextBackend, font: &FontSpec, baseline: &str) -> f64 {
    let (ascent, descent) = normalized_typo(backend, font);
    match baseline {
        "top" => ascent,
        "bottom" => -descent,
        "middle" => (ascent - descent) / 2.0,
        _ => 0.0,
    }
}

/// Skia's glyph positioning: x snapped to a quarter device pixel, y rounded.
pub fn glyph_origin(device_x: f64, device_y: f64) -> (i32, i32, u8) {
    let quarter = (device_x * 4.0 + 0.5).floor();
    let x = (quarter / 4.0).floor();
    let phase = (quarter - x * 4.0) as u8;
    (x as i32, (device_y + 0.5).floor() as i32, phase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upstream_fonts() {
        let system = "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
        let cases = [
            (format!("12px {system}"), 12.0, 400),
            (format!("200 14px {system}"), 14.0, 200),
            ("bold 12px sans-serif".to_owned(), 12.0, 700),
            ("italic 16px sans-serif".to_owned(), 16.0, 400),
            ("13.5px sans-serif".to_owned(), 13.5, 400),
        ];
        for (css, size, weight) in cases {
            assert_eq!(FontSpec::parse(&css), FontSpec { size, weight }, "{css}");
        }
    }

    #[test]
    fn glyph_origin_quantizes_to_quarter_pixels() {
        assert_eq!(glyph_origin(10.0, 4.49), (10, 4, 0));
        assert_eq!(glyph_origin(10.12, 4.5), (10, 5, 0));
        assert_eq!(glyph_origin(10.13, 0.0), (10, 0, 1));
        assert_eq!(glyph_origin(10.875, 0.0), (11, 0, 0));
        assert_eq!(glyph_origin(-0.3, -0.5), (-1, 0, 3));
    }
}
