//! Skia's stroker (`SkStroke.cpp`, `SkStrokerPriv.cpp`) for lines, quads, conics and
//! cubics. Round caps and joins are emitted as conics, which the scan converter later splits
//! into quads in device space, exactly as Chromium does; tiny-skia's port splits them in user
//! space and so produces different curves.

use tiny_skia::{LineCap, LineJoin};

use crate::geometry::{self, find_unit_quad_roots};
use crate::hairline::P;
use crate::skpath::{SkPath, Verb};

const NEARLY_ZERO: f32 = 1.0 / 4096.0;
const ROOT2_OVER2: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn p(x: f32, y: f32) -> P {
    P { x, y }
}

fn add(a: P, b: P) -> P {
    p(a.x + b.x, a.y + b.y)
}

fn sub(a: P, b: P) -> P {
    p(a.x - b.x, a.y - b.y)
}

fn scale(a: P, s: f32) -> P {
    p(a.x * s, a.y * s)
}

fn dot(a: P, b: P) -> f32 {
    a.x * b.x + a.y * b.y
}

fn cross(a: P, b: P) -> f32 {
    a.x * b.y - a.y * b.x
}

fn is_zero(v: P) -> bool {
    v.x == 0.0 && v.y == 0.0
}

fn can_normalize(v: P) -> bool {
    v.x.is_finite() && v.y.is_finite() && (v.x != 0.0 || v.y != 0.0)
}

/// `SkPoint::setLength`, computed in double like Skia.
fn set_length(v: P, length: f32) -> Option<P> {
    let (xx, yy) = (f64::from(v.x), f64::from(v.y));
    let dscale = f64::from(length) / (xx * xx + yy * yy).sqrt();
    let (x, y) = ((xx * dscale) as f32, (yy * dscale) as f32);
    if !(x.is_finite() && y.is_finite()) || (x == 0.0 && y == 0.0) {
        return None;
    }
    Some(p(x, y))
}

fn rotate_ccw(v: P) -> P {
    p(v.y, -v.x)
}

fn rotate_cw(v: P) -> P {
    p(-v.y, v.x)
}

fn distance_sqd(a: P, b: P) -> f32 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    dx * dx + dy * dy
}

fn quad_eval(src: &[P; 3], t: f32) -> P {
    let c = src[0];
    let b = p((src[1].x - c.x) * 2.0, (src[1].y - c.y) * 2.0);
    let a = p(
        src[2].x - src[1].x * 2.0 + c.x,
        src[2].y - src[1].y * 2.0 + c.y,
    );
    p((a.x * t + b.x) * t + c.x, (a.y * t + b.y) * t + c.y)
}

fn quad_tangent(src: &[P; 3], t: f32) -> P {
    if (t == 0.0 && src[0] == src[1]) || (t == 1.0 && src[1] == src[2]) {
        return sub(src[2], src[0]);
    }
    let b = sub(src[1], src[0]);
    let a = sub(sub(src[2], src[1]), b);
    let tt = p(a.x * t + b.x, a.y * t + b.y);
    add(tt, tt)
}

#[derive(Clone, Copy)]
struct Conic {
    pts: [P; 3],
    w: f32,
}

impl Conic {
    fn eval(&self, t: f32) -> P {
        let [p0, p1, p2] = self.pts;
        let w = self.w;
        let p1w = scale(p1, w);
        let nc = p0;
        let na = p(p2.x - p1w.x * 2.0 + p0.x, p2.y - p1w.y * 2.0 + p0.y);
        let nb = p((p1w.x - p0.x) * 2.0, (p1w.y - p0.y) * 2.0);
        let dc = 1.0f32;
        let db = (w - dc) * 2.0;
        let da = 0.0 - db;
        let numer = p((na.x * t + nb.x) * t + nc.x, (na.y * t + nb.y) * t + nc.y);
        let denom = (da * t + db) * t + dc;
        p(numer.x / denom, numer.y / denom)
    }

    fn tangent(&self, t: f32) -> P {
        let [p0, p1, p2] = self.pts;
        if (t == 0.0 && p0 == p1) || (t == 1.0 && p1 == p2) {
            return sub(p2, p0);
        }
        let p20 = sub(p2, p0);
        let p10 = sub(p1, p0);
        let c = scale(p10, self.w);
        let a = sub(scale(p20, self.w), p20);
        let b = sub(sub(p20, c), c);
        p((a.x * t + b.x) * t + c.x, (a.y * t + b.y) * t + c.y)
    }
}

fn find_quad_max_curvature(src: &[P; 3]) -> f32 {
    let ax = src[1].x - src[0].x;
    let ay = src[1].y - src[0].y;
    let bx = src[0].x - src[1].x - src[1].x + src[2].x;
    let by = src[0].y - src[1].y - src[1].y + src[2].y;
    let mut numer = -(ax * bx + ay * by);
    let mut denom = bx * bx + by * by;
    if denom < 0.0 {
        numer = -numer;
        denom = -denom;
    }
    if numer <= 0.0 {
        return 0.0;
    }
    if numer >= denom {
        return 1.0;
    }
    numer / denom
}

#[derive(Clone, Copy, Default)]
struct QuadConstruct {
    quad: [P; 3],
    tangent_start: P,
    tangent_end: P,
    start_t: f32,
    mid_t: f32,
    end_t: f32,
    start_set: bool,
    end_set: bool,
    opposite_tangents: bool,
}

impl QuadConstruct {
    fn init(&mut self, start: f32, end: f32) -> bool {
        self.start_t = start;
        self.mid_t = (start + end) * 0.5;
        self.end_t = end;
        self.start_set = false;
        self.end_set = false;
        self.start_t < self.mid_t && self.mid_t < self.end_t
    }

    fn init_with_start(&mut self, parent: &QuadConstruct) -> bool {
        if !self.init(parent.start_t, parent.mid_t) {
            return false;
        }
        self.quad[0] = parent.quad[0];
        self.tangent_start = parent.tangent_start;
        self.start_set = true;
        true
    }

    fn init_with_end(&mut self, parent: &QuadConstruct) -> bool {
        if !self.init(parent.mid_t, parent.end_t) {
            return false;
        }
        self.quad[2] = parent.quad[2];
        self.tangent_end = parent.tangent_end;
        self.end_set = true;
        true
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ResultType {
    Split,
    Degenerate,
    Quad,
}

#[derive(Clone, Copy, PartialEq)]
enum Reduction {
    Point,
    Line,
    Quad,
    Degenerate(P),
}

impl SkPath {
    fn set_last_pt(&mut self, pt: P) {
        match self.pts.last_mut() {
            Some(last) => *last = pt,
            None => self.move_to(pt),
        }
    }

    /// `SkPathBuilder::privateReversePathTo`: the last contour of `path`, backwards.
    fn reverse_path_to(&mut self, path: &SkPath) {
        let mut pi = path.pts.len();
        let mut wi = path.weights.len();
        for &verb in path.verbs.iter().rev() {
            match verb {
                Verb::Move => return,
                Verb::Line => {
                    pi -= 1;
                    self.line_to(path.pts[pi - 1]);
                }
                Verb::Quad => {
                    pi -= 2;
                    self.quad_to(path.pts[pi], path.pts[pi - 1]);
                }
                Verb::Conic => {
                    pi -= 2;
                    wi -= 1;
                    self.conic_to(path.pts[pi], path.pts[pi - 1], path.weights[wi]);
                }
                Verb::Cubic => {
                    pi -= 3;
                    self.cubic_to(path.pts[pi + 1], path.pts[pi], path.pts[pi - 1]);
                }
                Verb::Close => {}
            }
        }
    }
}

fn is_clockwise(before: P, after: P) -> bool {
    before.x * after.y > before.y * after.x
}

#[derive(Clone, Copy, PartialEq)]
enum Angle {
    Nearly180,
    Sharp,
    Shallow,
    NearlyLine,
}

fn dot_to_angle(dot: f32) -> Angle {
    if dot >= 0.0 {
        if (1.0 - dot).abs() <= NEARLY_ZERO {
            Angle::NearlyLine
        } else {
            Angle::Shallow
        }
    } else if (1.0 + dot).abs() <= NEARLY_ZERO {
        Angle::Nearly180
    } else {
        Angle::Sharp
    }
}

fn handle_inner_join(inner: &mut SkPath, pivot: P, after: P) {
    inner.line_to(pivot);
    inner.line_to(sub(pivot, after));
}

struct Stroker {
    radius: f32,
    inv_miter_limit: f32,
    res_scale: f32,
    inv_res_scale: f32,
    inv_res_scale_squared: f32,
    first_normal: P,
    prev_normal: P,
    first_unit_normal: P,
    prev_unit_normal: P,
    first_pt: P,
    prev_pt: P,
    first_outer_pt: P,
    first_outer_pt_index: usize,
    segment_count: i32,
    prev_is_line: bool,
    cap: LineCap,
    join: LineJoin,
    inner: SkPath,
    outer: SkPath,
    stroke_type: f32,
    recursion_depth: i32,
    found_tangents: bool,
    join_completed: bool,
    cusper: SkPath,
}

/// `kRecursiveLimits` as Chromium m142 ships them (later Skia lowers them to 15, 24, 16, 16),
/// indexed tangent, cubic, conic, quad.
const RECURSIVE_LIMITS: [i32; 4] = [5 * 3, 26 * 3, 11 * 3, 11 * 3];
const TANGENT_LIMIT: usize = 0;
const CUBIC_LIMIT: usize = 1;
const CONIC_LIMIT: usize = 2;
const QUAD_LIMIT: usize = 3;

impl Stroker {
    fn new(radius: f32, miter_limit: f32, cap: LineCap, join: LineJoin, res_scale: f32) -> Stroker {
        let mut join = join;
        let mut inv_miter_limit = 0.0;
        if join == LineJoin::Miter {
            if miter_limit <= 1.0 {
                join = LineJoin::Bevel;
            } else {
                inv_miter_limit = 1.0 / miter_limit;
            }
        }
        let inv_res_scale = 1.0 / (res_scale * 4.0);
        let zero = p(0.0, 0.0);
        Stroker {
            radius,
            inv_miter_limit,
            res_scale,
            inv_res_scale,
            inv_res_scale_squared: inv_res_scale * inv_res_scale,
            first_normal: zero,
            prev_normal: zero,
            first_unit_normal: zero,
            prev_unit_normal: zero,
            first_pt: zero,
            prev_pt: zero,
            first_outer_pt: zero,
            first_outer_pt_index: 0,
            segment_count: -1,
            prev_is_line: false,
            cap,
            join,
            inner: SkPath::new(),
            outer: SkPath::new(),
            stroke_type: 1.0,
            recursion_depth: 0,
            found_tangents: false,
            join_completed: false,
            cusper: SkPath::new(),
        }
    }

    fn set_normal_unit_normal(&self, before: P, after: P) -> Option<(P, P)> {
        let unit = set_length(
            p(
                (after.x - before.x) * self.res_scale,
                (after.y - before.y) * self.res_scale,
            ),
            1.0,
        )?;
        let unit = rotate_ccw(unit);
        Some((scale(unit, self.radius), unit))
    }

    fn joiner(
        &mut self,
        join: LineJoin,
        before_unit: P,
        pivot: P,
        after_unit: P,
        prev_line: bool,
        curr_line: bool,
    ) {
        let radius = self.radius;
        let (outer, inner) = (&mut self.outer, &mut self.inner);
        match join {
            LineJoin::Bevel => {
                let mut after = scale(after_unit, radius);
                let (outer, inner) = if is_clockwise(before_unit, after_unit) {
                    (outer, inner)
                } else {
                    after = p(-after.x, -after.y);
                    (inner, outer)
                };
                outer.line_to(add(pivot, after));
                handle_inner_join(inner, pivot, after);
            }
            LineJoin::Round => {
                let dot_prod = dot(before_unit, after_unit);
                if dot_to_angle(dot_prod) == Angle::NearlyLine {
                    return;
                }
                let (mut before, mut after) = (before_unit, after_unit);
                let mut ccw = false;
                let (outer, inner) = if is_clockwise(before, after) {
                    (outer, inner)
                } else {
                    before = p(-before.x, -before.y);
                    after = p(-after.x, -after.y);
                    ccw = true;
                    (inner, outer)
                };
                let start = crate::path::Pt {
                    x: before.x,
                    y: before.y,
                };
                let stop = crate::path::Pt {
                    x: after.x,
                    y: after.y,
                };
                if let Some(conics) = crate::path::build_unit_arc(
                    start,
                    stop,
                    ccw,
                    [radius, radius, pivot.x, pivot.y],
                ) && !conics.is_empty()
                {
                    for (pts, w) in conics {
                        outer.conic_to(p(pts[1].x, pts[1].y), p(pts[2].x, pts[2].y), w);
                    }
                    let after = scale(after, radius);
                    handle_inner_join(inner, pivot, after);
                }
            }
            LineJoin::Miter | LineJoin::MiterClip => {
                let dot_prod = dot(before_unit, after_unit);
                let angle = dot_to_angle(dot_prod);
                let (mut before, mut after) = (before_unit, after_unit);
                let mut curr_line = curr_line;
                if angle == Angle::NearlyLine {
                    return;
                }
                let mut outer = outer;
                let mut inner = inner;
                let mut miter: Option<P> = None;
                if angle == Angle::Nearly180 {
                    curr_line = false;
                } else {
                    let ccw = !is_clockwise(before, after);
                    if ccw {
                        std::mem::swap(&mut outer, &mut inner);
                        before = p(-before.x, -before.y);
                        after = p(-after.x, -after.y);
                    }
                    if dot_prod == 0.0 && self.inv_miter_limit <= 0.707_106_77 {
                        miter = Some(scale(add(before, after), radius));
                    } else {
                        let sin_half = (0.5 * (1.0 + dot_prod)).sqrt();
                        if sin_half < self.inv_miter_limit {
                            curr_line = false;
                        } else {
                            let mut mid = if angle == Angle::Sharp {
                                let m = p(after.y - before.y, before.x - after.x);
                                if ccw { p(-m.x, -m.y) } else { m }
                            } else {
                                p(before.x + after.x, before.y + after.y)
                            };
                            if let Some(m) = set_length(mid, radius / sin_half) {
                                mid = m;
                            }
                            miter = Some(mid);
                        }
                    }
                }
                if let Some(mid) = miter {
                    if prev_line {
                        outer.set_last_pt(add(pivot, mid));
                    } else {
                        outer.line_to(add(pivot, mid));
                    }
                }
                let after = scale(after, radius);
                if !curr_line {
                    outer.line_to(add(pivot, after));
                }
                handle_inner_join(inner, pivot, after);
            }
        }
    }

    fn capper(&mut self, pivot: P, normal: P, stop: P, extend_last: bool) {
        let outer = &mut self.outer;
        match self.cap {
            LineCap::Butt => outer.line_to(stop),
            LineCap::Round => {
                let parallel = rotate_cw(normal);
                let center = add(pivot, parallel);
                outer.conic_to(add(center, normal), center, ROOT2_OVER2);
                outer.conic_to(sub(center, normal), stop, ROOT2_OVER2);
            }
            LineCap::Square => {
                let parallel = rotate_cw(normal);
                let a = p(
                    pivot.x + normal.x + parallel.x,
                    pivot.y + normal.y + parallel.y,
                );
                let b = p(
                    pivot.x - normal.x + parallel.x,
                    pivot.y - normal.y + parallel.y,
                );
                if extend_last {
                    outer.set_last_pt(a);
                    outer.line_to(b);
                } else {
                    outer.line_to(a);
                    outer.line_to(b);
                    outer.line_to(stop);
                }
            }
        }
    }

    fn pre_join_to(&mut self, curr: P, curr_is_line: bool) -> Option<(P, P)> {
        let (normal, unit) = match self.set_normal_unit_normal(self.prev_pt, curr) {
            Some(v) => v,
            None => {
                if self.cap == LineCap::Butt {
                    return None;
                }
                (p(self.radius, 0.0), p(1.0, 0.0))
            }
        };
        if self.segment_count == 0 {
            self.first_normal = normal;
            self.first_unit_normal = unit;
            self.first_outer_pt = add(self.prev_pt, normal);
            self.outer.move_to(self.first_outer_pt);
            self.inner.move_to(sub(self.prev_pt, normal));
        } else {
            let (prev_unit, prev_pt, prev_line) =
                (self.prev_unit_normal, self.prev_pt, self.prev_is_line);
            self.joiner(self.join, prev_unit, prev_pt, unit, prev_line, curr_is_line);
        }
        self.prev_is_line = curr_is_line;
        Some((normal, unit))
    }

    fn post_join_to(&mut self, curr: P, normal: P, unit: P) {
        self.join_completed = true;
        self.prev_pt = curr;
        self.prev_unit_normal = unit;
        self.prev_normal = normal;
        self.segment_count += 1;
    }

    fn finish_contour(&mut self, close: bool, curr_is_line: bool) {
        if self.segment_count > 0 {
            if close {
                let (prev_unit, prev_pt, first_unit, prev_line) = (
                    self.prev_unit_normal,
                    self.prev_pt,
                    self.first_unit_normal,
                    self.prev_is_line,
                );
                self.joiner(
                    self.join,
                    prev_unit,
                    prev_pt,
                    first_unit,
                    prev_line,
                    curr_is_line,
                );
                self.outer.close();
                if let Some(pt) = self.inner.last_pt() {
                    let inner = std::mem::take(&mut self.inner);
                    self.outer.move_to(pt);
                    self.outer.reverse_path_to(&inner);
                    self.outer.close();
                }
            } else if let Some(pt) = self.inner.last_pt() {
                let (prev_pt, prev_normal) = (self.prev_pt, self.prev_normal);
                self.capper(prev_pt, prev_normal, pt, curr_is_line);
                let inner = std::mem::take(&mut self.inner);
                self.outer.reverse_path_to(&inner);
                let (first_pt, first_normal, first_outer, prev_line) = (
                    self.first_pt,
                    self.first_normal,
                    self.first_outer_pt,
                    self.prev_is_line,
                );
                self.capper(
                    first_pt,
                    p(-first_normal.x, -first_normal.y),
                    first_outer,
                    prev_line,
                );
                self.outer.close();
            }
            if !self.cusper.is_empty() {
                let cusper = std::mem::take(&mut self.cusper);
                self.outer.append(&cusper);
            }
        }
        self.inner = SkPath::new();
        self.segment_count = -1;
        self.first_outer_pt_index = self.outer.pts.len();
    }

    fn move_to(&mut self, pt: P) {
        if self.segment_count > 0 {
            self.finish_contour(false, false);
        }
        self.segment_count = 0;
        self.first_pt = pt;
        self.prev_pt = pt;
        self.join_completed = false;
    }

    fn line_to(&mut self, curr: P, has_valid_tangent: bool) {
        let teeny = (self.prev_pt.x - curr.x).abs() <= NEARLY_ZERO * self.inv_res_scale
            && (self.prev_pt.y - curr.y).abs() <= NEARLY_ZERO * self.inv_res_scale;
        if self.cap == LineCap::Butt && teeny {
            return;
        }
        if teeny && (self.join_completed || has_valid_tangent) {
            return;
        }
        let Some((normal, unit)) = self.pre_join_to(curr, true) else {
            return;
        };
        self.outer.line_to(add(curr, normal));
        self.inner.line_to(sub(curr, normal));
        self.post_join_to(curr, normal, unit);
    }

    fn is_current_contour_empty(&self) -> bool {
        let zero_since = |pts: &[P], start: usize| {
            let pts = &pts[start.min(pts.len())..];
            pts.len() < 2 || pts.iter().all(|&q| q == pts[0])
        };
        zero_since(&self.inner.pts, 0) && zero_since(&self.outer.pts, self.first_outer_pt_index)
    }

    fn check_linear(pts: &[P; 3], conic: Option<f32>) -> Reduction {
        let degenerate_ab = !can_normalize(sub(pts[1], pts[0]));
        let degenerate_bc = !can_normalize(sub(pts[2], pts[1]));
        if degenerate_ab && degenerate_bc {
            return Reduction::Point;
        }
        if degenerate_ab || degenerate_bc {
            return Reduction::Line;
        }
        if !quad_in_line(pts) {
            return Reduction::Quad;
        }
        let t = find_quad_max_curvature(pts);
        match conic {
            Some(w) => {
                if t == 0.0 || t.is_nan() {
                    return Reduction::Line;
                }
                Reduction::Degenerate(Conic { pts: *pts, w }.eval(t))
            }
            None => {
                if t == 0.0 || t == 1.0 {
                    return Reduction::Line;
                }
                Reduction::Degenerate(quad_eval(pts, t))
            }
        }
    }

    fn curve_to(&mut self, pt1: P, pt2: P, weight: Option<f32>) {
        let pts = [self.prev_pt, pt1, pt2];
        match Stroker::check_linear(&pts, weight) {
            Reduction::Point | Reduction::Line => {
                self.line_to(pt2, false);
                return;
            }
            Reduction::Degenerate(reduction) => {
                self.line_to(reduction, false);
                let save = self.join;
                self.join = LineJoin::Round;
                self.line_to(pt2, false);
                self.join = save;
                return;
            }
            Reduction::Quad => {}
        }
        let Some((normal_ab, unit_ab)) = self.pre_join_to(pt1, false) else {
            self.line_to(pt2, false);
            return;
        };
        let curve = Curve { pts, w: weight };
        let mut quad = QuadConstruct::default();
        self.init(1.0, &mut quad, 0.0, 1.0);
        self.curve_stroke(&curve, &mut quad);
        self.init(-1.0, &mut quad, 0.0, 1.0);
        self.curve_stroke(&curve, &mut quad);
        let (normal_bc, unit_bc) = self
            .set_normal_unit_normal(pts[1], pts[2])
            .unwrap_or((normal_ab, unit_ab));
        self.post_join_to(pt2, normal_bc, unit_bc);
    }

    fn set_ray_pts(&self, t_pt: P, dxy: P) -> (P, P) {
        let dxy = set_length(dxy, self.radius).unwrap_or(p(self.radius, 0.0));
        let on = p(
            t_pt.x + self.stroke_type * dxy.y,
            t_pt.y - self.stroke_type * dxy.x,
        );
        (on, dxy)
    }

    /// The point on the curve at `t`, the stroke point beside it and the tangent.
    fn perp_ray(&self, c: &Curve, t: f32) -> (P, P, P) {
        let (t_pt, mut dxy) = match c.w {
            Some(w) => {
                let conic = Conic { pts: c.pts, w };
                (conic.eval(t), conic.tangent(t))
            }
            None => (quad_eval(&c.pts, t), quad_tangent(&c.pts, t)),
        };
        if is_zero(dxy) {
            dxy = sub(c.pts[2], c.pts[0]);
        }
        let (on, tangent) = self.set_ray_pts(t_pt, dxy);
        (t_pt, on, tangent)
    }

    /// `SkPathStroker::init`.
    fn init(&mut self, stroke_type: f32, q: &mut QuadConstruct, start: f32, end: f32) {
        self.stroke_type = stroke_type;
        self.found_tangents = false;
        self.recursion_depth = 0;
        q.init(start, end);
    }

    /// `intersectRay`; `set_ctrl` is `kCtrlPt_RayType`.
    fn intersect_ray(&self, q: &mut QuadConstruct, set_ctrl: bool) -> ResultType {
        let start = q.quad[0];
        let end = q.quad[2];
        let a_len = q.tangent_start;
        let b_len = q.tangent_end;
        let denom = cross(a_len, b_len);
        if denom == 0.0 || !denom.is_finite() {
            q.opposite_tangents = dot(a_len, b_len) < 0.0;
            return ResultType::Degenerate;
        }
        q.opposite_tangents = false;
        let ab0 = sub(start, end);
        let mut numer_a = cross(b_len, ab0);
        let numer_b = cross(a_len, ab0);
        if (numer_a >= 0.0) == (numer_b >= 0.0) {
            let dist1 = pt_to_tangent_line(start, end, q.tangent_end);
            let dist2 = pt_to_tangent_line(end, start, q.tangent_start);
            if dist1.max(dist2) <= self.inv_res_scale_squared {
                return ResultType::Degenerate;
            }
            return ResultType::Split;
        }
        numer_a /= denom;
        if numer_a > numer_a - 1.0 {
            if set_ctrl {
                q.quad[1] = add(start, scale(q.tangent_start, numer_a));
            }
            return ResultType::Quad;
        }
        q.opposite_tangents = dot(a_len, b_len) < 0.0;
        ResultType::Degenerate
    }

    fn pt_in_quad_bounds(&self, quad: &[P; 3], pt: P) -> bool {
        let x_min = quad[0].x.min(quad[1].x).min(quad[2].x);
        if pt.x + self.inv_res_scale < x_min {
            return false;
        }
        let x_max = quad[0].x.max(quad[1].x).max(quad[2].x);
        if pt.x - self.inv_res_scale > x_max {
            return false;
        }
        let y_min = quad[0].y.min(quad[1].y).min(quad[2].y);
        if pt.y + self.inv_res_scale < y_min {
            return false;
        }
        let y_max = quad[0].y.max(quad[1].y).max(quad[2].y);
        pt.y - self.inv_res_scale <= y_max
    }

    fn stroke_close_enough(&self, stroke: &[P; 3], ray: [P; 2], q: &QuadConstruct) -> ResultType {
        let stroke_mid = quad_eval(stroke, 0.5);
        if points_within_dist(ray[0], stroke_mid, self.inv_res_scale) {
            if sharp_angle(&q.quad) {
                return ResultType::Split;
            }
            return ResultType::Quad;
        }
        if !self.pt_in_quad_bounds(stroke, ray[0]) {
            return ResultType::Split;
        }
        let roots = intersect_quad_ray(ray, stroke);
        if roots.len() != 1 {
            return ResultType::Split;
        }
        let quad_pt = quad_eval(stroke, roots[0]);
        let error = self.inv_res_scale * (1.0 - (roots[0] - 0.5).abs() * 2.0);
        if points_within_dist(ray[0], quad_pt, error) {
            if sharp_angle(&q.quad) {
                return ResultType::Split;
            }
            return ResultType::Quad;
        }
        ResultType::Split
    }

    fn compare_quad(&self, c: &Curve, q: &mut QuadConstruct) -> ResultType {
        if !q.start_set {
            let (_, on, tangent) = self.perp_ray(c, q.start_t);
            q.quad[0] = on;
            q.tangent_start = tangent;
            q.start_set = true;
        }
        if !q.end_set {
            let (_, on, tangent) = self.perp_ray(c, q.end_t);
            q.quad[2] = on;
            q.tangent_end = tangent;
            q.end_set = true;
        }
        let result = self.intersect_ray(q, true);
        if result != ResultType::Quad {
            return result;
        }
        let (t_pt, on, _) = self.perp_ray(c, q.mid_t);
        self.stroke_close_enough(&q.quad, [on, t_pt], q)
    }

    fn sink(&mut self) -> &mut SkPath {
        if self.stroke_type > 0.0 {
            &mut self.outer
        } else {
            &mut self.inner
        }
    }

    fn curve_stroke(&mut self, c: &Curve, q: &mut QuadConstruct) -> bool {
        let result = self.compare_quad(c, q);
        if result == ResultType::Quad {
            let stroke = q.quad;
            self.sink().quad_to(stroke[1], stroke[2]);
            return true;
        }
        if result == ResultType::Degenerate {
            let end = q.quad[2];
            self.sink().line_to(end);
            return true;
        }
        if !(q.quad[2].x.is_finite() && q.quad[2].y.is_finite()) {
            return false;
        }
        let limit = RECURSIVE_LIMITS[if c.w.is_some() {
            CONIC_LIMIT
        } else {
            QUAD_LIMIT
        }];
        self.split(q, limit, |s, half| s.curve_stroke(c, half))
    }

    fn add_degenerate_line(&mut self, q: &QuadConstruct) {
        let end = q.quad[2];
        self.sink().line_to(end);
    }

    /// The recursive tail shared by `quadStroke`, `conicStroke` and `cubicStroke`.
    fn split(
        &mut self,
        q: &QuadConstruct,
        limit: i32,
        mut stroke: impl FnMut(&mut Self, &mut QuadConstruct) -> bool,
    ) -> bool {
        self.recursion_depth += 1;
        if self.recursion_depth > limit {
            self.add_degenerate_line(q);
            self.recursion_depth -= 1;
            return true;
        }
        let mut half = QuadConstruct::default();
        if !half.init_with_start(q) {
            self.add_degenerate_line(q);
            self.recursion_depth -= 1;
            return true;
        }
        if !stroke(self, &mut half) {
            return false;
        }
        if !half.init_with_end(q) {
            self.add_degenerate_line(q);
            self.recursion_depth -= 1;
            return true;
        }
        if !stroke(self, &mut half) {
            return false;
        }
        self.recursion_depth -= 1;
        true
    }

    /// `SkPathStroker::CheckCubicLinear`.
    fn check_cubic_linear(cubic: &[P; 4]) -> CubicReduction {
        let degenerate_ab = !can_normalize(sub(cubic[1], cubic[0]));
        let degenerate_bc = !can_normalize(sub(cubic[2], cubic[1]));
        let degenerate_cd = !can_normalize(sub(cubic[3], cubic[2]));
        if degenerate_ab && degenerate_bc && degenerate_cd {
            return CubicReduction::Point;
        }
        if u8::from(degenerate_ab) + u8::from(degenerate_bc) + u8::from(degenerate_cd) == 2 {
            return CubicReduction::Line;
        }
        if !cubic_in_line(cubic) {
            return CubicReduction::Quad(if degenerate_ab { cubic[2] } else { cubic[1] });
        }
        let mut reduction = Vec::new();
        for t in geometry::find_cubic_max_curvature(cubic) {
            if 0.0 >= t || t >= 1.0 {
                continue;
            }
            let pt = geometry::eval_cubic_pos(cubic, t);
            if pt != cubic[0] && pt != cubic[3] {
                reduction.push(pt);
            }
        }
        if reduction.is_empty() {
            return CubicReduction::Line;
        }
        CubicReduction::Degenerate(reduction)
    }

    /// `SkPathStroker::cubicTo`.
    fn cubic_to(&mut self, pt1: P, pt2: P, pt3: P) {
        let cubic = [self.prev_pt, pt1, pt2, pt3];
        let tangent_pt = match Stroker::check_cubic_linear(&cubic) {
            CubicReduction::Point | CubicReduction::Line => {
                self.line_to(pt3, false);
                return;
            }
            CubicReduction::Degenerate(reduction) => {
                self.line_to(reduction[0], false);
                let save = self.join;
                self.join = LineJoin::Round;
                for &pt in &reduction[1..] {
                    self.line_to(pt, false);
                }
                self.line_to(pt3, false);
                self.join = save;
                return;
            }
            CubicReduction::Quad(tangent_pt) => tangent_pt,
        };
        let Some((normal_ab, unit_ab)) = self.pre_join_to(tangent_pt, false) else {
            self.line_to(pt3, false);
            return;
        };
        let mut last_t = 0.0;
        let inflections = geometry::find_cubic_inflections(&cubic);
        for index in 0..=inflections.len() {
            let next_t = inflections.get(index).copied().unwrap_or(1.0);
            let mut quad = QuadConstruct::default();
            self.init(1.0, &mut quad, last_t, next_t);
            self.cubic_stroke(&cubic, &mut quad);
            self.init(-1.0, &mut quad, last_t, next_t);
            self.cubic_stroke(&cubic, &mut quad);
            last_t = next_t;
        }
        let cusp = geometry::find_cubic_cusp(&cubic);
        if cusp > 0.0 {
            let loc = geometry::eval_cubic_pos(&cubic, cusp);
            self.cusper.add_circle(loc, self.radius);
        }
        let (normal_cd, unit_cd) = self.cubic_end_normal(&cubic, normal_ab, unit_ab);
        self.post_join_to(pt3, normal_cd, unit_cd);
    }

    /// `setCubicEndNormal`.
    fn cubic_end_normal(&self, cubic: &[P; 4], normal_ab: P, unit_ab: P) -> (P, P) {
        let mut ab = sub(cubic[1], cubic[0]);
        let mut cd = sub(cubic[3], cubic[2]);
        let mut degenerate_ab = !can_normalize(ab);
        let mut degenerate_cd = !can_normalize(cd);
        if degenerate_ab && degenerate_cd {
            return (normal_ab, unit_ab);
        }
        if degenerate_ab {
            ab = sub(cubic[2], cubic[0]);
            degenerate_ab = !can_normalize(ab);
        }
        if degenerate_cd {
            cd = sub(cubic[3], cubic[1]);
            degenerate_cd = !can_normalize(cd);
        }
        if degenerate_ab || degenerate_cd {
            return (normal_ab, unit_ab);
        }
        let unit = set_length(cd, 1.0).map_or(p(0.0, 0.0), rotate_ccw);
        (scale(unit, self.radius), unit)
    }

    /// `cubicPerpRay`: the point on the cubic at `t`, the stroke point beside it and the tangent.
    fn cubic_perp_ray(&self, cubic: &[P; 4], t: f32) -> (P, P, P) {
        let t_pt = geometry::eval_cubic_pos(cubic, t);
        let mut dxy = geometry::eval_cubic_tangent(cubic, t);
        if is_zero(dxy) {
            let mut c_pts = *cubic;
            if t.abs() <= NEARLY_ZERO {
                dxy = sub(cubic[2], cubic[0]);
            } else if (1.0 - t).abs() <= NEARLY_ZERO {
                dxy = sub(cubic[3], cubic[1]);
            } else {
                let chopped = geometry::chop_cubic_at(cubic, t);
                dxy = sub(chopped[3], chopped[2]);
                if is_zero(dxy) {
                    dxy = sub(chopped[3], chopped[1]);
                    c_pts = [chopped[0], chopped[1], chopped[2], chopped[3]];
                }
            }
            if is_zero(dxy) {
                dxy = sub(c_pts[3], c_pts[0]);
            }
        }
        let (on, tangent) = self.set_ray_pts(t_pt, dxy);
        (t_pt, on, tangent)
    }

    /// `cubicQuadEnds`.
    fn cubic_quad_ends(&self, cubic: &[P; 4], q: &mut QuadConstruct) {
        if !q.start_set {
            let (_, on, tangent) = self.cubic_perp_ray(cubic, q.start_t);
            q.quad[0] = on;
            q.tangent_start = tangent;
            q.start_set = true;
        }
        if !q.end_set {
            let (_, on, tangent) = self.cubic_perp_ray(cubic, q.end_t);
            q.quad[2] = on;
            q.tangent_end = tangent;
            q.end_set = true;
        }
    }

    /// `tangentsMeet`.
    fn tangents_meet(&self, cubic: &[P; 4], q: &mut QuadConstruct) -> ResultType {
        self.cubic_quad_ends(cubic, q);
        self.intersect_ray(q, false)
    }

    /// `compareQuadCubic`.
    fn compare_quad_cubic(&self, cubic: &[P; 4], q: &mut QuadConstruct) -> ResultType {
        self.cubic_quad_ends(cubic, q);
        let result = self.intersect_ray(q, true);
        if result != ResultType::Quad {
            return result;
        }
        let (t_pt, on, _) = self.cubic_perp_ray(cubic, q.mid_t);
        self.stroke_close_enough(&q.quad, [on, t_pt], q)
    }

    /// `cubicMidOnLine`.
    fn cubic_mid_on_line(&self, cubic: &[P; 4], q: &QuadConstruct) -> bool {
        let (_, stroke_mid, _) = self.cubic_perp_ray(cubic, q.mid_t);
        pt_to_line(stroke_mid, q.quad[0], q.quad[2]) < self.inv_res_scale_squared
    }

    /// `cubicStroke`.
    fn cubic_stroke(&mut self, cubic: &[P; 4], q: &mut QuadConstruct) -> bool {
        if !self.found_tangents {
            let result = self.tangents_meet(cubic, q);
            if result != ResultType::Quad {
                if (result == ResultType::Degenerate
                    || points_within_dist(q.quad[0], q.quad[2], self.inv_res_scale))
                    && self.cubic_mid_on_line(cubic, q)
                {
                    self.add_degenerate_line(q);
                    return true;
                }
            } else {
                self.found_tangents = true;
            }
        }
        if self.found_tangents {
            let result = self.compare_quad_cubic(cubic, q);
            if result == ResultType::Quad {
                let stroke = q.quad;
                self.sink().quad_to(stroke[1], stroke[2]);
                return true;
            }
            if result == ResultType::Degenerate && !q.opposite_tangents {
                self.add_degenerate_line(q);
                return true;
            }
        }
        if !(q.quad[2].x.is_finite() && q.quad[2].y.is_finite()) {
            return false;
        }
        let limit = RECURSIVE_LIMITS[if self.found_tangents {
            CUBIC_LIMIT
        } else {
            TANGENT_LIMIT
        }];
        self.split(q, limit, |s, half| s.cubic_stroke(cubic, half))
    }
}

enum CubicReduction {
    Point,
    Line,
    Quad(P),
    Degenerate(Vec<P>),
}

impl SkPath {
    /// `SkPathBuilder::addCircle(x, y, r)`: `addOval` clockwise from the right-hand point.
    fn add_circle(&mut self, c: P, r: f32) {
        if r < 0.0 {
            return;
        }
        let (l, t, rt, b) = (c.x - r, c.y - r, c.x + r, c.y + r);
        let mid = |a: f32, b: f32| ((f64::from(a) + f64::from(b)) * 0.5) as f32;
        let (cx, cy) = (mid(l, rt), mid(t, b));
        self.move_to(p(rt, cy));
        self.conic_to(p(rt, b), p(cx, b), ROOT2_OVER2);
        self.conic_to(p(l, b), p(l, cy), ROOT2_OVER2);
        self.conic_to(p(l, t), p(cx, t), ROOT2_OVER2);
        self.conic_to(p(rt, t), p(rt, cy), ROOT2_OVER2);
        self.close();
    }

    /// `SkPathBuilder::addPath` in append mode onto a path that does not end in a move.
    fn append(&mut self, src: &SkPath) {
        let (mut pi, mut wi) = (0, 0);
        for &verb in &src.verbs {
            match verb {
                Verb::Move => self.move_to(src.pts[pi]),
                Verb::Line => self.line_to(src.pts[pi]),
                Verb::Quad => self.quad_to(src.pts[pi], src.pts[pi + 1]),
                Verb::Conic => {
                    self.conic_to(src.pts[pi], src.pts[pi + 1], src.weights[wi]);
                    wi += 1;
                }
                Verb::Cubic => self.cubic_to(src.pts[pi], src.pts[pi + 1], src.pts[pi + 2]),
                Verb::Close => self.close(),
            }
            pi += verb.points();
        }
    }
}

struct Curve {
    pts: [P; 3],
    w: Option<f32>,
}

fn pt_to_line(pt: P, start: P, end: P) -> f32 {
    let dxy = sub(end, start);
    let ab0 = sub(pt, start);
    let numer = dot(dxy, ab0);
    let denom = dot(dxy, dxy);
    let t = numer / denom;
    if (0.0..=1.0).contains(&t) {
        let hit = p(
            start.x * (1.0 - t) + end.x * t,
            start.y * (1.0 - t) + end.y * t,
        );
        distance_sqd(hit, pt)
    } else {
        distance_sqd(pt, start)
    }
}

fn pt_to_tangent_line(pt: P, start: P, tangent: P) -> f32 {
    let ab0 = sub(pt, start);
    let numer = dot(tangent, ab0);
    let denom = dot(tangent, tangent);
    let t = numer / denom;
    if (0.0..=1.0).contains(&t) {
        let hit = add(start, scale(tangent, t));
        distance_sqd(hit, pt)
    } else {
        distance_sqd(pt, start)
    }
}

fn quad_in_line(quad: &[P; 3]) -> bool {
    let mut pt_max = -1.0f32;
    let (mut outer1, mut outer2) = (0, 1);
    for index in 0..2 {
        for inner in index + 1..3 {
            let d = sub(quad[inner], quad[index]);
            let test_max = d.x.abs().max(d.y.abs());
            if pt_max < test_max {
                outer1 = index;
                outer2 = inner;
                pt_max = test_max;
            }
        }
    }
    let mid = outer1 ^ outer2 ^ 3;
    let line_slop = pt_max * pt_max * 0.000_005;
    pt_to_line(quad[mid], quad[outer1], quad[outer2]) <= line_slop
}

fn cubic_in_line(cubic: &[P; 4]) -> bool {
    let mut pt_max = -1.0f32;
    let (mut outer1, mut outer2) = (0usize, 1usize);
    for index in 0..3 {
        for inner in index + 1..4 {
            let d = sub(cubic[inner], cubic[index]);
            let test_max = d.x.abs().max(d.y.abs());
            if pt_max < test_max {
                outer1 = index;
                outer2 = inner;
                pt_max = test_max;
            }
        }
    }
    let mid1 = (1 + (2 >> outer2)) >> outer1;
    let mid2 = outer1 ^ outer2 ^ mid1;
    let line_slop = pt_max * pt_max * 0.000_01;
    pt_to_line(cubic[mid1], cubic[outer1], cubic[outer2]) <= line_slop
        && pt_to_line(cubic[mid2], cubic[outer1], cubic[outer2]) <= line_slop
}

fn points_within_dist(a: P, b: P, limit: f32) -> bool {
    distance_sqd(a, b) <= limit * limit
}

fn sharp_angle(quad: &[P; 3]) -> bool {
    let mut smaller = sub(quad[1], quad[0]);
    let mut larger = sub(quad[1], quad[2]);
    let smaller_len = dot(smaller, smaller);
    let mut larger_len = dot(larger, larger);
    if smaller_len > larger_len {
        std::mem::swap(&mut smaller, &mut larger);
        larger_len = smaller_len;
    }
    let Some(smaller) = set_length(smaller, larger_len) else {
        return false;
    };
    dot(smaller, larger) > 0.0
}

fn intersect_quad_ray(line: [P; 2], quad: &[P; 3]) -> Vec<f32> {
    let vec = sub(line[1], line[0]);
    let r: Vec<f32> = quad.iter().map(|&q| cross(vec, sub(q, line[0]))).collect();
    let (mut a, mut b, c) = (r[2], r[1], r[0]);
    a += c - 2.0 * b;
    b -= c;
    find_unit_quad_roots(a, 2.0 * b, c)
}

/// One record of `SkPath::Iter` (not forced closed): the closing line of a contour comes as a
/// line before its close, and a trailing move is dropped.
#[derive(Clone, Copy)]
struct Rec {
    verb: Verb,
    pts: [P; 4],
    w: f32,
}

fn iter_records(path: &SkPath) -> Vec<Rec> {
    let mut out = Vec::new();
    let (mut pi, mut wi) = (0usize, 0usize);
    let (mut move_to, mut last) = (p(0.0, 0.0), p(0.0, 0.0));
    let n = path.verbs.len();
    for (vi, &verb) in path.verbs.iter().enumerate() {
        match verb {
            Verb::Move => {
                if vi + 1 == n {
                    break;
                }
                move_to = path.pts[pi];
                last = move_to;
                pi += 1;
                out.push(Rec {
                    verb,
                    pts: [move_to; 4],
                    w: 1.0,
                });
            }
            Verb::Line => {
                let e = path.pts[pi];
                out.push(Rec {
                    verb,
                    pts: [last, e, e, e],
                    w: 1.0,
                });
                last = e;
                pi += 1;
            }
            Verb::Quad | Verb::Conic => {
                let w = if verb == Verb::Conic {
                    wi += 1;
                    path.weights[wi - 1]
                } else {
                    1.0
                };
                out.push(Rec {
                    verb,
                    pts: [last, path.pts[pi], path.pts[pi + 1], path.pts[pi + 1]],
                    w,
                });
                last = path.pts[pi + 1];
                pi += 2;
            }
            Verb::Cubic => {
                out.push(Rec {
                    verb,
                    pts: [last, path.pts[pi], path.pts[pi + 1], path.pts[pi + 2]],
                    w: 1.0,
                });
                last = path.pts[pi + 2];
                pi += 3;
            }
            Verb::Close => {
                let nan =
                    last.x.is_nan() || last.y.is_nan() || move_to.x.is_nan() || move_to.y.is_nan();
                if last != move_to && !nan {
                    out.push(Rec {
                        verb: Verb::Line,
                        pts: [last, move_to, move_to, move_to],
                        w: 1.0,
                    });
                }
                out.push(Rec {
                    verb,
                    pts: [move_to; 4],
                    w: 1.0,
                });
                last = move_to;
            }
        }
    }
    out
}

fn has_valid_tangent(rest: &[Rec]) -> bool {
    for r in rest {
        match r.verb {
            Verb::Move | Verb::Close => return false,
            Verb::Line => {
                if r.pts[0] == r.pts[1] {
                    continue;
                }
                return true;
            }
            Verb::Quad | Verb::Conic => {
                if r.pts[0] == r.pts[1] && r.pts[0] == r.pts[2] {
                    continue;
                }
                return true;
            }
            Verb::Cubic => {
                if r.pts[0] == r.pts[1] && r.pts[0] == r.pts[2] && r.pts[0] == r.pts[3] {
                    continue;
                }
                return true;
            }
        }
    }
    false
}

/// The stroke parameters `SkStroke` takes from the paint.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub width: f32,
    pub miter_limit: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub res_scale: f32,
}

/// `SkStroke::strokePath`: the outline of `src` stroked with `style`, to be filled nonzero.
pub fn stroke_path(src: &SkPath, style: &Style) -> SkPath {
    let radius = style.width * 0.5;
    if radius.is_nan() || radius <= 0.0 {
        return SkPath::new();
    }
    if let Some((rect, true, cw)) = src.rect_contour() {
        return stroke_rect(rect, cw, style);
    }
    let mut stroker = Stroker::new(
        radius,
        style.miter_limit,
        style.cap,
        style.join,
        style.res_scale,
    );
    let records = iter_records(src);
    let mut last_segment = Verb::Move;
    for (i, r) in records.iter().enumerate() {
        match r.verb {
            Verb::Move => stroker.move_to(r.pts[0]),
            Verb::Line => {
                let valid = has_valid_tangent(&records[i + 1..]);
                stroker.line_to(r.pts[1], valid);
                last_segment = Verb::Line;
            }
            Verb::Quad => {
                stroker.curve_to(r.pts[1], r.pts[2], None);
                last_segment = Verb::Quad;
            }
            Verb::Conic => {
                stroker.curve_to(r.pts[1], r.pts[2], Some(r.w));
                last_segment = Verb::Conic;
            }
            Verb::Cubic => {
                stroker.cubic_to(r.pts[1], r.pts[2], r.pts[3]);
                last_segment = Verb::Cubic;
            }
            Verb::Close => {
                if style.cap != LineCap::Butt {
                    if stroker.segment_count == 0 {
                        let first = stroker.first_pt;
                        stroker.line_to(first, false);
                        last_segment = Verb::Line;
                        continue;
                    }
                    if stroker.is_current_contour_empty() {
                        last_segment = Verb::Line;
                        continue;
                    }
                }
                stroker.finish_contour(true, last_segment == Verb::Line);
            }
        }
    }
    stroker.finish_contour(false, last_segment == Verb::Line);
    stroker.outer
}

/// `SkStroke::strokeRect` for a closed rect contour.
fn stroke_rect(rect: crate::skpath::Rect, cw: bool, style: &Style) -> SkPath {
    let radius = style.width * 0.5;
    let mut dst = SkPath::new();
    let (l, t, r, b) = (rect.left, rect.top, rect.right, rect.bottom);
    let (rw, rh) = (r - l, b - t);
    let outer = crate::skpath::Rect {
        left: l - radius,
        top: t - radius,
        right: r + radius,
        bottom: b + radius,
    };
    let mut join = style.join;
    if join == LineJoin::Miter && style.miter_limit < std::f32::consts::SQRT_2 {
        join = LineJoin::Bevel;
    }
    match join {
        LineJoin::Bevel => {
            let mut pts = [
                p(l, outer.top),
                p(r, outer.top),
                p(outer.right, t),
                p(outer.right, b),
                p(r, outer.bottom),
                p(l, outer.bottom),
                p(outer.left, b),
                p(outer.left, t),
            ];
            if !cw {
                pts = [
                    p(outer.left, t),
                    p(outer.left, b),
                    p(l, outer.bottom),
                    p(r, outer.bottom),
                    p(outer.right, b),
                    p(outer.right, t),
                    p(r, outer.top),
                    p(l, outer.top),
                ];
            }
            dst.move_to(pts[0]);
            for &q in &pts[1..] {
                dst.line_to(q);
            }
            dst.close();
        }
        LineJoin::Round => add_rrect(&mut dst, outer, radius, cw),
        _ => add_rect(&mut dst, outer, cw),
    }
    if style.width < rw.min(rh) {
        let inner = crate::skpath::Rect {
            left: l + radius,
            top: t + radius,
            right: r - radius,
            bottom: b - radius,
        };
        add_rect(&mut dst, inner, !cw);
    }
    dst
}

/// `SkPathBuilder::addRRect(SkRRect::MakeRectXY(r, radius, radius), dir)` with its default
/// start index, including `SkRRect`'s reduction to a rect or an oval.
fn add_rrect(dst: &mut SkPath, r: crate::skpath::Rect, radius: f32, cw: bool) {
    let finite = [r.left, r.top, r.right, r.bottom]
        .iter()
        .all(|v| v.is_finite());
    let bounds = if finite {
        crate::skpath::Rect {
            left: r.left.min(r.right),
            top: r.top.min(r.bottom),
            right: r.left.max(r.right),
            bottom: r.top.max(r.bottom),
        }
    } else {
        crate::skpath::Rect {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        }
    };
    let (w, h) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
    let index: usize = if cw { 6 } else { 7 };
    let advance = |i: usize, n: usize| if cw { (i + 1) % n } else { (i + n - 1) % n };
    let corners = [
        p(bounds.left, bounds.top),
        p(bounds.right, bounds.top),
        p(bounds.right, bounds.bottom),
        p(bounds.left, bounds.bottom),
    ];
    let (mut rx, mut ry) = if radius.is_finite() {
        (radius, radius)
    } else {
        (0.0, 0.0)
    };
    if !(w > 0.0 && h > 0.0) || !finite {
        rx = 0.0;
    } else if w < rx + rx || h < ry + ry {
        let scale = (w / (rx + rx)).min(h / (ry + ry));
        rx *= scale;
        ry *= scale;
    }
    if rx <= 0.0 || ry <= 0.0 {
        let mut i = (index + 1) / 2 % 4;
        dst.move_to(corners[i]);
        for _ in 0..3 {
            i = advance(i, 4);
            dst.line_to(corners[i]);
        }
        dst.close();
        return;
    }
    let w_conic = std::f32::consts::FRAC_1_SQRT_2;
    let mut rect_i = (index / 2 + usize::from(!cw)) % 4;
    if rx >= w / 2.0 && ry >= h / 2.0 {
        let mid = |a: f32, b: f32| ((f64::from(a) + f64::from(b)) * 0.5) as f32;
        let (cx, cy) = (
            mid(bounds.left, bounds.right),
            mid(bounds.top, bounds.bottom),
        );
        let oval = [
            p(cx, bounds.top),
            p(bounds.right, cy),
            p(cx, bounds.bottom),
            p(bounds.left, cy),
        ];
        let mut oval_i = index / 2 % 4;
        dst.move_to(oval[oval_i]);
        for _ in 0..4 {
            rect_i = advance(rect_i, 4);
            oval_i = advance(oval_i, 4);
            dst.conic_to(corners[rect_i], oval[oval_i], w_conic);
        }
        dst.close();
        return;
    }
    let (l, t, rr, b) = (bounds.left, bounds.top, bounds.right, bounds.bottom);
    let pts = [
        p(l + rx, t),
        p(rr - rx, t),
        p(rr, t + ry),
        p(rr, b - ry),
        p(rr - rx, b),
        p(l + rx, b),
        p(l, b - ry),
        p(l, t + ry),
    ];
    let mut i = index % 8;
    dst.move_to(pts[i]);
    for _ in 0..4 {
        i = advance(i, 8);
        dst.line_to(pts[i]);
        rect_i = advance(rect_i, 4);
        i = advance(i, 8);
        dst.conic_to(corners[rect_i], pts[i], w_conic);
    }
    dst.close();
}

/// `SkPathBuilder::addRect(rect, dir)` from the top-left corner.
fn add_rect(dst: &mut SkPath, r: crate::skpath::Rect, cw: bool) {
    let corners = [
        p(r.left, r.top),
        p(r.right, r.top),
        p(r.right, r.bottom),
        p(r.left, r.bottom),
    ];
    dst.move_to(corners[0]);
    if cw {
        for &c in &corners[1..] {
            dst.line_to(c);
        }
    } else {
        for &c in corners[1..].iter().rev() {
            dst.line_to(c);
        }
    }
    dst.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(width: f32, cap: LineCap) -> Style {
        Style {
            width,
            miter_limit: 10.0,
            cap,
            join: LineJoin::Miter,
            res_scale: 1.0,
        }
    }

    #[test]
    fn round_caps_are_two_conics_each() {
        let mut src = SkPath::new();
        src.move_to(p(10.0, 10.0));
        src.line_to(p(50.0, 10.0));
        let out = stroke_path(&src, &style(6.0, LineCap::Round));
        assert_eq!(out.verbs.iter().filter(|v| **v == Verb::Conic).count(), 4);
        assert_eq!(out.pts[0], p(10.0, 7.0));
    }

    #[test]
    fn closed_rects_stroke_as_two_rects() {
        let mut src = SkPath::new();
        src.move_to(p(10.0, 10.0));
        src.line_to(p(50.0, 10.0));
        src.line_to(p(50.0, 30.0));
        src.line_to(p(10.0, 30.0));
        src.close();
        let out = stroke_path(&src, &style(2.0, LineCap::Butt));
        assert_eq!(out.pts.len(), 8);
        assert_eq!(out.pts[0], p(9.0, 9.0));
        assert_eq!(out.pts[4], p(11.0, 11.0));
        assert_eq!(out.pts[5], p(11.0, 29.0));
    }

    #[test]
    fn straight_cubic_strokes_like_its_line() {
        let mut line = SkPath::new();
        line.move_to(p(10.0, 10.0));
        line.line_to(p(70.0, 10.0));
        let mut cubic = SkPath::new();
        cubic.move_to(p(10.0, 10.0));
        cubic.cubic_to(p(30.0, 10.0), p(50.0, 10.0), p(70.0, 10.0));
        let s = style(6.0, LineCap::Butt);
        assert_eq!(stroke_path(&cubic, &s), stroke_path(&line, &s));
    }

    #[test]
    fn curved_cubic_outline_stays_within_the_radius() {
        let c = [
            p(10.0, 100.0),
            p(40.0, 10.0),
            p(160.0, 10.0),
            p(190.0, 100.0),
        ];
        let mut src = SkPath::new();
        src.move_to(c[0]);
        src.cubic_to(c[1], c[2], c[3]);
        let out = stroke_path(&src, &style(8.0, LineCap::Butt));
        let curve: Vec<P> = (0..=2000)
            .map(|i| geometry::eval_cubic_pos(&c, i as f32 / 2000.0))
            .collect();
        let mut on_outline = Vec::new();
        let mut pi = 0;
        for &v in &out.verbs {
            if v == Verb::Quad {
                let q = [out.pts[pi - 1], out.pts[pi], out.pts[pi + 1]];
                on_outline.extend([quad_eval(&q, 0.25), quad_eval(&q, 0.5), quad_eval(&q, 0.75)]);
            }
            if v != Verb::Close {
                on_outline.push(out.pts[pi + v.points() - 1]);
            }
            pi += v.points();
        }
        assert!(out.verbs.iter().filter(|v| **v == Verb::Quad).count() >= 4);
        for q in &on_outline {
            let d = curve
                .iter()
                .map(|&k| distance_sqd(k, *q).sqrt())
                .fold(f32::INFINITY, f32::min);
            assert!(d < 4.0 + 0.3, "{q:?} is {d} from the curve");
        }
    }

    #[test]
    fn cusp_cubic_gets_a_finite_outline_and_a_circle() {
        let mut src = SkPath::new();
        src.move_to(p(0.0, 0.0));
        src.cubic_to(p(100.0, 100.0), p(0.0, 100.0), p(100.0, 0.0));
        let out = stroke_path(&src, &style(10.0, LineCap::Round));
        assert!(out.pts.iter().all(|q| q.x.is_finite() && q.y.is_finite()));
        let moves = out.verbs.iter().filter(|v| **v == Verb::Move).count();
        assert_eq!(moves, 2);
        let closes = out.verbs.iter().filter(|v| **v == Verb::Close).count();
        assert_eq!(closes, 2);
    }

    #[test]
    fn closed_cubic_contour_reverses_its_inner_side() {
        let mut src = SkPath::new();
        src.move_to(p(10.0, 10.0));
        src.cubic_to(p(60.0, 0.0), p(60.0, 60.0), p(10.0, 50.0));
        src.close();
        let out = stroke_path(&src, &style(4.0, LineCap::Butt));
        let moves = out.verbs.iter().filter(|v| **v == Verb::Move).count();
        assert_eq!(moves, 2);
        assert!(out.pts.iter().all(|q| q.x.is_finite() && q.y.is_finite()));
    }
}
