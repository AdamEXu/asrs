//! Skia's antialiased hairlines (`SkScan_Antihair`, `SkScan_Hairline`): a stroke at most one
//! device pixel wide is drawn as one-pixel bands, one per line segment, each blended on its own.
//! An x-major segment covers one pixel vertically at every column between its end points, a
//! y-major one one pixel horizontally. Square and round caps push the open ends outward.

use std::f32::consts::PI;

use tiny_skia::{LineCap, PathSegment};

use crate::clip::{self, ClipRect};
use crate::scan::Coverage;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Move,
    Draw,
    Close,
}

struct Piece {
    kind: Kind,
    points: Vec<P>,
}

fn quad_lines(p0: P, p1: P, p2: P) -> Vec<P> {
    let dx = ((p0.x + p2.x) * 0.5 - p1.x).abs().ceil() as u32;
    let dy = ((p0.y + p2.y) * 0.5 - p1.y).abs().ceil() as u32;
    let dist = if dx > dy {
        dx + (dy >> 1)
    } else {
        dy + (dx >> 1)
    };
    let level = ((33 - dist.leading_zeros()) >> 1).min(5);
    let lines = 1u32 << level;
    (0..=lines)
        .map(|i| {
            let t = i as f32 / lines as f32;
            let mt = 1.0 - t;
            P {
                x: mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
                y: mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
            }
        })
        .collect()
}

fn cubic_lines(p0: P, p1: P, p2: P, p3: P) -> Vec<P> {
    let third = |a: f32, b: f32| a / 3.0 + b * (2.0 / 3.0);
    let diff = [
        (p1.x - third(p3.x, p0.x)).abs(),
        (p1.y - third(p3.y, p0.y)).abs(),
        (p2.x - third(p0.x, p3.x)).abs(),
        (p2.y - third(p0.y, p3.y)).abs(),
    ]
    .into_iter()
    .fold(0.0f32, f32::max);
    let mut tol = 1.0 / 8.0;
    let mut lines = 1u32 << 9;
    for i in 0..9 {
        if diff < tol {
            lines = 1 << i;
            break;
        }
        tol *= 4.0;
    }
    (0..=lines)
        .map(|i| {
            let t = i as f32 / lines as f32;
            let mt = 1.0 - t;
            let (a, b, c, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
            P {
                x: a * p0.x + b * p1.x + c * p2.x + d * p3.x,
                y: a * p0.y + b * p1.y + c * p2.y + d * p3.y,
            }
        })
        .collect()
}

/// `extend_pts`: moves the open ends of a piece outward along its end tangents.
fn extend(points: &mut [P], at_start: bool, at_end: bool, outset: f32) {
    let n = points.len();
    let tangent = |from: P, to: P| {
        let (x, y) = (from.x - to.x, from.y - to.y);
        let len = (x * x + y * y).sqrt();
        (len > 0.0).then(|| (x / len, y / len))
    };
    if at_start {
        let t = (1..n).find_map(|i| tangent(points[0], points[i]));
        match t {
            Some((tx, ty)) => {
                points[0].x += tx * outset;
                points[0].y += ty * outset;
            }
            None => {
                for p in &mut points[..n - 1] {
                    p.x += outset;
                }
            }
        }
    }
    if at_end {
        let t = (0..n - 1)
            .rev()
            .find_map(|i| tangent(points[n - 1], points[i]));
        match t {
            Some((tx, ty)) => {
                points[n - 1].x += tx * outset;
                points[n - 1].y += ty * outset;
            }
            None => {
                for p in &mut points[1..] {
                    p.x -= outset;
                }
            }
        }
    }
}

/// The line segments Skia blits for a device-space hairline path.
pub fn segments(path: &tiny_skia::Path, cap: LineCap) -> Vec<(P, P)> {
    let outset = match cap {
        LineCap::Butt => None,
        LineCap::Square => Some(0.5),
        LineCap::Round => Some(PI / 8.0),
    };
    let point = |p: tiny_skia::Point| P { x: p.x, y: p.y };
    let mut pieces: Vec<Piece> = Vec::new();
    let (mut first, mut last) = (P { x: 0.0, y: 0.0 }, P { x: 0.0, y: 0.0 });
    for segment in path.segments() {
        let (kind, points) = match segment {
            PathSegment::MoveTo(p) => {
                first = point(p);
                last = first;
                (Kind::Move, Vec::new())
            }
            PathSegment::LineTo(p) => (Kind::Draw, vec![last, point(p)]),
            PathSegment::QuadTo(p1, p2) => (Kind::Draw, quad_lines(last, point(p1), point(p2))),
            PathSegment::CubicTo(p1, p2, p3) => (
                Kind::Draw,
                cubic_lines(last, point(p1), point(p2), point(p3)),
            ),
            PathSegment::Close => (Kind::Close, vec![last, first]),
        };
        if let Some(&end) = points.last() {
            last = end;
        }
        pieces.push(Piece { kind, points });
    }
    let mut out = Vec::new();
    for i in 0..pieces.len() {
        let prev = if i == 0 {
            Kind::Move
        } else {
            pieces[i - 1].kind
        };
        let next = pieces.get(i + 1).map_or(Kind::Move, |p| p.kind);
        let piece = &pieces[i];
        let mut points = piece.points.clone();
        if points.len() < 2 {
            continue;
        }
        if let Some(outset) = outset {
            let (start, end) = match piece.kind {
                Kind::Close => (prev == Kind::Move, prev == Kind::Move),
                _ => (
                    prev == Kind::Move,
                    next == Kind::Move || (next == Kind::Close && prev == Kind::Move),
                ),
            };
            extend(&mut points, start, end, outset);
        }
        out.extend(points.windows(2).map(|w| (w[0], w[1])));
    }
    out
}

/// Coverage of one segment, clipped to the canvas; one segment never touches a pixel twice.
struct Plot {
    x0: i32,
    y0: i32,
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

impl Plot {
    fn put(&mut self, x: i32, y: i32, a: u32) {
        let (cx, cy) = (x - self.x0, y - self.y0);
        if a == 0 || cx < 0 || cy < 0 || cx as usize >= self.width || cy as usize >= self.height {
            return;
        }
        self.alpha[cy as usize * self.width + cx as usize] = a.min(255) as u8;
    }
}

const FIXED_HALF: i32 = 1 << 15;

/// `do_anti_hairline` from `SkScan_Antihair.cpp`, in 26.6 and 16.16 fixed point.
fn anti_hairline(x0: i32, y0: i32, x1: i32, y1: i32, plot: &mut Plot) {
    if (x1 - x0).abs() > 511 * 64 || (y1 - y0).abs() > 511 * 64 {
        let (hx, hy) = ((x0 >> 1) + (x1 >> 1), (y0 >> 1) + (y1 >> 1));
        anti_hairline(x0, y0, hx, hy, plot);
        anti_hairline(hx, hy, x1, y1, plot);
        return;
    }
    let horizontal = (x1 - x0).abs() > (y1 - y0).abs();
    let (major0, minor0, major1, minor1) = if horizontal {
        if x0 > x1 {
            (x1, y1, x0, y0)
        } else {
            (x0, y0, x1, y1)
        }
    } else if y0 > y1 {
        (y1, x1, y0, x0)
    } else {
        (y0, x0, y1, x1)
    };
    if major0 == major1 {
        return;
    }
    let istart = major0 >> 6;
    let istop = (major1 + 63) >> 6;
    let mut fstart = minor0 << 10;
    let slope = if minor0 == minor1 {
        0
    } else {
        let s = ((i64::from(minor1 - minor0) << 16) / i64::from(major1 - major0)) as i32;
        fstart += ((i64::from(s) * i64::from(32 - (major0 & 63)) + 32) >> 6) as i32;
        s
    };
    let (scale_start, scale_stop) = if istop - istart == 1 {
        (major1 - major0, 0)
    } else {
        (64 - (major0 & 63), major1 & 63)
    };
    let mut f = cap(plot, horizontal, istart, fstart, scale_start) + slope;
    let full = istop - istart - 1 - i32::from(scale_stop > 0);
    for major in istart + 1..istart + 1 + full.max(0) {
        cap(plot, horizontal, major, f, 64);
        f += slope;
    }
    if scale_stop > 0 {
        cap(plot, horizontal, istop - 1, f, scale_stop);
    }
}

/// One step along the major axis: the two pixels straddling `f` (16.16, the line's minor
/// coordinate at the centre of `major`) share its coverage, scaled by `mod64 / 64`.
fn cap(plot: &mut Plot, horizontal: bool, major: i32, f: i32, mod64: i32) -> i32 {
    let f = f + FIXED_HALF;
    let lower = f >> 16;
    let a = ((f >> 8) & 0xff) as u32;
    let scale = |v: u32| {
        if mod64 == 64 {
            v
        } else {
            (v * mod64 as u32) >> 6
        }
    };
    let (lo, up) = (scale(a), scale(255 - a));
    if horizontal {
        plot.put(major, lower, lo);
        plot.put(major, lower - 1, up);
    } else {
        plot.put(lower, major, lo);
        plot.put(lower - 1, major, up);
    }
    f - FIXED_HALF
}

/// The coverage Skia's antihair blitter gives one segment, clipped to the canvas.
pub fn coverage(a: P, b: P, width: u32, height: u32) -> Option<Coverage> {
    let fixed = ClipRect {
        left: -32767.0,
        top: -32767.0,
        right: 32767.0,
        bottom: 32767.0,
    };
    let canvas = ClipRect {
        left: -1.0,
        top: -1.0,
        right: width as f32 + 1.0,
        bottom: height as f32 + 1.0,
    };
    let [a, b] = clip::intersect_line([a, b], fixed)?;
    let [a, b] = clip::intersect_line([a, b], canvas)?;
    let dot6 = |v: f32| (v * 64.0) as i32;
    let x0 = (a.x.min(b.x).floor() as i32 - 1).max(0);
    let y0 = (a.y.min(b.y).floor() as i32 - 1).max(0);
    let x1 = (a.x.max(b.x).ceil() as i32 + 2).min(width as i32);
    let y1 = (a.y.max(b.y).ceil() as i32 + 2).min(height as i32);
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut plot = Plot {
        x0,
        y0,
        width: w,
        height: h,
        alpha: vec![0; w * h],
    };
    anti_hairline(dot6(a.x), dot6(a.y), dot6(b.x), dot6(b.y), &mut plot);
    Some(Coverage {
        x: x0,
        y: y0,
        width: w,
        height: h,
        alpha: plot.alpha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::PathBuilder;

    fn path(points: &[(f32, f32)], close: bool) -> tiny_skia::Path {
        let mut b = PathBuilder::new();
        b.move_to(points[0].0, points[0].1);
        for &(x, y) in &points[1..] {
            b.line_to(x, y);
        }
        if close {
            b.close();
        }
        b.finish().unwrap()
    }

    #[test]
    fn square_caps_extend_open_ends_only() {
        let segs = segments(
            &path(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false),
            LineCap::Square,
        );
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].0, P { x: -0.5, y: 0.0 });
        assert_eq!(segs[0].1, P { x: 10.0, y: 0.0 });
        assert_eq!(segs[1].1, P { x: 10.0, y: 10.5 });
    }

    #[test]
    fn closed_contours_draw_their_closing_segment() {
        let segs = segments(
            &path(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)], true),
            LineCap::Butt,
        );
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[2], (P { x: 4.0, y: 4.0 }, P { x: 0.0, y: 0.0 }));
    }

    fn alphas(a: P, b: P, w: u32, h: u32) -> Vec<u8> {
        let c = coverage(a, b, w, h).unwrap();
        let mut out = vec![0; (w * h) as usize];
        for row in 0..c.height {
            for col in 0..c.width {
                out[(c.y as usize + row) * w as usize + c.x as usize + col] =
                    c.alpha[row * c.width + col];
            }
        }
        out
    }

    #[test]
    fn line_on_a_pixel_edge_splits_127_128() {
        let px = alphas(P { x: 2.0, y: 0.0 }, P { x: 2.0, y: 4.0 }, 4, 4);
        assert_eq!(&px[4..8], [0, 127, 128, 0]);
    }

    #[test]
    fn diagonal_through_pixel_centres_covers_one_pixel_per_row() {
        let px = alphas(P { x: 0.0, y: 0.0 }, P { x: 4.0, y: 4.0 }, 4, 4);
        assert_eq!(&px[4..8], [0, 255, 0, 0]);
        assert_eq!(&px[8..12], [0, 0, 255, 0]);
    }
}
