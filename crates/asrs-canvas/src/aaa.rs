// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! Skia's analytic anti-aliasing (`SkScan_AAAPath.cpp`, `SkAnalyticEdge.cpp` and the analytic
//! half of `SkEdgeBuilder.cpp`, Chromium m142), ported operation for operation in 16.16 fixed
//! point. Chromium's CPU canvas fills every antialiased path with it, so its rounding (quarter
//! pixel y snapping, 1/16 pixel x snapping, the trapezoid alpha approximations, the RLE blitter's
//! alpha snapping) decides the low bits of every edge pixel.

use crate::blit::Spans;
use crate::clip::{self, ClipRect, Piece};
use crate::hairline::P;
use crate::skpath::{Rect, SkPath, Verb};

const ONE: i32 = 1 << 16;
const HALF: i32 = 1 << 15;
const NONE: usize = usize::MAX;
const ACCURACY: i32 = 2;

fn fmul(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> 16) as i32
}

fn fdiv(n: i32, d: i32) -> i32 {
    ((i64::from(n) << 16) / i64::from(d)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn fdot6_div(a: i32, b: i32) -> i32 {
    if i32::from(a as i16) == a {
        (a << 16) / b
    } else {
        fdiv(a, b)
    }
}

fn fdot6_to_fixed(x: i32) -> i32 {
    ((x as u32) << 10) as i32
}

fn fixed_to_fdot6(x: i32) -> i32 {
    x >> 10
}

fn ceil_to_int(x: i32) -> i32 {
    x.wrapping_add(ONE - 1) >> 16
}

fn floor_to_int(x: i32) -> i32 {
    x >> 16
}

fn round_to_int(x: i32) -> i32 {
    x.wrapping_add(HALF) >> 16
}

fn round_to_fixed(x: i32) -> i32 {
    (x.wrapping_add(HALF) as u32 & 0xFFFF_0000) as i32
}

fn ceil_to_fixed(x: i32) -> i32 {
    (x.wrapping_add(ONE - 1) as u32 & 0xFFFF_0000) as i32
}

fn floor_to_fixed(x: i32) -> i32 {
    (x as u32 & 0xFFFF_0000) as i32
}

fn int_to_fixed(n: i32) -> i32 {
    ((n as u32) << 16) as i32
}

fn sat_add(a: i32, b: i32) -> i32 {
    (i64::from(a) + i64::from(b)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn sat_sub(a: i32, b: i32) -> i32 {
    (i64::from(a) - i64::from(b)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn snap_y(y: i32) -> i32 {
    ((y as u32).wrapping_add((ONE >> (ACCURACY + 1)) as u32) >> (16 - ACCURACY) << (16 - ACCURACY))
        as i32
}

/// The `quick_inverse` table of `SkAnalyticEdge.cpp`, which is exactly `(1 << 22) / x`
/// truncated toward zero for `0 < |x| <= 1024`.
fn quick_inverse(x: i32) -> i32 {
    match x {
        0 => 0,
        x if x > 0 => (1 << 22) / x,
        x => -((1 << 22) / -x),
    }
}

fn quick_div(a: i32, b: i32) -> i32 {
    let (abs_a, abs_b) = (a.wrapping_abs(), b.wrapping_abs());
    if (8..1024).contains(&abs_b) && abs_a < (1 << 12) {
        (a * quick_inverse(b)) >> 6
    } else {
        fdot6_div(a, b)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Line,
    Quad,
    Cubic,
}

#[derive(Clone, Copy)]
struct Edge {
    next: usize,
    prev: usize,
    x: i32,
    dx: i32,
    upper_x: i32,
    y: i32,
    upper_y: i32,
    lower_y: i32,
    dy: i32,
    kind: Kind,
    curve_count: i8,
    curve_shift: u8,
    winding: i8,
    qx: i32,
    qy: i32,
    qdx: i32,
    qdy: i32,
    qddx: i32,
    qddy: i32,
    cdddx: i32,
    cdddy: i32,
    to_fixed_shift: u8,
    q_last_x: i32,
    q_last_y: i32,
    snapped_x: i32,
    snapped_y: i32,
}

impl Edge {
    fn blank() -> Edge {
        Edge {
            next: NONE,
            prev: NONE,
            x: 0,
            dx: 0,
            upper_x: 0,
            y: 0,
            upper_y: 0,
            lower_y: 0,
            dy: 0,
            kind: Kind::Line,
            curve_count: 0,
            curve_shift: 0,
            winding: 1,
            qx: 0,
            qy: 0,
            qdx: 0,
            qdy: 0,
            qddx: 0,
            qddy: 0,
            cdddx: 0,
            cdddy: 0,
            to_fixed_shift: 0,
            q_last_x: 0,
            q_last_y: 0,
            snapped_x: 0,
            snapped_y: 0,
        }
    }

    fn go_y(&mut self, y: i32) {
        if y == self.y.wrapping_add(ONE) {
            self.x = self.x.wrapping_add(self.dx);
            self.y = y;
        } else if y != self.y {
            self.x = self
                .upper_x
                .wrapping_add(fmul(self.dx, y.wrapping_sub(self.upper_y)));
            self.y = y;
        }
    }

    fn go_y_shift(&mut self, y: i32, shift: i32) {
        self.y = y;
        self.x = self.x.wrapping_add(self.dx >> shift);
    }

    fn set_line(p0: P, p1: P) -> Option<Edge> {
        let fx = |v: f32| fdot6_to_fixed((v * 4.0 * 64.0) as i32) >> ACCURACY;
        let (mut x0, mut y0) = (fx(p0.x), snap_y(fx(p0.y)));
        let (mut x1, mut y1) = (fx(p1.x), snap_y(fx(p1.y)));
        let mut winding = 1;
        if y0 > y1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
            winding = -1;
        }
        let dy = fixed_to_fdot6(y1.wrapping_sub(y0));
        if dy == 0 {
            return None;
        }
        let dx = fixed_to_fdot6(x1.wrapping_sub(x0));
        let slope = quick_div(dx, dy);
        let abs_slope = slope.wrapping_abs();
        let mut e = Edge::blank();
        e.x = x0;
        e.dx = slope;
        e.upper_x = x0;
        e.y = y0;
        e.upper_y = y0;
        e.lower_y = y1;
        e.dy = if dx == 0 || slope == 0 {
            i32::MAX
        } else if abs_slope < 1024 {
            quick_inverse(abs_slope)
        } else {
            quick_div(dy, dx).wrapping_abs()
        };
        e.winding = winding;
        Some(e)
    }

    fn update_line(
        &mut self,
        mut x0: i32,
        mut y0: i32,
        mut x1: i32,
        mut y1: i32,
        slope: i32,
    ) -> bool {
        if y0 > y1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
            self.winding = -self.winding;
        }
        let dx = fixed_to_fdot6(x1.wrapping_sub(x0));
        let dy = fixed_to_fdot6(y1.wrapping_sub(y0));
        if dy == 0 {
            return false;
        }
        let abs_slope = fixed_to_fdot6(slope).wrapping_abs();
        self.x = x0;
        self.dx = slope;
        self.upper_x = x0;
        self.y = y0;
        self.upper_y = y0;
        self.lower_y = y1;
        self.dy = if dx == 0 || slope == 0 {
            i32::MAX
        } else if abs_slope < 1024 {
            quick_inverse(abs_slope)
        } else {
            quick_div(dy, dx).wrapping_abs()
        };
        true
    }

    fn set_quad(pts: &[P; 3]) -> Option<Edge> {
        let scale = (1 << (ACCURACY + 6)) as f32;
        let i = |v: f32| (v * scale) as i32;
        let (mut x0, mut y0) = (i(pts[0].x), i(pts[0].y));
        let (x1, y1) = (i(pts[1].x), i(pts[1].y));
        let (mut x2, mut y2) = (i(pts[2].x), i(pts[2].y));
        let mut winding = 1;
        if y0 > y2 {
            std::mem::swap(&mut x0, &mut x2);
            std::mem::swap(&mut y0, &mut y2);
            winding = -1;
        }
        if (y0.wrapping_add(32) >> 6) == (y2.wrapping_add(32) >> 6) {
            return None;
        }
        let dx = (((x1 as u32) << 1) as i32)
            .wrapping_sub(x0)
            .wrapping_sub(x2)
            >> 2;
        let dy = (((y1 as u32) << 1) as i32)
            .wrapping_sub(y0)
            .wrapping_sub(y2)
            >> 2;
        let mut shift = diff_to_shift(dx, dy, ACCURACY);
        if shift == 0 {
            shift = 1;
        } else if shift > 6 {
            shift = 6;
        }
        let mut e = Edge::blank();
        e.winding = winding;
        e.kind = Kind::Quad;
        e.curve_count = (1i32 << shift) as i8;
        e.curve_shift = (shift - 1) as u8;
        let div2 = |v: i32| ((v as u32) << 9) as i32;
        let a = div2(x0.wrapping_sub(x1).wrapping_sub(x1).wrapping_add(x2));
        let b = fdot6_to_fixed(x1.wrapping_sub(x0));
        e.qx = fdot6_to_fixed(x0);
        e.qdx = b.wrapping_add(a >> shift);
        e.qddx = a >> (shift - 1);
        let a = div2(y0.wrapping_sub(y1).wrapping_sub(y1).wrapping_add(y2));
        let b = fdot6_to_fixed(y1.wrapping_sub(y0));
        e.qy = fdot6_to_fixed(y0);
        e.qdy = b.wrapping_add(a >> shift);
        e.qddy = a >> (shift - 1);
        e.q_last_x = fdot6_to_fixed(x2);
        e.q_last_y = fdot6_to_fixed(y2);
        e.qx >>= ACCURACY;
        e.qy >>= ACCURACY;
        e.qdx >>= ACCURACY;
        e.qdy >>= ACCURACY;
        e.qddx >>= ACCURACY;
        e.qddy >>= ACCURACY;
        e.q_last_x >>= ACCURACY;
        e.q_last_y >>= ACCURACY;
        e.qy = snap_y(e.qy);
        e.q_last_y = snap_y(e.q_last_y);
        e.snapped_x = e.qx;
        e.snapped_y = e.qy;
        e.update_quad().then_some(e)
    }

    fn update_quad(&mut self) -> bool {
        let mut success = false;
        let mut count = i32::from(self.curve_count);
        let (mut oldx, mut oldy) = (self.qx, self.qy);
        let (mut dx, mut dy) = (self.qdx, self.qdy);
        let (mut newx, mut newy, mut new_snapped_x, mut new_snapped_y);
        let shift = i32::from(self.curve_shift);
        loop {
            let slope;
            count -= 1;
            if count > 0 {
                newx = oldx.wrapping_add(dx >> shift);
                newy = oldy.wrapping_add(dy >> shift);
                if (dy >> shift).wrapping_abs() >= ONE * 2
                    && (i64::from(dy.wrapping_abs()) << 6) > i64::from(dx.wrapping_abs())
                {
                    let diff_y = fixed_to_fdot6(newy.wrapping_sub(self.snapped_y));
                    slope = if diff_y != 0 {
                        quick_div(fixed_to_fdot6(newx.wrapping_sub(self.snapped_x)), diff_y)
                    } else {
                        i32::MAX
                    };
                    new_snapped_y = self.q_last_y.min(round_to_fixed(newy));
                    new_snapped_x =
                        newx.wrapping_sub(fmul(slope, newy.wrapping_sub(new_snapped_y)));
                } else {
                    new_snapped_y = self.q_last_y.min(snap_y(newy));
                    new_snapped_x = newx;
                    let diff_y = fixed_to_fdot6(new_snapped_y.wrapping_sub(self.snapped_y));
                    slope = if diff_y != 0 {
                        quick_div(fixed_to_fdot6(newx.wrapping_sub(self.snapped_x)), diff_y)
                    } else {
                        i32::MAX
                    };
                }
                dx = dx.wrapping_add(self.qddx);
                dy = dy.wrapping_add(self.qddy);
            } else {
                newx = self.q_last_x;
                newy = self.q_last_y;
                new_snapped_y = newy;
                new_snapped_x = newx;
                let diff_y = fixed_to_fdot6(newy.wrapping_sub(self.snapped_y));
                slope = if diff_y != 0 {
                    quick_div(fixed_to_fdot6(newx.wrapping_sub(self.snapped_x)), diff_y)
                } else {
                    i32::MAX
                };
            }
            if slope < i32::MAX {
                success = self.update_line(
                    self.snapped_x,
                    self.snapped_y,
                    new_snapped_x,
                    new_snapped_y,
                    slope,
                );
            }
            oldx = newx;
            oldy = newy;
            if !(count > 0 && !success) {
                break;
            }
        }
        self.qx = newx;
        self.qy = newy;
        self.qdx = dx;
        self.qdy = dy;
        self.snapped_x = new_snapped_x;
        self.snapped_y = new_snapped_y;
        self.curve_count = count as i8;
        success
    }

    /// `SkAnalyticCubicEdge::setCubic`, with `setCubicWithoutUpdate`.
    fn set_cubic(pts: &[P; 4]) -> Option<Edge> {
        let scale = (1 << (ACCURACY + 6)) as f32;
        let i = |v: f32| (v * scale) as i32;
        let [mut x0, mut x1, mut x2, mut x3] = pts.map(|p| i(p.x));
        let [mut y0, mut y1, mut y2, mut y3] = pts.map(|p| i(p.y));
        let mut winding = 1;
        if y0 > y3 {
            std::mem::swap(&mut x0, &mut x3);
            std::mem::swap(&mut x1, &mut x2);
            std::mem::swap(&mut y0, &mut y3);
            std::mem::swap(&mut y1, &mut y2);
            winding = -1;
        }
        if (y0.wrapping_add(32) >> 6) == (y3.wrapping_add(32) >> 6) {
            return None;
        }
        let dx = cubic_delta_from_line(x0, x1, x2, x3);
        let dy = cubic_delta_from_line(y0, y1, y2, y3);
        let shift = (diff_to_shift(dx, dy, 2) + 1).min(6);
        let mut up_shift = 6;
        let mut down_shift = shift + up_shift - 10;
        if down_shift < 0 {
            down_shift = 0;
            up_shift = 10 - shift;
        }
        let mut e = Edge::blank();
        e.winding = winding;
        e.kind = Kind::Cubic;
        e.curve_count = (-1i32 << shift) as i8;
        e.curve_shift = shift as u8;
        e.to_fixed_shift = down_shift as u8;
        let up = |v: i32| ((v as u32) << up_shift) as i32;
        let coeffs = |p0: i32, p1: i32, p2: i32, p3: i32| {
            let b = up(p1.wrapping_sub(p0).wrapping_mul(3));
            let c = up(p0
                .wrapping_sub(p1)
                .wrapping_sub(p1)
                .wrapping_add(p2)
                .wrapping_mul(3));
            let d = up(p3
                .wrapping_add(p1.wrapping_sub(p2).wrapping_mul(3))
                .wrapping_sub(p0));
            let d3 = d.wrapping_mul(3) >> (shift - 1);
            (
                b.wrapping_add(c >> shift).wrapping_add(d >> (2 * shift)),
                c.wrapping_mul(2).wrapping_add(d3),
                d3,
            )
        };
        (e.qdx, e.qddx, e.cdddx) = coeffs(x0, x1, x2, x3);
        (e.qdy, e.qddy, e.cdddy) = coeffs(y0, y1, y2, y3);
        e.qx = fdot6_to_fixed(x0) >> ACCURACY;
        e.qy = snap_y(fdot6_to_fixed(y0) >> ACCURACY);
        e.qdx >>= ACCURACY;
        e.qdy >>= ACCURACY;
        e.qddx >>= ACCURACY;
        e.qddy >>= ACCURACY;
        e.cdddx >>= ACCURACY;
        e.cdddy >>= ACCURACY;
        e.q_last_x = fdot6_to_fixed(x3) >> ACCURACY;
        e.q_last_y = snap_y(fdot6_to_fixed(y3) >> ACCURACY);
        e.snapped_y = e.qy;
        e.update_cubic().then_some(e)
    }

    /// `SkAnalyticCubicEdge::updateCubic`.
    fn update_cubic(&mut self) -> bool {
        let mut success;
        let mut count = i32::from(self.curve_count);
        let (mut oldx, mut oldy) = (self.qx, self.qy);
        let (mut newx, mut newy);
        let ddshift = i32::from(self.curve_shift);
        let dshift = i32::from(self.to_fixed_shift);
        loop {
            count += 1;
            if count < 0 {
                newx = oldx.wrapping_add(self.qdx >> dshift);
                self.qdx = self.qdx.wrapping_add(self.qddx >> ddshift);
                self.qddx = self.qddx.wrapping_add(self.cdddx);
                newy = oldy.wrapping_add(self.qdy >> dshift);
                self.qdy = self.qdy.wrapping_add(self.qddy >> ddshift);
                self.qddy = self.qddy.wrapping_add(self.cdddy);
            } else {
                newx = self.q_last_x;
                newy = self.q_last_y;
            }
            if newy < oldy {
                newy = oldy;
            }
            let mut new_snapped_y = snap_y(newy);
            if self.q_last_y < new_snapped_y {
                new_snapped_y = self.q_last_y;
                count = 0;
            }
            let diff_y = fixed_to_fdot6(new_snapped_y.wrapping_sub(self.snapped_y));
            let slope = if diff_y == 0 {
                i32::MAX
            } else {
                fdot6_div(fixed_to_fdot6(newx.wrapping_sub(oldx)), diff_y)
            };
            success = self.update_line(oldx, self.snapped_y, newx, new_snapped_y, slope);
            oldx = newx;
            oldy = newy;
            self.snapped_y = new_snapped_y;
            if !(count < 0 && !success) {
                break;
            }
        }
        self.qx = newx;
        self.qy = newy;
        self.curve_count = count as i8;
        success
    }

    fn keep_continuous(&mut self) {
        if self.kind == Kind::Cubic {
            self.qx = self.x;
        } else {
            self.snapped_x = self.x;
        }
        self.snapped_y = self.y;
    }

    /// `SkAnalyticEdge::update`.
    fn update(&mut self) -> bool {
        match self.curve_count {
            0 => false,
            c if c < 0 => self.update_cubic(),
            _ => self.update_quad(),
        }
    }
}

fn cheap_distance(dx: i32, dy: i32) -> i32 {
    let (dx, dy) = (dx.wrapping_abs(), dy.wrapping_abs());
    if dx > dy {
        dx.wrapping_add(dy >> 1)
    } else {
        dy.wrapping_add(dx >> 1)
    }
}

fn cubic_delta_from_line(a: i32, b: i32, c: i32, d: i32) -> i32 {
    let one_third = a
        .wrapping_mul(8)
        .wrapping_sub(b.wrapping_mul(15))
        .wrapping_add(c.wrapping_mul(6))
        .wrapping_add(d)
        .wrapping_mul(19)
        >> 9;
    let two_third = a
        .wrapping_add(b.wrapping_mul(6))
        .wrapping_sub(c.wrapping_mul(15))
        .wrapping_add(d.wrapping_mul(8))
        .wrapping_mul(19)
        >> 9;
    one_third.wrapping_abs().max(two_third.wrapping_abs())
}

fn diff_to_shift(dx: i32, dy: i32, shift_aa: i32) -> i32 {
    let dist = cheap_distance(dx, dy);
    let dist = dist.wrapping_add(1 << (2 + shift_aa)) >> (3 + shift_aa);
    (32 - dist.leading_zeros() as i32) >> 1
}

/// `SkAnalyticEdgeBuilder`.
struct Builder {
    edges: Vec<Edge>,
}

enum Combine {
    No,
    Partial,
    Total,
}

impl Builder {
    fn combine_vertical(edge: &Edge, last: &mut Edge) -> Combine {
        let close = |a: i32, b: i32| a.wrapping_sub(b).wrapping_abs() < 0x100;
        if last.kind != Kind::Line || last.dx != 0 || edge.x != last.x {
            return Combine::No;
        }
        if edge.winding == last.winding {
            if edge.lower_y == last.upper_y {
                last.upper_y = edge.upper_y;
                last.y = last.upper_y;
                return Combine::Partial;
            }
            if close(edge.upper_y, last.lower_y) {
                last.lower_y = edge.lower_y;
                return Combine::Partial;
            }
            return Combine::No;
        }
        if close(edge.upper_y, last.upper_y) {
            if close(edge.lower_y, last.lower_y) {
                return Combine::Total;
            }
            if edge.lower_y < last.lower_y {
                last.upper_y = edge.lower_y;
                last.y = last.upper_y;
                return Combine::Partial;
            }
            last.upper_y = last.lower_y;
            last.y = last.upper_y;
            last.lower_y = edge.lower_y;
            last.winding = edge.winding;
            return Combine::Partial;
        }
        if close(edge.lower_y, last.lower_y) {
            if edge.upper_y > last.upper_y {
                last.lower_y = edge.upper_y;
                return Combine::Partial;
            }
            last.lower_y = last.upper_y;
            last.upper_y = edge.upper_y;
            last.y = last.upper_y;
            last.winding = edge.winding;
            return Combine::Partial;
        }
        Combine::No
    }

    fn add_line(&mut self, a: P, b: P) {
        let Some(edge) = Edge::set_line(a, b) else {
            return;
        };
        let vertical = edge.dx == 0 && edge.kind == Kind::Line;
        let combine = match self.edges.last_mut() {
            Some(last) if vertical => Builder::combine_vertical(&edge, last),
            _ => Combine::No,
        };
        match combine {
            Combine::Total => {
                self.edges.pop();
            }
            Combine::Partial => {}
            Combine::No => self.edges.push(edge),
        }
    }

    fn add_quad(&mut self, pts: &[P; 3]) {
        if let Some(edge) = Edge::set_quad(pts) {
            self.edges.push(edge);
        }
    }

    fn add_cubic(&mut self, pts: &[P; 4]) {
        if let Some(edge) = Edge::set_cubic(pts) {
            self.edges.push(edge);
        }
    }

    fn add_piece(&mut self, piece: Piece) -> bool {
        match piece {
            Piece::Line([a, b]) => {
                if !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite()) {
                    return false;
                }
                self.add_line(a, b);
            }
            Piece::Quad(q) => {
                if !q.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
                    return false;
                }
                self.add_quad(&q);
            }
            Piece::Cubic(c) => {
                if !c.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
                    return false;
                }
                self.add_cubic(&c);
            }
        }
        true
    }

    /// `SkEdgeBuilder::buildEdges`.
    fn build(path: &SkPath, clip: Option<ClipRect>, convex: bool) -> Vec<Edge> {
        let cull_right = !convex;
        let mut b = Builder { edges: Vec::new() };
        let mut pieces = Vec::new();
        let mut finite = true;
        for e in path.edges() {
            let quads: Vec<[P; 3]> = match e.verb {
                Verb::Line => {
                    match clip {
                        Some(c) => {
                            pieces.clear();
                            clip::clip_line_cull([e.pts[0], e.pts[1]], c, cull_right, &mut pieces);
                            for &p in &pieces {
                                finite &= b.add_piece(p);
                            }
                        }
                        None => b.add_line(e.pts[0], e.pts[1]),
                    }
                    continue;
                }
                Verb::Quad => vec![e.quad_pts()],
                Verb::Conic => crate::path::conic_to_quads(e.quad_pts(), e.weight, 0.25),
                Verb::Cubic => {
                    let pts = e.pts;
                    match clip {
                        Some(c) => {
                            pieces.clear();
                            clip::clip_cubic_cull(pts, c, cull_right, &mut pieces);
                            for &p in &pieces {
                                finite &= b.add_piece(p);
                            }
                        }
                        None => {
                            for mono in clip::chop_cubic_at_y_extrema(pts) {
                                b.add_cubic(&mono);
                            }
                        }
                    }
                    continue;
                }
                Verb::Move | Verb::Close => continue,
            };
            for q in quads {
                match clip {
                    Some(c) => {
                        pieces.clear();
                        clip::clip_quad_cull(q, c, cull_right, &mut pieces);
                        for &p in &pieces {
                            finite &= b.add_piece(p);
                        }
                    }
                    None => {
                        for mono in clip::chop_quad_at_y_extrema(q) {
                            b.add_quad(&mono);
                        }
                    }
                }
            }
        }
        if finite { b.edges } else { Vec::new() }
    }
}

/// `SkTQSort`: introsort with Skia's partition and insertion sort, so that edges comparing
/// equal end up in the same order as in Chromium.
fn tq_sort<T: Copy>(a: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    let n = a.len();
    if n <= 1 {
        return;
    }
    let depth = 2 * (32 - ((n - 1) as u32 - 1).leading_zeros() as i32).max(0);
    let depth = if n - 1 <= 1 { 0 } else { depth };
    intro_sort(depth, a, less);
}

fn insertion_sort<T: Copy>(a: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    for next in 1..a.len() {
        if !less(&a[next], &a[next - 1]) {
            continue;
        }
        let insert = a[next];
        let mut hole = next;
        loop {
            a[hole] = a[hole - 1];
            hole -= 1;
            if !(hole > 0 && less(&insert, &a[hole - 1])) {
                break;
            }
        }
        a[hole] = insert;
    }
}

fn heap_sift_down<T: Copy>(
    a: &mut [T],
    mut root: usize,
    bottom: usize,
    less: &impl Fn(&T, &T) -> bool,
) {
    let x = a[root - 1];
    let mut child = root << 1;
    while child <= bottom {
        if child < bottom && less(&a[child - 1], &a[child]) {
            child += 1;
        }
        if less(&x, &a[child - 1]) {
            a[root - 1] = a[child - 1];
            root = child;
            child = root << 1;
        } else {
            break;
        }
    }
    a[root - 1] = x;
}

fn heap_sift_up<T: Copy>(
    a: &mut [T],
    mut root: usize,
    bottom: usize,
    less: &impl Fn(&T, &T) -> bool,
) {
    let x = a[root - 1];
    let start = root;
    let mut j = root << 1;
    while j <= bottom {
        if j < bottom && less(&a[j - 1], &a[j]) {
            j += 1;
        }
        a[root - 1] = a[j - 1];
        root = j;
        j = root << 1;
    }
    j = root >> 1;
    while j >= start {
        if less(&a[j - 1], &x) {
            a[root - 1] = a[j - 1];
            root = j;
            j = root >> 1;
        } else {
            break;
        }
    }
    a[root - 1] = x;
}

fn heap_sort<T: Copy>(a: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    let count = a.len();
    for i in (1..=count >> 1).rev() {
        heap_sift_down(a, i, count, less);
    }
    for i in (1..count).rev() {
        a.swap(0, i);
        heap_sift_up(a, 1, i, less);
    }
}

fn intro_sort<T: Copy>(mut depth: i32, a: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    let mut a = a;
    loop {
        let count = a.len();
        if count <= 32 {
            insertion_sort(a, less);
            return;
        }
        if depth == 0 {
            heap_sort(a, less);
            return;
        }
        depth -= 1;
        let middle = (count - 1) >> 1;
        let right = count - 1;
        let pivot_value = a[middle];
        a.swap(middle, right);
        let mut new_pivot = 0;
        for left in 0..right {
            if less(&a[left], &pivot_value) {
                a.swap(left, new_pivot);
                new_pivot += 1;
            }
        }
        a.swap(new_pivot, right);
        let (lo, hi) = a.split_at_mut(new_pivot);
        intro_sort(depth, lo, less);
        a = &mut hi[1..];
    }
}

/// The additive blitters of `SkScan_AAAPath.cpp`: a small A8 mask, or one RLE row flushed to
/// the real blitter when y moves on.
enum Additive {
    Mask {
        data: Vec<u8>,
        left: i32,
        top: i32,
        width: i32,
        clip: [i32; 4],
    },
    Rle {
        row: Vec<u8>,
        left: i32,
        top: i32,
        curr_y: i32,
        safe: bool,
    },
}

fn catch_overflow(a: i32) -> u8 {
    (a - (a >> 8)) as u8
}

struct Ctx<'a> {
    blitter: Additive,
    out: &'a mut Spans,
}

impl Ctx<'_> {
    fn is_mask(&self) -> bool {
        matches!(self.blitter, Additive::Mask { .. })
    }

    fn mask_index(&self, x: i32, y: i32) -> Option<usize> {
        let Additive::Mask {
            data,
            left,
            top,
            width,
            ..
        } = &self.blitter
        else {
            return None;
        };
        let i = 1 + i64::from(y - top) * i64::from(*width) + i64::from(x - left);
        (0..data.len() as i64).contains(&i).then_some(i as usize)
    }

    fn mask_set(&mut self, x: i32, y: i32, a: u8) {
        if let Some(i) = self.mask_index(x, y)
            && let Additive::Mask { data, .. } = &mut self.blitter
        {
            data[i] = a;
        }
    }

    fn mask_add(&mut self, x: i32, y: i32, a: u8, safe: bool) {
        if let Some(i) = self.mask_index(x, y)
            && let Additive::Mask { data, .. } = &mut self.blitter
        {
            let sum = i32::from(data[i]) + i32::from(a);
            data[i] = if safe {
                sum.min(255) as u8
            } else {
                catch_overflow(sum)
            };
        }
    }

    fn flush(&mut self) {
        if let Additive::Rle {
            row,
            left,
            top,
            curr_y,
            ..
        } = &mut self.blitter
            && *curr_y >= *top
        {
            for a in row.iter_mut() {
                *a = if *a > 247 {
                    255
                } else if *a < 8 {
                    0
                } else {
                    *a
                };
            }
            self.out.anti_h(*left, *curr_y, row);
            row.fill(0);
            *curr_y = *top - 1;
        }
    }

    fn check_y(&mut self, y: i32) {
        let changed = matches!(self.blitter, Additive::Rle { curr_y, .. } if curr_y != y);
        if changed {
            self.flush();
            if let Additive::Rle { curr_y, .. } = &mut self.blitter {
                *curr_y = y;
            }
        }
    }

    fn flush_if_y_changed(&mut self, y: i32, next_y: i32) {
        if !self.is_mask() && floor_to_int(y) != floor_to_int(next_y) {
            self.flush();
        }
    }

    /// `AdditiveBlitter::blitAntiH(x, y, antialias, len)`.
    fn add_anti_h(&mut self, x: i32, y: i32, aa: &[u8]) {
        if self.is_mask() {
            return;
        }
        self.check_y(y);
        if let Additive::Rle {
            row, left, safe, ..
        } = &mut self.blitter
        {
            let width = row.len() as i32;
            let mut x = x - *left;
            let mut aa = aa;
            if x < 0 {
                aa = &aa[(-x) as usize..];
                x = 0;
            }
            let len = (aa.len() as i32).min(width - x).max(0);
            for i in 0..len {
                let cell = &mut row[(x + i) as usize];
                let sum = i32::from(*cell) + i32::from(aa[i as usize]);
                *cell = if *safe {
                    sum.min(255) as u8
                } else {
                    catch_overflow(sum)
                };
            }
        }
    }

    /// `AdditiveBlitter::blitAntiH(x, y, width, alpha)`, and the single-pixel form with width 1.
    fn add_anti_hn(&mut self, x: i32, y: i32, width: i32, alpha: u8) {
        if self.is_mask() {
            for i in 0..width {
                self.mask_add(x + i, y, alpha, false);
            }
            return;
        }
        self.check_y(y);
        if let Additive::Rle {
            row, left, safe, ..
        } = &mut self.blitter
        {
            let x = x - *left;
            if !(x >= 0 && x + width <= row.len() as i32) {
                return;
            }
            for i in 0..width {
                let cell = &mut row[(x + i) as usize];
                let sum = i32::from(*cell) + i32::from(alpha);
                *cell = if *safe {
                    sum.min(255) as u8
                } else {
                    catch_overflow(sum)
                };
            }
        }
    }

    fn real_v(&mut self, x: i32, y: i32, height: i32, alpha: u8) {
        if self.is_mask() {
            if alpha == 0 {
                return;
            }
            for i in 0..height {
                self.mask_set(x, y + i, alpha);
            }
        } else {
            self.out.v(x, y, height, alpha);
        }
    }

    fn real_rect(&mut self, x: i32, y: i32, width: i32, height: i32) {
        if self.is_mask() {
            for j in 0..height {
                for i in 0..width {
                    self.mask_set(x + i, y + j, 0xFF);
                }
            }
        } else {
            self.out.rect(x, y, width, height);
        }
    }

    fn real_anti_rect(&mut self, x: i32, y: i32, width: i32, height: i32, la: u8, ra: u8) {
        if self.is_mask() {
            self.real_v(x, y, height, la);
            self.real_v(x + 1 + width, y, height, ra);
            self.real_rect(x + 1, y, width, height);
        } else {
            self.out.anti_rect(x, y, width, height, la, ra);
        }
    }

    fn finish(mut self) {
        match &self.blitter {
            Additive::Mask { clip, .. } => {
                let [l, t, r, b] = *clip;
                for y in t..b {
                    let row: Vec<u8> = (l..r)
                        .map(|x| self.mask_index(x, y).map_or(0, |i| self.mask_data(i)))
                        .collect();
                    self.out.anti_h(l, y, &row);
                }
            }
            Additive::Rle { .. } => self.flush(),
        }
    }

    fn mask_data(&self, i: usize) -> u8 {
        match &self.blitter {
            Additive::Mask { data, .. } => data[i],
            Additive::Rle { .. } => 0,
        }
    }
}

fn trapezoid_to_alpha(l1: i32, l2: i32) -> u8 {
    ((l1.wrapping_add(l2) / 2) >> 8) as u8
}

fn partial_triangle_to_alpha(a: i32, b: i32) -> u8 {
    let area = (a >> 11).wrapping_mul(a >> 11).wrapping_mul(b >> 11);
    ((area >> 8) & 0xFF) as u8
}

fn partial_alpha_fixed(alpha: u8, partial_height: i32) -> u8 {
    round_to_int(i32::from(alpha).wrapping_mul(partial_height)) as u8
}

fn partial_alpha(alpha: u8, full: u8) -> u8 {
    ((u32::from(alpha) * u32::from(full)) >> 8) as u8
}

fn fixed_to_alpha(f: i32) -> u8 {
    partial_alpha_fixed(0xFF, f)
}

fn approximate_intersection(mut l1: i32, mut r1: i32, mut l2: i32, mut r2: i32) -> i32 {
    if l1 > r1 {
        std::mem::swap(&mut l1, &mut r1);
    }
    if l2 > r2 {
        std::mem::swap(&mut l2, &mut r2);
    }
    l1.max(l2).wrapping_add(r1.min(r2)) / 2
}

fn compute_alpha_above_line(alphas: &mut [u8], l: i32, r: i32, dy: i32, full: u8) {
    let rr = ceil_to_int(r);
    if rr == 0 {
        return;
    }
    if rr == 1 {
        let v = ((rr << 17).wrapping_sub(l).wrapping_sub(r)) >> 9;
        alphas[0] = partial_alpha(v as u8, full);
        return;
    }
    let first = ONE - l;
    let last = r - ((rr - 1) << 16);
    let first_h = fmul(first, dy);
    alphas[0] = (fmul(first, first_h) >> 9) as u8;
    let mut alpha16 = sat_add(first_h, dy >> 1);
    for a in alphas.iter_mut().take((rr - 1) as usize).skip(1) {
        *a = (alpha16 >> 8) as u8;
        alpha16 = sat_add(alpha16, dy);
    }
    alphas[(rr - 1) as usize] =
        (i32::from(full) - i32::from(partial_triangle_to_alpha(last, dy))) as u8;
}

fn compute_alpha_below_line(alphas: &mut [u8], l: i32, r: i32, dy: i32, full: u8) {
    let rr = ceil_to_int(r);
    if rr == 0 {
        return;
    }
    if rr == 1 {
        alphas[0] = partial_alpha(trapezoid_to_alpha(l, r), full);
        return;
    }
    let first = ONE - l;
    let last = r - ((rr - 1) << 16);
    let last_h = fmul(last, dy);
    alphas[(rr - 1) as usize] = (fmul(last, last_h) >> 9) as u8;
    let mut alpha16 = sat_add(last_h, dy >> 1);
    for i in (1..rr - 1).rev() {
        alphas[i as usize] = ((alpha16 >> 8) & 0xFF) as u8;
        alpha16 = sat_add(alpha16, dy);
    }
    alphas[0] = (i32::from(full) - i32::from(partial_triangle_to_alpha(first, dy))) as u8;
}

#[allow(clippy::too_many_arguments)]
fn blit_single_alpha(
    c: &mut Ctx<'_>,
    y: i32,
    x: i32,
    alpha: u8,
    full: u8,
    mask: bool,
    no_real: bool,
) {
    if mask {
        if full == 0xFF && !no_real {
            c.mask_set(x, y, alpha);
        } else {
            c.mask_add(x, y, partial_alpha(alpha, full), true);
        }
    } else if full == 0xFF && !no_real {
        c.real_v(x, y, 1, alpha);
    } else {
        c.add_anti_hn(x, y, 1, partial_alpha(alpha, full));
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_two_alphas(
    c: &mut Ctx<'_>,
    y: i32,
    x: i32,
    a1: u8,
    a2: u8,
    full: u8,
    mask: bool,
    no_real: bool,
) {
    if mask {
        c.mask_add(x, y, a1, true);
        c.mask_add(x + 1, y, a2, true);
    } else if full == 0xFF && !no_real {
        c.out.anti_h2(x, y, a1, a2);
    } else {
        c.add_anti_hn(x, y, 1, a1);
        c.add_anti_hn(x + 1, y, 1, a2);
    }
}

fn blit_full_alpha(c: &mut Ctx<'_>, y: i32, x: i32, len: i32, full: u8, mask: bool, no_real: bool) {
    if mask {
        for i in 0..len {
            c.mask_add(x + i, y, full, true);
        }
    } else if full == 0xFF && !no_real {
        c.out.h(x, y, len);
    } else {
        c.add_anti_hn(x, y, len, full);
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_aaa_trapezoid_row(
    c: &mut Ctx<'_>,
    y: i32,
    ul: i32,
    ur: i32,
    ll: i32,
    lr: i32,
    l_dy: i32,
    r_dy: i32,
    full: u8,
    mask: bool,
    no_real: bool,
) {
    let big_l = floor_to_int(ul);
    let big_r = ceil_to_int(lr);
    let len = big_r - big_l;
    if len == 1 {
        let alpha = trapezoid_to_alpha(ur.wrapping_sub(ul), lr.wrapping_sub(ll));
        blit_single_alpha(c, y, big_l, alpha, full, mask, no_real);
        return;
    }
    if len <= 0 {
        return;
    }
    let n = len as usize;
    let mut alphas = vec![full; n];
    let mut temp = vec![0u8; n + 2];
    let ul_i = floor_to_int(ul);
    let ll_i = ceil_to_int(ll);
    if ul_i + 2 == ll_i {
        let first = int_to_fixed(ul_i) + ONE - ul;
        let second = ll - ul - first;
        let a1 = (i32::from(full) - i32::from(partial_triangle_to_alpha(first, l_dy))) as u8;
        let a2 = partial_triangle_to_alpha(second, l_dy);
        alphas[0] = alphas[0].saturating_sub(a1);
        alphas[1] = alphas[1].saturating_sub(a2);
    } else {
        let off = (ul_i - big_l) as usize;
        compute_alpha_below_line(
            &mut temp[off..],
            ul - int_to_fixed(ul_i),
            ll - int_to_fixed(ul_i),
            l_dy,
            full,
        );
        for i in ul_i..ll_i {
            let k = (i - big_l) as usize;
            alphas[k] = alphas[k].saturating_sub(temp[k]);
        }
    }
    let ur_i = floor_to_int(ur);
    let lr_i = ceil_to_int(lr);
    if ur_i + 2 == lr_i {
        let first = int_to_fixed(ur_i) + ONE - ur;
        let second = lr - ur - first;
        let a1 = partial_triangle_to_alpha(first, r_dy);
        let a2 = (i32::from(full) - i32::from(partial_triangle_to_alpha(second, r_dy))) as u8;
        alphas[n - 2] = alphas[n - 2].saturating_sub(a1);
        alphas[n - 1] = alphas[n - 1].saturating_sub(a2);
    } else {
        let off = (ur_i - big_l) as usize;
        compute_alpha_above_line(
            &mut temp[off..],
            ur - int_to_fixed(ur_i),
            lr - int_to_fixed(ur_i),
            r_dy,
            full,
        );
        for i in ur_i..lr_i {
            let k = (i - big_l) as usize;
            alphas[k] = alphas[k].saturating_sub(temp[k]);
        }
    }
    if mask {
        for (i, &a) in alphas.iter().enumerate() {
            c.mask_add(big_l + i as i32, y, a, true);
        }
    } else if full == 0xFF && !no_real {
        c.out.anti_h(big_l, y, &alphas);
    } else {
        c.add_anti_h(big_l, y, &alphas);
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_trapezoid_row(
    c: &mut Ctx<'_>,
    y: i32,
    mut ul: i32,
    mut ur: i32,
    mut ll: i32,
    mut lr: i32,
    l_dy: i32,
    r_dy: i32,
    full: u8,
    mask: bool,
    no_real: bool,
) {
    if ul > ur {
        return;
    }
    if ll > lr {
        ll = approximate_intersection(ul, ll, ur, lr);
        lr = ll;
    }
    if ul == ur && ll == lr {
        return;
    }
    if ul > ll {
        std::mem::swap(&mut ul, &mut ll);
    }
    if ur > lr {
        std::mem::swap(&mut ur, &mut lr);
    }
    let join_left = ceil_to_fixed(ll);
    let join_rite = floor_to_fixed(ur);
    if join_left <= join_rite {
        if ul < join_left {
            let len = ceil_to_int(join_left - ul);
            if len == 1 {
                let alpha = trapezoid_to_alpha(join_left - ul, join_left - ll);
                blit_single_alpha(c, y, ul >> 16, alpha, full, mask, no_real);
            } else if len == 2 {
                let first = join_left - ONE - ul;
                let second = ll - ul - first;
                let a1 = partial_triangle_to_alpha(first, l_dy);
                let a2 =
                    (i32::from(full) - i32::from(partial_triangle_to_alpha(second, l_dy))) as u8;
                blit_two_alphas(c, y, ul >> 16, a1, a2, full, mask, no_real);
            } else {
                blit_aaa_trapezoid_row(
                    c,
                    y,
                    ul,
                    join_left,
                    ll,
                    join_left,
                    l_dy,
                    i32::MAX,
                    full,
                    mask,
                    no_real,
                );
            }
        }
        if join_left < join_rite {
            blit_full_alpha(
                c,
                y,
                floor_to_int(join_left),
                floor_to_int(join_rite - join_left),
                full,
                mask,
                no_real,
            );
        }
        if lr > join_rite {
            let len = ceil_to_int(lr - join_rite);
            if len == 1 {
                let alpha = trapezoid_to_alpha(ur - join_rite, lr - join_rite);
                blit_single_alpha(c, y, join_rite >> 16, alpha, full, mask, no_real);
            } else if len == 2 {
                let first = join_rite + ONE - ur;
                let second = lr - ur - first;
                let a1 =
                    (i32::from(full) - i32::from(partial_triangle_to_alpha(first, r_dy))) as u8;
                let a2 = partial_triangle_to_alpha(second, r_dy);
                blit_two_alphas(c, y, join_rite >> 16, a1, a2, full, mask, no_real);
            } else {
                blit_aaa_trapezoid_row(
                    c,
                    y,
                    join_rite,
                    ur,
                    join_rite,
                    lr,
                    i32::MAX,
                    r_dy,
                    full,
                    mask,
                    no_real,
                );
            }
        }
    } else {
        blit_aaa_trapezoid_row(c, y, ul, ur, ll, lr, l_dy, r_dy, full, mask, no_real);
    }
}

struct List {
    e: Vec<Edge>,
}

const HEAD: usize = 0;
const TAIL: usize = 1;

impl List {
    fn remove(&mut self, i: usize) {
        let (p, n) = (self.e[i].prev, self.e[i].next);
        self.e[p].next = n;
        self.e[n].prev = p;
    }

    fn insert_after(&mut self, i: usize, after: usize) {
        let n = self.e[after].next;
        self.e[i].prev = after;
        self.e[i].next = n;
        self.e[n].prev = i;
        self.e[after].next = i;
    }

    fn backward_insert_based_on_x(&mut self, i: usize) {
        let x = self.e[i].x;
        let mut prev = self.e[i].prev;
        while self.e[prev].prev != NONE && self.e[prev].x > x {
            prev = self.e[prev].prev;
        }
        if self.e[prev].next != i {
            self.remove(i);
            self.insert_after(i, prev);
        }
    }

    fn backward_insert_start(&self, mut prev: usize, x: i32) -> usize {
        while self.e[prev].prev != NONE && self.e[prev].x > x {
            prev = self.e[prev].prev;
        }
        prev
    }
}

fn is_smooth_pair(l: &List, this: usize, next: usize) -> bool {
    let t = &l.e[this];
    if t.curve_count < 0 {
        let ddshift = i32::from(t.curve_shift);
        return t.qdx.wrapping_abs() >> 1 >= t.qddx.wrapping_abs() >> ddshift
            && t.qdy.wrapping_abs() >> 1 >= t.qddy.wrapping_abs() >> ddshift
            && (t.qdy.wrapping_sub(t.qddy >> ddshift) >> t.to_fixed_shift) >= ONE;
    }
    if t.curve_count > 0 {
        let shift = i32::from(t.curve_shift);
        return t.qdx.wrapping_abs() >> 1 >= t.qddx.wrapping_abs()
            && t.qdy.wrapping_abs() >> 1 >= t.qddy.wrapping_abs()
            && (t.qdy.wrapping_sub(t.qddy) >> shift) >= ONE;
    }
    let n = &l.e[next];
    sat_sub(n.dx, t.dx).wrapping_abs() <= ONE && n.lower_y.wrapping_sub(n.upper_y) >= ONE
}

fn is_smooth_enough(l: &List, left: usize, rite: usize, mut curr: usize, stop_y: i32) -> bool {
    if l.e[curr].upper_y >= ((stop_y as u32) << 16) as i32 {
        return false;
    }
    let (le, re) = (&l.e[left], &l.e[rite]);
    if le.lower_y.wrapping_add(ONE) < re.lower_y {
        return is_smooth_pair(l, left, curr);
    } else if le.lower_y > re.lower_y.wrapping_add(ONE) {
        return is_smooth_pair(l, rite, curr);
    }
    let mut next = l.e[curr].next;
    if l.e[next].upper_y >= ((stop_y as u32) << 16) as i32 {
        return false;
    }
    if l.e[next].upper_x < l.e[curr].upper_x {
        std::mem::swap(&mut curr, &mut next);
    }
    is_smooth_pair(l, left, curr) && is_smooth_pair(l, rite, next)
}

fn walk_convex(
    l: &mut List,
    c: &mut Ctx<'_>,
    stop_y: i32,
    left_bound: i32,
    rite_bound: i32,
    mask: bool,
) {
    let mut left_e = l.e[HEAD].next;
    let mut rite_e = l.e[left_e].next;
    let mut curr_e = l.e[rite_e].next;
    let mut y = l.e[left_e].upper_y.max(l.e[rite_e].upper_y);
    loop {
        while l.e[left_e].lower_y <= y {
            if !l.e[left_e].update() {
                if floor_to_int(l.e[curr_e].upper_y) >= stop_y {
                    return;
                }
                left_e = curr_e;
                curr_e = l.e[curr_e].next;
            }
        }
        while l.e[rite_e].lower_y <= y {
            if !l.e[rite_e].update() {
                if floor_to_int(l.e[curr_e].upper_y) >= stop_y {
                    return;
                }
                rite_e = curr_e;
                curr_e = l.e[curr_e].next;
            }
        }
        if floor_to_int(y) >= stop_y {
            break;
        }
        l.e[left_e].go_y(y);
        l.e[rite_e].go_y(y);
        {
            let (le, re) = (&l.e[left_e], &l.e[rite_e]);
            if le.x > re.x || (le.x == re.x && le.dx > re.dx) {
                std::mem::swap(&mut left_e, &mut rite_e);
            }
        }
        let mut local_bot = l.e[left_e].lower_y.min(l.e[rite_e].lower_y);
        if is_smooth_enough(l, left_e, rite_e, curr_e, stop_y) {
            local_bot = ceil_to_fixed(local_bot);
        }
        local_bot = local_bot.min(int_to_fixed(stop_y));
        let mut left = left_bound.max(l.e[left_e].x);
        let d_left = l.e[left_e].dx;
        let mut rite = rite_bound.min(l.e[rite_e].x);
        let d_rite = l.e[rite_e].dx;
        if (d_left | d_rite) == 0 {
            let full_left = ceil_to_int(left);
            let full_rite = floor_to_int(rite);
            let partial_left = int_to_fixed(full_left) - left;
            let partial_rite = rite - int_to_fixed(full_rite);
            let full_top = ceil_to_int(y);
            let full_bot = floor_to_int(local_bot);
            let mut partial_top = int_to_fixed(full_top) - y;
            let mut partial_bot = local_bot - int_to_fixed(full_bot);
            if full_top > full_bot {
                partial_top -= ONE - partial_bot;
                partial_bot = 0;
            }
            if full_rite >= full_left {
                if partial_top > 0 {
                    if partial_left > 0 {
                        c.add_anti_hn(
                            full_left - 1,
                            full_top - 1,
                            1,
                            fixed_to_alpha(fmul(partial_top, partial_left)),
                        );
                    }
                    c.add_anti_hn(
                        full_left,
                        full_top - 1,
                        full_rite - full_left,
                        fixed_to_alpha(partial_top),
                    );
                    if partial_rite > 0 {
                        c.add_anti_hn(
                            full_rite,
                            full_top - 1,
                            1,
                            fixed_to_alpha(fmul(partial_top, partial_rite)),
                        );
                    }
                    c.flush_if_y_changed(y, y + partial_top);
                }
                if full_bot > full_top
                    && (full_rite > full_left
                        || fixed_to_alpha(partial_left) > 0
                        || fixed_to_alpha(partial_rite) > 0)
                {
                    c.real_anti_rect(
                        full_left - 1,
                        full_top,
                        full_rite - full_left,
                        full_bot - full_top,
                        fixed_to_alpha(partial_left),
                        fixed_to_alpha(partial_rite),
                    );
                }
                if partial_bot > 0 {
                    if partial_left > 0 {
                        c.add_anti_hn(
                            full_left - 1,
                            full_bot,
                            1,
                            fixed_to_alpha(fmul(partial_bot, partial_left)),
                        );
                    }
                    c.add_anti_hn(
                        full_left,
                        full_bot,
                        full_rite - full_left,
                        fixed_to_alpha(partial_bot),
                    );
                    if partial_rite > 0 {
                        c.add_anti_hn(
                            full_rite,
                            full_bot,
                            1,
                            fixed_to_alpha(fmul(partial_bot, partial_rite)),
                        );
                    }
                }
            } else {
                let width = rite - left;
                if width > 0 {
                    if partial_top > 0 {
                        c.add_anti_hn(
                            full_left - 1,
                            full_top - 1,
                            1,
                            fixed_to_alpha(fmul(partial_top, width)),
                        );
                        c.flush_if_y_changed(y, y + partial_top);
                    }
                    if full_bot > full_top {
                        c.real_v(
                            full_left - 1,
                            full_top,
                            full_bot - full_top,
                            fixed_to_alpha(width),
                        );
                    }
                    if partial_bot > 0 {
                        c.add_anti_hn(
                            full_left - 1,
                            full_bot,
                            1,
                            fixed_to_alpha(fmul(partial_bot, width)),
                        );
                    }
                }
            }
            y = local_bot;
        } else {
            const SNAP_DIGIT: i32 = ONE >> 4;
            const SNAP_HALF: i32 = SNAP_DIGIT >> 1;
            const SNAP_MASK: i32 = -1 ^ (SNAP_DIGIT - 1);
            left = left.wrapping_add(SNAP_HALF);
            rite = rite.wrapping_add(SNAP_HALF);
            let mut count = ceil_to_int(local_bot) - floor_to_int(y);
            let (ldy, rdy) = (l.e[left_e].dy, l.e[rite_e].dy);
            if count > 1 {
                if (y as u32 & 0xFFFF_0000) as i32 != y {
                    count -= 1;
                    let next_y = ceil_to_fixed(y + 1);
                    let dy = next_y - y;
                    let next_left = left.wrapping_add(fmul(d_left, dy));
                    let next_rite = rite.wrapping_add(fmul(d_rite, dy));
                    blit_trapezoid_row(
                        c,
                        y >> 16,
                        left & SNAP_MASK,
                        rite & SNAP_MASK,
                        next_left & SNAP_MASK,
                        next_rite & SNAP_MASK,
                        ldy,
                        rdy,
                        partial_alpha_fixed(0xFF, dy),
                        mask,
                        false,
                    );
                    c.flush_if_y_changed(y, next_y);
                    left = next_left;
                    rite = next_rite;
                    y = next_y;
                }
                while count > 1 {
                    count -= 1;
                    let next_y = y + ONE;
                    let next_left = left.wrapping_add(d_left);
                    let next_rite = rite.wrapping_add(d_rite);
                    blit_trapezoid_row(
                        c,
                        y >> 16,
                        left & SNAP_MASK,
                        rite & SNAP_MASK,
                        next_left & SNAP_MASK,
                        next_rite & SNAP_MASK,
                        ldy,
                        rdy,
                        0xFF,
                        mask,
                        false,
                    );
                    c.flush_if_y_changed(y, next_y);
                    left = next_left;
                    rite = next_rite;
                    y = next_y;
                }
            }
            let dy = local_bot - y;
            let next_left = left
                .wrapping_add(fmul(d_left, dy))
                .max(left_bound.wrapping_add(SNAP_HALF));
            let next_rite = rite
                .wrapping_add(fmul(d_rite, dy))
                .min(rite_bound.wrapping_add(SNAP_HALF));
            blit_trapezoid_row(
                c,
                y >> 16,
                left & SNAP_MASK,
                rite & SNAP_MASK,
                next_left & SNAP_MASK,
                next_rite & SNAP_MASK,
                ldy,
                rdy,
                partial_alpha_fixed(0xFF, dy),
                mask,
                false,
            );
            c.flush_if_y_changed(y, local_bot);
            left = next_left.wrapping_sub(SNAP_HALF);
            rite = next_rite.wrapping_sub(SNAP_HALF);
            y = local_bot;
        }
        l.e[left_e].x = left;
        l.e[rite_e].x = rite;
        l.e[left_e].y = y;
        l.e[rite_e].y = y;
    }
}

fn update_next_next_y(y: i32, next_y: i32, nn: &mut i32) {
    if y > next_y && y < *nn {
        *nn = y;
    }
}

fn check_intersection(l: &List, i: usize, next_y: i32, nn: &mut i32) {
    let p = l.e[i].prev;
    if l.e[p].prev != NONE && l.e[p].x.wrapping_add(l.e[p].dx) > l.e[i].x.wrapping_add(l.e[i].dx) {
        *nn = next_y + (ONE >> ACCURACY);
    }
}

fn check_intersection_fwd(l: &List, i: usize, next_y: i32, nn: &mut i32) {
    let n = l.e[i].next;
    if l.e[n].next != NONE && l.e[i].x.wrapping_add(l.e[i].dx) > l.e[n].x.wrapping_add(l.e[n].dx) {
        *nn = next_y + (ONE >> ACCURACY);
    }
}

fn insert_new_edges(l: &mut List, mut new_edge: usize, y: i32, nn: &mut i32) {
    if l.e[new_edge].upper_y > y {
        update_next_next_y(l.e[new_edge].upper_y, y, nn);
        return;
    }
    let prev = l.e[new_edge].prev;
    if l.e[prev].x <= l.e[new_edge].x {
        while l.e[new_edge].upper_y <= y {
            check_intersection(l, new_edge, y, nn);
            update_next_next_y(l.e[new_edge].lower_y, y, nn);
            new_edge = l.e[new_edge].next;
        }
        update_next_next_y(l.e[new_edge].upper_y, y, nn);
        return;
    }
    let mut start = l.backward_insert_start(prev, l.e[new_edge].x);
    loop {
        let next = l.e[new_edge].next;
        let mut already = false;
        loop {
            if l.e[start].next == new_edge {
                already = true;
                break;
            }
            let after = l.e[start].next;
            if l.e[after].x >= l.e[new_edge].x {
                break;
            }
            start = after;
        }
        if !already {
            l.remove(new_edge);
            l.insert_after(new_edge, start);
        }
        check_intersection(l, new_edge, y, nn);
        check_intersection_fwd(l, new_edge, y, nn);
        update_next_next_y(l.e[new_edge].lower_y, y, nn);
        start = new_edge;
        new_edge = next;
        if l.e[new_edge].upper_y > y {
            break;
        }
    }
    update_next_next_y(l.e[new_edge].upper_y, y, nn);
}

fn edges_too_close(l: &List, prev: usize, next: usize, lower_y: i32) -> bool {
    if prev == NONE || next == NONE {
        return false;
    }
    let (p, n) = (&l.e[prev], &l.e[next]);
    n.upper_y < lower_y && p.x.wrapping_add(ONE) >= n.x.wrapping_sub(n.dx.wrapping_abs())
}

fn edges_too_close_rite(prev_rite: i32, ul: i32, ll: i32) -> bool {
    prev_rite > floor_to_int(ul) || prev_rite > floor_to_int(ll)
}

#[allow(clippy::too_many_arguments)]
fn walk_edges(
    l: &mut List,
    even_odd: bool,
    c: &mut Ctx<'_>,
    start_y: i32,
    stop_y: i32,
    left_clip: i32,
    right_clip: i32,
    mask: bool,
    skip_intersect: bool,
) {
    l.e[HEAD].x = left_clip;
    l.e[HEAD].upper_x = left_clip;
    l.e[TAIL].x = right_clip;
    l.e[TAIL].upper_x = right_clip;
    let mut y = l.e[l.e[HEAD].next].upper_y.max(int_to_fixed(start_y));
    let mut nn = i32::MAX;
    {
        let mut edge = l.e[HEAD].next;
        while l.e[edge].upper_y <= y {
            l.e[edge].go_y(y);
            update_next_next_y(l.e[edge].lower_y, y, &mut nn);
            edge = l.e[edge].next;
        }
        update_next_next_y(l.e[edge].upper_y, y, &mut nn);
    }
    let winding_mask = if even_odd { 1 } else { -1 };
    loop {
        let mut w = 0i32;
        let mut in_interval = false;
        let mut prev_x = l.e[HEAD].x;
        let mut next_y = nn.min(ceil_to_fixed(y + 1));
        let mut curr_e = l.e[HEAD].next;
        let mut left_e = HEAD;
        let mut left = left_clip;
        let mut left_dy = 0;
        let mut prev_rite = floor_to_int(left_clip);
        nn = i32::MAX;
        let mut y_shift = 0;
        if (next_y - y) & (ONE >> 2) != 0 {
            y_shift = 2;
            next_y = y + (ONE >> 2);
        } else if (next_y - y) & (ONE >> 1) != 0 {
            y_shift = 1;
        }
        let full = fixed_to_alpha(next_y - y);
        let no_real = false;
        while l.e[curr_e].upper_y <= y {
            w += i32::from(l.e[curr_e].winding);
            let prev_in = in_interval;
            in_interval = (w & winding_mask) != 0;
            let is_left = in_interval && !prev_in;
            let is_rite = !in_interval && prev_in;
            if is_rite {
                let mut rite = l.e[curr_e].x;
                l.e[curr_e].go_y_shift(next_y, y_shift);
                let next_left = left_clip.max(l.e[left_e].x);
                rite = right_clip.min(rite);
                let next_rite = right_clip.min(l.e[curr_e].x);
                let too_close = full == 0xFF
                    && (edges_too_close_rite(prev_rite, left, l.e[left_e].x)
                        || edges_too_close(l, curr_e, l.e[curr_e].next, next_y));
                blit_trapezoid_row(
                    c,
                    y >> 16,
                    left,
                    rite,
                    next_left,
                    next_rite,
                    left_dy,
                    l.e[curr_e].dy,
                    full,
                    mask,
                    no_real || too_close,
                );
                prev_rite = ceil_to_int(rite.max(l.e[curr_e].x));
            } else {
                if is_left {
                    left = l.e[curr_e].x.max(left_clip);
                    left_dy = l.e[curr_e].dy;
                    left_e = curr_e;
                }
                l.e[curr_e].go_y_shift(next_y, y_shift);
            }
            let next = l.e[curr_e].next;
            while l.e[curr_e].lower_y <= next_y {
                if l.e[curr_e].curve_count == 0 {
                    break;
                }
                l.e[curr_e].keep_continuous();
                if !l.e[curr_e].update() {
                    break;
                }
            }
            if l.e[curr_e].lower_y <= next_y {
                l.remove(curr_e);
            } else {
                update_next_next_y(l.e[curr_e].lower_y, next_y, &mut nn);
                let new_x = l.e[curr_e].x;
                if new_x < prev_x {
                    l.backward_insert_based_on_x(curr_e);
                } else {
                    prev_x = new_x;
                }
                if !skip_intersect {
                    check_intersection(l, curr_e, next_y, &mut nn);
                }
            }
            curr_e = next;
        }
        if in_interval {
            let too_close = full == 0xFF && edges_too_close(l, l.e[left_e].prev, left_e, next_y);
            blit_trapezoid_row(
                c,
                y >> 16,
                left,
                right_clip,
                left_clip.max(l.e[left_e].x),
                right_clip,
                left_dy,
                0,
                full,
                mask,
                no_real || too_close,
            );
        }
        y = next_y;
        if y >= int_to_fixed(stop_y) {
            break;
        }
        insert_new_edges(l, curr_e, y, &mut nn);
    }
}

/// `SkScan::AntiFillPath` for a canvas-sized rect clip: the blits Skia's analytic AA makes for
/// `path` (device space, nonzero or even-odd), in order.
pub fn fill_path(path: &SkPath, width: u32, height: u32, even_odd: bool, out: &mut Spans) {
    let clip = [0, 0, width as i32, height as i32];
    fill_path_clipped(path, clip, false, even_odd, out);
}

/// `SkScan::AntiFillPath` for the rect clip `[left, top, right, bottom)`.
pub fn fill_path_clipped(
    path: &SkPath,
    clip: [i32; 4],
    force_rle: bool,
    even_odd: bool,
    out: &mut Spans,
) {
    let bounds = path.bounds();
    if !path.is_finite() {
        return;
    }
    let limit = i32::MAX >> 2;
    let [l, t, r, b] = bounds.round_out();
    let ir = [l.max(-limit), t.max(-limit), r.min(limit), b.min(limit)];
    if ir[0] >= ir[2] || ir[1] >= ir[3] {
        return;
    }
    let sect = [
        ir[0].max(clip[0]),
        ir[1].max(clip[1]),
        ir[2].min(clip[2]),
        ir[3].min(clip[3]),
    ];
    if sect[0] >= sect[2] || sect[1] >= sect[3] {
        return;
    }
    let contained = clip[0] <= ir[0] && clip[1] <= ir[1] && clip[2] >= ir[2] && clip[3] >= ir[3];
    let convex = path.is_convex();
    let (iw, ih) = (ir[2] - ir[0], ir[3] - ir[1]);
    let can_mask = !force_rle && iw <= 32 && i64::from((iw + 3) & !3) * i64::from(ih) <= 1024;
    if can_mask {
        if try_blit_fat_anti_rect(path, clip, out) {
            return;
        }
        let blitter = Additive::Mask {
            data: vec![0; (iw * ih + 2) as usize],
            left: ir[0],
            top: ir[1],
            width: iw,
            clip: sect,
        };
        let mut ctx = Ctx { blitter, out };
        fill_edges(path, clip, ir, contained, convex, true, even_odd, &mut ctx);
        ctx.finish();
    } else {
        let blitter = Additive::Rle {
            row: vec![0; (sect[2] - sect[0]) as usize],
            left: sect[0],
            top: sect[1],
            curr_y: sect[1] - 1,
            safe: !convex,
        };
        let mut ctx = Ctx { blitter, out };
        fill_edges(path, clip, ir, contained, convex, false, even_odd, &mut ctx);
        ctx.finish();
    }
}

fn try_blit_fat_anti_rect(path: &SkPath, clip: [i32; 4], out: &mut Spans) -> bool {
    let Some(rect) = path.is_rect() else {
        return false;
    };
    let clip_rect = Rect {
        left: clip[0] as f32,
        top: clip[1] as f32,
        right: clip[2] as f32,
        bottom: clip[3] as f32,
    };
    let Some(rect) = rect.intersect(&clip_rect) else {
        return true;
    };
    let b = rect.round_out();
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    if bw < 3 {
        return false;
    }
    if bh == 0 {
        return true;
    }
    let to_alpha = |a: f32| {
        let alpha = (a * 255.0) as u8;
        if alpha > 247 {
            255
        } else if alpha < 8 {
            0
        } else {
            alpha
        }
    };
    let partial_l = (b[0] + 1) as f32 - rect.left;
    let partial_r = rect.right - (b[2] - 1) as f32;
    let mut partial_t = (b[1] + 1) as f32 - rect.top;
    let partial_b = rect.bottom - (b[3] - 1) as f32;
    if bh == 1 {
        partial_t = rect.bottom - rect.top;
    }
    let row = |pl: f32, pm: f32, pr: f32| {
        let mut alphas = vec![to_alpha(pm); bw as usize];
        alphas[0] = to_alpha(pl);
        alphas[bw as usize - 1] = to_alpha(pr);
        alphas
    };
    out.anti_h(
        b[0],
        b[1],
        &row(partial_l * partial_t, partial_t, partial_r * partial_t),
    );
    if bh > 2 {
        out.anti_rect(
            b[0],
            b[1] + 1,
            bw - 2,
            bh - 2,
            to_alpha(partial_l),
            to_alpha(partial_r),
        );
    }
    if bh > 1 {
        out.anti_h(
            b[0],
            b[3] - 1,
            &row(partial_l * partial_b, partial_b, partial_r * partial_b),
        );
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn fill_edges(
    path: &SkPath,
    clip: [i32; 4],
    ir: [i32; 4],
    contained: bool,
    convex: bool,
    mask: bool,
    even_odd: bool,
    c: &mut Ctx<'_>,
) {
    let clip_rect = ClipRect {
        left: clip[0] as f32,
        top: clip[1] as f32,
        right: clip[2] as f32,
        bottom: clip[3] as f32,
    };
    let edges = Builder::build(path, (!contained).then_some(clip_rect), convex);
    if edges.is_empty() {
        return;
    }
    let count = edges.len();
    let mut l = List {
        e: Vec::with_capacity(count + 2),
    };
    l.e.push(Edge::blank());
    l.e.push(Edge::blank());
    l.e.extend(edges);
    let mut order: Vec<usize> = (2..count + 2).collect();
    {
        let e = &l.e;
        tq_sort(&mut order, &|&a: &usize, &b: &usize| {
            let (a, b) = (&e[a], &e[b]);
            if a.upper_y != b.upper_y {
                return a.upper_y < b.upper_y;
            }
            if a.x != b.x {
                return a.x < b.x;
            }
            a.dx < b.dx
        });
    }
    for w in order.windows(2) {
        l.e[w[0]].next = w[1];
        l.e[w[1]].prev = w[0];
    }
    let (first, last) = (order[0], order[count - 1]);
    {
        let h = &mut l.e[HEAD];
        h.prev = NONE;
        h.next = first;
        h.upper_y = i32::MIN;
        h.lower_y = i32::MIN;
        h.x = i32::MIN;
        h.dx = 0;
        h.dy = i32::MAX;
        h.upper_x = i32::MIN;
    }
    l.e[first].prev = HEAD;
    {
        let t = &mut l.e[TAIL];
        t.prev = last;
        t.next = NONE;
        t.upper_y = i32::MAX;
        t.lower_y = i32::MAX;
        t.x = i32::MAX;
        t.dx = 0;
        t.dy = i32::MAX;
        t.upper_x = i32::MAX;
    }
    l.e[last].next = TAIL;
    let (mut start_y, mut stop_y) = (ir[1], ir[3]);
    if !contained {
        start_y = start_y.max(clip[1]);
        stop_y = stop_y.min(clip[3]);
    }
    let mut left_bound = int_to_fixed(clip[0]);
    let mut right_bound = int_to_fixed(clip[2]);
    if mask {
        let [pl, _, pr, _] = path.bounds().round_out();
        left_bound = left_bound.max(int_to_fixed(pl));
        right_bound = right_bound.min(int_to_fixed(pr));
    }
    if convex && count >= 2 {
        walk_convex(&mut l, c, stop_y, left_bound, right_bound, mask);
    } else {
        let skip_intersect = path.pts.len() as i64 > i64::from(stop_y - start_y) * 2;
        walk_edges(
            &mut l,
            even_odd,
            c,
            start_y,
            stop_y,
            left_bound,
            right_bound,
            mask,
            skip_intersect,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_path(l: f32, t: f32, r: f32, b: f32) -> SkPath {
        let mut p = SkPath::new();
        p.move_to(P { x: l, y: t });
        p.line_to(P { x: r, y: t });
        p.line_to(P { x: r, y: b });
        p.line_to(P { x: l, y: b });
        p.close();
        p
    }

    #[test]
    fn quick_inverse_matches_the_table_ends() {
        assert_eq!(quick_inverse(-1024), -4096);
        assert_eq!(quick_inverse(-1009), -4156);
        assert_eq!(quick_inverse(-3), -1398101);
        assert_eq!(quick_inverse(1), 4194304);
    }

    #[test]
    fn sort_is_stable_enough_and_sorted() {
        let mut v: Vec<i32> = (0..200).map(|i| (i * 7919) % 101).collect();
        tq_sort(&mut v, &|a: &i32, b: &i32| a < b);
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn fat_rect_half_row_truncates_to_127() {
        let mut spans = Spans::new(8, 8);
        fill_path(&rect_path(0.0, 2.5, 8.0, 4.0), 8, 8, false, &mut spans);
        let px = spans.flatten(8, 8);
        assert_eq!(px[2 * 8 + 4], 127);
        assert_eq!(px[3 * 8 + 4], 255);
    }

    fn circle_path(cx: f32, cy: f32, r: f32) -> SkPath {
        let k = r * 0.552_284_8;
        let p = |x: f32, y: f32| P {
            x: cx + x,
            y: cy + y,
        };
        let mut path = SkPath::new();
        path.move_to(p(r, 0.0));
        path.cubic_to(p(r, k), p(k, r), p(0.0, r));
        path.cubic_to(p(-k, r), p(-r, k), p(-r, 0.0));
        path.cubic_to(p(-r, -k), p(-k, -r), p(0.0, -r));
        path.cubic_to(p(k, -r), p(r, -k), p(r, 0.0));
        path.close();
        path
    }

    fn coverage(path: &SkPath, size: u32) -> Vec<u8> {
        let mut spans = Spans::new(size, size);
        fill_path(path, size, size, false, &mut spans);
        spans.flatten(size as usize, size as usize)
    }

    fn area(px: &[u8]) -> f64 {
        px.iter().map(|&a| f64::from(a) / 255.0).sum()
    }

    #[test]
    fn cubic_circle_has_the_circle_area() {
        let expected = std::f64::consts::PI * 20.0 * 20.0;
        let whole = area(&coverage(&circle_path(32.0, 32.0, 20.0), 64));
        assert!((whole / expected - 1.0).abs() < 0.005, "{whole}");
        let half = area(&coverage(&circle_path(0.0, 32.0, 20.0), 64));
        assert!((half / (expected / 2.0) - 1.0).abs() < 0.005, "{half}");
    }

    #[test]
    fn straight_cubics_fill_like_their_chords() {
        let p = |x: f32, y: f32| P { x, y };
        let fill = |tip: P, curved: bool| {
            let (a, b) = (p(4.0, 3.0), p(60.0, 10.0));
            let mut path = SkPath::new();
            path.move_to(a);
            if curved {
                let third = |t: f32| p(a.x + (tip.x - a.x) * t, a.y + (tip.y - a.y) * t);
                path.cubic_to(third(1.0 / 3.0), third(2.0 / 3.0), tip);
            } else {
                path.line_to(tip);
            }
            path.line_to(b);
            path.close();
            coverage(&path, 64)
        };
        for (tip, tolerance) in [(p(4.0, 58.25), 4), (p(40.5, 58.25), 32)] {
            let (line, curve) = (fill(tip, false), fill(tip, true));
            let (a, b) = (area(&line), area(&curve));
            assert!((a - b).abs() < 3.0, "{a} {b}");
            let worst = line
                .iter()
                .zip(&curve)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap();
            assert!(worst <= tolerance, "{worst}");
        }
    }
}
