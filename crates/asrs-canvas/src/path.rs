use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

use tiny_skia::PathBuilder;

use crate::hairline::P;

const NEARLY_ZERO: f32 = 1.0 / 4096.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

const fn pt(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

fn nearly_equal(a: f32, b: f32) -> bool {
    (a - b).abs() <= NEARLY_ZERO
}

fn nearly_zero(v: f32) -> bool {
    v.abs() <= NEARLY_ZERO
}

/// A 2x3 affine matrix in Blink's `AffineTransform` layout: `x' = a x + c y + e`,
/// `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translation(x: f64, y: f64) -> Affine {
        Affine {
            e: x,
            f: y,
            ..Affine::IDENTITY
        }
    }

    pub fn scaling(x: f64, y: f64) -> Affine {
        Affine {
            a: x,
            d: y,
            ..Affine::IDENTITY
        }
    }

    pub fn rotation(angle: f64) -> Affine {
        let (s, c) = (angle.sin(), angle.cos());
        Affine {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// `self * other`: `other` applies first, as `ctx.translate`/`scale`/`rotate` compose.
    pub fn then(&self, other: &Affine) -> Affine {
        Affine {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    pub fn is_invertible(&self) -> bool {
        let det = self.determinant();
        det.is_finite() && det != 0.0
    }

    pub fn inverse(&self) -> Option<Affine> {
        if !self.is_invertible() {
            return None;
        }
        let det = self.determinant();
        Some(Affine {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
            e: (self.c * self.f - self.d * self.e) / det,
            f: (self.b * self.e - self.a * self.f) / det,
        })
    }

    pub fn is_scale_translate(&self) -> bool {
        self.b == 0.0 && self.c == 0.0
    }

    pub fn to_skia(self) -> tiny_skia::Transform {
        tiny_skia::Transform::from_row(
            self.a as f32,
            self.b as f32,
            self.c as f32,
            self.d as f32,
            self.e as f32,
            self.f as f32,
        )
    }

    fn map32(&self, p: Pt) -> Pt {
        let m = self.to_skia();
        pt(
            p.x * m.sx + p.y * m.kx + m.tx,
            p.x * m.ky + p.y * m.sy + m.ty,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Verb {
    Move(Pt),
    Line(Pt),
    Conic(Pt, Pt, f32),
    Close,
}

/// The current default path of a canvas context, with Blink's `CanvasPath` entry checks and
/// Skia's `SkPath` building rules (move-to injection after close, `arcTo` with conics).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    verbs: Vec<Verb>,
    last_move: Option<Pt>,
    needs_move: bool,
}

impl Path {
    pub fn new() -> Path {
        Path::default()
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    fn last_point(&self) -> Option<Pt> {
        self.verbs.iter().rev().find_map(|v| match *v {
            Verb::Move(p) | Verb::Line(p) | Verb::Conic(_, p, _) => Some(p),
            Verb::Close => None,
        })
    }

    fn inject_move(&mut self) {
        if self.needs_move {
            self.needs_move = false;
            let p = self.last_move.unwrap_or(pt(0.0, 0.0));
            self.verbs.push(Verb::Move(p));
        }
    }

    fn sk_move_to(&mut self, p: Pt) {
        if let Some(Verb::Move(last)) = self.verbs.last_mut() {
            *last = p;
        } else {
            self.verbs.push(Verb::Move(p));
        }
        self.last_move = Some(p);
        self.needs_move = false;
    }

    fn sk_line_to(&mut self, p: Pt) {
        if self.verbs.is_empty() {
            self.sk_move_to(pt(0.0, 0.0));
        }
        self.inject_move();
        self.verbs.push(Verb::Line(p));
    }

    fn sk_conic_to(&mut self, p1: Pt, p2: Pt, w: f32) {
        self.inject_move();
        self.verbs.push(Verb::Conic(p1, p2, w));
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.sk_move_to(pt(x as f32, y as f32));
        }
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let p = pt(x as f32, y as f32);
        if self.is_empty() {
            self.sk_move_to(p);
        }
        self.sk_line_to(p);
    }

    pub fn close(&mut self) {
        if !self.is_empty() && self.verbs.last() != Some(&Verb::Close) {
            self.verbs.push(Verb::Close);
        }
        if !self.is_empty() {
            self.needs_move = true;
        }
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return;
        }
        let (x0, y0) = (x as f32, y as f32);
        let (x1, y1) = ((x + w) as f32, (y + h) as f32);
        self.sk_move_to(pt(x0, y0));
        self.sk_line_to(pt(x1, y0));
        self.sk_line_to(pt(x1, y1));
        self.sk_line_to(pt(x0, y1));
        self.close();
    }

    pub fn arc(&mut self, x: f64, y: f64, radius: f64, start: f64, end: f64, ccw: bool) {
        if ![x, y, radius, start, end].iter().all(|v| v.is_finite()) || radius < 0.0 {
            return;
        }
        let (x, y, r) = (x as f32, y as f32, radius as f32);
        let (mut start, mut end) = (start as f32, end as f32);
        if r == 0.0 || start == end {
            self.line_to(
                f64::from(x + r * start.cos()),
                f64::from(y + r * start.sin()),
            );
            return;
        }
        canonicalize_angle(&mut start, &mut end);
        let end = adjust_end_angle(start, end, ccw);
        self.add_ellipse(pt(x, y), r, r, start, end, None);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        x: f64,
        y: f64,
        rx: f64,
        ry: f64,
        rotation: f64,
        start: f64,
        end: f64,
        ccw: bool,
    ) {
        if ![x, y, rx, ry, rotation, start, end]
            .iter()
            .all(|v| v.is_finite())
            || rx < 0.0
            || ry < 0.0
        {
            return;
        }
        let (mut s, mut e) = (start as f32, end as f32);
        canonicalize_angle(&mut s, &mut e);
        let e = adjust_end_angle(s, e, ccw);
        if rx == 0.0 || ry == 0.0 || s == e {
            self.degenerate_ellipse(x, y, rx, ry, rotation, f64::from(s), f64::from(e), ccw);
            return;
        }
        let rotate = (rotation != 0.0).then(|| {
            Affine::translation(f64::from(x as f32), f64::from(y as f32))
                .then(&Affine::rotation(rotation))
        });
        let center = if rotate.is_some() {
            pt(0.0, 0.0)
        } else {
            pt(x as f32, y as f32)
        };
        self.add_ellipse(center, rx as f32, ry as f32, s, e, rotate);
    }

    #[allow(clippy::too_many_arguments)]
    fn degenerate_ellipse(
        &mut self,
        x: f64,
        y: f64,
        rx: f64,
        ry: f64,
        rotation: f64,
        start: f64,
        end: f64,
        ccw: bool,
    ) {
        let rotate = Affine::rotation(rotation);
        let mut line = |angle: f64| {
            let (px, py) = (rx * angle.cos(), ry * angle.sin());
            self.line_to(
                x + rotate.a * px + rotate.c * py,
                y + rotate.b * px + rotate.d * py,
            );
        };
        line(start);
        if (rx == 0.0 && ry == 0.0) || start == end {
            return;
        }
        let quarter = std::f64::consts::FRAC_PI_2;
        if !ccw {
            let mut angle = start - start % quarter + quarter;
            while angle < end {
                line(angle);
                angle += quarter;
            }
        } else {
            let mut angle = start - start % quarter;
            while angle > end {
                line(angle);
                angle -= quarter;
            }
        }
        line(end);
    }

    /// Blink's `Path::AddEllipse`: degrees in f32 and two half sweeps for a full turn.
    fn add_ellipse(
        &mut self,
        c: Pt,
        rx: f32,
        ry: f32,
        start: f32,
        end: f32,
        rotate: Option<Affine>,
    ) {
        let deg = 180.0 / PI;
        let (start_deg, end_deg) = (start * deg, end * deg);
        let sweep = end_deg - start_deg;
        let oval = [c.x - rx, c.y - ry, c.x + rx, c.y + ry];
        if nearly_equal(sweep, 360.0) {
            self.arc_to(oval, start_deg, 180.0, rotate);
            self.arc_to(oval, start_deg + 180.0, 180.0, rotate);
        } else if nearly_equal(sweep, -360.0) {
            self.arc_to(oval, start_deg, -180.0, rotate);
            self.arc_to(oval, start_deg - 180.0, -180.0, rotate);
        } else {
            self.arc_to(oval, start_deg, sweep, rotate);
        }
    }

    fn arc_to(&mut self, oval: [f32; 4], start: f32, sweep: f32, rotate: Option<Affine>) {
        let force_move = self.verbs.is_empty() || self.needs_move;
        self.arc_to_forced(oval, start, sweep, rotate, force_move);
    }

    /// `SkPathBuilder::arcTo(oval, startAngle, sweepAngle, forceMoveTo)` in degrees.
    pub fn arc_to_oval(&mut self, oval: [f32; 4], start: f32, sweep: f32, force_move: bool) {
        let force_move = force_move || self.verbs.is_empty() || self.needs_move;
        self.arc_to_forced(oval, start, sweep, None, force_move);
    }

    /// `SkPathBuilder::addOval(oval)`: clockwise from the right-hand point, four quarter conics.
    pub fn add_oval(&mut self, oval: [f32; 4]) {
        let [l, t, r, b] = oval;
        let (cx, cy) = (midpoint(l, r), midpoint(t, b));
        let w = FRAC_1_SQRT_2;
        self.sk_move_to(pt(r, cy));
        self.sk_conic_to(pt(r, b), pt(cx, b), w);
        self.sk_conic_to(pt(l, b), pt(l, cy), w);
        self.sk_conic_to(pt(l, t), pt(cx, t), w);
        self.sk_conic_to(pt(r, t), pt(r, cy), w);
        self.close();
    }

    /// `SkPath::arcTo(oval, startAngle, sweepAngle, forceMoveTo)`.
    fn arc_to_forced(
        &mut self,
        oval: [f32; 4],
        start: f32,
        sweep: f32,
        rotate: Option<Affine>,
        force_move: bool,
    ) {
        let [l, t, r, b] = oval;
        if r - l < 0.0 || b - t < 0.0 {
            return;
        }
        let map = |p: Pt| rotate.map_or(p, |m| m.map32(p));
        let start = start % 360.0;
        let center = pt(midpoint(l, r), midpoint(t, b));
        let lone = if sweep == 0.0 && (start == 0.0 || start == 360.0) {
            Some(pt(r, center.y))
        } else if r - l == 0.0 && b - t == 0.0 {
            Some(pt(r, t))
        } else {
            None
        };
        let add_point = |path: &mut Path, p: Pt| {
            let p = map(p);
            if force_move {
                path.sk_move_to(p);
            } else if !path
                .last_point()
                .is_some_and(|last| nearly_equal(last.x, p.x) && nearly_equal(last.y, p.y))
            {
                path.sk_line_to(p);
            }
        };
        if let Some(p) = lone {
            add_point(self, p);
            return;
        }
        let (start_v, stop_v, ccw) = angles_to_unit_vectors(start, sweep);
        if start_v == stop_v {
            let end = (start + sweep) * (PI / 180.0);
            let (rx, ry) = ((r - l) / 2.0, (b - t) / 2.0);
            add_point(
                self,
                pt(center.x + rx * end.cos(), center.y + ry * end.sin()),
            );
            return;
        }
        let user = [(r - l) * 0.5, (b - t) * 0.5, center.x, center.y];
        match build_unit_arc(start_v, stop_v, ccw, user) {
            Some(conics) => {
                add_point(self, conics[0].0[0]);
                for (p, w) in conics {
                    self.sk_conic_to(map(p[1]), map(p[2]), w);
                }
            }
            None => add_point(
                self,
                pt(user[0] * stop_v.x + user[2], user[1] * stop_v.y + user[3]),
            ),
        }
    }

    /// Maps every point through `m` in f32, as `SkPath::transform` does.
    pub fn transform(&mut self, m: &Affine) {
        for v in &mut self.verbs {
            match v {
                Verb::Move(p) | Verb::Line(p) => *p = m.map32(*p),
                Verb::Conic(p1, p2, _) => {
                    *p1 = m.map32(*p1);
                    *p2 = m.map32(*p2);
                }
                Verb::Close => {}
            }
        }
        if let Some(p) = self.last_move.as_mut() {
            *p = m.map32(*p);
        }
    }

    /// The path as an `SkPath`, mapped through `m` first when given.
    pub fn to_sk(&self, m: Option<&Affine>) -> crate::skpath::SkPath {
        let map = |p: Pt| {
            let p = P { x: p.x, y: p.y };
            m.map_or(p, |m| crate::skpath::map_point(m, p))
        };
        let mut out = crate::skpath::SkPath::new();
        for v in &self.verbs {
            match *v {
                Verb::Move(p) => out.move_to(map(p)),
                Verb::Line(p) => out.line_to(map(p)),
                Verb::Conic(p1, p2, w) => out.conic_to(map(p1), map(p2), w),
                Verb::Close => out.close(),
            }
        }
        out
    }

    /// The path as tiny-skia geometry, mapped through `m` first when given, with conics split
    /// into quads to within `conic_tolerance` (Skia uses 0.25 device pixels).
    pub fn to_skia(&self, m: Option<&Affine>, conic_tolerance: f32) -> Option<tiny_skia::Path> {
        let map = |p: Pt| m.map_or(p, |m| m.map32(p));
        let mut builder = PathBuilder::new();
        let mut last = pt(0.0, 0.0);
        for v in &self.verbs {
            match *v {
                Verb::Move(p) => {
                    let p = map(p);
                    builder.move_to(p.x, p.y);
                    last = p;
                }
                Verb::Line(p) => {
                    let p = map(p);
                    builder.line_to(p.x, p.y);
                    last = p;
                }
                Verb::Conic(p1, p2, w) => {
                    let (p1, p2) = (map(p1), map(p2));
                    let conic = Conic {
                        pts: [last, p1, p2],
                        w,
                    };
                    for q in conic.to_quads(conic_tolerance) {
                        builder.quad_to(q[0].x, q[0].y, q[1].x, q[1].y);
                    }
                    last = p2;
                }
                Verb::Close => builder.close(),
            }
        }
        builder.finish()
    }
}

/// `SkAutoConicToQuads::computeQuads`: the quads approximating a conic to within `tol`.
pub fn conic_to_quads(pts: [P; 3], w: f32, tol: f32) -> Vec<[P; 3]> {
    let conic = Conic {
        pts: pts.map(|p| pt(p.x, p.y)),
        w,
    };
    let mut start = pts[0];
    conic
        .to_quads(tol)
        .into_iter()
        .map(|[c, e]| {
            let q = [start, P { x: c.x, y: c.y }, P { x: e.x, y: e.y }];
            start = q[2];
            q
        })
        .collect()
}

fn midpoint(a: f32, b: f32) -> f32 {
    ((f64::from(a) + f64::from(b)) * 0.5) as f32
}

/// Blink's `CanonicalizeAngle` then `AdjustEndAngle`: the arc's start and end in radians.
pub fn arc_angles(start: f32, end: f32, ccw: bool) -> (f32, f32) {
    let (mut s, mut e) = (start, end);
    canonicalize_angle(&mut s, &mut e);
    (s, adjust_end_angle(s, e, ccw))
}

fn canonicalize_angle(start: &mut f32, end: &mut f32) {
    let mut new_start = *start % TAU;
    if new_start < 0.0 {
        new_start += TAU;
        if new_start >= TAU {
            new_start -= TAU;
        }
    }
    let delta = new_start - *start;
    *start = new_start;
    *end += delta;
}

fn adjust_end_angle(start: f32, end: f32, ccw: bool) -> f32 {
    if !ccw && end - start >= TAU {
        start + TAU
    } else if ccw && start - end >= TAU {
        start - TAU
    } else if !ccw && start > end {
        start + (TAU - (start - end) % TAU)
    } else if ccw && start < end {
        start - (TAU - (end - start) % TAU)
    } else {
        end
    }
}

fn snap_to_zero(v: f32) -> f32 {
    if nearly_zero(v) { 0.0 } else { v }
}

fn angles_to_unit_vectors(start: f32, sweep: f32) -> (Pt, Pt, bool) {
    let rad = PI / 180.0;
    let start_rad = start * rad;
    let mut stop_rad = (start + sweep) * rad;
    let unit = |a: f32| pt(snap_to_zero(a.cos()), snap_to_zero(a.sin()));
    let start_v = unit(start_rad);
    let mut stop_v = unit(stop_rad);
    if start_v == stop_v {
        let sw = sweep.abs();
        if sw < 360.0 && sw > 359.0 {
            let delta = (1.0f32 / 512.0).copysign(sweep);
            while start_v == stop_v {
                stop_rad -= delta;
                stop_v = unit(stop_rad);
            }
        }
    }
    (start_v, stop_v, sweep <= 0.0)
}

pub(crate) type ConicPts = ([Pt; 3], f32);

/// `SkConic::BuildUnitArc` followed by the oval's scale and translate (`user` is
/// `[sx, sy, tx, ty]`).
pub(crate) fn build_unit_arc(
    start: Pt,
    stop: Pt,
    ccw: bool,
    user: [f32; 4],
) -> Option<Vec<ConicPts>> {
    let x = start.x * stop.x + start.y * stop.y;
    let mut y = start.x * stop.y - start.y * stop.x;
    let abs_y = y.abs();
    if abs_y <= NEARLY_ZERO && x > 0.0 && ((y >= 0.0 && !ccw) || (y <= 0.0 && ccw)) {
        return None;
    }
    if ccw {
        y = -y;
    }
    let mut quadrant = 0;
    if y == 0.0 {
        quadrant = 2;
    } else if x == 0.0 {
        quadrant = if y > 0.0 { 1 } else { 3 };
    } else {
        if y < 0.0 {
            quadrant += 2;
        }
        if (x < 0.0) != (y < 0.0) {
            quadrant += 1;
        }
    }
    const QUADRANT_PTS: [Pt; 8] = [
        pt(1.0, 0.0),
        pt(1.0, 1.0),
        pt(0.0, 1.0),
        pt(-1.0, 1.0),
        pt(-1.0, 0.0),
        pt(-1.0, -1.0),
        pt(0.0, -1.0),
        pt(1.0, -1.0),
    ];
    let mut conics: Vec<ConicPts> = (0..quadrant)
        .map(|i| {
            (
                [
                    QUADRANT_PTS[i * 2],
                    QUADRANT_PTS[i * 2 + 1],
                    QUADRANT_PTS[(i * 2 + 2) % 8],
                ],
                FRAC_1_SQRT_2,
            )
        })
        .collect();
    let last_q = QUADRANT_PTS[quadrant * 2];
    let dot = last_q.x * x + last_q.y * y;
    if dot < 1.0 {
        let mut off = pt(last_q.x + x, last_q.y + y);
        let cos_half = ((1.0 + dot) / 2.0).sqrt();
        set_length(&mut off, 1.0 / cos_half);
        if last_q != off {
            conics.push(([last_q, off, pt(x, y)], cos_half));
        }
    }
    let (sin, cos) = (start.y, start.x);
    let (m00, m01, m10, m11) = if ccw {
        (cos, sin, sin, -cos)
    } else {
        (cos, -sin, sin, cos)
    };
    let [sx, sy, tx, ty] = user;
    let (a, b, c, d) = (sx * m00, sx * m01, sy * m10, sy * m11);
    for (pts, _) in &mut conics {
        for p in pts.iter_mut() {
            *p = pt(tx + a * p.x + b * p.y, ty + c * p.x + d * p.y);
        }
    }
    Some(conics)
}

fn set_length(v: &mut Pt, length: f32) {
    let (x, y) = (f64::from(v.x), f64::from(v.y));
    let scale = f64::from(length) / (x * x + y * y).sqrt();
    *v = pt((x * scale) as f32, (y * scale) as f32);
}

const MAX_CONIC_TO_QUAD_POW2: u32 = 5;

#[derive(Clone, Copy)]
struct Conic {
    pts: [Pt; 3],
    w: f32,
}

impl Conic {
    fn quad_pow2(&self, tol: f32) -> u32 {
        let a = self.w - 1.0;
        let k = a / (4.0 * (2.0 + a));
        let [p0, p1, p2] = self.pts;
        let x = k * (p0.x - 2.0 * p1.x + p2.x);
        let y = k * (p0.y - 2.0 * p1.y + p2.y);
        let mut error = (x * x + y * y).sqrt();
        let mut pow2 = 0;
        while pow2 < MAX_CONIC_TO_QUAD_POW2 {
            if error <= tol {
                break;
            }
            error *= 0.25;
            pow2 += 1;
        }
        pow2
    }

    /// `SkConic::chop` as Chromium builds it (`SK_SUPPORT_LEGACY_CONIC_CHOP`): the halves at
    /// t = 1/2.
    fn chop(&self) -> [Conic; 2] {
        let scale = 1.0 / (1.0 + self.w);
        let new_w = (0.5 + self.w * 0.5).sqrt();
        let [p0, p1, p2] = self.pts;
        let wp1 = pt(self.w * p1.x, self.w * p1.y);
        let mut m = pt(
            (p0.x + (wp1.x + wp1.x) + p2.x) * scale * 0.5,
            (p0.y + (wp1.y + wp1.y) + p2.y) * scale * 0.5,
        );
        if !(m.x.is_finite() && m.y.is_finite()) {
            let w = f64::from(self.w);
            let scale_half = 1.0 / (1.0 + w) * 0.5;
            let mid = |a: f32, b: f32, c: f32| {
                ((f64::from(a) + w * 2.0 * f64::from(b) + f64::from(c)) * scale_half) as f32
            };
            m = pt(mid(p0.x, p1.x, p2.x), mid(p0.y, p1.y, p2.y));
        }
        [
            Conic {
                pts: [p0, pt((p0.x + wp1.x) * scale, (p0.y + wp1.y) * scale), m],
                w: new_w,
            },
            Conic {
                pts: [m, pt((wp1.x + p2.x) * scale, (wp1.y + p2.y) * scale), p2],
                w: new_w,
            },
        ]
    }

    fn subdivide(&self, level: u32, out: &mut Vec<[Pt; 2]>) {
        if level == 0 {
            out.push([self.pts[1], self.pts[2]]);
            return;
        }
        let mut dst = self.chop();
        let start_y = self.pts[0].y;
        let end_y = self.pts[2].y;
        if between(start_y, self.pts[1].y, end_y) {
            let mid_y = dst[0].pts[2].y;
            if !between(start_y, mid_y, end_y) {
                let closer = if (mid_y - start_y).abs() < (mid_y - end_y).abs() {
                    start_y
                } else {
                    end_y
                };
                dst[0].pts[2].y = closer;
                dst[1].pts[0].y = closer;
            }
            if !between(start_y, dst[0].pts[1].y, dst[0].pts[2].y) {
                dst[0].pts[1].y = start_y;
            }
            if !between(dst[1].pts[0].y, dst[1].pts[1].y, end_y) {
                dst[1].pts[1].y = end_y;
            }
        }
        dst[0].subdivide(level - 1, out);
        dst[1].subdivide(level - 1, out);
    }

    fn to_quads(self, tol: f32) -> Vec<[Pt; 2]> {
        let pow2 = self.quad_pow2(tol);
        let mut out = Vec::with_capacity(1 << pow2);
        self.subdivide(pow2, &mut out);
        out
    }
}

fn between(a: f32, b: f32, c: f32) -> bool {
    (a - b) * (c - b) <= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verbs(p: &Path) -> &[Verb] {
        &p.verbs
    }

    #[test]
    fn line_to_on_an_empty_path_starts_a_subpath() {
        let mut p = Path::new();
        p.line_to(3.0, 4.0);
        assert_eq!(
            verbs(&p),
            [Verb::Move(pt(3.0, 4.0)), Verb::Line(pt(3.0, 4.0))]
        );
    }

    #[test]
    fn close_then_line_reinjects_the_subpath_start() {
        let mut p = Path::new();
        p.move_to(1.0, 1.0);
        p.line_to(5.0, 1.0);
        p.close();
        p.line_to(5.0, 5.0);
        assert_eq!(
            verbs(&p),
            [
                Verb::Move(pt(1.0, 1.0)),
                Verb::Line(pt(5.0, 1.0)),
                Verb::Close,
                Verb::Move(pt(1.0, 1.0)),
                Verb::Line(pt(5.0, 5.0)),
            ]
        );
    }

    #[test]
    fn full_circle_is_four_quadrant_conics() {
        let mut p = Path::new();
        p.arc(10.0, 10.0, 5.0, 0.0, std::f64::consts::TAU, false);
        let conics = verbs(&p)
            .iter()
            .filter(|v| matches!(v, Verb::Conic(..)))
            .count();
        assert_eq!(conics, 4);
        assert_eq!(verbs(&p)[0], Verb::Move(pt(15.0, 10.0)));
        let Some(Verb::Conic(_, end, w)) = verbs(&p).last().copied() else {
            panic!("ends in a conic");
        };
        assert_eq!(w, FRAC_1_SQRT_2);
        assert!((end.x - 15.0).abs() < 1e-4 && (end.y - 10.0).abs() < 1e-4);
    }

    #[test]
    fn arc_connects_from_the_current_point() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0);
        p.arc(10.0, 0.0, 5.0, std::f64::consts::PI, 0.0, false);
        assert_eq!(verbs(&p)[1], Verb::Line(pt(5.0, 0.0)));
    }

    #[test]
    fn empty_arc_is_a_line_to_its_start() {
        let mut p = Path::new();
        p.arc(10.0, 10.0, 4.0, 1.0, 1.0, false);
        let Verb::Line(end) = verbs(&p)[1] else {
            panic!("a line");
        };
        assert!((end.x - (10.0 + 4.0 * 1f32.cos())).abs() < 1e-5);
    }

    #[test]
    fn conic_quads_stay_on_the_circle() {
        let c = Conic {
            pts: [pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)],
            w: FRAC_1_SQRT_2,
        };
        let quads = c.to_quads(0.25);
        assert_eq!(quads.len(), 2);
        let end = quads.last().unwrap()[1];
        assert_eq!(end, pt(0.0, 10.0));
        let mid = quads[0][1];
        assert!(((mid.x * mid.x + mid.y * mid.y).sqrt() - 10.0).abs() < 1e-4);
    }

    #[test]
    fn affine_compose_and_invert() {
        let m = Affine::scaling(2.0, 2.0)
            .then(&Affine::translation(3.0, 4.0))
            .then(&Affine::rotation(0.5));
        let inv = m.inverse().unwrap();
        let id = m.then(&inv);
        for (got, want) in [id.a, id.b, id.c, id.d, id.e, id.f]
            .iter()
            .zip([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        {
            assert!((got - want).abs() < 1e-12);
        }
        assert_eq!(Affine::scaling(0.0, 1.0).inverse(), None);
    }
}
