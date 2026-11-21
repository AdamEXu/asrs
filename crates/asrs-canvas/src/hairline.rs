// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011 Google Inc.). See THIRD_PARTY_NOTICES.md.

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
enum Verb {
    Move,
    Line,
    Quad,
    Cubic,
    Close,
}

fn finite(p: P) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// `compute_int_quad_dist` and `compute_quad_level`.
fn quad_level(pts: &[P; 3]) -> u32 {
    let dx = ((pts[0].x + pts[2].x) / 2.0 - pts[1].x).abs();
    let dy = ((pts[0].y + pts[2].y) / 2.0 - pts[1].y).abs();
    let (idx, idy) = (dx.ceil() as i32 as u32, dy.ceil() as i32 as u32);
    let dist = if idx > idy {
        idx + (idy >> 1)
    } else {
        idy + (idx >> 1)
    };
    ((33 - dist.leading_zeros()) >> 1).min(5)
}

/// `hair_quad`: the polyline through `1 << level` steps of `SkQuadCoeff`, or nothing if a step
/// is not finite.
fn quad_lines(pts: &[P; 3], level: u32) -> Vec<P> {
    let c = pts[0];
    let b = P {
        x: (pts[1].x - c.x) + (pts[1].x - c.x),
        y: (pts[1].y - c.y) + (pts[1].y - c.y),
    };
    let a = P {
        x: pts[2].x - (pts[1].x + pts[1].x) + c.x,
        y: pts[2].y - (pts[1].y + pts[1].y) + c.y,
    };
    let lines = 1u32 << level;
    let dt = 1.0 / lines as f32;
    let mut t = 0.0f32;
    let mut out = vec![pts[0]];
    for _ in 1..lines {
        t += dt;
        let q = P {
            x: (a.x * t + b.x) * t + c.x,
            y: (a.y * t + b.y) * t + c.y,
        };
        out.push(q);
    }
    out.push(pts[2]);
    out
}

const MAX_CUBIC_SUBDIVIDE_LEVEL: u32 = 9;

/// `compute_cubic_segs` in `SkScan_Hairline.cpp`.
fn cubic_segs(pts: &[P; 4]) -> u32 {
    let (one_third, two_third) = (1.0f32 / 3.0, 2.0f32 / 3.0);
    let p13 = |a: f32, b: f32| one_third * a + two_third * b;
    let diff_x = (pts[1].x - p13(pts[3].x, pts[0].x))
        .abs()
        .max((pts[2].x - p13(pts[0].x, pts[3].x)).abs());
    let diff_y = (pts[1].y - p13(pts[3].y, pts[0].y))
        .abs()
        .max((pts[2].y - p13(pts[0].y, pts[3].y)).abs());
    let diff = diff_x.max(diff_y);
    let mut tol = 1.0f32 / 8.0;
    for i in 0..MAX_CUBIC_SUBDIVIDE_LEVEL {
        if diff < tol {
            return 1 << i;
        }
        tol *= 4.0;
    }
    1 << MAX_CUBIC_SUBDIVIDE_LEVEL
}

/// `hair_cubic`: the polyline through `SkCubicCoeff` steps, or nothing if a step is not finite.
fn cubic_lines(pts: &[P; 4]) -> Vec<P> {
    let lines = cubic_segs(pts);
    if lines == 1 {
        return vec![pts[0], pts[3]];
    }
    let coeff = crate::geometry::CubicCoeff::new(pts);
    let dt = 1.0 / lines as f32;
    let mut t = 0.0f32;
    let mut out = vec![pts[0]];
    for _ in 1..lines {
        t += dt;
        let q = coeff.eval(t);
        if !finite(q) {
            return Vec::new();
        }
        out.push(q);
    }
    out.push(pts[3]);
    out
}

fn lt_90(p0: P, pivot: P, p2: P) -> bool {
    (p0.x - pivot.x) * (p2.x - pivot.x) + (p0.y - pivot.y) * (p2.y - pivot.y) >= 0.0
}

/// `haircubic`: cubics whose controls stray outside their ends are first cut at their points of
/// maximum curvature.
fn hair_cubic(pts: &[P; 4], out: &mut Vec<Vec<P>>) {
    let nice = lt_90(pts[1], pts[0], pts[3])
        && lt_90(pts[2], pts[0], pts[3])
        && lt_90(pts[1], pts[3], pts[0])
        && lt_90(pts[2], pts[3], pts[0]);
    if nice {
        out.push(cubic_lines(pts));
    } else {
        for c in crate::geometry::chop_cubic_at_max_curvature(pts) {
            out.push(cubic_lines(&c));
        }
    }
}

/// `SkPoint::normalize`, computed in double.
fn normalize(v: P) -> P {
    let (x, y) = (f64::from(v.x), f64::from(v.y));
    let scale = 1.0 / (x * x + y * y).sqrt();
    let n = P {
        x: (x * scale) as f32,
        y: (y * scale) as f32,
    };
    if !finite(n) || (n.x == 0.0 && n.y == 0.0) {
        return P::default();
    }
    n
}

/// `extend_pts`: moves the open ends of a segment's control points outward along their end
/// tangents, dragging along controls that coincide with the end.
fn extend(prev: Option<Verb>, next: Option<Verb>, pts: &mut [P], outset: f32) {
    let n = pts.len();
    let is_zero = |v: P| v.x == 0.0 && v.y == 0.0;
    if prev == Some(Verb::Move) {
        let mut ctrl = 0;
        let mut controls = n - 1;
        let mut tangent;
        loop {
            ctrl += 1;
            tangent = P {
                x: pts[0].x - pts[ctrl].x,
                y: pts[0].y - pts[ctrl].y,
            };
            controls -= 1;
            if !(is_zero(tangent) && controls > 0) {
                break;
            }
        }
        controls += 1;
        if is_zero(tangent) {
            tangent = P { x: 1.0, y: 0.0 };
            controls = n - 1;
        } else {
            tangent = normalize(tangent);
        }
        let mut i = 0;
        loop {
            pts[i].x += tangent.x * outset;
            pts[i].y += tangent.y * outset;
            i += 1;
            controls += 1;
            if controls >= n {
                break;
            }
        }
    }
    if matches!(next, None | Some(Verb::Move) | Some(Verb::Close)) {
        let mut ctrl = n - 1;
        let mut controls = n - 1;
        let mut tangent;
        loop {
            ctrl -= 1;
            tangent = P {
                x: pts[n - 1].x - pts[ctrl].x,
                y: pts[n - 1].y - pts[ctrl].y,
            };
            controls -= 1;
            if !(is_zero(tangent) && controls > 0) {
                break;
            }
        }
        controls += 1;
        if is_zero(tangent) {
            tangent = P { x: -1.0, y: 0.0 };
            controls = n - 1;
        } else {
            tangent = normalize(tangent);
        }
        let mut i = n - 1;
        loop {
            pts[i].x += tangent.x * outset;
            pts[i].y += tangent.y * outset;
            controls += 1;
            if controls >= n {
                break;
            }
            i -= 1;
        }
    }
}

/// `is_next_contour_closed`, from the move at `i`.
fn contour_closed(verbs: &[(Verb, Vec<P>)], i: usize) -> bool {
    for (verb, _) in &verbs[i + 1..] {
        match verb {
            Verb::Close => return true,
            Verb::Move => return false,
            _ => {}
        }
    }
    false
}

/// `hair_path`: the line segments Skia's antialiased hairline blits for a device-space path.
pub fn segments(path: &tiny_skia::Path, cap: LineCap) -> Vec<(P, P)> {
    let outset = match cap {
        LineCap::Butt => None,
        LineCap::Square => Some(0.5),
        LineCap::Round => Some(PI / 8.0),
    };
    let point = |p: tiny_skia::Point| P { x: p.x, y: p.y };
    let mut verbs: Vec<(Verb, Vec<P>)> = Vec::new();
    let mut last = P::default();
    for segment in path.segments() {
        let (verb, pts) = match segment {
            PathSegment::MoveTo(p) => (Verb::Move, vec![point(p)]),
            PathSegment::LineTo(p) => (Verb::Line, vec![last, point(p)]),
            PathSegment::QuadTo(p1, p2) => (Verb::Quad, vec![last, point(p1), point(p2)]),
            PathSegment::CubicTo(p1, p2, p3) => {
                (Verb::Cubic, vec![last, point(p1), point(p2), point(p3)])
            }
            PathSegment::Close => (Verb::Close, Vec::new()),
        };
        if let Some(&end) = pts.last() {
            last = end;
        }
        verbs.push((verb, pts));
    }
    let mut polylines: Vec<Vec<P>> = Vec::new();
    let (mut first, mut last_pt) = (P::default(), P::default());
    let mut closed = false;
    let mut prev: Option<Verb> = None;
    for (i, (verb, src)) in verbs.iter().enumerate() {
        let verb = *verb;
        let next = verbs.get(i + 1).map(|v| v.0);
        let mut pts = src.clone();
        let all_equal = |pts: &[P]| pts.iter().all(|&q| q == pts[0]);
        match verb {
            Verb::Move => {
                first = pts[0];
                last_pt = first;
                closed = outset.is_some() && contour_closed(&verbs, i);
            }
            Verb::Line | Verb::Quad | Verb::Cubic => {
                if let Some(outset) = outset
                    && (!closed || all_equal(&pts))
                {
                    extend(prev, next, &mut pts, outset);
                }
                match verb {
                    Verb::Line => polylines.push(pts.clone()),
                    Verb::Quad => {
                        let q = [pts[0], pts[1], pts[2]];
                        polylines.push(quad_lines(&q, quad_level(&q)));
                    }
                    _ => hair_cubic(&[pts[0], pts[1], pts[2], pts[3]], &mut polylines),
                }
                last_pt = pts[pts.len() - 1];
            }
            Verb::Close => {
                pts = vec![last_pt, first];
                if let Some(outset) = outset
                    && prev == Some(Verb::Move)
                {
                    extend(prev, next, &mut pts, outset);
                }
                polylines.push(pts.clone());
            }
        }
        if outset.is_some() {
            if prev == Some(Verb::Move) && matches!(verb, Verb::Line | Verb::Quad | Verb::Cubic) {
                first = pts[0];
            }
            prev = Some(verb);
        }
    }
    polylines
        .iter()
        .flat_map(|pts| pts.windows(2).map(|w| (w[0], w[1])))
        .collect()
}

/// Coverage of one segment, clipped to the canvas; one segment never touches a pixel twice.
struct Plot {
    x0: i32,
    y0: i32,
    width: usize,
    height: usize,
    alpha: Vec<u8>,
    pair: bool,
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
    plot.pair |= slope != 0;
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
        pair: false,
    };
    anti_hairline(dot6(a.x), dot6(a.y), dot6(b.x), dot6(b.y), &mut plot);
    Some(Coverage {
        x: x0,
        y: y0,
        width: w,
        height: h,
        alpha: plot.alpha,
        pair: plot.pair,
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
    fn huge_quads_subdivide_without_overflow() {
        let q = [P::default(), P { x: 3e38, y: 3e38 }, P::default()];
        assert_eq!(quad_level(&q), 5);
        assert_eq!(quad_lines(&q, quad_level(&q)).len(), 33);
    }

    #[test]
    fn flat_cubic_is_one_segment_and_curved_cubic_many() {
        let mut b = PathBuilder::new();
        b.move_to(0.0, 0.0);
        b.cubic_to(10.0, 0.0, 20.0, 0.0, 30.0, 0.0);
        assert_eq!(segments(&b.finish().unwrap(), LineCap::Butt).len(), 1);
        let mut b = PathBuilder::new();
        b.move_to(0.0, 0.0);
        b.cubic_to(10.0, 40.0, 20.0, 40.0, 30.0, 0.0);
        let segs = segments(&b.finish().unwrap(), LineCap::Butt);
        assert_eq!(segs.len(), 32);
        assert_eq!(segs[31].1, P { x: 30.0, y: 0.0 });
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
