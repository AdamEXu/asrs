//! Skia's antialiased hairlines (`SkScan_Antihair`, `SkScan_Hairline`): a stroke at most one
//! device pixel wide is drawn as one-pixel bands, one per line segment, each blended on its own.
//! An x-major segment covers one pixel vertically at every column between its end points, a
//! y-major one one pixel horizontally. Square and round caps push the open ends outward.

use std::f32::consts::PI;

use tiny_skia::{LineCap, PathBuilder, PathSegment};

#[derive(Clone, Copy, Debug, PartialEq)]
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

/// The one-pixel band Skia's antihair blitter covers for one segment.
pub fn band(a: P, b: P) -> Option<tiny_skia::Path> {
    if a == b {
        return None;
    }
    let (ox, oy) = if (b.x - a.x).abs() > (b.y - a.y).abs() {
        (0.0, 0.5)
    } else {
        (0.5, 0.0)
    };
    let mut builder = PathBuilder::new();
    builder.move_to(a.x - ox, a.y - oy);
    builder.line_to(b.x - ox, b.y - oy);
    builder.line_to(b.x + ox, b.y + oy);
    builder.line_to(a.x + ox, a.y + oy);
    builder.close();
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn x_major_bands_are_one_pixel_tall() {
        let band = band(P { x: 0.0, y: 2.0 }, P { x: 8.0, y: 3.0 }).unwrap();
        let b = band.bounds();
        assert_eq!((b.top(), b.bottom()), (1.5, 3.5));
        assert_eq!((b.left(), b.right()), (0.0, 8.0));
    }
}
