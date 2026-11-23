//! Boxes Blink centres with `left: 50%; top: 50%; transform: translate(-50%, -50%)`, and the
//! decorations the compositor draws for them at the fractional offset the transform leaves.

use asrs_canvas::blit::Spans;
use asrs_canvas::hairline::P;
use asrs_canvas::skpath::SkPath;
use asrs_canvas::{Pixmap, aaa, scan};
use asrs_js::math;

use super::lu;

/// A centred box in device px. Blink rounds its paint offset to whole pixels, snaps its size
/// there, and applies the translate, which halves the unsnapped size, exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CentredBox {
    /// Where the border box's snapped corner lands.
    pub origin: [f64; 2],
    /// The border box's size after pixel snapping.
    pub size: [f64; 2],
    /// The border box's size as laid out.
    pub layout: [f64; 2],
    /// The paint offset's fraction, which the box's children keep.
    pub fraction: [f64; 2],
}

impl CentredBox {
    /// The box of border-box size `layout` (device px) in `container` (CSS px).
    pub fn new(container: [f64; 4], layout: [f64; 2], scale: f64) -> CentredBox {
        let mut out = CentredBox {
            origin: [0.0; 2],
            size: [0.0; 2],
            layout,
            fraction: [0.0; 2],
        };
        for axis in 0..2 {
            let offset = lu(container[axis] * scale) + lu(container[axis + 2] * scale * 0.5);
            let rounded = math::floor(offset + 0.5);
            let fraction = offset - rounded;
            let start = math::floor(fraction + 0.5);
            out.origin[axis] = rounded + start - layout[axis] / 2.0;
            out.size[axis] = math::floor(fraction + layout[axis] + 0.5) - start;
            out.fraction[axis] = fraction;
        }
        out
    }

    /// `getBoundingClientRect()` in CSS px.
    pub fn client_rect(&self, scale: f64) -> [f64; 4] {
        let at = |axis: usize| (self.origin[axis] + self.fraction[axis]) / scale;
        [at(0), at(1), self.layout[0] / scale, self.layout[1] / scale]
    }

    /// `[left, top, right, bottom]` in device px.
    pub fn edges(&self) -> [f64; 4] {
        [
            self.origin[0],
            self.origin[1],
            self.origin[0] + self.size[0],
            self.origin[1] + self.size[1],
        ]
    }
}

/// A pixmap over whole device pixels of the window.
pub struct Layer {
    pub pixmap: Pixmap,
    pub x: i32,
    pub y: i32,
}

impl Layer {
    /// A transparent layer enclosing `edges` outset by `outset` device px.
    pub fn enclosing(edges: [f64; 4], outset: f64) -> Option<Layer> {
        let x = math::floor(edges[0] - outset);
        let y = math::floor(edges[1] - outset);
        let width = math::ceil(edges[2] + outset) - x;
        let height = math::ceil(edges[3] + outset) - y;
        let side = 0.0..=16384.0;
        if !(side.contains(&width) && side.contains(&height)) {
            return None;
        }
        Some(Layer {
            pixmap: Pixmap::new(width as u32, height as u32)?,
            x: x as i32,
            y: y as i32,
        })
    }

    pub fn local(&self, edges: [f64; 4]) -> [f32; 4] {
        let (x, y) = (f64::from(self.x), f64::from(self.y));
        [edges[0] - x, edges[1] - y, edges[2] - x, edges[3] - y].map(|v| v as f32)
    }

    pub fn fill_rect(&mut self, edges: [f64; 4], color: [u8; 4]) {
        let r = self.local(edges);
        let spans = scan::anti_fill_rect(r, self.pixmap.width(), self.pixmap.height());
        scan::blit_spans_solid(&mut self.pixmap, &spans, color);
    }

    /// A solid border `width` device px wide inside `edges`.
    pub fn border(&mut self, edges: [f64; 4], width: f64, color: [u8; 4]) {
        let [l, t, r, b] = self.local(edges);
        let w = width as f32;
        let mut path = SkPath::new();
        for [l, t, r, b] in [[l, t, r, b], [l + w, t + w, r - w, b - w]] {
            path.move_to(P { x: l, y: t });
            path.line_to(P { x: r, y: t });
            path.line_to(P { x: r, y: b });
            path.line_to(P { x: l, y: b });
            path.close();
        }
        let (pw, ph) = (self.pixmap.width(), self.pixmap.height());
        let mut spans = Spans::new(pw, ph);
        aaa::fill_path(&path, pw, ph, true, &mut spans);
        scan::blit_spans_solid(&mut self.pixmap, &spans, color);
    }

    /// Source-over of `top` with its corner at whole device px `(x, y)`.
    pub fn over(&mut self, top: &Pixmap, x: i32, y: i32) {
        let (dx, dy) = (x - self.x, y - self.y);
        let (width, height) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let (tw, th) = (top.width() as i32, top.height() as i32);
        let columns = (-dx).max(0)..tw.min(width - dx);
        if columns.is_empty() {
            return;
        }
        let source = top.data();
        let data = self.pixmap.data_mut();
        for row in (-dy).max(0)..th.min(height - dy) {
            let from = ((row * tw + columns.start) * 4) as usize;
            let to = (((row + dy) * width + columns.start + dx) * 4) as usize;
            let len = columns.len() * 4;
            let pixels = data[to..to + len].as_chunks_mut::<4>().0;
            for (d, s) in pixels
                .iter_mut()
                .zip(source[from..from + len].as_chunks::<4>().0)
            {
                let inverse = 255 - u32::from(s[3]);
                for (d, &s) in d.iter_mut().zip(s) {
                    *d = (u32::from(s) + (u32::from(*d) * inverse + 127) / 255) as u8;
                }
            }
        }
    }
}

pub fn opaque(rgb: u32) -> [u8; 4] {
    let [_, r, g, b] = rgb.to_be_bytes();
    [r, g, b, 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_match_blink() {
        let swerve = CentredBox::new([300.0, 120.0, 800.0, 330.0], [332.0, 332.0], 1.0);
        assert_eq!(swerve.client_rect(1.0), [534.0, 119.0, 332.0, 332.0]);
        let points = CentredBox::new([300.0, 120.0, 800.0, 330.0], [443.0, 333.0], 1.0);
        assert_eq!(points.client_rect(1.0), [478.5, 118.5, 443.0, 333.0]);
        assert_eq!(points.edges(), [478.5, 118.5, 921.5, 451.5]);
        let mechanism = CentredBox::new([300.0, 120.0, 800.0, 330.0], [990.0, 660.0], 2.0);
        assert_eq!(mechanism.client_rect(2.0), [452.5, 120.0, 495.0, 330.0]);
        assert_eq!(mechanism.edges(), [905.0, 240.0, 1895.0, 900.0]);
    }

    #[test]
    fn half_pixel_edges_take_half_coverage() {
        let mut layer = Layer::enclosing([0.5, 0.0, 3.5, 2.0], 0.0).expect("layer");
        layer.fill_rect([0.5, 0.0, 3.5, 2.0], [255, 255, 255, 255]);
        let row: Vec<u8> = layer.pixmap.data()[..16]
            .iter()
            .skip(3)
            .step_by(4)
            .copied()
            .collect();
        assert!(
            (126..=129).contains(&row[0]) && (126..=129).contains(&row[3]),
            "{row:?}"
        );
        assert_eq!(row[1..3], [255, 255]);
    }
}
