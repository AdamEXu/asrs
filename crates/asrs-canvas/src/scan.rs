//! Exact-area coverage for device-space paths under the nonzero rule. Chromium's CPU canvas
//! fills with Skia's analytic AA, which computes the covered area of every pixel of the union;
//! tiny-skia only has Skia's older 4x supersampler, which quantizes thin and near-axis edges to
//! quarter pixels.

use tiny_skia::{FillRule, Mask, PathSegment, Pixmap, Transform};

const FLATTEN_TOLERANCE: f32 = 0.02;

/// 8-bit coverage over the integer rect `[x, x + width) x [y, y + height)` of the canvas.
pub struct Coverage {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<u8>,
}

#[derive(Clone, Copy)]
struct P {
    x: f32,
    y: f32,
}

struct Accumulator {
    x0: f32,
    y0: f32,
    width: usize,
    height: usize,
    acc: Vec<f32>,
}

impl Accumulator {
    fn line(&mut self, a: P, b: P) {
        let w = self.width as f32;
        let (a, b) = (
            P {
                x: a.x - self.x0,
                y: a.y - self.y0,
            },
            P {
                x: b.x - self.x0,
                y: b.y - self.y0,
            },
        );
        let mut cuts = vec![0.0f32, 1.0];
        for edge in [0.0, w] {
            if (a.x - edge) * (b.x - edge) < 0.0 {
                cuts.push((edge - a.x) / (b.x - a.x));
            }
        }
        cuts.sort_by(f32::total_cmp);
        let at = |t: f32| P {
            x: a.x + (b.x - a.x) * t,
            y: a.y + (b.y - a.y) * t,
        };
        for pair in cuts.windows(2) {
            let (p, q) = (at(pair[0]), at(pair[1]));
            let mid = (p.x + q.x) * 0.5;
            if mid <= 0.0 {
                self.clipped_line(P { x: 0.0, y: p.y }, P { x: 0.0, y: q.y });
            } else if mid >= w {
                continue;
            } else {
                self.clipped_line(
                    P {
                        x: p.x.clamp(0.0, w),
                        y: p.y,
                    },
                    P {
                        x: q.x.clamp(0.0, w),
                        y: q.y,
                    },
                );
            }
        }
    }

    /// Signed-area accumulation of one edge already inside `[0, width]` horizontally.
    fn clipped_line(&mut self, p0: P, p1: P) {
        if p0.y == p1.y {
            return;
        }
        let (dir, p0, p1) = if p0.y < p1.y {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let stride = self.width + 2;
        let dxdy = (p1.x - p0.x) / (p1.y - p0.y);
        let y_start = p0.y.max(0.0);
        let y_end = p1.y.min(self.height as f32);
        if y_start >= y_end {
            return;
        }
        let mut x = p0.x + (y_start - p0.y) * dxdy;
        let mut row = y_start.floor() as usize;
        while (row as f32) < y_end {
            let top = (row as f32).max(y_start);
            let bottom = ((row + 1) as f32).min(y_end);
            let dy = bottom - top;
            let x_next = x + dxdy * dy;
            let d = dy * dir;
            let (xa, xb) = if x < x_next { (x, x_next) } else { (x_next, x) };
            let line = &mut self.acc[row * stride..(row + 1) * stride];
            let xa_floor = xa.floor();
            let xa_i = xa_floor as usize;
            let xb_ceil = xb.ceil();
            let xb_i = xb_ceil as usize;
            if xb_i <= xa_i + 1 {
                let xmf = 0.5 * (x + x_next) - xa_floor;
                line[xa_i] += d - d * xmf;
                line[xa_i + 1] += d * xmf;
            } else {
                let s = 1.0 / (xb - xa);
                let xa_f = xa - xa_floor;
                let a0 = 0.5 * s * (1.0 - xa_f) * (1.0 - xa_f);
                let xb_f = xb - xb_ceil + 1.0;
                let am = 0.5 * s * xb_f * xb_f;
                line[xa_i] += d * a0;
                if xb_i == xa_i + 2 {
                    line[xa_i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - xa_f);
                    line[xa_i + 1] += d * (a1 - a0);
                    for cell in &mut line[xa_i + 2..xb_i - 1] {
                        *cell += d * s;
                    }
                    let a2 = a1 + (xb_i - xa_i - 3) as f32 * s;
                    line[xb_i - 1] += d * (1.0 - a2 - am);
                }
                line[xb_i] += d * am;
            }
            x = x_next;
            row += 1;
        }
    }
}

/// A path edge in device space, top to bottom; `dir` is +1 where the path ran downward.
#[derive(Clone, Copy)]
struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    dir: i32,
}

impl Edge {
    fn x_at(&self, y: f64) -> f64 {
        self.x0 + (y - self.y0) * (self.x1 - self.x0) / (self.y1 - self.y0)
    }
}

#[derive(Default)]
struct Edges(Vec<Edge>);

impl Edges {
    fn line(&mut self, a: P, b: P) {
        let (a, b) = (
            (f64::from(a.x), f64::from(a.y)),
            (f64::from(b.x), f64::from(b.y)),
        );
        if a.1 == b.1 {
            return;
        }
        let (dir, (x0, y0), (x1, y1)) = if a.1 < b.1 { (1, a, b) } else { (-1, b, a) };
        self.0.push(Edge {
            x0,
            y0,
            x1,
            y1,
            dir,
        });
    }

    fn quad(&mut self, p0: P, p1: P, p2: P) {
        let dd = ((p0.x - 2.0 * p1.x + p2.x).powi(2) + (p0.y - 2.0 * p1.y + p2.y).powi(2)).sqrt();
        let n = ((dd / (8.0 * FLATTEN_TOLERANCE)).sqrt().ceil() as usize).clamp(1, 256);
        let mut last = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let p = P {
                x: mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
                y: mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
            };
            self.line(last, p);
            last = p;
        }
    }

    fn cubic(&mut self, p0: P, p1: P, p2: P, p3: P) {
        let d1 = ((p0.x - 2.0 * p1.x + p2.x).powi(2) + (p0.y - 2.0 * p1.y + p2.y).powi(2)).sqrt();
        let d2 = ((p1.x - 2.0 * p2.x + p3.x).powi(2) + (p1.y - 2.0 * p2.y + p3.y).powi(2)).sqrt();
        let n = ((0.75 * d1.max(d2) / FLATTEN_TOLERANCE).sqrt().ceil() as usize).clamp(1, 512);
        let mut last = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let (a, b, c, e) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
            let p = P {
                x: a * p0.x + b * p1.x + c * p2.x + e * p3.x,
                y: a * p0.y + b * p1.y + c * p2.y + e * p3.y,
            };
            self.line(last, p);
            last = p;
        }
    }
}

/// Skia's analytic AA turns a 0..=1 area into alpha as `a - (a >> 8)` on a 0..=256 scale.
fn to_alpha(area: f32) -> u8 {
    let a = (area.clamp(0.0, 1.0) * 256.0 + 0.5) as u32;
    (a - (a >> 8)) as u8
}

const EPSILON: f64 = 1e-9;

/// The coverage of `path` (device space, nonzero rule) clipped to a `width` x `height` canvas.
/// Each pixel row is cut into strips at edge end points and crossings; inside a strip no two
/// edges cross, so the filled region is a set of disjoint trapezoids bounded by the edges where
/// the winding number leaves or returns to zero, and their exact areas are accumulated.
pub fn rasterize(path: &tiny_skia::Path, width: u32, height: u32) -> Option<Coverage> {
    let b = path.bounds();
    let left = b.left().floor().max(0.0);
    let top = b.top().floor().max(0.0);
    let right = b.right().ceil().min(width as f32);
    let bottom = b.bottom().ceil().min(height as f32);
    if !(left < right && top < bottom) {
        return None;
    }
    let (w, h) = ((right - left) as usize, (bottom - top) as usize);
    let mut edges = Edges::default();
    let point = |p: tiny_skia::Point| P { x: p.x, y: p.y };
    let (mut start, mut last) = (P { x: 0.0, y: 0.0 }, P { x: 0.0, y: 0.0 });
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                edges.line(last, start);
                start = point(p);
                last = start;
            }
            PathSegment::LineTo(p) => {
                let p = point(p);
                edges.line(last, p);
                last = p;
            }
            PathSegment::QuadTo(p1, p2) => {
                let (p1, p2) = (point(p1), point(p2));
                edges.quad(last, p1, p2);
                last = p2;
            }
            PathSegment::CubicTo(p1, p2, p3) => {
                let (p1, p2, p3) = (point(p1), point(p2), point(p3));
                edges.cubic(last, p1, p2, p3);
                last = p3;
            }
            PathSegment::Close => {
                edges.line(last, start);
                last = start;
            }
        }
    }
    edges.line(last, start);
    let edges = edges.0;
    let mut acc = Accumulator {
        x0: left,
        y0: top,
        width: w,
        height: h,
        acc: vec![0.0; (w + 2) * h],
    };
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); h];
    let top64 = f64::from(top);
    for (i, e) in edges.iter().enumerate() {
        let r0 = ((e.y0 - top64).floor().max(0.0)) as usize;
        let r1 = ((e.y1 - top64).ceil().max(0.0) as usize).min(h);
        for row in rows.iter_mut().take(r1).skip(r0) {
            row.push(i);
        }
    }
    let mut cuts: Vec<f64> = Vec::new();
    let mut spanning: Vec<(f64, usize)> = Vec::new();
    for (r, active) in rows.iter().enumerate() {
        if active.is_empty() {
            continue;
        }
        let (ya, yb) = (top64 + r as f64, top64 + r as f64 + 1.0);
        cuts.clear();
        cuts.extend([ya, yb]);
        for &i in active {
            let e = &edges[i];
            for y in [e.y0, e.y1] {
                if y > ya && y < yb {
                    cuts.push(y);
                }
            }
        }
        for (k, &i) in active.iter().enumerate() {
            let a = &edges[i];
            let sa = (a.x1 - a.x0) / (a.y1 - a.y0);
            for &j in &active[k + 1..] {
                let b = &edges[j];
                let sb = (b.x1 - b.x0) / (b.y1 - b.y0);
                if sa == sb {
                    continue;
                }
                let y = (b.x0 - b.y0 * sb - a.x0 + a.y0 * sa) / (sa - sb);
                let lo = ya.max(a.y0).max(b.y0);
                let hi = yb.min(a.y1).min(b.y1);
                if y > lo && y < hi {
                    cuts.push(y);
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < EPSILON);
        for strip in cuts.windows(2) {
            let (sy0, sy1) = (strip[0], strip[1]);
            if sy1 - sy0 < EPSILON {
                continue;
            }
            let mid = (sy0 + sy1) * 0.5;
            spanning.clear();
            spanning.extend(active.iter().filter_map(|&i| {
                let e = &edges[i];
                (e.y0 <= sy0 + EPSILON && e.y1 >= sy1 - EPSILON).then(|| (e.x_at(mid), i))
            }));
            spanning.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for &(_, i) in &spanning {
                let e = &edges[i];
                let before = winding;
                winding += e.dir;
                let (top_x, bottom_x) = (e.x_at(sy0) as f32, e.x_at(sy1) as f32);
                let (t, bt) = (sy0 as f32, sy1 as f32);
                if before == 0 && winding != 0 {
                    acc.line(P { x: top_x, y: t }, P { x: bottom_x, y: bt });
                } else if before != 0 && winding == 0 {
                    acc.line(P { x: bottom_x, y: bt }, P { x: top_x, y: t });
                }
            }
        }
    }
    let stride = w + 2;
    let mut alpha = vec![0u8; w * h];
    for row in 0..h {
        let mut sum = 0.0f32;
        for col in 0..w {
            sum += acc.acc[row * stride + col];
            alpha[row * w + col] = to_alpha(sum);
        }
    }
    Some(Coverage {
        x: left as i32,
        y: top as i32,
        width: w,
        height: h,
        alpha,
    })
}

fn is_rect(path: &tiny_skia::Path) -> bool {
    let mut points = Vec::new();
    for (i, segment) in path.segments().enumerate() {
        match (i, segment) {
            (0, PathSegment::MoveTo(p)) => points.push(p),
            (1..=4, PathSegment::LineTo(p)) => points.push(p),
            (4 | 5, PathSegment::Close) => {}
            _ => return false,
        }
    }
    if points.len() == 5 && points[4] == points[0] {
        points.pop();
    }
    points.len() == 4
        && (0..4).all(|i| {
            let (a, b) = (points[i], points[(i + 1) % 4]);
            (a.x == b.x) != (a.y == b.y)
        })
}

/// Skia's `ShouldUseAAA`: rects and paths with few points for their size get analytic coverage;
/// denser paths (dashes, round caps, long polylines) go to the 4x supersampler.
pub fn should_use_analytic(path: &tiny_skia::Path, skia_points: usize) -> bool {
    if is_rect(path) {
        return true;
    }
    let b = path.bounds();
    (skia_points as f32) < b.width().max(b.height()) / 2.0 - 10.0
}

/// Skia's supersampling scan converter (`SkScan_AntiPath`), which tiny-skia ports.
pub fn rasterize_supersampled(path: &tiny_skia::Path, width: u32, height: u32) -> Option<Coverage> {
    let b = path.bounds();
    let left = b.left().floor().max(0.0);
    let top = b.top().floor().max(0.0);
    let right = b.right().ceil().min(width as f32);
    let bottom = b.bottom().ceil().min(height as f32);
    if !(left < right && top < bottom) {
        return None;
    }
    let (w, h) = ((right - left) as u32, (bottom - top) as u32);
    let mut mask = Mask::new(w, h)?;
    mask.fill_path(
        path,
        FillRule::Winding,
        true,
        Transform::from_translate(-left, -top),
    );
    Some(Coverage {
        x: left as i32,
        y: top as i32,
        width: w as usize,
        height: h as usize,
        alpha: mask.data().to_vec(),
    })
}

/// Skia's NEON `div255`, the exact `(v + 127) / 255`.
pub fn div255(v: u32) -> u32 {
    (v + ((v + 128) >> 8) + 128) >> 8
}

pub enum Blend {
    SourceOver,
    Clear,
}

/// Blends a premultiplied source (`source(x, y)` in canvas pixels) through `coverage` with
/// Skia's 8-bit pipeline arithmetic.
pub fn blit(
    pixmap: &mut Pixmap,
    coverage: &Coverage,
    blend: Blend,
    source: impl Fn(usize, usize) -> [u8; 4],
) {
    let stride = pixmap.width() as usize;
    let data = pixmap.data_mut();
    for row in 0..coverage.height {
        let y = coverage.y as usize + row;
        for col in 0..coverage.width {
            let c = u32::from(coverage.alpha[row * coverage.width + col]);
            if c == 0 {
                continue;
            }
            let x = coverage.x as usize + col;
            let i = (y * stride + x) * 4;
            let dst = &mut data[i..i + 4];
            match blend {
                Blend::Clear => {
                    for d in dst.iter_mut() {
                        *d = div255(u32::from(*d) * (255 - c)) as u8;
                    }
                }
                Blend::SourceOver => {
                    let s = source(x, y).map(|v| div255(u32::from(v) * c));
                    let inv = 255 - s[3];
                    for (d, s) in dst.iter_mut().zip(s) {
                        *d = (s + div255(u32::from(*d) * inv)).min(255) as u8;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{PathBuilder, Rect};

    fn rect(l: f32, t: f32, r: f32, b: f32) -> tiny_skia::Path {
        PathBuilder::from_rect(Rect::from_ltrb(l, t, r, b).unwrap())
    }

    fn row(c: &Coverage, y: usize) -> &[u8] {
        &c.alpha[y * c.width..(y + 1) * c.width]
    }

    #[test]
    fn thin_vertical_band_gets_exact_area() {
        let c = rasterize(&rect(359.375, 0.0, 360.625, 4.0), 400, 4).unwrap();
        assert_eq!(c.x, 359);
        assert_eq!(row(&c, 1), [160, 160]);
    }

    #[test]
    fn partially_offscreen_paths_are_clipped() {
        let c = rasterize(&rect(-5.0, 0.5, 2.25, 1.0), 4, 2).unwrap();
        assert_eq!((c.x, c.width), (0, 3));
        assert_eq!(row(&c, 0), [128, 128, 32]);
    }

    #[test]
    fn overlapping_same_direction_contours_saturate() {
        let mut b = PathBuilder::new();
        b.push_rect(Rect::from_ltrb(0.0, 0.0, 2.0, 1.0).unwrap());
        b.push_rect(Rect::from_ltrb(1.0, 0.0, 3.0, 1.0).unwrap());
        let c = rasterize(&b.finish().unwrap(), 3, 1).unwrap();
        assert_eq!(row(&c, 0), [255, 255, 255]);
    }

    #[test]
    fn div255_rounds() {
        for v in 0..=255 * 255 {
            assert_eq!(div255(v), (v + 127) / 255, "{v}");
        }
    }
}
