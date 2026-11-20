//! Skia's `SkLineClipper` and the quad half of `SkEdgeClipper`: device geometry is cut to the
//! clip before the scan converters turn it into fixed point, which overflows a few thousand
//! pixels off the canvas.

use crate::hairline::P;

#[derive(Clone, Copy)]
pub struct ClipRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

pub enum Piece {
    Line([P; 2]),
    Quad([P; 3]),
}

fn nearly_zero(v: f32) -> bool {
    v.abs() <= 1.0 / 4096.0
}

fn midpoint(a: f32, b: f32) -> f32 {
    ((f64::from(a) + f64::from(b)) * 0.5) as f32
}

fn pin_unsorted<T: PartialOrd>(value: T, a: T, b: T) -> T {
    let (lo, hi) = if b < a { (b, a) } else { (a, b) };
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

fn sect_with_horizontal(a: P, b: P, y: f32) -> f32 {
    if nearly_zero(b.y - a.y) {
        return midpoint(a.x, b.x);
    }
    let (x0, y0, x1, y1) = (
        f64::from(a.x),
        f64::from(a.y),
        f64::from(b.x),
        f64::from(b.y),
    );
    pin_unsorted(x0 + (f64::from(y) - y0) * (x1 - x0) / (y1 - y0), x0, x1) as f32
}

fn sect_with_vertical(a: P, b: P, x: f32) -> f32 {
    if nearly_zero(b.x - a.x) {
        return midpoint(a.y, b.y);
    }
    let (x0, y0, x1, y1) = (
        f64::from(a.x),
        f64::from(a.y),
        f64::from(b.x),
        f64::from(b.y),
    );
    (y0 + (f64::from(x) - x0) * (y1 - y0) / (x1 - x0)) as f32
}

fn sect_clamp_with_vertical(a: P, b: P, x: f32) -> f32 {
    pin_unsorted(sect_with_vertical(a, b, x), a.y, b.y)
}

/// `SkLineClipper::IntersectLine`: the part of the segment inside `clip`, in its own direction.
pub fn intersect_line(src: [P; 2], clip: ClipRect) -> Option<[P; 2]> {
    let (l, r) = (src[0].x.min(src[1].x), src[0].x.max(src[1].x));
    let (t, b) = (src[0].y.min(src[1].y), src[0].y.max(src[1].y));
    if clip.left <= l && clip.top <= t && clip.right >= r && clip.bottom >= b {
        return Some(src);
    }
    let nested_lt = |a: f32, b: f32, dim: f32| a <= b && (a < b || dim > 0.0);
    if nested_lt(r, clip.left, r - l)
        || nested_lt(clip.right, l, r - l)
        || nested_lt(b, clip.top, b - t)
        || nested_lt(clip.bottom, t, b - t)
    {
        return None;
    }
    let (i0, i1) = if src[0].y < src[1].y { (0, 1) } else { (1, 0) };
    let mut tmp = src;
    if tmp[i0].y < clip.top {
        tmp[i0] = P {
            x: sect_with_horizontal(src[0], src[1], clip.top),
            y: clip.top,
        };
    }
    if tmp[i1].y > clip.bottom {
        tmp[i1] = P {
            x: sect_with_horizontal(src[0], src[1], clip.bottom),
            y: clip.bottom,
        };
    }
    let (i0, i1) = if tmp[0].x < tmp[1].x { (0, 1) } else { (1, 0) };
    if (tmp[i1].x <= clip.left || tmp[i0].x >= clip.right)
        && (tmp[0].x != tmp[1].x || tmp[0].x < clip.left || tmp[0].x > clip.right)
    {
        return None;
    }
    if tmp[i0].x < clip.left {
        tmp[i0] = P {
            x: clip.left,
            y: sect_with_vertical(tmp[0], tmp[1], clip.left),
        };
    }
    if tmp[i1].x > clip.right {
        tmp[i1] = P {
            x: clip.right,
            y: sect_with_vertical(tmp[0], tmp[1], clip.right),
        };
    }
    Some(tmp)
}

/// `SkLineClipper::ClipLine` without culling to the right: the parts left or right of `clip`
/// become vertical lines on its edge, so the winding inside is unchanged.
pub fn clip_line(pts: [P; 2], clip: ClipRect, out: &mut Vec<Piece>) {
    let (i0, i1) = if pts[0].y < pts[1].y { (0, 1) } else { (1, 0) };
    if pts[i1].y <= clip.top || pts[i0].y >= clip.bottom {
        return;
    }
    let mut tmp = pts;
    if pts[i0].y < clip.top {
        tmp[i0] = P {
            x: sect_with_horizontal(pts[0], pts[1], clip.top),
            y: clip.top,
        };
    }
    if tmp[i1].y > clip.bottom {
        tmp[i1] = P {
            x: sect_with_horizontal(pts[0], pts[1], clip.bottom),
            y: clip.bottom,
        };
    }
    let (i0, i1, mut reverse) = if pts[0].x < pts[1].x {
        (0, 1, false)
    } else {
        (1, 0, true)
    };
    let mut result = Vec::with_capacity(4);
    if tmp[i1].x <= clip.left || tmp[i0].x >= clip.right {
        let x = if tmp[i1].x <= clip.left {
            clip.left
        } else {
            clip.right
        };
        result.extend(tmp.map(|p| P { x, y: p.y }));
        reverse = false;
    } else {
        if tmp[i0].x < clip.left {
            result.push(P {
                x: clip.left,
                y: tmp[i0].y,
            });
            result.push(P {
                x: clip.left,
                y: sect_clamp_with_vertical(tmp[0], tmp[1], clip.left),
            });
        } else {
            result.push(tmp[i0]);
        }
        if tmp[i1].x > clip.right {
            result.push(P {
                x: clip.right,
                y: sect_clamp_with_vertical(tmp[0], tmp[1], clip.right),
            });
            result.push(P {
                x: clip.right,
                y: tmp[i1].y,
            });
        } else {
            result.push(tmp[i1]);
        }
    }
    if reverse {
        result.reverse();
    }
    out.extend(result.windows(2).map(|w| Piece::Line([w[0], w[1]])));
}

fn valid_unit_divide(numer: f32, denom: f32) -> Option<f32> {
    let (numer, denom) = if numer < 0.0 {
        (-numer, -denom)
    } else {
        (numer, denom)
    };
    if denom == 0.0 || numer == 0.0 || numer >= denom {
        return None;
    }
    let r = numer / denom;
    (!r.is_nan() && r != 0.0).then_some(r)
}

/// The smaller root in (0, 1) of `a t^2 + b t + c`, as `SkFindUnitQuadRoots` orders them.
fn first_unit_quad_root(a: f32, b: f32, c: f32) -> Option<f32> {
    if a == 0.0 {
        return valid_unit_divide(-c, b);
    }
    let dr = f64::from(b) * f64::from(b) - 4.0 * f64::from(a) * f64::from(c);
    if dr < 0.0 {
        return None;
    }
    let r = dr.sqrt() as f32;
    if !r.is_finite() {
        return None;
    }
    let q = if b < 0.0 {
        -(b - r) / 2.0
    } else {
        -(b + r) / 2.0
    };
    match (valid_unit_divide(q, a), valid_unit_divide(c, q)) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    }
}

fn chop_mono_quad_at(c0: f32, c1: f32, c2: f32, target: f32) -> Option<f32> {
    first_unit_quad_root(c0 - c1 - c1 + c2, 2.0 * (c1 - c0), c0 - target)
}

fn chop_quad_at(src: [P; 3], t: f32) -> [P; 5] {
    let interp = |a: P, b: P| P {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    };
    let (p01, p12) = (interp(src[0], src[1]), interp(src[1], src[2]));
    [src[0], p01, interp(p01, p12), p12, src[2]]
}

fn is_not_monotonic(a: f32, b: f32, c: f32) -> bool {
    let ab = a - b;
    let bc = if ab < 0.0 { c - b } else { b - c };
    ab == 0.0 || bc < 0.0
}

fn coord(p: P, y: bool) -> f32 {
    if y { p.y } else { p.x }
}

fn set_coord(p: &mut P, y: bool, v: f32) {
    if y {
        p.y = v;
    } else {
        p.x = v;
    }
}

/// `SkChopQuadAtYExtrema` (`y`) or `SkChopQuadAtXExtrema`.
fn chop_quad_at_extrema(src: [P; 3], y: bool) -> Vec<[P; 3]> {
    let (a, b, c) = (coord(src[0], y), coord(src[1], y), coord(src[2], y));
    let mut middle = b;
    if is_not_monotonic(a, b, c) {
        if let Some(t) = valid_unit_divide(a - b, a - b - b + c) {
            let mut d = chop_quad_at(src, t);
            let extremum = coord(d[2], y);
            set_coord(&mut d[1], y, extremum);
            set_coord(&mut d[3], y, extremum);
            return vec![[d[0], d[1], d[2]], [d[2], d[3], d[4]]];
        }
        middle = if (a - b).abs() < (b - c).abs() { a } else { c };
    }
    let mut out = src;
    set_coord(&mut out[1], y, middle);
    vec![out]
}

fn vline(x: f32, y0: f32, y1: f32, reverse: bool) -> Piece {
    let (y0, y1) = if reverse { (y1, y0) } else { (y0, y1) };
    Piece::Line([P { x, y: y0 }, P { x, y: y1 }])
}

fn quad(pts: [P; 3], reverse: bool) -> Piece {
    Piece::Quad(if reverse {
        [pts[2], pts[1], pts[0]]
    } else {
        pts
    })
}

fn chop_quad_in_y(pts: &mut [P; 3], clip: ClipRect) {
    if pts[0].y < clip.top {
        match chop_mono_quad_at(pts[0].y, pts[1].y, pts[2].y, clip.top) {
            Some(t) => {
                let mut tmp = chop_quad_at(*pts, t);
                tmp[2].y = clip.top;
                tmp[3].y = tmp[3].y.max(clip.top);
                pts[0] = tmp[2];
                pts[1] = tmp[3];
            }
            None => {
                for p in pts.iter_mut() {
                    p.y = p.y.max(clip.top);
                }
            }
        }
    }
    if pts[2].y > clip.bottom {
        match chop_mono_quad_at(pts[0].y, pts[1].y, pts[2].y, clip.bottom) {
            Some(t) => {
                let mut tmp = chop_quad_at(*pts, t);
                tmp[1].y = tmp[1].y.min(clip.bottom);
                tmp[2].y = clip.bottom;
                pts[1] = tmp[1];
                pts[2] = tmp[2];
            }
            None => {
                for p in pts.iter_mut() {
                    p.y = p.y.min(clip.bottom);
                }
            }
        }
    }
}

/// `SkEdgeClipper::clipMonoQuad` without culling to the right.
fn clip_mono_quad(src: [P; 3], clip: ClipRect, out: &mut Vec<Piece>) {
    let (mut pts, mut reverse) = if src[0].y > src[2].y {
        ([src[2], src[1], src[0]], true)
    } else {
        (src, false)
    };
    if pts[2].y <= clip.top || pts[0].y >= clip.bottom {
        return;
    }
    chop_quad_in_y(&mut pts, clip);
    if pts[0].x > pts[2].x {
        pts.swap(0, 2);
        reverse = !reverse;
    }
    if pts[2].x <= clip.left {
        out.push(vline(clip.left, pts[0].y, pts[2].y, reverse));
        return;
    }
    if pts[0].x >= clip.right {
        out.push(vline(clip.right, pts[0].y, pts[2].y, reverse));
        return;
    }
    if pts[0].x < clip.left {
        let Some(t) = chop_mono_quad_at(pts[0].x, pts[1].x, pts[2].x, clip.left) else {
            out.push(vline(clip.left, pts[0].y, pts[2].y, reverse));
            return;
        };
        let mut tmp = chop_quad_at(pts, t);
        out.push(vline(clip.left, tmp[0].y, tmp[2].y, reverse));
        tmp[2].x = clip.left;
        tmp[3].x = tmp[3].x.max(clip.left);
        pts[0] = tmp[2];
        pts[1] = tmp[3];
    }
    if pts[2].x > clip.right {
        match chop_mono_quad_at(pts[0].x, pts[1].x, pts[2].x, clip.right) {
            Some(t) => {
                let mut tmp = chop_quad_at(pts, t);
                tmp[1].x = tmp[1].x.min(clip.right);
                tmp[2].x = clip.right;
                out.push(quad([tmp[0], tmp[1], tmp[2]], reverse));
                out.push(vline(clip.right, tmp[2].y, tmp[4].y, reverse));
            }
            None => {
                pts[1].x = pts[1].x.min(clip.right);
                pts[2].x = pts[2].x.min(clip.right);
                out.push(quad(pts, reverse));
            }
        }
    } else {
        out.push(quad(pts, reverse));
    }
}

/// `SkEdgeClipper::clipQuad`: x- and y-monotonic quads inside `clip`, plus vertical lines on
/// its left and right edges for the parts outside.
pub fn clip_quad(src: [P; 3], clip: ClipRect, out: &mut Vec<Piece>) {
    let top = src[0].y.min(src[1].y).min(src[2].y);
    let bottom = src[0].y.max(src[1].y).max(src[2].y);
    if top >= clip.bottom || bottom <= clip.top {
        return;
    }
    for mono_y in chop_quad_at_extrema(src, true) {
        for mono in chop_quad_at_extrema(mono_y, false) {
            clip_mono_quad(mono, clip, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: ClipRect = ClipRect {
        left: 0.0,
        top: 0.0,
        right: 10.0,
        bottom: 10.0,
    };

    fn p(x: f32, y: f32) -> P {
        P { x, y }
    }

    #[test]
    fn lines_off_the_sides_become_edge_verticals() {
        let mut out = Vec::new();
        clip_line([p(-10.0, 0.0), p(20.0, 10.0)], CLIP, &mut out);
        let lines: Vec<[P; 2]> = out
            .into_iter()
            .map(|piece| match piece {
                Piece::Line(l) => l,
                Piece::Quad(_) => panic!("a line clips to lines"),
            })
            .collect();
        let (a, b) = ((10.0f64 / 3.0) as f32, (20.0f64 / 3.0) as f32);
        assert_eq!(
            lines,
            [
                [p(0.0, 0.0), p(0.0, a)],
                [p(0.0, a), p(10.0, b)],
                [p(10.0, b), p(10.0, 10.0)],
            ]
        );
    }

    #[test]
    fn intersect_line_keeps_direction() {
        let clipped = intersect_line([p(20.0, 5.0), p(-20.0, 5.0)], CLIP).unwrap();
        assert_eq!(clipped, [p(10.0, 5.0), p(0.0, 5.0)]);
        assert!(intersect_line([p(-5.0, 20.0), p(5.0, 30.0)], CLIP).is_none());
    }

    #[test]
    fn quads_are_cut_at_the_clip() {
        let mut out = Vec::new();
        clip_quad([p(-5.0, 15.0), p(5.0, -5.0), p(15.0, 15.0)], CLIP, &mut out);
        for piece in &out {
            let pts: &[P] = match piece {
                Piece::Line(l) => l,
                Piece::Quad(q) => q,
            };
            for q in pts {
                assert!(
                    (0.0..=10.0).contains(&q.x) && (0.0..=10.0).contains(&q.y),
                    "{q:?}"
                );
            }
        }
        assert!(out.iter().any(|piece| matches!(piece, Piece::Quad(_))));
    }
}
