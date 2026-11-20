//! Exact-area coverage for device-space paths under the nonzero rule. Chromium's CPU canvas
//! fills every antialiased path with Skia's analytic AA, which computes the covered area of each
//! pixel of the union (after snapping edge end points); tiny-skia only has Skia's retired 4x
//! supersampler, which quantizes thin and near-axis edges to quarter pixels.

use tiny_skia::{PathSegment, Pixmap};

use crate::clip::{self, ClipRect, Piece};
use crate::hairline::P;

const FLATTEN_TOLERANCE: f32 = 0.02;

/// 8-bit coverage over the integer rect `[x, x + width) x [y, y + height)` of the canvas.
pub struct Coverage {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<u8>,
    /// Blitted pixel pair by pixel pair (`blitAntiH2`, `blitAntiV2`), as slanted hairlines are.
    pub pair: bool,
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

struct Edges {
    edges: Vec<Edge>,
    snap: bool,
    clip: Option<ClipRect>,
}

/// `SkAnalyticEdge::setLine` quantizes x to 1/256 pixel and snaps y to the nearest quarter
/// pixel (`SnapY`) before computing coverage.
fn snap(p: P) -> (f64, f64) {
    let fixed = |v: f32| (((v * 4.0 * 64.0) as i32) << 10) >> 2;
    let y = fixed(p.y);
    let y = (((y as u32).wrapping_add(1 << 13)) >> 14 << 14) as i32;
    (f64::from(fixed(p.x)) / 65536.0, f64::from(y) / 65536.0)
}

impl Edges {
    fn line(&mut self, a: P, b: P) {
        let Some(clip) = self.clip else {
            self.push_line(a, b);
            return;
        };
        let mut pieces = Vec::new();
        clip::clip_line([a, b], clip, &mut pieces);
        self.push_pieces(pieces);
    }

    fn push_pieces(&mut self, pieces: Vec<Piece>) {
        for piece in pieces {
            match piece {
                Piece::Line([a, b]) => self.push_line(a, b),
                Piece::Quad([p0, p1, p2]) => self.monotonic_quad(p0, p1, p2),
                Piece::Cubic(_) => unreachable!("only lines and quads are clipped here"),
            }
        }
    }

    fn push_line(&mut self, a: P, b: P) {
        let exact = |p: P| (f64::from(p.x), f64::from(p.y));
        let (a, b) = if self.snap {
            (snap(a), snap(b))
        } else {
            (exact(a), exact(b))
        };
        if a.1 == b.1 {
            return;
        }
        let (dir, (x0, y0), (x1, y1)) = if a.1 < b.1 { (1, a, b) } else { (-1, b, a) };
        self.edges.push(Edge {
            x0,
            y0,
            x1,
            y1,
            dir,
        });
    }

    fn quad(&mut self, p0: P, p1: P, p2: P) {
        if let Some(clip) = self.clip {
            let mut pieces = Vec::new();
            clip::clip_quad([p0, p1, p2], clip, &mut pieces);
            self.push_pieces(pieces);
            return;
        }
        if self.snap {
            let denom = p0.y - 2.0 * p1.y + p2.y;
            let t = if denom != 0.0 {
                (p0.y - p1.y) / denom
            } else {
                0.0
            };
            if t > 0.0 && t < 1.0 {
                let lerp = |a: P, b: P| P {
                    x: a.x + (b.x - a.x) * t,
                    y: a.y + (b.y - a.y) * t,
                };
                let (mut a, mut b) = (lerp(p0, p1), lerp(p1, p2));
                let mid = lerp(a, b);
                // SkChopQuadAtYExtrema flattens both halves onto the extremum.
                a.y = mid.y;
                b.y = mid.y;
                self.monotonic_quad(p0, a, mid);
                self.monotonic_quad(mid, b, p2);
            } else {
                self.monotonic_quad(p0, p1, p2);
            }
            return;
        }
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

    /// `SkAnalyticQuadraticEdge`: a y-monotonic quad stepped in `1 << shift` lines whose
    /// interior vertices snap to quarter pixels, or to whole pixels for steps two or more
    /// pixels tall (sliding x along the step).
    fn monotonic_quad(&mut self, p0: P, p1: P, p2: P) {
        let dot8 = |v: f32| (v * 256.0) as i32;
        let (x0, y0, x1, y1, x2, y2) = (
            dot8(p0.x),
            dot8(p0.y),
            dot8(p1.x),
            dot8(p1.y),
            dot8(p2.x),
            dot8(p2.y),
        );
        if (y0.min(y2) + 32) >> 6 == (y0.max(y2) + 32) >> 6 {
            return;
        }
        let dx = (((x1 << 1) - x0 - x2) >> 2).abs();
        let dy = (((y1 << 1) - y0 - y2) >> 2).abs();
        let dist = if dx > dy {
            dx + (dy >> 1)
        } else {
            dy + (dx >> 1)
        };
        let dist = (dist + (1 << 4)) >> 5;
        let shift = ((32 - dist.leading_zeros()) >> 1).clamp(1, 6);
        let steps = 1u32 << shift;
        let (down, a, b, c) = if p0.y <= p2.y {
            (true, p0, p1, p2)
        } else {
            (false, p2, p1, p0)
        };
        let fixed = |v: f32| f64::from(dot8(v)) / 256.0;
        let eval = |t: f32| {
            let mt = 1.0 - t;
            P {
                x: mt * mt * a.x + 2.0 * mt * t * b.x + t * t * c.x,
                y: mt * mt * a.y + 2.0 * mt * t * b.y + t * t * c.y,
            }
        };
        let quarter = |v: f64| (v * 4.0 + 0.5).floor() / 4.0;
        let last_y = quarter(fixed(c.y));
        let mut vertices = vec![(fixed(a.x), quarter(fixed(a.y)))];
        let mut raw_prev = a;
        for i in 1..steps {
            let p = eval(i as f32 / steps as f32);
            let (px, py) = (fixed(p.x), fixed(p.y));
            let &(sx, sy) = vertices.last().expect("seeded");
            let vertex = if (p.y - raw_prev.y).abs() >= 2.0 {
                let snapped = py.round().min(last_y);
                let slope = if py != sy { (px - sx) / (py - sy) } else { 0.0 };
                (px - slope * (py - snapped), snapped)
            } else {
                (px, quarter(py).min(last_y))
            };
            vertices.push(vertex);
            raw_prev = p;
        }
        vertices.push((fixed(c.x), last_y));
        for w in vertices.windows(2) {
            let ((xa, ya), (xb, yb)) = (w[0], w[1]);
            if ya == yb {
                continue;
            }
            let (x0, y0, x1, y1) = if ya < yb {
                (xa, ya, xb, yb)
            } else {
                (xb, yb, xa, ya)
            };
            let flipped = ya > yb;
            self.edges.push(Edge {
                x0,
                y0,
                x1,
                y1,
                dir: if down != flipped { 1 } else { -1 },
            });
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
    sweep(path, width, height, false)
}

/// Skia's analytic AA proper: [`rasterize`] over edges snapped like `SkAnalyticEdge`.
pub fn rasterize_analytic(path: &tiny_skia::Path, width: u32, height: u32) -> Option<Coverage> {
    sweep(path, width, height, true)
}

fn sweep(path: &tiny_skia::Path, width: u32, height: u32, snap: bool) -> Option<Coverage> {
    let b = path.bounds();
    let left = b.left().floor().max(0.0);
    let top = b.top().floor().max(0.0);
    let right = b.right().ceil().min(width as f32);
    let bottom = b.bottom().ceil().min(height as f32);
    if !(left < right && top < bottom) {
        return None;
    }
    let (w, h) = ((right - left) as usize, (bottom - top) as usize);
    let contained = b.left().floor() >= 0.0
        && b.top().floor() >= 0.0
        && b.right().ceil() <= width as f32
        && b.bottom().ceil() <= height as f32;
    let mut edges = Edges {
        edges: Vec::new(),
        snap,
        clip: (snap && !contained).then_some(ClipRect {
            left: 0.0,
            top: 0.0,
            right: width as f32,
            bottom: height as f32,
        }),
    };
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
    let edges = edges.edges;
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
        pair: false,
    })
}

impl Coverage {
    pub fn to_spans(&self, width: u32, height: u32) -> crate::blit::Spans {
        let mut spans = crate::blit::Spans::new(width, height);
        for row in 0..self.height {
            let alphas = &self.alpha[row * self.width..(row + 1) * self.width];
            if self.pair {
                for (i, &a) in alphas.iter().enumerate() {
                    if a != 0 {
                        spans.anti_v2(self.x + i as i32, self.y + row as i32, a, 0);
                    }
                }
            } else {
                spans.anti_h(self.x, self.y + row as i32, alphas);
            }
        }
        spans
    }
}

/// Skia's NEON `div255`, the exact `(v + 127) / 255`.
pub fn div255(v: u32) -> u32 {
    (v + ((v + 128) >> 8) + 128) >> 8
}

/// `SkScan::AntiFillRect` (`antifilldot8`) for a device rect, clipped to the canvas.
pub fn anti_fill_rect(r: [f32; 4], width: u32, height: u32) -> crate::blit::Spans {
    let mut out = crate::blit::Spans::new(width, height);
    let l = r[0].max(0.0);
    let t = r[1].max(0.0);
    let rr = r[2].min(width as f32);
    let b = r[3].min(height as f32);
    if !(l < rr && t < b) {
        return out;
    }
    let dot8 = |v: f32| ((v * 65536.0) as i32).wrapping_add(0x80) >> 8;
    let (l, t, r, b) = (dot8(l), dot8(t), dot8(rr), dot8(b));
    if l >= r || t >= b {
        return out;
    }
    let scanline = |out: &mut crate::blit::Spans, top: i32, alpha: i32| {
        let mul = |a: i32, b: i32| ((a * b) >> 8) as u8;
        if l >> 8 == (r - 1) >> 8 {
            out.v(l >> 8, top, 1, mul(alpha, r - l));
            return;
        }
        let mut left = l >> 8;
        if l & 0xFF != 0 {
            out.v(left, top, 1, mul(alpha, 256 - (l & 0xFF)));
            left += 1;
        }
        let rite = r >> 8;
        if rite - left > 0 {
            out.anti_h(left, top, &vec![alpha as u8; (rite - left) as usize]);
        }
        if r & 0xFF != 0 {
            out.v(rite, top, 1, mul(alpha, r & 0xFF));
        }
    };
    let mut top = t >> 8;
    if top == (b - 1) >> 8 {
        scanline(&mut out, top, b - t - 1);
        return out;
    }
    if t & 0xFF != 0 {
        scanline(&mut out, top, 256 - (t & 0xFF));
        top += 1;
    }
    let bot = b >> 8;
    let h = bot - top;
    if h > 0 {
        let mut left = l >> 8;
        if left == (r - 1) >> 8 {
            out.v(left, top, h, (r - l - 1) as u8);
        } else {
            if l & 0xFF != 0 {
                out.v(left, top, h, (256 - (l & 0xFF)) as u8);
                left += 1;
            }
            let rite = r >> 8;
            if rite - left > 0 {
                out.rect(left, top, rite - left, h);
            }
            if r & 0xFF != 0 {
                out.v(rite, top, h, (r & 0xFF) as u8);
            }
        }
    }
    if b & 0xFF != 0 {
        scanline(&mut out, bot, b & 0xFF);
    }
    out
}

/// `SkPreMultiplyColor`.
pub fn premultiply_color(c: [u8; 4]) -> [u8; 4] {
    let a = u32::from(c[3]);
    let mul = |v: u8| {
        if a == 255 {
            return v;
        }
        let prod = u32::from(v) * a + 128;
        ((prod + (prod >> 8)) >> 8) as u8
    };
    [mul(c[0]), mul(c[1]), mul(c[2]), c[3]]
}

/// Blends an unpremultiplied solid colour through `spans` the way Chromium's CPU canvas does:
/// with Skia's legacy ARGB32 blitters (`SkBlitter_ARGB32.cpp`), whose `blitAntiH`, `blitV` and
/// `blitMask` scale by `alpha + 1` and shift, and whose `blitAntiH2`/`blitAntiV2` interpolate.
pub fn blit_spans_solid(pixmap: &mut Pixmap, spans: &crate::blit::Spans, color: [u8; 4]) {
    if color[3] == 0 {
        return;
    }
    let pm = premultiply_color(color);
    let black = color == [0, 0, 0, 255];
    let opaque = color[3] == 255;
    let stride = pixmap.width() as usize;
    let data = pixmap.data_mut();
    for (x0, y, alphas, pair) in spans.iter_kinds() {
        let row = y as usize * stride;
        for (i, &c) in alphas.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let k = (row + x0 as usize + i) * 4;
            let dst = &mut data[k..k + 4];
            let c = u32::from(c);
            if pair && !black {
                if opaque {
                    let scale = c + (c >> 7);
                    for (d, &s) in dst.iter_mut().zip(&pm) {
                        *d = ((u32::from(s) * scale + (256 - scale) * u32::from(*d)) >> 8) as u8;
                    }
                } else {
                    let src_scale = c + 1;
                    let prod = 0xFFFF - u32::from(pm[3]) * src_scale;
                    let dst_scale = (prod + (prod >> 8)) >> 8;
                    for (d, &s) in dst.iter_mut().zip(&pm) {
                        *d = ((u32::from(s) * src_scale + u32::from(*d) * dst_scale) >> 8) as u8;
                    }
                }
                continue;
            }
            let sc = pm.map(|s| ((u32::from(s) * (c + 1)) >> 8) as u8);
            match sc[3] {
                0 => {}
                255 => dst.copy_from_slice(&sc),
                a => {
                    let inv = 256 - u32::from(a);
                    for (d, &s) in dst.iter_mut().zip(&sc) {
                        *d = (((u32::from(*d) * inv) >> 8) + u32::from(s)) as u8;
                    }
                }
            }
        }
    }
}

/// Blends a premultiplied source through `spans` in blit order with Skia's 8-bit pipeline
/// arithmetic.
pub fn blit_spans(
    pixmap: &mut Pixmap,
    spans: &crate::blit::Spans,
    blend: Blend,
    source: impl Fn(usize, usize) -> [u8; 4],
) {
    let stride = pixmap.width() as usize;
    let data = pixmap.data_mut();
    for (x0, y, alphas) in spans.iter() {
        let y = y as usize;
        for (i, &c) in alphas.iter().enumerate() {
            let c = u32::from(c);
            if c == 0 {
                continue;
            }
            let x = x0 as usize + i;
            let k = (y * stride + x) * 4;
            let dst = &mut data[k..k + 4];
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

#[derive(Clone, Copy)]
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
    fn overlapping_partial_contours_cover_their_union_once() {
        let mut b = PathBuilder::new();
        b.push_rect(Rect::from_ltrb(0.0, 0.5, 2.0, 1.0).unwrap());
        b.push_rect(Rect::from_ltrb(1.0, 0.5, 3.0, 1.0).unwrap());
        let c = rasterize(&b.finish().unwrap(), 3, 1).unwrap();
        assert_eq!(row(&c, 0), [128, 128, 128]);
    }

    #[test]
    fn analytic_edges_snap_to_quarter_pixels() {
        let band = rect(0.0, 0.3, 4.0, 1.0);
        assert_eq!(row(&rasterize(&band, 4, 1).unwrap(), 0)[0], 179);
        assert_eq!(row(&rasterize_analytic(&band, 4, 1).unwrap(), 0)[0], 192);
    }

    #[test]
    fn div255_rounds() {
        for v in 0..=255 * 255 {
            assert_eq!(div255(v), (v + 127) / 255, "{v}");
        }
    }
}
