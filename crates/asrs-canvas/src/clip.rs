//! Skia's `SkLineClipper` and the curve half of `SkEdgeClipper`: device geometry is cut to the
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

#[derive(Clone, Copy)]
pub enum Piece {
    Line([P; 2]),
    Quad([P; 3]),
    Cubic([P; 4]),
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
    clip_line_cull(pts, clip, false, out);
}

/// `SkLineClipper::ClipLine`; with `cull_right`, a segment wholly right of `clip` is dropped.
pub fn clip_line_cull(pts: [P; 2], clip: ClipRect, cull_right: bool, out: &mut Vec<Piece>) {
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
    if tmp[i0].x >= clip.right && tmp[i1].x > clip.left && cull_right {
        return;
    }
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

/// `SkFindUnitQuadRoots`: the roots in (0, 1) of `a t^2 + b t + c`, ascending.
fn find_unit_quad_roots(a: f32, b: f32, c: f32) -> ([f32; 2], usize) {
    let mut roots = [0.0f32; 2];
    if a == 0.0 {
        return match valid_unit_divide(-c, b) {
            Some(r) => ([r, 0.0], 1),
            None => (roots, 0),
        };
    }
    let dr = f64::from(b) * f64::from(b) - 4.0 * f64::from(a) * f64::from(c);
    if dr < 0.0 {
        return (roots, 0);
    }
    let r = dr.sqrt() as f32;
    if !r.is_finite() {
        return (roots, 0);
    }
    let q = if b < 0.0 {
        -(b - r) / 2.0
    } else {
        -(b + r) / 2.0
    };
    let mut n = 0;
    for root in [valid_unit_divide(q, a), valid_unit_divide(c, q)]
        .into_iter()
        .flatten()
    {
        roots[n] = root;
        n += 1;
    }
    if n == 2 {
        if roots[0] > roots[1] {
            roots.swap(0, 1);
        } else if roots[0] == roots[1] {
            n = 1;
        }
    }
    (roots, n)
}

fn first_unit_quad_root(a: f32, b: f32, c: f32) -> Option<f32> {
    let (roots, n) = find_unit_quad_roots(a, b, c);
    (n > 0).then_some(roots[0])
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

pub fn chop_quad_at_y_extrema(src: [P; 3]) -> Vec<[P; 3]> {
    chop_quad_at_extrema(src, true)
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
fn clip_mono_quad(src: [P; 3], clip: ClipRect, cull_right: bool, out: &mut Vec<Piece>) {
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
        if !cull_right {
            out.push(vline(clip.right, pts[0].y, pts[2].y, reverse));
        }
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
    clip_quad_cull(src, clip, false, out);
}

pub fn clip_quad_cull(src: [P; 3], clip: ClipRect, cull_right: bool, out: &mut Vec<Piece>) {
    let top = src[0].y.min(src[1].y).min(src[2].y);
    let bottom = src[0].y.max(src[1].y).max(src[2].y);
    if top >= clip.bottom || bottom <= clip.top {
        return;
    }
    for mono_y in chop_quad_at_extrema(src, true) {
        for mono in chop_quad_at_extrema(mono_y, false) {
            clip_mono_quad(mono, clip, cull_right, out);
        }
    }
}

/// `SkFindCubicExtrema`.
fn find_cubic_extrema(a: f32, b: f32, c: f32, d: f32) -> ([f32; 2], usize) {
    let qa = d - a + 3.0 * (b - c);
    let qb = 2.0 * (a - b - b + c);
    let qc = b - a;
    find_unit_quad_roots(qa, qb, qc)
}

fn unchecked_mix(a: P, b: P, t: f32) -> P {
    P {
        x: (b.x - a.x) * t + a.x,
        y: (b.y - a.y) * t + a.y,
    }
}

/// `SkChopCubicAt` at one `t`.
fn chop_cubic_at(src: [P; 4], t: f32) -> [P; 7] {
    if t == 1.0 {
        return [src[0], src[1], src[2], src[3], src[3], src[3], src[3]];
    }
    let ab = unchecked_mix(src[0], src[1], t);
    let bc = unchecked_mix(src[1], src[2], t);
    let cd = unchecked_mix(src[2], src[3], t);
    let abc = unchecked_mix(ab, bc, t);
    let bcd = unchecked_mix(bc, cd, t);
    let abcd = unchecked_mix(abc, bcd, t);
    [src[0], ab, abc, abcd, bcd, cd, src[3]]
}

/// `SkChopCubicAt` at two `t`s, which Skia evaluates side by side in one SIMD register.
fn chop_cubic_at2(src: [P; 4], t0: f32, t1: f32) -> [P; 10] {
    if t1 == 1.0 {
        let d = chop_cubic_at(src, t0);
        return [d[0], d[1], d[2], d[3], d[4], d[5], d[6], src[3], src[3], src[3]];
    }
    let at = |t: f32| {
        let ab = unchecked_mix(src[0], src[1], t);
        let bc = unchecked_mix(src[1], src[2], t);
        let cd = unchecked_mix(src[2], src[3], t);
        let abc = unchecked_mix(ab, bc, t);
        let bcd = unchecked_mix(bc, cd, t);
        (ab, bc, cd, abc, bcd, unchecked_mix(abc, bcd, t))
    };
    let (ab0, _, _, abc0, bcd0, abcd0) = at(t0);
    let (_, _, cd1, abc1, bcd1, abcd1) = at(t1);
    [
        src[0],
        ab0,
        abc0,
        abcd0,
        unchecked_mix(abc0, bcd0, t1),
        unchecked_mix(abc1, bcd1, t0),
        abcd1,
        bcd1,
        cd1,
        src[3],
    ]
}

/// `SkChopCubicAtYExtrema` (`y`) or `SkChopCubicAtXExtrema`.
fn chop_cubic_at_extrema(src: [P; 4], y: bool) -> Vec<[P; 4]> {
    let c = src.map(|p| coord(p, y));
    let (t, n) = find_cubic_extrema(c[0], c[1], c[2], c[3]);
    let mut dst = [src[0]; 10];
    match n {
        0 => return vec![src],
        1 => dst[..7].copy_from_slice(&chop_cubic_at(src, t[0])),
        _ => dst = chop_cubic_at2(src, t[0], t[1]),
    }
    for i in 0..n {
        let extremum = coord(dst[i * 3 + 3], y);
        set_coord(&mut dst[i * 3 + 2], y, extremum);
        set_coord(&mut dst[i * 3 + 4], y, extremum);
    }
    (0..=n)
        .map(|i| [dst[i * 3], dst[i * 3 + 1], dst[i * 3 + 2], dst[i * 3 + 3]])
        .collect()
}

pub fn chop_cubic_at_y_extrema(src: [P; 4]) -> Vec<[P; 4]> {
    chop_cubic_at_extrema(src, true)
}

/// `sk_double_nearly_zero`.
fn double_nearly_zero(a: f64) -> bool {
    a == 0.0 || a.abs() < f64::from(f32::EPSILON)
}

fn magnitude(a: f64) -> f64 {
    f64::from_bits(a.to_bits() & 0x7FF0_0000_0000_0000)
}

/// `sk_doubles_nearly_equal_ulps` with the default 16 ulps.
fn doubles_nearly_equal_ulps(a: f64, b: f64) -> bool {
    let max_magnitude = magnitude(a).max(f64::MIN_POSITIVE).max(magnitude(b));
    let tolerance = max_magnitude * (f64::EPSILON * 17.0);
    a == b || (b - a).abs() < tolerance
}

fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

fn std_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `SkQuads::Discriminant`.
fn quad_discriminant(a: f64, b: f64, c: f64) -> f64 {
    let b2 = b * b;
    let ac = a * c;
    let rough = b2 - ac;
    if 3.0 * rough.abs() >= b2 + ac {
        return rough;
    }
    let b2_error = b.mul_add(b, -b2);
    let ac_error = a.mul_add(c, -ac);
    (b2 - ac) + (b2_error - ac_error)
}

/// `SkQuads::RootsReal`.
fn quad_roots_real(a: f64, b: f64, c: f64, solution: &mut [f64; 2]) -> usize {
    let close_to_linear = a == 0.0 || (b / a).abs() >= 1.0e16;
    if close_to_linear {
        if double_nearly_zero(b) {
            solution[0] = 0.0;
            return usize::from(double_nearly_zero(c));
        }
        solution[0] = -c / b;
        return usize::from(solution[0].is_finite());
    }
    let half_b = -0.5 * b;
    let discriminant = quad_discriminant(a, half_b, c);
    let (root0, root1) = if discriminant == 0.0 {
        (half_b / a, half_b / a)
    } else if discriminant > 0.0 {
        let d = discriminant.sqrt();
        let r = if half_b > 0.0 { half_b + d } else { half_b - d };
        (r / a, c / r)
    } else {
        (f64::NAN, f64::NAN)
    };
    if !discriminant.is_finite() || discriminant < 0.0 {
        return 0;
    }
    let zero_if_tiny = |x: f64| if double_nearly_zero(x) { 0.0 } else { x };
    let mut n = 0;
    for r in [zero_if_tiny(root0), zero_if_tiny(root1)] {
        if r.is_finite() {
            solution[n] = r;
            n += 1;
        }
    }
    if n == 2 && doubles_nearly_equal_ulps(solution[0], solution[1]) {
        n = 1;
    }
    n
}

/// `SkCubics::RootsReal`.
fn cubic_roots_real(a: f64, b: f64, c: f64, d: f64, solution: &mut [f64; 3]) -> usize {
    let close_to_a_quadratic = if double_nearly_zero(b) {
        double_nearly_zero(a)
    } else {
        (a / b).abs() < 1.0e-7
    };
    let mut two = [0.0f64; 2];
    if close_to_a_quadratic {
        let n = quad_roots_real(b, c, d, &mut two);
        solution[..2].copy_from_slice(&two);
        return n;
    }
    if double_nearly_zero(d) {
        let mut n = quad_roots_real(a, b, c, &mut two);
        solution[..2].copy_from_slice(&two);
        if solution[..n].iter().any(|&s| double_nearly_zero(s)) {
            return n;
        }
        solution[n] = 0.0;
        n += 1;
        return n;
    }
    if double_nearly_zero(a + b + c + d) {
        let mut n = quad_roots_real(a, a + b, -d, &mut two);
        solution[..2].copy_from_slice(&two);
        if solution[..n]
            .iter()
            .any(|&s| doubles_nearly_equal_ulps(s, 1.0))
        {
            return n;
        }
        solution[n] = 1.0;
        n += 1;
        return n;
    }
    let inv_a = 1.0 / a;
    let (a, b, c) = (b * inv_a, c * inv_a, d * inv_a);
    let a2 = a * a;
    let q = (a2 - b * 3.0) / 9.0;
    let r = (2.0 * a2 * a - 9.0 * a * b + 27.0 * c) / 54.0;
    let r2 = r * r;
    let q3 = q * q * q;
    let r2_minus_q3 = r2 - q3;
    if !r2_minus_q3.is_finite() {
        return 0;
    }
    let adiv3 = a / 3.0;
    let nearly_equal = |x: f64, y: f64| {
        if double_nearly_zero(x) {
            double_nearly_zero(y)
        } else {
            doubles_nearly_equal_ulps(x, y)
        }
    };
    let mut n = 0;
    if r2_minus_q3 < 0.0 {
        let theta = std_max(-1.0, std_min(r / q3.sqrt(), 1.0)).acos();
        let neg2_root_q = -2.0 * q.sqrt();
        solution[n] = neg2_root_q * (theta / 3.0).cos() - adiv3;
        n += 1;
        let root = neg2_root_q * ((theta + 2.0 * std::f64::consts::PI) / 3.0).cos() - adiv3;
        if !nearly_equal(solution[0], root) {
            solution[n] = root;
            n += 1;
        }
        let root = neg2_root_q * ((theta - 2.0 * std::f64::consts::PI) / 3.0).cos() - adiv3;
        if !nearly_equal(solution[0], root) && (n == 1 || !nearly_equal(solution[1], root)) {
            solution[n] = root;
            n += 1;
        }
    } else {
        let mut big_a = (r.abs() + r2_minus_q3.sqrt()).cbrt();
        if r > 0.0 {
            big_a = -big_a;
        }
        if !double_nearly_zero(big_a) {
            big_a += q / big_a;
        }
        solution[n] = big_a - adiv3;
        n += 1;
        if !double_nearly_zero(r2) && doubles_nearly_equal_ulps(r2, q3) {
            let root = -big_a / 2.0 - adiv3;
            if !nearly_equal(solution[0], root) {
                solution[n] = root;
                n += 1;
            }
        }
    }
    n
}

/// `SkCubics::RootsValidT`.
fn cubic_roots_valid_t(a: f64, b: f64, c: f64, d: f64, solution: &mut [f64; 3]) -> usize {
    let mut all = [0.0f64; 3];
    let real = cubic_roots_real(a, b, c, d, &mut all);
    let mut found = 0;
    for &t in &all[..real] {
        if t <= 1.00005 && (t >= 1.0 || doubles_nearly_equal_ulps(t, 1.0)) {
            if (found < 1 || !doubles_nearly_equal_ulps(solution[0], 1.0))
                && (found < 2 || !doubles_nearly_equal_ulps(solution[1], 1.0))
            {
                solution[found] = 1.0;
                found += 1;
            }
        } else if t >= -0.00005 && (t <= 0.0 || double_nearly_zero(t)) {
            if (found < 1 || !double_nearly_zero(solution[0]))
                && (found < 2 || !double_nearly_zero(solution[1]))
            {
                solution[found] = 0.0;
                found += 1;
            }
        } else if t > 0.0 && t < 1.0 {
            solution[found] = t;
            found += 1;
        }
    }
    found
}

/// `SkCubics::EvalAt`.
fn cubic_eval_at(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    t.mul_add(t.mul_add(t.mul_add(a, b), c), d)
}

fn approximately_zero(x: f64) -> bool {
    x.abs() < 0.000_000_01
}

fn cubic_binary_search(a: f64, b: f64, c: f64, d: f64, mut start: f64, mut stop: f64) -> f64 {
    let left = cubic_eval_at(a, b, c, d, start);
    if approximately_zero(left) {
        return start;
    }
    let right = cubic_eval_at(a, b, c, d, stop);
    if !(left.is_finite() && right.is_finite()) {
        return -1.0;
    }
    if (left > 0.0 && right > 0.0) || (left < 0.0 && right < 0.0) {
        return -1.0;
    }
    for _ in 0..1000 {
        let step = (start + stop) / 2.0;
        let curr = cubic_eval_at(a, b, c, d, step);
        if approximately_zero(curr) {
            return step;
        }
        if (curr < 0.0 && left < 0.0) || (curr > 0.0 && left > 0.0) {
            start = step;
        } else {
            stop = step;
        }
    }
    -1.0
}

/// `SkCubics::BinarySearchRootsValidT`.
fn cubic_binary_search_roots_valid_t(
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    solution: &mut [f64; 3],
) -> usize {
    if !(a.is_finite() && b.is_finite() && c.is_finite() && d.is_finite()) {
        return 0;
    }
    let mut regions = [0.0, 0.0, 0.0, 1.0];
    let mut roots = [0.0f64; 2];
    let num = quad_roots_real(3.0 * a, 2.0 * b, c, &mut roots);
    let mut min_max = [0.0f64; 2];
    let mut extrema = 0;
    for &t in &roots[..num] {
        if (0.0..=1.0).contains(&t) {
            min_max[extrema] = t;
            extrema += 1;
        }
    }
    let mut start = 2 - extrema;
    if extrema == 1 {
        regions[start + 1] = min_max[0];
    }
    if extrema == 2 {
        regions[start + 1] = std_min(min_max[0], min_max[1]);
        regions[start + 2] = std_max(min_max[0], min_max[1]);
    }
    let mut found = 0;
    while start < 3 {
        let root = cubic_binary_search(a, b, c, d, regions[start], regions[start + 1]);
        if root >= 0.0
            && (found < 1 || !approximately_zero(solution[0] - root))
            && (found < 2 || !approximately_zero(solution[1] - root))
        {
            solution[found] = root;
            found += 1;
        }
        start += 1;
    }
    found
}

/// `first_axis_intersection` of `SkGeometry.cpp`.
fn first_axis_intersection(p: [f64; 4], intercept: f64) -> Option<f64> {
    let a = -p[0] + 3.0 * p[1] - 3.0 * p[2] + p[3];
    let b = 3.0 * p[0] - 6.0 * p[1] + 3.0 * p[2];
    let c = -3.0 * p[0] + 3.0 * p[1];
    let d = p[0] - intercept;
    let mut roots = [0.0f64; 3];
    let accurate = |roots: &[f64]| {
        roots
            .iter()
            .copied()
            .find(|&t| cubic_eval_at(a, b, c, d, t).abs() < 0.00001)
    };
    let n = cubic_roots_valid_t(a, b, c, d, &mut roots);
    if n == 0 {
        return None;
    }
    if let Some(t) = accurate(&roots[..n]) {
        return Some(t);
    }
    let n = cubic_binary_search_roots_valid_t(a, b, c, d, &mut roots);
    accurate(&roots[..n])
}

/// `SkChopMonoCubicAtY` (`y`) or `SkChopMonoCubicAtX`, falling back to `mono_cubic_closestT`
/// as `SkEdgeClipper` does.
fn chop_mono_cubic_at(src: [P; 4], y: bool, value: f32) -> [P; 7] {
    let c = src.map(|p| f64::from(coord(p, y)));
    if let Some(t) = first_axis_intersection(c, f64::from(value)) {
        let interp = |a: f64, b: f64| a + (b - a) * t;
        let sub = |v: [f64; 4]| {
            let (v01, v12, v23) = (interp(v[0], v[1]), interp(v[1], v[2]), interp(v[2], v[3]));
            let (a2, b1) = (interp(v01, v12), interp(v12, v23));
            [v[0], v01, a2, interp(a2, b1), b1, v23, v[3]]
        };
        let xs = sub(src.map(|p| f64::from(p.x)));
        let ys = sub(src.map(|p| f64::from(p.y)));
        return std::array::from_fn(|i| P {
            x: xs[i] as f32,
            y: ys[i] as f32,
        });
    }
    chop_cubic_at(src, mono_cubic_closest_t(src.map(|p| coord(p, y)), value))
}

fn mono_cubic_closest_t(src: [f32; 4], x: f32) -> f32 {
    let mut t = 0.5f32;
    let mut best_t = 0.0f32;
    let mut step = 0.25f32;
    let d = src[0];
    let a = src[3] + 3.0 * (src[1] - src[2]) - d;
    let b = 3.0 * (src[2] - src[1] - src[1] + d);
    let c = 3.0 * (src[1] - d);
    let x = x - d;
    let mut closest = f32::MAX;
    loop {
        let loc = ((a * t + b) * t + c) * t;
        let dist = (loc - x).abs();
        if closest > dist {
            closest = dist;
            best_t = t;
        }
        let last_t = t;
        t += if loc < x { step } else { -step };
        step *= 0.5;
        if !(closest > 0.25 && last_t != t) {
            return best_t;
        }
    }
}

fn clamp_ge(v: &mut f32, min: f32) {
    if *v < min {
        *v = min;
    }
}

fn clamp_le(v: &mut f32, max: f32) {
    if *v > max {
        *v = max;
    }
}

fn chop_cubic_in_y(pts: &mut [P; 4], clip: ClipRect) {
    if pts[0].y < clip.top {
        let mut tmp = chop_mono_cubic_at(*pts, true, clip.top);
        if tmp[3].y < clip.top && tmp[4].y < clip.top && tmp[5].y < clip.top {
            tmp = chop_mono_cubic_at([tmp[3], tmp[4], tmp[5], tmp[6]], true, clip.top);
        }
        tmp[3].y = clip.top;
        clamp_ge(&mut tmp[4].y, clip.top);
        pts[0] = tmp[3];
        pts[1] = tmp[4];
        pts[2] = tmp[5];
    }
    if pts[3].y > clip.bottom {
        let mut tmp = chop_mono_cubic_at(*pts, true, clip.bottom);
        tmp[3].y = clip.bottom;
        clamp_le(&mut tmp[2].y, clip.bottom);
        pts[1] = tmp[1];
        pts[2] = tmp[2];
        pts[3] = tmp[3];
    }
}

fn cubic(pts: [P; 4], reverse: bool) -> Piece {
    Piece::Cubic(if reverse {
        [pts[3], pts[2], pts[1], pts[0]]
    } else {
        pts
    })
}

/// `SkEdgeClipper::clipMonoCubic`.
fn clip_mono_cubic(src: [P; 4], clip: ClipRect, cull_right: bool, out: &mut Vec<Piece>) {
    let (mut pts, mut reverse) = if src[0].y > src[3].y {
        ([src[3], src[2], src[1], src[0]], true)
    } else {
        (src, false)
    };
    if pts[3].y <= clip.top || pts[0].y >= clip.bottom {
        return;
    }
    chop_cubic_in_y(&mut pts, clip);
    if pts[0].x > pts[3].x {
        pts.swap(0, 3);
        pts.swap(1, 2);
        reverse = !reverse;
    }
    if pts[3].x <= clip.left {
        out.push(vline(clip.left, pts[0].y, pts[3].y, reverse));
        return;
    }
    if pts[0].x >= clip.right {
        if !cull_right {
            out.push(vline(clip.right, pts[0].y, pts[3].y, reverse));
        }
        return;
    }
    if pts[0].x < clip.left {
        let mut tmp = chop_mono_cubic_at(pts, false, clip.left);
        out.push(vline(clip.left, tmp[0].y, tmp[3].y, reverse));
        tmp[3].x = clip.left;
        clamp_ge(&mut tmp[4].x, clip.left);
        pts[0] = tmp[3];
        pts[1] = tmp[4];
        pts[2] = tmp[5];
    }
    if pts[3].x > clip.right {
        let mut tmp = chop_mono_cubic_at(pts, false, clip.right);
        tmp[3].x = clip.right;
        clamp_le(&mut tmp[2].x, clip.right);
        out.push(cubic([tmp[0], tmp[1], tmp[2], tmp[3]], reverse));
        out.push(vline(clip.right, tmp[3].y, tmp[6].y, reverse));
    } else {
        out.push(cubic(pts, reverse));
    }
}

/// `SkEdgeClipper::clipCubic`. Cubics reaching past 2^22 become their chord, as in Skia.
pub fn clip_cubic_cull(src: [P; 4], clip: ClipRect, cull_right: bool, out: &mut Vec<Piece>) {
    let (mut l, mut t, mut r, mut b) = (src[0].x, src[0].y, src[0].x, src[0].y);
    for p in &src[1..] {
        l = l.min(p.x);
        t = t.min(p.y);
        r = r.max(p.x);
        b = b.max(p.y);
    }
    if !src.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
        (l, t, r, b) = (0.0, 0.0, 0.0, 0.0);
    }
    if !(b > clip.top && t < clip.bottom) {
        return;
    }
    let limit = (1 << 22) as f32;
    if l < -limit || t < -limit || r > limit || b > limit {
        clip_line_cull([src[0], src[3]], clip, cull_right, out);
        return;
    }
    for mono_y in chop_cubic_at_extrema(src, true) {
        for mono in chop_cubic_at_extrema(mono_y, false) {
            clip_mono_cubic(mono, clip, cull_right, out);
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
                Piece::Quad(_) | Piece::Cubic(_) => panic!("a line clips to lines"),
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
                Piece::Cubic(c) => c,
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

    fn monotonic(v: [f32; 4]) -> bool {
        v.windows(2).all(|w| w[0] <= w[1]) || v.windows(2).all(|w| w[0] >= w[1])
    }

    #[test]
    fn cubics_chop_into_monotonic_pieces() {
        let src = [p(0.0, 0.0), p(30.0, 40.0), p(-20.0, -30.0), p(10.0, 5.0)];
        let pieces = chop_cubic_at_y_extrema(src);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0][0], src[0]);
        assert_eq!(pieces[2][3], src[3]);
        for w in pieces.windows(2) {
            assert_eq!(w[0][3], w[1][0]);
        }
        for piece in &pieces {
            assert!(monotonic(piece.map(|q| q.y)), "{piece:?}");
            for mono in chop_cubic_at_extrema(*piece, false) {
                assert!(monotonic(mono.map(|q| q.x)), "{mono:?}");
                assert!(monotonic(mono.map(|q| q.y)), "{mono:?}");
            }
        }
    }

    #[test]
    fn cubics_are_cut_at_the_clip() {
        let mut out = Vec::new();
        clip_cubic_cull(
            [p(-5.0, 15.0), p(0.0, -10.0), p(10.0, -10.0), p(15.0, 15.0)],
            CLIP,
            false,
            &mut out,
        );
        assert!(out.iter().any(|piece| matches!(piece, Piece::Cubic(_))));
        for piece in &out {
            let pts: &[P] = match piece {
                Piece::Line(l) => l,
                Piece::Quad(q) => q,
                Piece::Cubic(c) => c,
            };
            for q in pts {
                assert!(
                    (0.0..=10.0).contains(&q.x) && (0.0..=10.0).contains(&q.y),
                    "{q:?}"
                );
            }
        }
        let mut culled = Vec::new();
        clip_cubic_cull(
            [p(12.0, 1.0), p(20.0, 3.0), p(20.0, 6.0), p(12.0, 9.0)],
            CLIP,
            true,
            &mut culled,
        );
        assert!(culled.is_empty());
    }

    #[test]
    fn huge_cubics_clip_as_their_chord() {
        let mut out = Vec::new();
        clip_cubic_cull(
            [p(-1.0e7, -1.0e7), p(0.0, 5.0), p(5.0, 0.0), p(1.0e7, 1.0e7)],
            CLIP,
            false,
            &mut out,
        );
        assert!(out.iter().all(|piece| matches!(piece, Piece::Line(_))));
        assert!(!out.is_empty());
    }
}
