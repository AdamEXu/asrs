// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! Skia's antialiased hairlines (`SkScan_Antihair`, `SkScan_Hairline`): a stroke at most one
//! device pixel wide is drawn as one-pixel bands, one per line segment, each blended on its own.
//! An x-major segment covers one pixel vertically at every column between its end points, a
//! y-major one one pixel horizontally. Square and round caps push the open ends outward.

use std::f32::consts::PI;

use tiny_skia::{LineCap, PathSegment};

use crate::blit::Spans;
use crate::clip::{self, ClipRect};

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

/// One line of a hairline path, and whether `hair_path` hands it the canvas clip, which [`spans`]
/// then narrows to the line or drops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: P,
    pub b: P,
    pub clipped: bool,
}

fn point_bounds(pts: &[P]) -> [f32; 4] {
    pts.iter().fold(
        [pts[0].x, pts[0].y, pts[0].x, pts[0].y],
        |[l, t, r, b], q| [l.min(q.x), t.min(q.y), r.max(q.x), b.max(q.y)],
    )
}

/// The per-curve test of `hairquad` and `haircubic`: `None` when the curve's control points
/// miss the outset clip, otherwise whether it still needs the clip.
fn curve_clip(clip: Option<([f32; 4], [f32; 4])>, pts: &[P]) -> Option<bool> {
    let Some((inset, outset)) = clip else {
        return Some(false);
    };
    let [l, t, r, b] = point_bounds(pts);
    if !(outset[0] < r && l < outset[2] && outset[1] < b && t < outset[3]) {
        return None;
    }
    Some(!(r <= inset[2] && l >= inset[0] && b <= inset[3] && t >= inset[1]))
}

/// `hair_path`: the line segments Skia's antialiased hairline blits for a device-space path
/// on a `width` x `height` canvas.
pub fn segments(path: &tiny_skia::Path, cap: LineCap, width: u32, height: u32) -> Vec<Segment> {
    let (w, h) = (width as i32, height as i32);
    let bounds = path.bounds();
    let cap_out = if cap == LineCap::Butt { 1 } else { 2 };
    let ib = [
        (bounds.left().floor() as i32).saturating_sub(cap_out),
        (bounds.top().floor() as i32).saturating_sub(cap_out),
        (bounds.right().ceil() as i32).saturating_add(cap_out),
        (bounds.bottom().ceil() as i32).saturating_add(cap_out),
    ];
    if !(ib[0] < w && 0 < ib[2] && ib[1] < h && 0 < ib[3]) {
        return Vec::new();
    }
    let contained = 0 <= ib[0] && 0 <= ib[1] && ib[2] <= w && ib[3] <= h;
    let clip = (!contained).then(|| {
        let (fw, fh) = (width as f32, height as f32);
        let inset = if 1.0 > fw - 1.0 || 1.0 > fh - 1.0 {
            [0.0; 4]
        } else {
            [1.0, 1.0, fw - 1.0, fh - 1.0]
        };
        (inset, [-1.0, -1.0, fw + 1.0, fh + 1.0])
    });
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
    let mut polylines: Vec<(Vec<P>, bool)> = Vec::new();
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
                    Verb::Line => polylines.push((pts.clone(), clip.is_some())),
                    Verb::Quad => {
                        if let Some(clipped) = curve_clip(clip, &pts) {
                            let q = [pts[0], pts[1], pts[2]];
                            polylines.push((quad_lines(&q, quad_level(&q)), clipped));
                        }
                    }
                    _ => {
                        if let Some(clipped) = curve_clip(clip, &pts) {
                            let mut lines = Vec::new();
                            hair_cubic(&[pts[0], pts[1], pts[2], pts[3]], &mut lines);
                            polylines.extend(lines.into_iter().map(|l| (l, clipped)));
                        }
                    }
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
                polylines.push((pts.clone(), clip.is_some()));
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
        .flat_map(|(pts, clipped)| {
            pts.windows(2).map(|w| Segment {
                a: w[0],
                b: w[1],
                clipped: *clipped,
            })
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Hair {
    HLine,
    Horish,
    VLine,
    Vertish,
}

const FIXED_HALF: i32 = 1 << 15;

fn dot6_scale(value: u32, mod64: i32) -> u8 {
    ((value * mod64 as u32) >> 6) as u8
}

/// `contribution_64`: the fractional part of a 26.6 ordinate, with whole numbers giving 64.
fn contribution_64(ordinate: i32) -> i32 {
    ((ordinate - 1) & 63) + 1
}

/// `blitAntiV2`; through `SkRectClipBlitter` it is `SkBlitter`'s default, two `blitAntiH`s.
fn anti_v2(out: &mut Spans, clipped: bool, x: i32, y: i32, a0: u8, a1: u8) {
    if clipped {
        out.anti_h(x, y, &[a0]);
        out.anti_h(x, y + 1, &[a1]);
    } else {
        out.anti_v2(x, y, a0, a1);
    }
}

/// `blitAntiH2`; through `SkRectClipBlitter` it is `SkBlitter`'s default, one `blitAntiH`.
fn anti_h2(out: &mut Spans, clipped: bool, x: i32, y: i32, a0: u8, a1: u8) {
    if clipped {
        out.anti_h(x, y, &[a0, a1]);
    } else {
        out.anti_h2(x, y, a0, a1);
    }
}

/// `do_anti_hairline` from `SkScan_Antihair.cpp`, in 26.6 and 16.16 fixed point, with `clip` as
/// `[left, top, right, bottom]`. Segments longer than 511 px are halved, and the halves meet in
/// one pixel that both blit into, so the blits are kept in Skia's order and blended one after
/// another.
fn anti_hairline(x0: i32, y0: i32, x1: i32, y1: i32, clip: Option<[i32; 4]>, out: &mut Spans) {
    if (x1 - x0).abs() > 511 * 64 || (y1 - y0).abs() > 511 * 64 {
        let (hx, hy) = ((x0 >> 1) + (x1 >> 1), (y0 >> 1) + (y1 >> 1));
        anti_hairline(x0, y0, hx, hy, clip, out);
        anti_hairline(hx, hy, x1, y1, clip, out);
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
    let mut istart = major0 >> 6;
    let mut istop = (major1 + 63) >> 6;
    let mut fstart = minor0 << 10;
    let slope = if minor0 == minor1 {
        0
    } else {
        let s = ((i64::from(minor1 - minor0) << 16) / i64::from(major1 - major0)) as i32;
        fstart += ((i64::from(s) * i64::from(32 - (major0 & 63)) + 32) >> 6) as i32;
        s
    };
    let hair = match (horizontal, minor0 == minor1) {
        (true, true) => Hair::HLine,
        (true, false) => Hair::Horish,
        (false, true) => Hair::VLine,
        (false, false) => Hair::Vertish,
    };
    let (mut scale_start, mut scale_stop) = if istop - istart == 1 {
        (major1 - major0, 0)
    } else {
        (64 - (major0 & 63), major1 & 63)
    };
    let mut clipped = false;
    if let Some([l, t, r, b]) = clip {
        let (major_lo, major_hi, minor_lo, minor_hi) = if horizontal {
            (l, r, t, b)
        } else {
            (t, b, l, r)
        };
        if istart >= major_hi || istop <= major_lo {
            return;
        }
        if istart < major_lo {
            fstart = fstart.wrapping_add(slope.wrapping_mul(major_lo - istart));
            istart = major_lo;
            scale_start = 64;
            if istop - istart == 1 {
                scale_start = contribution_64(major1);
                scale_stop = 0;
            }
        }
        if istop > major_hi {
            istop = major_hi;
            scale_stop = 0;
        }
        if istart == istop {
            return;
        }
        let span = (istop - istart - 1).wrapping_mul(slope);
        let (lo, hi) = if slope >= 0 {
            (
                fstart.wrapping_sub(FIXED_HALF) >> 16,
                fstart.wrapping_add(span).wrapping_add(FIXED_HALF + 0xffff) >> 16,
            )
        } else {
            (
                fstart.wrapping_add(span).wrapping_sub(FIXED_HALF) >> 16,
                fstart.wrapping_add(FIXED_HALF + 0xffff) >> 16,
            )
        };
        // Skia builds with OUTSET_BEFORE_CLIP_TEST defined.
        let (lo, hi) = (lo - 1, hi + 1);
        if lo >= minor_hi || hi <= minor_lo {
            return;
        }
        clipped = !(minor_lo <= lo && minor_hi >= hi);
    }
    let mut f = draw_cap(out, hair, clipped, istart, fstart, slope, scale_start);
    let full = istop - istart - 1 - i32::from(scale_stop > 0);
    if full > 0 {
        f = draw_line(out, hair, clipped, istart + 1, istart + 1 + full, f, slope);
    }
    if scale_stop > 0 {
        draw_cap(out, hair, clipped, istop - 1, f, slope, scale_stop);
    }
}

/// `SkAntiHairBlitter::drawCap`: one step along the major axis, the two pixels straddling `f`
/// (16.16, the minor coordinate at the centre of `major`) sharing its coverage scaled by
/// `mod64 / 64`.
fn draw_cap(
    out: &mut Spans,
    hair: Hair,
    clipped: bool,
    major: i32,
    f: i32,
    slope: i32,
    mod64: i32,
) -> i32 {
    let f = f + FIXED_HALF;
    let lower = f >> 16;
    let a = ((f >> 8) & 0xff) as u32;
    let (lo, up) = (dot6_scale(a, mod64), dot6_scale(255 - a, mod64));
    match hair {
        Hair::HLine => {
            out.anti_h(major, lower, &[lo]);
            out.anti_h(major, lower - 1, &[up]);
        }
        Hair::VLine => {
            out.v(lower, major, 1, lo);
            out.v(lower - 1, major, 1, up);
        }
        Hair::Horish => anti_v2(out, clipped, major, lower - 1, up, lo),
        Hair::Vertish => anti_h2(out, clipped, lower - 1, major, up, lo),
    }
    f + slope - FIXED_HALF
}

/// `SkAntiHairBlitter::drawLine` over `start..stop` on the major axis.
fn draw_line(
    out: &mut Spans,
    hair: Hair,
    clipped: bool,
    start: i32,
    stop: i32,
    f: i32,
    slope: i32,
) -> i32 {
    let mut f = f + FIXED_HALF;
    let count = stop - start;
    let lower = f >> 16;
    let a = ((f >> 8) & 0xff) as u8;
    match hair {
        Hair::HLine => {
            out.anti_h(start, lower, &vec![a; count as usize]);
            out.anti_h(start, lower - 1, &vec![255 - a; count as usize]);
        }
        Hair::VLine => {
            out.v(lower, start, count, a);
            out.v(lower - 1, start, count, 255 - a);
        }
        Hair::Horish | Hair::Vertish => {
            for major in start..stop {
                let lower = f >> 16;
                let a = ((f >> 8) & 0xff) as u8;
                if hair == Hair::Horish {
                    anti_v2(out, clipped, major, lower - 1, 255 - a, a);
                } else {
                    anti_h2(out, clipped, lower - 1, major, 255 - a, a);
                }
                f += slope;
            }
        }
    }
    f - FIXED_HALF
}

/// `SkScan::AntiHairLineRgn` for one segment: the blits Skia's antihair blitter makes for it, in
/// order, on a `width` x `height` canvas. A segment that is not inside the canvas gets the
/// `SkRegion::Cliperator` rect, the canvas cut to the segment's own bounds, as its clip.
pub fn spans(seg: Segment, width: u32, height: u32) -> Option<Spans> {
    let fixed = ClipRect {
        left: -32767.0,
        top: -32767.0,
        right: 32767.0,
        bottom: 32767.0,
    };
    let mut pts = clip::intersect_line([seg.a, seg.b], fixed)?;
    if seg.clipped {
        let outset = ClipRect {
            left: -1.0,
            top: -1.0,
            right: width as f32 + 1.0,
            bottom: height as f32 + 1.0,
        };
        pts = clip::intersect_line(pts, outset)?;
    }
    let dot6 = |v: f32| (v * 64.0) as i32;
    let [x0, y0, x1, y1] = [pts[0].x, pts[0].y, pts[1].x, pts[1].y].map(dot6);
    let mut clip = None;
    if seg.clipped {
        let (w, h) = (width as i32, height as i32);
        let ir = [
            (x0.min(x1) >> 6) - 1,
            (y0.min(y1) >> 6) - 1,
            ((x0.max(x1) + 63) >> 6) + 1,
            ((y0.max(y1) + 63) >> 6) + 1,
        ];
        if !(ir[0] < w && 0 < ir[2] && ir[1] < h && 0 < ir[3]) {
            return None;
        }
        if !(0 <= ir[0] && 0 <= ir[1] && ir[2] <= w && ir[3] <= h) {
            clip = Some([ir[0].max(0), ir[1].max(0), ir[2].min(w), ir[3].min(h)]);
        }
    }
    let mut out = Spans::new(width, height);
    anti_hairline(x0, y0, x1, y1, clip, &mut out);
    (!out.is_empty()).then_some(out)
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
        assert_eq!(
            segments(&b.finish().unwrap(), LineCap::Butt, 100, 100).len(),
            1
        );
        let mut b = PathBuilder::new();
        b.move_to(0.0, 0.0);
        b.cubic_to(10.0, 40.0, 20.0, 40.0, 30.0, 0.0);
        let segs = segments(&b.finish().unwrap(), LineCap::Butt, 100, 100);
        assert_eq!(segs.len(), 32);
        assert_eq!(segs[31].b, P { x: 30.0, y: 0.0 });
    }

    #[test]
    fn square_caps_extend_open_ends_only() {
        let segs = segments(
            &path(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false),
            LineCap::Square,
            100,
            100,
        );
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].a, P { x: -0.5, y: 0.0 });
        assert_eq!(segs[0].b, P { x: 10.0, y: 0.0 });
        assert_eq!(segs[1].b, P { x: 10.0, y: 10.5 });
    }

    #[test]
    fn closed_contours_draw_their_closing_segment() {
        let segs = segments(
            &path(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)], true),
            LineCap::Butt,
            100,
            100,
        );
        assert_eq!(segs.len(), 3);
        assert_eq!(
            (segs[2].a, segs[2].b),
            (P { x: 4.0, y: 4.0 }, P { x: 0.0, y: 0.0 })
        );
        assert!(segs.iter().all(|s| s.clipped));
    }

    fn seg(a: (f32, f32), b: (f32, f32), clipped: bool) -> Segment {
        Segment {
            a: P { x: a.0, y: a.1 },
            b: P { x: b.0, y: b.1 },
            clipped,
        }
    }

    fn alphas(a: P, b: P, w: u32, h: u32) -> Vec<u8> {
        let s = Segment {
            a,
            b,
            clipped: true,
        };
        spans(s, w, h).unwrap().flatten(w as usize, h as usize)
    }

    fn blits_at(spans: &Spans, x: i32, y: i32) -> Vec<(u8, bool)> {
        spans
            .iter_kinds()
            .filter(|&(sx, sy, a, _)| sy == y && (sx..sx + a.len() as i32).contains(&x))
            .map(|(sx, _, a, pair)| (a[(x - sx) as usize], pair))
            .collect()
    }

    #[test]
    fn halves_of_a_long_round_capped_line_both_blit_the_split_pixel() {
        let path = path(&[(55.0, 22.5), (735.0, 22.5)], false);
        let segs = segments(&path, LineCap::Round, 800, 60);
        assert_eq!(segs.len(), 1);
        assert!(!segs[0].clipped);
        let out = spans(segs[0], 800, 60).unwrap();
        assert_eq!(blits_at(&out, 394, 22), [(251, false), (3, false)]);
        assert_eq!(blits_at(&out, 393, 22), [(255, false)]);
        assert_eq!(blits_at(&out, 395, 22), [(255, false)]);
    }

    #[test]
    fn slanted_lines_blit_pixel_pairs_only_without_the_clip() {
        let inside = spans(seg((10.0, 10.5), (90.0, 30.5), true), 100, 100).unwrap();
        assert!(inside.iter_kinds().all(|(.., pair)| pair));
        let past_the_end = spans(seg((10.0, 10.5), (99.8, 30.5), true), 100, 100).unwrap();
        assert!(past_the_end.iter_kinds().all(|(.., pair)| pair));
        let edge = spans(seg((10.0, 0.3), (90.0, 20.5), true), 100, 100).unwrap();
        assert!(edge.iter_kinds().all(|(.., pair)| !pair));
        let unclipped = spans(seg((10.0, 0.3), (90.0, 20.5), false), 100, 100).unwrap();
        assert!(unclipped.iter_kinds().all(|(.., pair)| pair));
    }

    #[test]
    fn curves_inside_the_inset_canvas_drop_the_clip() {
        let mut b = PathBuilder::new();
        b.move_to(0.5, 50.0);
        b.line_to(20.0, 50.0);
        b.quad_to(50.0, 10.0, 80.0, 50.0);
        let segs = segments(&b.finish().unwrap(), LineCap::Butt, 100, 100);
        assert!(segs[0].clipped);
        assert!(segs[1..].iter().all(|s| !s.clipped));
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
