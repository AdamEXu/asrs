//! Gradient fills as Chromium's CPU canvas shades them: Blink's stop padding, Skia's gradient
//! colour stages and device-to-unit matrices, then the raster pipeline's highp tail (premul,
//! paint alpha, the ordered dither Blink turns on for every gradient, round-to-nearest-even
//! store). Arithmetic follows Skia on ARM64, where the pipeline's `mad` is a fused multiply-add.

use tiny_skia::{Pixmap, Transform};

use crate::color::Rgba;

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
    Linear(Transform),
    Radial(Transform),
    Concentric {
        matrix: Transform,
        scale: f32,
        bias: f32,
    },
}

fn pts_to_unit(p0: (f32, f32), p1: (f32, f32)) -> Transform {
    let (vx, vy) = (p1.0 - p0.0, p1.1 - p0.1);
    let mag2 = vx * vx + vy * vy;
    let mag = if mag2.is_finite() {
        mag2.sqrt()
    } else {
        (f64::from(vx) * f64::from(vx) + f64::from(vy) * f64::from(vy)).sqrt() as f32
    };
    let inv = if mag != 0.0 { 1.0 / mag } else { 0.0 };
    let (sin, cos) = (-(vy * inv), vx * inv);
    let one_minus_cos = 1.0 - cos;
    let tx = sin * p0.1 + one_minus_cos * p0.0;
    let ty = -sin * p0.0 + one_minus_cos * p0.1;
    let m = Transform::from_row(cos, sin, -sin, cos, tx + -p0.0, ty + -p0.1);
    m.post_scale(inv, inv)
}

fn radius_to_unit(center: (f32, f32), radius: f32) -> Transform {
    let inv = 1.0 / radius;
    Transform::from_translate(-center.0, -center.1).post_scale(inv, inv)
}

fn center_distance([x0, y0, _, x1, y1, _]: [f32; 6]) -> f32 {
    let (dx, dy) = (x0 - x1, y0 - y1);
    (dx * dx + dy * dy).sqrt()
}

/// Whether Skia treats the two circles of a radial gradient as concentric.
pub fn shares_center(g: [f32; 6]) -> bool {
    center_distance(g) <= NEARLY_ZERO
}

/// `SkRasterPipeline::appendMatrix` and the stage it picks.
fn apply(m: &Transform, x: f32, y: f32) -> (f32, f32) {
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
    pub fn linear(ctm: Transform, p0: (f32, f32), p1: (f32, f32)) -> Option<Geometry> {
        Some(Geometry::Linear(
            ctm.invert()?.post_concat(pts_to_unit(p0, p1)),
        ))
    }

    /// `SkShaders::TwoPointConicalGradient` for circles that `shares_center` accepts:
    /// `SkRadialGradient` when the start radius is nearly zero, else the concentric `kRadial`
    /// conical. `None` when Skia draws nothing.
    pub fn radial(ctm: Transform, g: [f32; 6]) -> Option<Geometry> {
        let [x0, y0, r0, x1, y1, r1] = g;
        let inverse = ctm.invert()?;
        if center_distance(g) <= DEGENERATE && r0 <= DEGENERATE && (r0 - r1).abs() > DEGENERATE {
            return Some(Geometry::Radial(
                inverse.post_concat(radius_to_unit((x0, y0), r1)),
            ));
        }
        if r0.max(r1) <= NEARLY_ZERO || (r0 - r1).abs() <= NEARLY_ZERO {
            return None;
        }
        let d = r1 - r0;
        Some(Geometry::Concentric {
            matrix: inverse.post_concat(radius_to_unit((x1, y1), r0.max(r1))),
            scale: r0.max(r1) / d,
            bias: -r0 / d,
        })
    }

    fn t(&self, x: f32, y: f32) -> f32 {
        match self {
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
                apply(
                    &Transform::from_row(*scale, 0.0, 0.0, 1.0, *bias, 0.0),
                    r,
                    y,
                )
                .0
            }
        }
    }
}

/// The highp `dither` stage's offset for device pixel `(x, y)`, in `(-0.5, 0.5)`.
fn dither(x: u32, y: u32) -> f32 {
    let y = y ^ x;
    let m = (y & 1) << 5 | (x & 1) << 4 | (y & 2) << 2 | (x & 2) << 1 | (y & 4) >> 1 | (x & 4) >> 2;
    (m as f32).mul_add(2.0 / 128.0, -63.0 / 128.0)
}

/// The premultiplied, dithered and stored colour of one device pixel: paint alpha
/// (`scale_1_float`), then `dither` at rate 1/255 clamped to `[0, a]`, then `store_8888`.
pub fn shade_pixel(c: Channels, paint_alpha: f32, x: u32, y: u32) -> [u8; 4] {
    let c = if paint_alpha != 1.0 {
        c.map(|v| v * paint_alpha)
    } else {
        c
    };
    let d = dither(x, y);
    let a = c[3];
    let store = |v: f32| (v * 255.0).clamp(0.0, 255.0).round_ties_even() as u8;
    let rgb = |v: f32| store(d.mul_add(1.0 / 255.0, v).min(a).max(0.0));
    [rgb(c[0]), rgb(c[1]), rgb(c[2]), store(a)]
}

/// Shades every pixel of a `width` x `height` device into a premultiplied pixmap.
pub fn shade(
    geometry: &Geometry,
    colors: &Colors,
    paint_alpha: f32,
    width: u32,
    height: u32,
) -> Option<Pixmap> {
    let mut out = Pixmap::new(width, height)?;
    let data = out.data_mut();
    for y in 0..height {
        for x in 0..width {
            let t = geometry.t(x as f32 + 0.5, y as f32 + 0.5);
            let i = ((y * width + x) * 4) as usize;
            data[i..i + 4].copy_from_slice(&shade_pixel(colors.at(t), paint_alpha, x, y));
        }
    }
    Some(out)
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
