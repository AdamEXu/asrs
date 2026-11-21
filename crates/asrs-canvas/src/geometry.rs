// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! Cubic helpers from Skia's `SkGeometry.cpp` with its f32 operation order (Chromium builds
//! with `-ffp-contract=off`, so nothing here may fuse into an FMA).

use crate::hairline::P;

fn p(x: f32, y: f32) -> P {
    P { x, y }
}

fn sub(a: P, b: P) -> P {
    p(a.x - b.x, a.y - b.y)
}

/// `SkTPin(x, lo, hi)`: `std::max(lo, std::min(x, hi))`, so NaN pins to `lo`.
fn pin(x: f32, lo: f32, hi: f32) -> f32 {
    let m = if hi < x { hi } else { x };
    if lo < m { m } else { lo }
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
    if r.is_nan() || r == 0.0 {
        return None;
    }
    Some(r)
}

/// `SkFindUnitQuadRoots`: the roots of `a t^2 + b t + c` strictly inside (0, 1), ascending.
pub fn find_unit_quad_roots(a: f32, b: f32, c: f32) -> Vec<f32> {
    if a == 0.0 {
        return valid_unit_divide(-c, b).into_iter().collect();
    }
    let dr = f64::from(b) * f64::from(b) - 4.0 * f64::from(a) * f64::from(c);
    if dr < 0.0 {
        return Vec::new();
    }
    let r = dr.sqrt() as f32;
    if !r.is_finite() {
        return Vec::new();
    }
    let q = if b < 0.0 {
        -(b - r) / 2.0
    } else {
        -(b + r) / 2.0
    };
    let mut roots: Vec<f32> = [valid_unit_divide(q, a), valid_unit_divide(c, q)]
        .into_iter()
        .flatten()
        .collect();
    if roots.len() == 2 {
        if roots[0] > roots[1] {
            roots.swap(0, 1);
        } else if roots[0] == roots[1] {
            roots.pop();
        }
    }
    roots
}

/// `SkCubicCoeff`: the cubic as `((A t + B) t + C) t + D`.
#[derive(Clone, Copy)]
pub struct CubicCoeff {
    pub a: P,
    pub b: P,
    pub c: P,
    pub d: P,
}

impl CubicCoeff {
    pub fn new(src: &[P; 4]) -> CubicCoeff {
        let [p0, p1, p2, p3] = *src;
        let f = |v0: f32, v1: f32, v2: f32, v3: f32| {
            (
                v3 + 3.0 * (v1 - v2) - v0,
                3.0 * (v2 - (v1 + v1) + v0),
                3.0 * (v1 - v0),
            )
        };
        let (ax, bx, cx) = f(p0.x, p1.x, p2.x, p3.x);
        let (ay, by, cy) = f(p0.y, p1.y, p2.y, p3.y);
        CubicCoeff {
            a: p(ax, ay),
            b: p(bx, by),
            c: p(cx, cy),
            d: p0,
        }
    }

    pub fn eval(&self, t: f32) -> P {
        p(
            ((self.a.x * t + self.b.x) * t + self.c.x) * t + self.d.x,
            ((self.a.y * t + self.b.y) * t + self.c.y) * t + self.d.y,
        )
    }
}

/// `eval_cubic_derivative`: a third of the derivative.
fn eval_cubic_derivative(src: &[P; 4], t: f32) -> P {
    let [p0, p1, p2, p3] = *src;
    let f = |v0: f32, v1: f32, v2: f32, v3: f32| {
        let a = v3 + 3.0 * (v1 - v2) - v0;
        let b2 = v2 - (v1 + v1) + v0;
        let b = b2 + b2;
        let c = v1 - v0;
        (a * t + b) * t + c
    };
    p(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y))
}

/// `SkEvalCubicAt` position.
pub fn eval_cubic_pos(src: &[P; 4], t: f32) -> P {
    CubicCoeff::new(src).eval(t)
}

/// `SkEvalCubicAt` tangent, with Skia's fallbacks for end points that equal their controls.
pub fn eval_cubic_tangent(src: &[P; 4], t: f32) -> P {
    if (t == 0.0 && src[0] == src[1]) || (t == 1.0 && src[2] == src[3]) {
        let mut tangent = if t == 0.0 {
            sub(src[2], src[0])
        } else {
            sub(src[3], src[1])
        };
        if tangent.x == 0.0 && tangent.y == 0.0 {
            tangent = sub(src[3], src[0]);
        }
        tangent
    } else {
        eval_cubic_derivative(src, t)
    }
}

fn mix(a: P, b: P, t: f32) -> P {
    p((b.x - a.x) * t + a.x, (b.y - a.y) * t + a.y)
}

/// `SkChopCubicAt(src, dst[7], t)`.
pub fn chop_cubic_at(src: &[P; 4], t: f32) -> [P; 7] {
    if t == 1.0 {
        return [src[0], src[1], src[2], src[3], src[3], src[3], src[3]];
    }
    let ab = mix(src[0], src[1], t);
    let bc = mix(src[1], src[2], t);
    let cd = mix(src[2], src[3], t);
    let abc = mix(ab, bc, t);
    let bcd = mix(bc, cd, t);
    let abcd = mix(abc, bcd, t);
    [src[0], ab, abc, abcd, bcd, cd, src[3]]
}

/// `SkChopCubicAt(src, dst[10], t0, t1)`: both chops computed from `src`, as Skia's SIMD does.
pub fn chop_cubic_at2(src: &[P; 4], t0: f32, t1: f32) -> [P; 10] {
    if t1 == 1.0 {
        let c = chop_cubic_at(src, t0);
        return [
            c[0], c[1], c[2], c[3], c[4], c[5], c[6], src[3], src[3], src[3],
        ];
    }
    let lane = |t: f32| {
        let ab = mix(src[0], src[1], t);
        let bc = mix(src[1], src[2], t);
        let cd = mix(src[2], src[3], t);
        let abc = mix(ab, bc, t);
        let bcd = mix(bc, cd, t);
        (ab, cd, abc, bcd, mix(abc, bcd, t))
    };
    let (ab0, _, abc0, bcd0, abcd0) = lane(t0);
    let (_, cd1, abc1, bcd1, abcd1) = lane(t1);
    [
        src[0],
        ab0,
        abc0,
        abcd0,
        mix(abc0, bcd0, t1),
        mix(abc1, bcd1, t0),
        abcd1,
        bcd1,
        cd1,
        src[3],
    ]
}

/// `SkChopCubicAt(src, dst, tValues, tCount)`: `3 * ts.len() + 4` points.
pub fn chop_cubic_at_ts(src: &[P; 4], ts: &[f32]) -> Vec<P> {
    let mut dst = src.to_vec();
    if ts.is_empty() {
        return dst;
    }
    dst.resize(3 * ts.len() + 4, p(0.0, 0.0));
    let mut base = 0;
    let mut i = 0;
    let cubic_at =
        |dst: &[P], base: usize| [dst[base], dst[base + 1], dst[base + 2], dst[base + 3]];
    while i + 1 < ts.len() {
        let (mut t0, mut t1) = (ts[i], ts[i + 1]);
        if i != 0 {
            let last = ts[i - 1];
            t0 = pin((t0 - last) / (1.0 - last), 0.0, 1.0);
            t1 = pin((t1 - last) / (1.0 - last), 0.0, 1.0);
        }
        let chopped = chop_cubic_at2(&cubic_at(&dst, base), t0, t1);
        dst[base..base + 10].copy_from_slice(&chopped);
        base += 6;
        i += 2;
    }
    if i < ts.len() {
        let mut t = ts[i];
        if i != 0 {
            let last = ts[i - 1];
            t = pin((t - last) / (1.0 - last), 0.0, 1.0);
        }
        let chopped = chop_cubic_at(&cubic_at(&dst, base), t);
        dst[base..base + 7].copy_from_slice(&chopped);
    }
    dst
}

/// `SkFindCubicInflections`.
pub fn find_cubic_inflections(src: &[P; 4]) -> Vec<f32> {
    let ax = src[1].x - src[0].x;
    let ay = src[1].y - src[0].y;
    let bx = src[2].x - 2.0 * src[1].x + src[0].x;
    let by = src[2].y - 2.0 * src[1].y + src[0].y;
    let cx = src[3].x + 3.0 * (src[1].x - src[2].x) - src[0].x;
    let cy = src[3].y + 3.0 * (src[1].y - src[2].y) - src[0].y;
    find_unit_quad_roots(bx * cy - by * cx, ax * cy - ay * cx, ax * by - ay * bx)
}

fn collapse_duplicates(values: &mut Vec<f32>) {
    let mut start = 0;
    let mut n = values.len();
    while n > 1 {
        if values[start] == values[start + 1] {
            values.remove(start);
        } else {
            start += 1;
        }
        n -= 1;
    }
}

/// `solve_cubic_poly`: the roots of `coeff[0] t^3 + ... + coeff[3]`, pinned to [0, 1].
fn solve_cubic_poly(coeff: [f32; 4]) -> Vec<f32> {
    if coeff[0].abs() <= 1.0 / 4096.0 {
        return find_unit_quad_roots(coeff[1], coeff[2], coeff[3]);
    }
    let inva = 1.0 / coeff[0];
    let a = coeff[1] * inva;
    let b = coeff[2] * inva;
    let c = coeff[3] * inva;
    let q = (a * a - b * 3.0) / 9.0;
    let r = (2.0 * a * a * a - 9.0 * a * b + 27.0 * c) / 54.0;
    let q3 = q * q * q;
    let r2_minus_q3 = r * r - q3;
    let adiv3 = a / 3.0;
    if r2_minus_q3 < 0.0 {
        let pi = std::f32::consts::PI;
        let theta = pin(r / q3.sqrt(), -1.0, 1.0).acos();
        let neg2_root_q = -2.0 * q.sqrt();
        let mut t = vec![
            pin(neg2_root_q * (theta / 3.0).cos() - adiv3, 0.0, 1.0),
            pin(
                neg2_root_q * ((theta + 2.0 * pi) / 3.0).cos() - adiv3,
                0.0,
                1.0,
            ),
            pin(
                neg2_root_q * ((theta - 2.0 * pi) / 3.0).cos() - adiv3,
                0.0,
                1.0,
            ),
        ];
        for i in (1..3).rev() {
            for j in (1..=i).rev() {
                if t[j] < t[j - 1] {
                    t.swap(j, j - 1);
                }
            }
        }
        collapse_duplicates(&mut t);
        t
    } else {
        let mut big_a = r.abs() + r2_minus_q3.sqrt();
        big_a = big_a.powf(0.333_333_3);
        if r > 0.0 {
            big_a = -big_a;
        }
        if big_a != 0.0 {
            big_a += q / big_a;
        }
        vec![pin(big_a - adiv3, 0.0, 1.0)]
    }
}

fn f1_dot_f2(v: [f32; 4]) -> [f32; 4] {
    let a = v[1] - v[0];
    let b = v[2] - 2.0 * v[1] + v[0];
    let c = v[3] + 3.0 * (v[1] - v[2]) - v[0];
    [c * c, 3.0 * b * c, 2.0 * b * b + c * a, a * b]
}

/// `SkFindCubicMaxCurvature`: up to three t values in [0, 1].
pub fn find_cubic_max_curvature(src: &[P; 4]) -> Vec<f32> {
    let cx = f1_dot_f2([src[0].x, src[1].x, src[2].x, src[3].x]);
    let cy = f1_dot_f2([src[0].y, src[1].y, src[2].y, src[3].y]);
    solve_cubic_poly([cx[0] + cy[0], cx[1] + cy[1], cx[2] + cy[2], cx[3] + cy[3]])
}

/// `SkChopCubicAtMaxCurvature`: the cubics between the interior max-curvature points.
pub fn chop_cubic_at_max_curvature(src: &[P; 4]) -> Vec<[P; 4]> {
    let ts: Vec<f32> = find_cubic_max_curvature(src)
        .into_iter()
        .filter(|&t| 0.0 < t && t < 1.0)
        .collect();
    let pts = chop_cubic_at_ts(src, &ts);
    (0..=ts.len())
        .map(|i| [pts[3 * i], pts[3 * i + 1], pts[3 * i + 2], pts[3 * i + 3]])
        .collect()
}

fn distance_sqd(a: P, b: P) -> f32 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    dx * dx + dy * dy
}

fn on_same_side(src: &[P; 4], test: usize, line: usize) -> bool {
    let origin = src[line];
    let l = sub(src[line + 1], origin);
    let cross = |i: usize| {
        let v = sub(src[test + i], origin);
        l.x * v.y - l.y * v.x
    };
    cross(0) * cross(1) >= 0.0
}

/// `SkFindCubicCusp`: the t of a cusp, or -1.
pub fn find_cubic_cusp(src: &[P; 4]) -> f32 {
    if src[0] == src[1] || src[2] == src[3] {
        return -1.0;
    }
    if on_same_side(src, 0, 2) || on_same_side(src, 2, 0) {
        return -1.0;
    }
    for t in find_cubic_max_curvature(src) {
        if 0.0 >= t || t >= 1.0 {
            continue;
        }
        let d = eval_cubic_derivative(src, t);
        let magnitude = d.x * d.x + d.y * d.y;
        let precision = (distance_sqd(src[1], src[0])
            + distance_sqd(src[2], src[1])
            + distance_sqd(src[3], src[2]))
            * 1e-8;
        if magnitude < precision {
            return t;
        }
    }
    -1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chop_halves_share_the_midpoint() {
        let c = [p(0.0, 0.0), p(10.0, 20.0), p(30.0, 20.0), p(40.0, 0.0)];
        let h = chop_cubic_at(&c, 0.5);
        assert_eq!(h[3], eval_cubic_pos(&c, 0.5));
        let two = chop_cubic_at2(&c, 0.25, 0.75);
        let pieces = chop_cubic_at_ts(&c, &[0.25, 0.75]);
        assert_eq!(&pieces[..], &two[..]);
        assert_eq!(pieces[9], c[3]);
    }

    #[test]
    fn symmetric_cusp_is_found_at_its_middle() {
        let c = [p(0.0, 0.0), p(100.0, 100.0), p(0.0, 100.0), p(100.0, 0.0)];
        let t = find_cubic_cusp(&c);
        assert!((t - 0.5).abs() < 1e-3, "{t}");
        let s = [p(0.0, 0.0), p(10.0, 20.0), p(30.0, 20.0), p(40.0, 0.0)];
        assert_eq!(find_cubic_cusp(&s), -1.0);
    }

    #[test]
    fn s_curve_has_one_inflection() {
        let c = [p(0.0, 0.0), p(30.0, 40.0), p(10.0, -40.0), p(40.0, 0.0)];
        let t = find_cubic_inflections(&c);
        assert_eq!(t.len(), 1);
        assert!((t[0] - 0.5).abs() < 1e-3);
    }
}
