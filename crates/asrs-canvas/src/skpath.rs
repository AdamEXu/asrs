//! A device-space path with Skia's `SkPath` layout (verbs, points, conic weights) and the queries
//! the scan converters make of it: bounds, convexity, `isRect` and the auto-closing edge walk.

use crate::hairline::P;
use crate::path::Affine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Move,
    Line,
    Quad,
    Conic,
    Cubic,
    Close,
}

impl Verb {
    pub fn points(self) -> usize {
        match self {
            Verb::Move | Verb::Line => 1,
            Verb::Quad | Verb::Conic => 2,
            Verb::Cubic => 3,
            Verb::Close => 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkPath {
    pub verbs: Vec<Verb>,
    pub pts: Vec<P>,
    pub weights: Vec<f32>,
    last_move: usize,
    needs_move: bool,
    convex: Option<bool>,
}

/// One segment of [`SkPath::edges`]: its points (2 for a line, 3 for a quad or conic, 4 for a
/// cubic; the rest repeat the last) and the conic weight.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub verb: Verb,
    pub pts: [P; 4],
    pub weight: f32,
}

impl Edge {
    pub fn quad_pts(&self) -> [P; 3] {
        [self.pts[0], self.pts[1], self.pts[2]]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn is_empty(&self) -> bool {
        !(self.left < self.right && self.top < self.bottom)
    }

    /// `SkRect::roundOut`, saturating like `sk_float_floor2int`.
    pub fn round_out(&self) -> [i32; 4] {
        [
            self.left.floor() as i32,
            self.top.floor() as i32,
            self.right.ceil() as i32,
            self.bottom.ceil() as i32,
        ]
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let l = self.left.max(o.left);
        let t = self.top.max(o.top);
        let r = self.right.min(o.right);
        let b = self.bottom.min(o.bottom);
        (l < r && t < b).then_some(Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        })
    }
}

/// `SkMatrix::mapPoints` for an affine matrix: scale-translate matrices map as `x * sx + tx`,
/// general ones as `tx + x * sx + y * kx`.
pub fn map_point(m: &Affine, p: P) -> P {
    let [sx, ky, kx, sy, tx, ty] = [m.a, m.b, m.c, m.d, m.e, m.f].map(|v| v as f32);
    if kx == 0.0 && ky == 0.0 {
        P {
            x: p.x * sx + tx,
            y: p.y * sy + ty,
        }
    } else {
        P {
            x: tx + p.x * sx + p.y * kx,
            y: ty + p.x * ky + p.y * sy,
        }
    }
}

impl SkPath {
    pub fn new() -> SkPath {
        SkPath::default()
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    pub fn last_pt(&self) -> Option<P> {
        self.pts.last().copied()
    }

    fn inject_move(&mut self) {
        if self.needs_move {
            let p = self
                .pts
                .get(self.last_move)
                .copied()
                .unwrap_or(P { x: 0.0, y: 0.0 });
            self.move_to(p);
        } else if self.verbs.is_empty() {
            self.move_to(P { x: 0.0, y: 0.0 });
        }
    }

    pub fn move_to(&mut self, p: P) {
        self.convex = None;
        if self.verbs.last() == Some(&Verb::Move) {
            let n = self.pts.len();
            self.pts[n - 1] = p;
        } else {
            self.verbs.push(Verb::Move);
            self.pts.push(p);
        }
        self.last_move = self.pts.len() - 1;
        self.needs_move = false;
    }

    pub fn line_to(&mut self, p: P) {
        self.inject_move();
        self.convex = None;
        self.verbs.push(Verb::Line);
        self.pts.push(p);
    }

    pub fn quad_to(&mut self, p1: P, p2: P) {
        self.inject_move();
        self.convex = None;
        self.verbs.push(Verb::Quad);
        self.pts.extend([p1, p2]);
    }

    pub fn cubic_to(&mut self, p1: P, p2: P, p3: P) {
        self.inject_move();
        self.convex = None;
        self.verbs.push(Verb::Cubic);
        self.pts.extend([p1, p2, p3]);
    }

    /// `SkPathBuilder::conicTo`: weight 1 is a quad, a non-finite or non-positive weight a line.
    pub fn conic_to(&mut self, p1: P, p2: P, w: f32) {
        if w == 1.0 {
            self.quad_to(p1, p2);
        } else {
            self.inject_move();
            self.convex = None;
            self.verbs.push(Verb::Conic);
            self.pts.extend([p1, p2]);
            self.weights.push(w);
        }
    }

    pub fn close(&mut self) {
        if !self.verbs.is_empty() && self.verbs.last() != Some(&Verb::Close) {
            self.verbs.push(Verb::Close);
            self.needs_move = true;
        }
    }

    /// The same geometry as a tiny-skia path (lines and quads only).
    pub fn from_tiny(path: &tiny_skia::Path) -> SkPath {
        let p = |q: tiny_skia::Point| P { x: q.x, y: q.y };
        let mut out = SkPath::new();
        for seg in path.segments() {
            match seg {
                tiny_skia::PathSegment::MoveTo(a) => out.move_to(p(a)),
                tiny_skia::PathSegment::LineTo(a) => out.line_to(p(a)),
                tiny_skia::PathSegment::QuadTo(a, b) => out.quad_to(p(a), p(b)),
                tiny_skia::PathSegment::CubicTo(a, b, c) => out.cubic_to(p(a), p(b), p(c)),
                tiny_skia::PathSegment::Close => out.close(),
            }
        }
        out
    }

    /// The path as tiny-skia geometry, conics split into quads to within a quarter pixel.
    pub fn to_tiny(&self) -> Option<tiny_skia::Path> {
        let mut b = tiny_skia::PathBuilder::new();
        let mut pi = 0;
        let mut wi = 0;
        let mut last = P { x: 0.0, y: 0.0 };
        for &v in &self.verbs {
            match v {
                Verb::Move => {
                    last = self.pts[pi];
                    b.move_to(last.x, last.y);
                    pi += 1;
                }
                Verb::Line => {
                    last = self.pts[pi];
                    b.line_to(last.x, last.y);
                    pi += 1;
                }
                Verb::Quad => {
                    let (c, e) = (self.pts[pi], self.pts[pi + 1]);
                    b.quad_to(c.x, c.y, e.x, e.y);
                    last = e;
                    pi += 2;
                }
                Verb::Conic => {
                    let pts = [last, self.pts[pi], self.pts[pi + 1]];
                    for q in crate::path::conic_to_quads(pts, self.weights[wi], 0.25) {
                        b.quad_to(q[1].x, q[1].y, q[2].x, q[2].y);
                    }
                    last = pts[2];
                    pi += 2;
                    wi += 1;
                }
                Verb::Cubic => {
                    let (c1, c2, e) = (self.pts[pi], self.pts[pi + 1], self.pts[pi + 2]);
                    b.cubic_to(c1.x, c1.y, c2.x, c2.y, e.x, e.y);
                    last = e;
                    pi += 3;
                }
                Verb::Close => b.close(),
            }
        }
        b.finish()
    }

    pub fn set_convex(&mut self, convex: bool) {
        self.convex = Some(convex);
    }

    /// `SkPath::transform` for an affine matrix: points mapped, conic weights kept.
    pub fn transformed(&self, m: &Affine) -> SkPath {
        let mut out = self.clone();
        for p in &mut out.pts {
            *p = map_point(m, *p);
        }
        out.convex = None;
        out
    }

    pub fn is_finite(&self) -> bool {
        self.pts.iter().all(|p| p.x.is_finite() && p.y.is_finite())
    }

    /// `SkRect::Bounds` of the points, empty when there are none or any is not finite.
    pub fn bounds(&self) -> Rect {
        let empty = Rect {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        if self.pts.is_empty() || !self.is_finite() {
            return empty;
        }
        let mut r = Rect {
            left: self.pts[0].x,
            top: self.pts[0].y,
            right: self.pts[0].x,
            bottom: self.pts[0].y,
        };
        for p in &self.pts[1..] {
            r.left = r.left.min(p.x);
            r.top = r.top.min(p.y);
            r.right = r.right.max(p.x);
            r.bottom = r.bottom.max(p.y);
        }
        r
    }

    pub fn only_lines(&self) -> bool {
        self.verbs
            .iter()
            .all(|v| matches!(v, Verb::Move | Verb::Line | Verb::Close))
    }

    /// `SkPathEdgeIter`: every segment, with a closing line for each contour that does not end
    /// where it started.
    pub fn edges(&self) -> Vec<Edge> {
        let mut out = Vec::new();
        let (mut pi, mut wi) = (0usize, 0usize);
        let mut move_pt = P { x: 0.0, y: 0.0 };
        let mut needs_close = false;
        let close_line = |out: &mut Vec<Edge>, last: P, first: P| {
            out.push(Edge {
                verb: Verb::Line,
                pts: [last, first, first, first],
                weight: 1.0,
            });
        };
        for &verb in &self.verbs {
            match verb {
                Verb::Move => {
                    if needs_close {
                        close_line(&mut out, self.pts[pi - 1], move_pt);
                        needs_close = false;
                    }
                    move_pt = self.pts[pi];
                    pi += 1;
                }
                Verb::Close => {
                    if needs_close {
                        close_line(&mut out, self.pts[pi - 1], move_pt);
                        needs_close = false;
                    }
                }
                _ => {
                    let n = verb.points();
                    let start = self.pts[pi - 1];
                    let mut pts = [start; 4];
                    pts[1..=n].copy_from_slice(&self.pts[pi..pi + n]);
                    for i in n + 1..4 {
                        pts[i] = pts[n];
                    }
                    let weight = if verb == Verb::Conic {
                        wi += 1;
                        self.weights[wi - 1]
                    } else {
                        1.0
                    };
                    out.push(Edge { verb, pts, weight });
                    pi += n;
                    needs_close = true;
                }
            }
        }
        if needs_close {
            close_line(&mut out, self.pts[pi - 1], move_pt);
        }
        out
    }

    /// `SkPath::isConvex`, computed like `SkPathPriv::ComputeConvexity` unless set.
    pub fn is_convex(&self) -> bool {
        if let Some(c) = self.convex {
            return c;
        }
        self.is_finite() && compute_convexity(&self.pts, &self.verbs)
    }

    /// `SkPathPriv::IsRectContour(pts, verbs, false)`: the rect when the path is one closed or
    /// open axis-aligned rectangle.
    pub fn is_rect(&self) -> Option<Rect> {
        self.rect_contour().map(|(r, _, _)| r)
    }

    /// `IsRectContour` with whether the contour is closed and runs clockwise.
    pub fn rect_contour(&self) -> Option<(Rect, bool, bool)> {
        if self.pts.len() < 4 {
            return None;
        }
        let pts = &self.pts;
        let mut pi = 0usize;
        let mut corners = 0;
        let mut line_start = P { x: 0.0, y: 0.0 };
        let mut first_pt: Option<usize> = None;
        let mut last_pt: Option<usize> = None;
        let mut first_corner = P { x: 0.0, y: 0.0 };
        let mut third_corner = P { x: 0.0, y: 0.0 };
        let mut directions = [-1i32; 5];
        let mut closed_or_moved = false;
        let mut auto_close = false;
        for &verb in &self.verbs {
            match verb {
                Verb::Close | Verb::Line => {
                    if verb == Verb::Close {
                        auto_close = true;
                    } else {
                        last_pt = Some(pi);
                    }
                    let line_end = if verb == Verb::Close {
                        pts[first_pt?]
                    } else {
                        pi += 1;
                        pts[pi - 1]
                    };
                    let (dx, dy) = (line_end.x - line_start.x, line_end.y - line_start.y);
                    if dx != 0.0 && dy != 0.0 {
                        return None;
                    }
                    if !(dx.is_finite() && dy.is_finite()) {
                        return None;
                    }
                    if line_start == line_end {
                        continue;
                    }
                    let next = i32::from(dx != 0.0) | (i32::from(dx > 0.0 || dy > 0.0) << 1);
                    if corners == 0 {
                        directions[0] = next;
                        corners = 1;
                        closed_or_moved = false;
                        line_start = line_end;
                        continue;
                    }
                    if closed_or_moved {
                        return None;
                    }
                    if auto_close && next == directions[0] {
                        continue;
                    }
                    closed_or_moved = auto_close;
                    if directions[corners - 1] == next {
                        if corners == 3 && verb == Verb::Line {
                            third_corner = line_end;
                        }
                        line_start = line_end;
                        continue;
                    }
                    directions[corners] = next;
                    corners += 1;
                    match corners {
                        2 => first_corner = line_start,
                        3 => {
                            if directions[0] ^ directions[2] != 2 {
                                return None;
                            }
                            third_corner = line_end;
                        }
                        4 => {
                            if directions[1] ^ directions[3] != 2 {
                                return None;
                            }
                        }
                        _ => return None,
                    }
                    line_start = line_end;
                }
                Verb::Quad | Verb::Conic | Verb::Cubic => return None,
                Verb::Move => {
                    if corners == 0 {
                        first_pt = Some(pi);
                    } else {
                        let (f, l) = (pts[first_pt?], pts[last_pt?]);
                        if f.x - l.x != 0.0 && f.y - l.y != 0.0 {
                            return None;
                        }
                    }
                    line_start = pts[pi];
                    pi += 1;
                    closed_or_moved = true;
                }
            }
        }
        if !(3..=4).contains(&corners) {
            return None;
        }
        let (f, l) = (pts[first_pt?], pts[last_pt?]);
        if f.x - l.x != 0.0 && f.y - l.y != 0.0 {
            return None;
        }
        let rect = Rect {
            left: first_corner.x.min(third_corner.x),
            top: first_corner.y.min(third_corner.y),
            right: first_corner.x.max(third_corner.x),
            bottom: first_corner.y.max(third_corner.y),
        };
        Some((rect, auto_close, directions[0] == ((directions[1] + 1) & 3)))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DirChange {
    Left,
    Right,
    Straight,
    Backwards,
    Unknown,
    Invalid,
}

struct Convexicator {
    first_pt: P,
    first_vec: P,
    last_pt: P,
    last_vec: P,
    expected: DirChange,
    first_direction_known: bool,
    reversals: i32,
}

impl Convexicator {
    fn new() -> Convexicator {
        let zero = P { x: 0.0, y: 0.0 };
        Convexicator {
            first_pt: zero,
            first_vec: zero,
            last_pt: zero,
            last_vec: zero,
            expected: DirChange::Invalid,
            first_direction_known: false,
            reversals: 0,
        }
    }

    fn set_move(&mut self, p: P) {
        self.first_pt = p;
        self.last_pt = p;
        self.expected = DirChange::Invalid;
    }

    fn add_pt(&mut self, p: P) -> bool {
        if self.last_pt == p {
            return true;
        }
        let v = P {
            x: p.x - self.last_pt.x,
            y: p.y - self.last_pt.y,
        };
        if self.first_pt == self.last_pt
            && self.expected == DirChange::Invalid
            && self.last_vec.x == 0.0
            && self.last_vec.y == 0.0
        {
            self.last_vec = v;
            self.first_vec = v;
        } else if !self.add_vec(v) {
            return false;
        }
        self.last_pt = p;
        true
    }

    fn direction_change(&self, v: P) -> DirChange {
        let cross = self.last_vec.x * v.y - self.last_vec.y * v.x;
        if !cross.is_finite() {
            return DirChange::Unknown;
        }
        if cross == 0.0 {
            let dot = self.last_vec.x * v.x + self.last_vec.y * v.y;
            return if dot < 0.0 {
                DirChange::Backwards
            } else {
                DirChange::Straight
            };
        }
        if cross > 0.0 {
            DirChange::Right
        } else {
            DirChange::Left
        }
    }

    fn add_vec(&mut self, v: P) -> bool {
        match self.direction_change(v) {
            d @ (DirChange::Left | DirChange::Right) => {
                if self.expected == DirChange::Invalid {
                    self.expected = d;
                    self.first_direction_known = true;
                } else if d != self.expected {
                    self.first_direction_known = false;
                    return false;
                }
                self.last_vec = v;
                true
            }
            DirChange::Straight => true,
            DirChange::Backwards => {
                self.last_vec = v;
                self.reversals += 1;
                self.reversals < 3
            }
            DirChange::Unknown | DirChange::Invalid => false,
        }
    }

    fn close(&mut self) -> bool {
        self.add_pt(self.first_pt) && self.add_vec(self.first_vec)
    }
}

fn concave_by_sign(points: &[P]) -> bool {
    if points.len() <= 3 {
        return false;
    }
    let sign = |v: f32| i32::from(v < 0.0);
    let mut curr = points[0];
    let (mut dxes, mut dyes) = (0, 0);
    let (mut last_sx, mut last_sy) = (2, 2);
    let rest = points[1..].iter().chain(std::iter::once(&points[0]));
    for &p in rest {
        let (vx, vy) = (p.x - curr.x, p.y - curr.y);
        if vx != 0.0 || vy != 0.0 {
            if !(vx.is_finite() && vy.is_finite()) {
                return true;
            }
            let (sx, sy) = (sign(vx), sign(vy));
            dxes += i32::from(sx != last_sx);
            dyes += i32::from(sy != last_sy);
            if dxes > 3 || dyes > 3 {
                return true;
            }
            last_sx = sx;
            last_sy = sy;
        }
        curr = p;
    }
    false
}

fn compute_convexity(pts: &[P], verbs: &[Verb]) -> bool {
    let mut verbs = verbs;
    let mut pts = pts;
    while let Some((&Verb::Move, rest)) = verbs.split_last() {
        verbs = rest;
        pts = &pts[..pts.len() - 1];
    }
    if verbs.is_empty() {
        return true;
    }
    if concave_by_sign(pts) {
        return false;
    }
    let mut contours = 0;
    let mut needs_close = false;
    let mut state = Convexicator::new();
    let mut pi = 0usize;
    for &verb in verbs {
        let start = pi.saturating_sub(1);
        let n = verb.points();
        let seg = &pts[if verb == Verb::Move { pi } else { start }..pi + n];
        pi += n;
        if contours == 0 {
            if verb == Verb::Move {
                state.set_move(seg[0]);
            } else {
                contours = 1;
                needs_close = true;
            }
        }
        if contours == 1 {
            if verb == Verb::Close || verb == Verb::Move {
                if !state.close() {
                    return false;
                }
                needs_close = false;
                contours += 1;
            } else {
                for &p in &seg[1..] {
                    if !state.add_pt(p) {
                        return false;
                    }
                }
            }
        } else if verb != Verb::Move {
            return false;
        }
    }
    if needs_close && !state.close() {
        return false;
    }
    !(!state.first_direction_known && state.reversals >= 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> P {
        P { x, y }
    }

    #[test]
    fn rect_contours_are_rects_and_convex() {
        let mut path = SkPath::new();
        path.move_to(p(1.0, 2.0));
        path.line_to(p(5.0, 2.0));
        path.line_to(p(5.0, 7.0));
        path.line_to(p(1.0, 7.0));
        path.close();
        assert_eq!(
            path.is_rect(),
            Some(Rect {
                left: 1.0,
                top: 2.0,
                right: 5.0,
                bottom: 7.0
            })
        );
        assert!(path.is_convex());
        path.move_to(p(10.0, 10.0));
        path.line_to(p(12.0, 10.0));
        path.line_to(p(12.0, 12.0));
        assert!(!path.is_convex());
    }

    #[test]
    fn bow_tie_is_concave() {
        let mut path = SkPath::new();
        path.move_to(p(0.0, 0.0));
        path.line_to(p(4.0, 4.0));
        path.line_to(p(4.0, 0.0));
        path.line_to(p(0.0, 4.0));
        path.close();
        assert!(!path.is_convex());
        assert_eq!(path.is_rect(), None);
    }

    #[test]
    fn edges_close_open_contours() {
        let mut path = SkPath::new();
        path.move_to(p(0.0, 0.0));
        path.line_to(p(4.0, 0.0));
        path.line_to(p(4.0, 4.0));
        let edges = path.edges();
        assert_eq!(edges.len(), 3);
        assert_eq!(edges[2].pts[..2], [p(4.0, 4.0), p(0.0, 0.0)]);
    }
}
