use super::layout::place_text;
use super::{FontSpec, TextBackend};

#[derive(Clone, Debug, PartialEq)]
pub enum TextOp {
    Scale(f64, f64),
    Font(String),
    TextAlign(String),
    TextBaseline(String),
    FillText {
        text: String,
        x: f64,
        y: f64,
        max_width: Option<f64>,
    },
}

/// Raw glyph coverage of a text-only display list, composited source-over into an A8 plane
/// the size of the canvas. This is what mask-gamma calibration pairs with Chromium's alpha.
pub fn render_coverage(
    width: u32,
    height: u32,
    ops: &[TextOp],
    backend: &mut dyn TextBackend,
) -> Vec<u8> {
    let mut plane = vec![0u8; width as usize * height as usize];
    let (mut sx, mut sy) = (1.0, 1.0);
    let mut font = FontSpec::default();
    let mut align = "start".to_owned();
    let mut baseline = "alphabetic".to_owned();
    for op in ops {
        match op {
            TextOp::Scale(x, y) => {
                sx *= x;
                sy *= y;
            }
            TextOp::Font(css) => {
                if let Some(f) = FontSpec::parse(css) {
                    font = f;
                }
            }
            TextOp::TextAlign(a) => align.clone_from(a),
            TextOp::TextBaseline(b) => baseline.clone_from(b),
            TextOp::FillText {
                text,
                x,
                y,
                max_width,
            } => {
                let ctm = [sx, 0.0, 0.0, sy, 0.0, 0.0];
                for glyph in place_text(
                    backend, &font, &align, &baseline, text, *x, *y, *max_width, ctm,
                ) {
                    blend(&mut plane, width, height, &glyph.mask, glyph.x, glyph.y);
                }
            }
        }
    }
    plane
}

fn blend(plane: &mut [u8], width: u32, height: u32, mask: &super::Mask, ix: i32, iy: i32) {
    for row in 0..mask.height as i32 {
        let py = iy + mask.top + row;
        if py < 0 || py >= height as i32 {
            continue;
        }
        for col in 0..mask.width as i32 {
            let px = ix + mask.left + col;
            if px < 0 || px >= width as i32 {
                continue;
            }
            let cov = f64::from(mask.data[(row * mask.width as i32 + col) as usize]);
            if cov == 0.0 {
                continue;
            }
            let i = py as usize * width as usize + px as usize;
            let d = f64::from(plane[i]);
            plane[i] = (cov + d * (1.0 - cov / 255.0)).round() as u8;
        }
    }
}
