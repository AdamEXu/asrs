// Ported from Skia and Blink, Chromium m142 (BSD-3-Clause, Copyright Google Inc. and The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Gradient fills as Chromium's CPU canvas shades them: Blink's stop padding, Skia's gradient
//! colour stages and device-to-unit matrices, then the raster pipeline's highp tail (premul,
//! paint alpha, the ordered dither Blink turns on for every gradient, the float blend,
//! round-to-nearest-even store). Arithmetic follows Skia on ARM64, where the pipeline's `mad` is a fused multiply-add.

use tiny_skia::Pixmap;

use crate::blit::Spans;
use crate::color::Rgba;
use crate::matrix::Matrix;

const NEARLY_ZERO: f32 = 1.0 / 4096.0;
const DEGENERATE: f32 = 1.0 / 32768.0;

type Channels = [f32; 4];

enum Lookup {
    TwoStop {
        factor: Channels,
        bias: Channels,
    },
    Even {
        factors: Vec<Channels>,
        biases: Vec<Channels>,
    },
    Stops {
        ts: Vec<f32>,
        factors: Vec<Channels>,
        biases: Vec<Channels>,
    },
}

/// `SkGradientBaseShader` built from Blink's `Gradient::FillSkiaStops` output, as the colour
/// half of the pipeline: `clamp_x_1` for evenly spaced stops, the gradient lookup, `premul`.
pub struct Colors {
    lookup: Lookup,
    clamp: bool,
    opaque: bool,
}

fn channels(c: Rgba) -> Channels {
    [c.r, c.g, c.b, c.a]
}

fn zip(a: Channels, b: Channels, f: impl Fn(f32, f32) -> f32) -> Channels {
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3])]
}

impl Colors {
    /// `stops` sorted by offset, offsets in `[0, 1]`, at least one.
    pub fn new(stops: &[(f32, Rgba)]) -> Colors {
        let mut pos = Vec::with_capacity(stops.len() + 2);
        let mut colors = Vec::with_capacity(stops.len() + 2);
        if stops[0].0 > 0.0 {
            pos.push(0.0);
            colors.push(channels(stops[0].1));
        }
        for &(offset, color) in stops {
            pos.push(offset);
            colors.push(channels(color));
        }
        if pos[pos.len() - 1] < 1.0 {
            pos.push(1.0);
            colors.push(colors[colors.len() - 1]);
        }

        let n = pos.len();
        let mut positions = vec![0.0f32; n];
        let mut prev = 0.0f32;
        let step = pos[1] - prev;
        let mut uniform = true;
        for i in 1..n {
            let curr = pos[i].clamp(prev, 1.0);
            uniform &= (step - (curr - prev)).abs() <= NEARLY_ZERO;
            positions[i] = curr;
            prev = curr;
        }
        let opaque = colors.iter().all(|c| c[3] == 1.0);

        let lookup = if uniform && n == 2 {
            Lookup::TwoStop {
                factor: zip(colors[1], colors[0], |r, l| r - l),
                bias: colors[0],
            }
        } else if uniform {
            let gaps = (n - 1) as f32;
            let mut factors = Vec::with_capacity(n);
            let mut biases = Vec::with_capacity(n);
            for i in 0..n - 1 {
                let factor = zip(colors[i + 1], colors[i], |r, l| (r - l) * gaps);
                let at = i as f32 / gaps;
                biases.push(zip(colors[i], factor, |l, f| l - f * at));
                factors.push(factor);
            }
            factors.push([0.0; 4]);
            biases.push(colors[n - 1]);
            Lookup::Even { factors, biases }
        } else {
            let (first, last) = if n > 2 {
                (
                    usize::from(colors[0] == colors[1]),
                    if colors[n - 2] != colors[n - 1] {
                        n - 1
                    } else {
                        n - 2
                    },
                )
            } else {
                (0, 1)
            };
            let mut ts = vec![0.0f32];
            let mut factors = vec![[0.0; 4]];
            let mut biases = vec![colors[first]];
            let (mut t_l, mut c_l) = (positions[first], colors[first]);
            for i in first..last {
                let (t_r, c_r) = (positions[i + 1], colors[i + 1]);
                if t_l < t_r {
                    let scale = 1.0 / (t_r - t_l);
                    if scale.is_finite() {
                        let factor = zip(c_r, c_l, |r, l| (r - l) * scale);
                        biases.push(zip(c_l, factor, |l, f| l - f * t_l));
                        factors.push(factor);
                        ts.push(t_l);
                    }
                }
                t_l = t_r;
                c_l = c_r;
            }
            ts.push(t_l);
            factors.push([0.0; 4]);
            biases.push(c_l);
            Lookup::Stops {
                ts,
                factors,
                biases,
            }
        };
        Colors {
            lookup,
            clamp: uniform,
            opaque,
        }
    }

    /// The premultiplied colour at gradient parameter `t`.
    pub fn at(&self, t: f32) -> Channels {
        let t = if self.clamp { t.clamp(0.0, 1.0) } else { t };
        let mad = |f: &Channels, b: &Channels| zip(*f, *b, |f, b| t.mul_add(f, b));
        let mut c = match &self.lookup {
            Lookup::TwoStop { factor, bias } => mad(factor, bias),
            Lookup::Even { factors, biases } => {
                let i = (t * (factors.len() - 1) as f32) as usize;
                mad(&factors[i], &biases[i])
            }
            Lookup::Stops {
                ts,
                factors,
                biases,
            } => {
                let i = ts[1..].iter().filter(|&&s| t >= s).count();
                mad(&factors[i], &biases[i])
            }
        };
        if !self.opaque {
            let a = c[3];
            for v in &mut c[..3] {
                *v *= a;
            }
        }
        c
    }
}

/// Where a device pixel centre lands in gradient space: the pipeline's matrix stage and the
/// shader's own `t` stages.
pub enum Geometry {
    Linear(Matrix),
    Radial(Matrix),
    Concentric {
        matrix: Matrix,
        scale: f32,
        bias: f32,
    },
    Strip {
        matrix: Matrix,
        radius2: f32,
    },
    Focal {
        matrix: Matrix,
        focal: Focal,
    },
}

fn length(x: f32, y: f32) -> f32 {
    let mag2 = x * x + y * y;
    if mag2.is_finite() {
        mag2.sqrt()
    } else {
        (f64::from(x) * f64::from(x) + f64::from(y) * f64::from(y)).sqrt() as f32
    }
}

fn pts_to_unit(p0: (f32, f32), p1: (f32, f32)) -> Matrix {
    let (vx, vy) = (p1.0 - p0.0, p1.1 - p0.1);
    let mag = length(vx, vy);
    let inv = if mag != 0.0 { 1.0 / mag } else { 0.0 };
    let (sin, cos) = (-(vy * inv), vx * inv);
    let one_minus_cos = 1.0 - cos;
    let m = Matrix {
        sx: cos,
        kx: -sin,
        tx: sin * p0.1 + one_minus_cos * p0.0,
        ky: sin,
        sy: cos,
        ty: -sin * p0.0 + one_minus_cos * p0.1,
    };
    m.post_translate(-p0.0, -p0.1).post_scale(inv, inv)
}

fn radius_to_unit(center: (f32, f32), radius: f32) -> Matrix {
    let inv = 1.0 / radius;
    Matrix::scale_translate(1.0, 1.0, -center.0, -center.1).post_scale(inv, inv)
}

fn center_distance([x0, y0, _, x1, y1, _]: [f32; 6]) -> f32 {
    length(x0 - x1, y0 - y1)
}

/// `SkMatrix::Poly2Proc`.
fn poly2(p0: (f32, f32), p1: (f32, f32)) -> Matrix {
    Matrix {
        sx: p1.1 - p0.1,
        ky: p0.0 - p1.0,
        kx: p1.0 - p0.0,
        sy: p1.1 - p0.1,
        tx: p0.0,
        ty: p0.1,
    }
}

/// `SkMatrix::PolyToPoly` of two points.
fn poly_to_poly(from: [(f32, f32); 2], to: [(f32, f32); 2]) -> Option<Matrix> {
    let inverse = poly2(from[0], from[1]).invert()?;
    Some(poly2(to[0], to[1]).concat(&inverse))
}

/// `SkConicalGradient::FocalData`.
pub struct Focal {
    r1: f32,
    x: f32,
    swapped: bool,
}

impl Focal {
    fn new(r0: f32, r1: f32, matrix: &mut Matrix) -> Option<Focal> {
        let (mut r0, mut r1) = (r0, r1);
        let mut x = r0 / (r0 - r1);
        let mut swapped = false;
        if (x - 1.0).abs() <= NEARLY_ZERO {
            *matrix = matrix.post_translate(-1.0, 0.0).post_scale(-1.0, 1.0);
            std::mem::swap(&mut r0, &mut r1);
            x = 0.0;
            swapped = true;
        }
        let to_focal = poly_to_poly([(x, 0.0), (1.0, 0.0)], [(0.0, 0.0), (1.0, 0.0)])?;
        *matrix = to_focal.concat(matrix);
        let scale = (1.0 - x).abs();
        let focal = Focal {
            r1: r1 / scale,
            x,
            swapped,
        };
        *matrix = if focal.on_circle() {
            matrix.post_scale(0.5, 0.5)
        } else {
            let d = focal.r1 * focal.r1 - 1.0;
            matrix.post_scale(focal.r1 / d, 1.0 / d.abs().sqrt())
        };
        *matrix = matrix.post_scale(scale, scale);
        Some(focal)
    }

    fn on_circle(&self) -> bool {
        (1.0 - self.r1).abs() <= NEARLY_ZERO
    }

    fn well_behaved(&self) -> bool {
        !self.on_circle() && self.r1 > 1.0
    }

    /// The `2pt_conical` stages, `None` outside the cone.
    fn t(&self, x: f32, y: f32) -> Option<f32> {
        let p0 = 1.0 / self.r1;
        let mut t = if self.on_circle() {
            x + y * y / x
        } else if self.well_behaved() {
            (x * x + y * y).sqrt() - x * p0
        } else if self.swapped || 1.0 - self.x < 0.0 {
            -(x * x - y * y).sqrt() - x * p0
        } else {
            (x * x - y * y).sqrt() - x * p0
        };
        if !self.well_behaved() && (t <= 0.0 || t.is_nan()) {
            return None;
        }
        if 1.0 - self.x < 0.0 {
            t = -t;
        }
        if self.x.abs() > NEARLY_ZERO {
            t += self.x;
        }
        Some(if self.swapped { 1.0 - t } else { t })
    }
}

/// `SkRasterPipeline::appendMatrix` and the stage it picks.
fn apply(m: &Matrix, x: f32, y: f32) -> (f32, f32) {
    if m.is_identity() {
        (x, y)
    } else if m.is_translate() {
        (x + m.tx, y + m.ty)
    } else if m.is_scale_translate() {
        (x.mul_add(m.sx, m.tx), y.mul_add(m.sy, m.ty))
    } else {
        (
            x.mul_add(m.sx, y.mul_add(m.kx, m.tx)),
            x.mul_add(m.ky, y.mul_add(m.sy, m.ty)),
        )
    }
}

impl Geometry {
    /// The linear gradient from `p0` to `p1` under `ctm`, or `None` when the CTM is singular.
    pub fn linear(ctm: &Matrix, p0: (f32, f32), p1: (f32, f32)) -> Option<Geometry> {
        Some(Geometry::Linear(pts_to_unit(p0, p1).concat(&ctm.invert()?)))
    }

    /// `SkGradientShader::MakeTwoPointConical`. `None` when Skia draws nothing.
    pub fn radial(ctm: &Matrix, g: [f32; 6]) -> Option<Geometry> {
        let [x0, y0, r0, x1, y1, r1] = g;
        let inverse = ctm.invert()?;
        let distance = center_distance(g);
        if distance <= DEGENERATE && r0 <= DEGENERATE && (r0 - r1).abs() > DEGENERATE {
            return Some(Geometry::Radial(
                radius_to_unit((x0, y0), r1).concat(&inverse),
            ));
        }
        if distance <= NEARLY_ZERO {
            if r0.max(r1) <= NEARLY_ZERO || (r0 - r1).abs() <= NEARLY_ZERO {
                return None;
            }
            let d = r1 - r0;
            return Some(Geometry::Concentric {
                matrix: radius_to_unit((x1, y1), r0.max(r1)).concat(&inverse),
                scale: r0.max(r1) / d,
                bias: -r0 / d,
            });
        }
        let mut to_unit = poly_to_poly([(x0, y0), (x1, y1)], [(0.0, 0.0), (1.0, 0.0)])?;
        if (r1 - r0).abs() <= NEARLY_ZERO {
            let r = r0 / distance;
            return Some(Geometry::Strip {
                matrix: to_unit.concat(&inverse),
                radius2: r * r,
            });
        }
        let focal = Focal::new(r0 / distance, r1 / distance, &mut to_unit)?;
        Some(Geometry::Focal {
            matrix: to_unit.concat(&inverse),
            focal,
        })
    }

    /// Whether the shader covers the plane: Skia takes every two-point conical for one that may
    /// not.
    fn covers(&self) -> bool {
        matches!(self, Geometry::Linear(_) | Geometry::Radial(_))
    }

    fn t(&self, x: f32, y: f32) -> Option<f32> {
        Some(match self {
            Geometry::Linear(m) => apply(m, x, y).0,
            Geometry::Radial(m) => {
                let (x, y) = apply(m, x, y);
                (x * x + y * y).sqrt()
            }
            Geometry::Concentric {
                matrix,
                scale,
                bias,
            } => {
                let (x, y) = apply(matrix, x, y);
                let r = (x * x + y * y).sqrt();
                apply(&Matrix::scale_translate(*scale, 1.0, *bias, 0.0), r, y).0
            }
            Geometry::Strip { matrix, radius2 } => {
                let (x, y) = apply(matrix, x, y);
                let t = x + (radius2 - y * y).sqrt();
                return (!t.is_nan()).then_some(t);
            }
            Geometry::Focal { matrix, focal } => {
                let (x, y) = apply(matrix, x, y);
                return focal.t(x, y);
            }
        })
    }
}

/// The highp `dither` stage's offset for device pixel `(x, y)`, in `(-0.5, 0.5)`.
fn dither(x: u32, y: u32) -> f32 {
    let y = y ^ x;
    let m = (y & 1) << 5 | (x & 1) << 4 | (y & 2) << 2 | (x & 2) << 1 | (y & 4) >> 1 | (x & 4) >> 2;
    (m as f32).mul_add(2.0 / 128.0, -63.0 / 128.0)
}

/// A gradient as a paint: the colour half of a `SkRasterPipelineBlitter`.
pub struct Paint {
    geometry: Geometry,
    colors: Colors,
    alpha: f32,
}

impl Paint {
    pub fn new(geometry: Geometry, colors: Colors, alpha: f32) -> Paint {
        Paint {
            geometry,
            colors,
            alpha,
        }
    }

    /// Skia strength-reduces source-over with an opaque shader to a copy, which lerps where it
    /// would scale.
    fn opaque(&self) -> bool {
        self.colors.opaque && self.alpha == 1.0 && self.geometry.covers()
    }

    /// The premultiplied, dithered colour of a device pixel: the shader, the paint alpha
    /// (`scale_1_float`), `dither` at rate 1/255 clamped to `[0, a]`, then `clamp_01`.
    fn at(&self, x: u32, y: u32) -> Channels {
        let c = match self.geometry.t(x as f32 + 0.5, y as f32 + 0.5) {
            Some(t) => self.colors.at(t),
            None => [0.0; 4],
        };
        let c = if self.alpha != 1.0 {
            c.map(|v| v * self.alpha)
        } else {
            c
        };
        let d = dither(x, y);
        let a = c[3];
        let rgb = |v: f32| d.mul_add(1.0 / 255.0, v).min(a).max(0.0);
        [rgb(c[0]), rgb(c[1]), rgb(c[2]), a].map(|v| v.max(0.0).min(1.0))
    }
}

/// Blends a gradient through `spans` in blit order as `SkRasterPipelineBlitter` does: in float,
/// from the 8-bit destination and back, with the coverage in float too.
pub fn blit(pixmap: &mut Pixmap, spans: &Spans, paint: &Paint) {
    let opaque = paint.opaque();
    let stride = pixmap.width() as usize;
    let data = pixmap.data_mut();
    for (x0, y, coverage) in spans.iter() {
        let row = y as usize * stride;
        for (i, &c) in coverage.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let x = x0 as usize + i;
            let k = (row + x) * 4;
            let dst = &mut data[k..k + 4];
            let mut src = paint.at(x as u32, y as u32);
            let scale = f32::from(c) * (1.0 / 255.0);
            if opaque {
                if c != 255 {
                    for (s, &d) in src.iter_mut().zip(dst.iter()) {
                        let d = f32::from(d) * (1.0 / 255.0);
                        *s = (*s - d).mul_add(scale, d);
                    }
                }
            } else {
                if c != 255 {
                    src = src.map(|v| v * scale);
                }
                let inv = 1.0 - src[3];
                for (s, &d) in src.iter_mut().zip(dst.iter()) {
                    let d = f32::from(d) * (1.0 / 255.0);
                    *s = d.mul_add(inv, *s);
                }
            }
            for (d, s) in dst.iter_mut().zip(src) {
                *d = (s * 255.0).max(0.0).min(255.0).round_ties_even() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dither_matrix_matches_skia() {
        let m = |x, y| (dither(x, y) + 63.0 / 128.0) * 64.0;
        assert_eq!(m(0, 0), 0.0);
        assert_eq!(m(1, 0), 48.0);
        assert_eq!(m(0, 1), 32.0);
        assert_eq!(m(1, 1), 16.0);
        assert_eq!(m(7, 7), 21.0);
    }

    #[test]
    fn heatmap_palette_stops_drop_the_padded_duplicate() {
        let blue = Rgba::from_u8(0, 0, 255, 1.0);
        let red = Rgba::from_u8(255, 0, 0, 1.0);
        let colors = Colors::new(&[(0.25, blue), (1.0, red)]);
        assert_eq!(colors.at(0.0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(colors.at(1.0), [1.0, 0.0, 0.0, 1.0]);
    }
}
