//! Skia's dasher (`SkDashPath::InternalFilter` over `SkContourMeasure`, Chromium m142) for
//! paths of lines, quads and conics. Conics are measured and cut as conics, as Skia does.

use tiny_skia::LineCap;

use crate::hairline::P;
use crate::skpath::{Rect, SkPath, Verb};

const MAX_T: i32 = 0x3FFF_FFFF;
const MAX_DASH_COUNT: f32 = 1_000_000.0;

fn p(x: f32, y: f32) -> P {
    P { x, y }
}

fn t_value(t: i32) -> f32 {
    t as f32 * (1.0 / MAX_T as f32)
}

fn interp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn length(a: P, b: P) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let mag2 = dx * dx + dy * dy;
    if mag2.is_finite() {
        mag2.sqrt()
    } else {
        (f64::from(dx) * f64::from(dx) + f64::from(dy) * f64::from(dy)).sqrt() as f32
    }
}

fn chop_quad(src: [P; 3], t: f32) -> [P; 5] {
    let l = |a: P, b: P| p(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
    let (p01, p12) = (l(src[0], src[1]), l(src[1], src[2]));
    [src[0], p01, l(p01, p12), p12, src[2]]
}

#[derive(Clone, Copy)]
struct Conic {
    pts: [P; 3],
    w: f32,
}

impl Conic {
    fn coeff_eval(&self, t: f32) -> (P, f32) {
        let [p0, p1, p2] = self.pts;
        let p1w = p(p1.x * self.w, p1.y * self.w);
        let na = p(p2.x - p1w.x * 2.0 + p0.x, p2.y - p1w.y * 2.0 + p0.y);
        let nb = p((p1w.x - p0.x) * 2.0, (p1w.y - p0.y) * 2.0);
        let db = (self.w - 1.0) * 2.0;
        let da = 0.0 - db;
        (
            p((na.x * t + nb.x) * t + p0.x, (na.y * t + nb.y) * t + p0.y),
            (da * t + db) * t + 1.0,
        )
    }

    fn eval(&self, t: f32) -> P {
        let (n, d) = self.coeff_eval(t);
        p(n.x / d, n.y / d)
    }

    fn chop_at(&self, t: f32) -> Option<[Conic; 2]> {
        let w = self.w;
        let src = [
            [self.pts[0].x, self.pts[0].y, 1.0],
            [self.pts[1].x * w, self.pts[1].y * w, w],
            [self.pts[2].x, self.pts[2].y, 1.0],
        ];
        let mut out = [[0.0f32; 3]; 3];
        for k in 0..3 {
            let ab = interp(src[0][k], src[1][k], t);
            let bc = interp(src[1][k], src[2][k], t);
            out[0][k] = ab;
            out[1][k] = interp(ab, bc, t);
            out[2][k] = bc;
        }
        let down = |v: [f32; 3]| p(v[0] / v[2], v[1] / v[2]);
        let root = out[1][2].sqrt();
        let mid = down(out[1]);
        let halves = [
            Conic {
                pts: [self.pts[0], down(out[0]), mid],
                w: out[0][2] / root,
            },
            Conic {
                pts: [mid, down(out[2]), self.pts[2]],
                w: out[2][2] / root,
            },
        ];
        let finite = halves
            .iter()
            .flat_map(|c| c.pts)
            .all(|q| q.x.is_finite() && q.y.is_finite());
        finite.then_some(halves)
    }

    fn chop_between(&self, t1: f32, t2: f32) -> Conic {
        if t1 == 0.0 || t2 == 1.0 {
            if t1 == 0.0 && t2 == 1.0 {
                return *self;
            }
            if let Some(pair) = self.chop_at(if t1 != 0.0 { t1 } else { t2 }) {
                return pair[usize::from(t1 != 0.0)];
            }
        }
        let (a_xy, a_zz) = self.coeff_eval(t1);
        let (d_xy, d_zz) = self.coeff_eval((t1 + t2) / 2.0);
        let (c_xy, c_zz) = self.coeff_eval(t2);
        let b_xy = p(
            d_xy.x * 2.0 - (a_xy.x + c_xy.x) * 0.5,
            d_xy.y * 2.0 - (a_xy.y + c_xy.y) * 0.5,
        );
        let b_zz = d_zz * 2.0 - (a_zz + c_zz) * 0.5;
        Conic {
            pts: [
                p(a_xy.x / a_zz, a_xy.y / a_zz),
                p(b_xy.x / b_zz, b_xy.y / b_zz),
                p(c_xy.x / c_zz, c_xy.y / c_zz),
            ],
            w: b_zz / (a_zz * c_zz).sqrt(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SegType {
    Line,
    Quad,
    Conic,
}

#[derive(Clone, Copy)]
struct Segment {
    distance: f32,
    pt_index: usize,
    t_value: i32,
    kind: SegType,
}

struct Contour {
    segments: Vec<Segment>,
    pts: Vec<P>,
    length: f32,
    closed: bool,
}

fn quad_too_curvy(pts: &[P; 3], tol: f32) -> bool {
    let dx = pts[1].x * 0.5 - (pts[0].x + pts[2].x) * 0.5 * 0.5;
    let dy = pts[1].y * 0.5 - (pts[0].y + pts[2].y) * 0.5 * 0.5;
    dx.abs().max(dy.abs()) > tol
}

fn conic_too_curvy(first: P, mid: P, last: P, tol: f32) -> bool {
    let me = p((first.x + last.x) * 0.5, (first.y + last.y) * 0.5);
    (mid.x - me.x).abs().max((mid.y - me.y).abs()) > tol
}

struct Builder {
    segments: Vec<Segment>,
    tol: f32,
}

impl Builder {
    fn push(&mut self, d: f32, distance: f32, pt_index: usize, kind: SegType, t_value: i32) -> f32 {
        let next = distance + d;
        if next > distance {
            self.segments.push(Segment {
                distance: next,
                pt_index,
                t_value,
                kind,
            });
        }
        next
    }

    fn quad_segs(
        &mut self,
        pts: [P; 3],
        distance: f32,
        mint: i32,
        maxt: i32,
        idx: usize,
        depth: i32,
    ) -> f32 {
        if depth < 8 && (maxt - mint) >> 10 != 0 && quad_too_curvy(&pts, self.tol) {
            let tmp = chop_quad(pts, 0.5);
            let half = (mint + maxt) >> 1;
            let d = self.quad_segs(
                [tmp[0], tmp[1], tmp[2]],
                distance,
                mint,
                half,
                idx,
                depth + 1,
            );
            self.quad_segs([tmp[2], tmp[3], tmp[4]], d, half, maxt, idx, depth + 1)
        } else {
            self.push(length(pts[0], pts[2]), distance, idx, SegType::Quad, maxt)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn conic_segs(
        &mut self,
        conic: &Conic,
        distance: f32,
        mint: i32,
        min_pt: P,
        maxt: i32,
        max_pt: P,
        idx: usize,
        depth: i32,
    ) -> f32 {
        let half = (mint + maxt) >> 1;
        let half_pt = conic.eval(t_value(half));
        if !(half_pt.x.is_finite() && half_pt.y.is_finite()) {
            return distance;
        }
        if depth < 8
            && (maxt - mint) >> 10 != 0
            && conic_too_curvy(min_pt, half_pt, max_pt, self.tol)
        {
            let d = self.conic_segs(conic, distance, mint, min_pt, half, half_pt, idx, depth + 1);
            self.conic_segs(conic, d, half, half_pt, maxt, max_pt, idx, depth + 1)
        } else {
            self.push(length(min_pt, max_pt), distance, idx, SegType::Conic, maxt)
        }
    }
}

/// `SkContourMeasureIter` over `path` (not force-closed).
fn contours(path: &SkPath, res_scale: f32) -> Vec<Contour> {
    let mut out = Vec::new();
    if !path.is_finite() {
        return out;
    }
    let tol = 0.5 * (1.0 / res_scale);
    let mut b = Builder {
        segments: Vec::new(),
        tol,
    };
    let mut pts: Vec<P> = Vec::new();
    let mut idx: isize = -1;
    let mut dist = 0.0f32;
    let mut seen_close = false;
    let mut seen_move = false;
    let (mut pi, mut wi) = (0usize, 0usize);
    let mut last = p(0.0, 0.0);
    let mut finish = |b: &mut Builder, pts: &mut Vec<P>, idx: isize, dist: f32, closed: bool| {
        let mut dist = dist;
        if dist.is_finite() && !b.segments.is_empty() {
            if closed {
                let first = pts[0];
                let prev = dist;
                dist = b.push(
                    length(pts[idx as usize], first),
                    dist,
                    idx as usize,
                    SegType::Line,
                    MAX_T,
                );
                if dist > prev {
                    pts.push(first);
                }
            }
            out.push(Contour {
                segments: std::mem::take(&mut b.segments),
                pts: std::mem::take(pts),
                length: dist,
                closed,
            });
        }
        b.segments.clear();
        pts.clear();
    };
    for &verb in &path.verbs {
        if seen_move && verb == Verb::Move {
            finish(&mut b, &mut pts, idx, dist, seen_close);
            idx = -1;
            dist = 0.0;
            seen_close = false;
            seen_move = false;
        }
        match verb {
            Verb::Cubic => todo!("cubic contour measure"),
            Verb::Move => {
                last = path.pts[pi];
                pi += 1;
                idx += 1;
                pts.push(last);
                seen_move = true;
            }
            Verb::Line => {
                let e = path.pts[pi];
                pi += 1;
                let prev = dist;
                dist = b.push(length(last, e), dist, idx as usize, SegType::Line, MAX_T);
                if dist > prev {
                    pts.push(e);
                    idx += 1;
                }
                last = e;
            }
            Verb::Quad => {
                let q = [last, path.pts[pi], path.pts[pi + 1]];
                pi += 2;
                let prev = dist;
                dist = b.quad_segs(q, dist, 0, MAX_T, idx as usize, 0);
                if dist > prev {
                    pts.extend([q[1], q[2]]);
                    idx += 2;
                }
                last = q[2];
            }
            Verb::Conic => {
                let conic = Conic {
                    pts: [last, path.pts[pi], path.pts[pi + 1]],
                    w: path.weights[wi],
                };
                pi += 2;
                wi += 1;
                let prev = dist;
                dist = b.conic_segs(
                    &conic,
                    dist,
                    0,
                    conic.pts[0],
                    MAX_T,
                    conic.pts[2],
                    idx as usize,
                    0,
                );
                if dist > prev {
                    pts.extend([p(conic.w, 0.0), conic.pts[1], conic.pts[2]]);
                    idx += 3;
                }
                last = conic.pts[2];
            }
            Verb::Close => seen_close = true,
        }
    }
    finish(&mut b, &mut pts, idx, dist, seen_close);
    out
}

impl Contour {
    fn distance_to_segment(&self, d: f32) -> (usize, f32) {
        let segs = &self.segments;
        let (mut lo, mut hi) = (0usize, segs.len() - 1);
        while lo < hi {
            let mid = (hi + lo) >> 1;
            if segs[mid].distance < d {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let index = if segs[hi].distance < d { hi + 1 } else { hi };
        let index = index.min(segs.len() - 1);
        let seg = &segs[index];
        let (mut start_t, mut start_d) = (0.0f32, 0.0f32);
        if index > 0 {
            start_d = segs[index - 1].distance;
            if segs[index - 1].pt_index == seg.pt_index {
                start_t = t_value(segs[index - 1].t_value);
            }
        }
        let t =
            start_t + (t_value(seg.t_value) - start_t) * (d - start_d) / (seg.distance - start_d);
        (index, t)
    }

    fn pos(&self, seg: &Segment, t: f32) -> P {
        let pts = &self.pts[seg.pt_index..];
        match seg.kind {
            SegType::Line => p(interp(pts[0].x, pts[1].x, t), interp(pts[0].y, pts[1].y, t)),
            SegType::Quad => {
                let c = pts[0];
                let b = p((pts[1].x - c.x) * 2.0, (pts[1].y - c.y) * 2.0);
                let a = p(
                    pts[2].x - pts[1].x * 2.0 + c.x,
                    pts[2].y - pts[1].y * 2.0 + c.y,
                );
                p((a.x * t + b.x) * t + c.x, (a.y * t + b.y) * t + c.y)
            }
            SegType::Conic => Conic {
                pts: [pts[0], pts[2], pts[3]],
                w: pts[1].x,
            }
            .eval(t),
        }
    }

    fn seg_to(&self, seg: &Segment, start_t: f32, stop_t: f32, dst: &mut SkPath) {
        let pts = &self.pts[seg.pt_index..];
        if start_t == stop_t {
            if let Some(lp) = dst.last_pt()
                && !dst.is_empty()
            {
                dst.line_to(lp);
            }
            return;
        }
        match seg.kind {
            SegType::Line => {
                if stop_t == 1.0 {
                    dst.line_to(pts[1]);
                } else {
                    dst.line_to(p(
                        interp(pts[0].x, pts[1].x, stop_t),
                        interp(pts[0].y, pts[1].y, stop_t),
                    ));
                }
            }
            SegType::Quad => {
                let q = [pts[0], pts[1], pts[2]];
                if start_t == 0.0 {
                    if stop_t == 1.0 {
                        dst.quad_to(q[1], q[2]);
                    } else {
                        let t = chop_quad(q, stop_t);
                        dst.quad_to(t[1], t[2]);
                    }
                } else {
                    let t0 = chop_quad(q, start_t);
                    if stop_t == 1.0 {
                        dst.quad_to(t0[3], t0[4]);
                    } else {
                        let t1 =
                            chop_quad([t0[2], t0[3], t0[4]], (stop_t - start_t) / (1.0 - start_t));
                        dst.quad_to(t1[1], t1[2]);
                    }
                }
            }
            SegType::Conic => {
                let conic = Conic {
                    pts: [pts[0], pts[2], pts[3]],
                    w: pts[1].x,
                };
                if start_t == 0.0 {
                    if stop_t == 1.0 {
                        dst.conic_to(conic.pts[1], conic.pts[2], conic.w);
                    } else if let Some(h) = conic.chop_at(stop_t) {
                        dst.conic_to(h[0].pts[1], h[0].pts[2], h[0].w);
                    }
                } else if stop_t == 1.0 {
                    if let Some(h) = conic.chop_at(start_t) {
                        dst.conic_to(h[1].pts[1], h[1].pts[2], h[1].w);
                    }
                } else {
                    let c = conic.chop_between(start_t, stop_t);
                    dst.conic_to(c.pts[1], c.pts[2], c.w);
                }
            }
        }
    }

    /// `SkContourMeasure::getSegment`.
    fn segment(&self, start_d: f32, stop_d: f32, dst: &mut SkPath, move_to: bool) {
        let start_d = start_d.max(0.0);
        let stop_d = stop_d.min(self.length);
        if start_d.partial_cmp(&stop_d).is_none_or(|o| o.is_gt()) || self.segments.is_empty() {
            return;
        }
        let (mut si, mut start_t) = self.distance_to_segment(start_d);
        if !start_t.is_finite() {
            return;
        }
        let (stop_i, stop_t) = self.distance_to_segment(stop_d);
        if !stop_t.is_finite() {
            return;
        }
        if move_to {
            let pt = self.pos(&self.segments[si], start_t);
            dst.move_to(pt);
        }
        let stop_pt = self.segments[stop_i].pt_index;
        if self.segments[si].pt_index == stop_pt {
            self.seg_to(&self.segments[si], start_t, stop_t, dst);
        } else {
            loop {
                self.seg_to(&self.segments[si], start_t, 1.0, dst);
                let idx = self.segments[si].pt_index;
                while self.segments[si].pt_index == idx {
                    si += 1;
                }
                start_t = 0.0;
                if self.segments[si].pt_index >= stop_pt {
                    break;
                }
            }
            self.seg_to(&self.segments[si], 0.0, stop_t, dst);
        }
    }
}

/// The dash parameters of `SkDashPathEffect`.
#[derive(Clone, Debug)]
pub struct Dash {
    pub intervals: Vec<f32>,
    pub phase: f32,
}

/// What `SkDashPath::InternalFilter` hands on: the dashed path and whether it is already the
/// stroke outline (`SpecialLineRec` turns a dashed butt-capped line into rects to fill).
pub struct Dashed {
    pub path: SkPath,
    pub filled: bool,
}

fn adjust_zero_length_line(pts: &mut [P; 2]) {
    pts[1].x += pts[1].x.max(1.001) * (1.0 / 4096.0);
}

fn clip_line(pts: &mut [P; 2], bounds: &Rect, interval_length: f32, prior_phase: f32) -> bool {
    let (dx, dy) = (pts[1].x - pts[0].x, pts[1].y - pts[0].y);
    if dx != 0.0 && dy != 0.0 {
        return false;
    }
    let vertical = dy != 0.0;
    let get = |q: P| if vertical { q.y } else { q.x };
    let (mut min, mut max) = (get(pts[0]), get(pts[1]));
    let swapped = max < min;
    if swapped {
        std::mem::swap(&mut min, &mut max);
    }
    let (lt, rb) = if vertical {
        (bounds.top, bounds.bottom)
    } else {
        (bounds.left, bounds.right)
    };
    if max < lt || min > rb {
        return false;
    }
    if min < lt {
        min = lt - (lt - min) % interval_length;
        if !swapped {
            min -= prior_phase;
        }
    }
    if max > rb {
        max = rb + (max - rb) % interval_length;
        if swapped {
            max += prior_phase;
        }
    }
    if swapped {
        std::mem::swap(&mut min, &mut max);
    }
    if vertical {
        pts[0].y = min;
        pts[1].y = max;
    } else {
        pts[0].x = min;
        pts[1].x = max;
    }
    if min == max {
        adjust_zero_length_line(pts);
    }
    true
}

fn as_line(path: &SkPath) -> Option<[P; 2]> {
    (path.verbs == [Verb::Move, Verb::Line]).then(|| [path.pts[0], path.pts[1]])
}

/// The style the dasher needs from the stroke.
#[derive(Clone, Copy)]
pub struct StrokeRec {
    pub width: f32,
    pub hairline: bool,
    pub cap: LineCap,
    pub miter: Option<f32>,
    pub res_scale: f32,
}

/// `SkDashPath::InternalFilter` with `cull` the conservative local clip bounds.
pub fn dash_path(src: &SkPath, dash: &Dash, rec: &StrokeRec, cull: Option<Rect>) -> Option<Dashed> {
    let intervals = &dash.intervals;
    let count = intervals.len();
    let len: f32 = intervals.iter().sum();
    if count < 2 || !count.is_multiple_of(2) || intervals.iter().any(|&v| v < 0.0 || !v.is_finite())
    {
        return None;
    }
    if !(len > 0.0 && len.is_finite() && dash.phase.is_finite()) {
        return None;
    }
    let (initial_len, initial_index, phase) = {
        let mut phase = dash.phase;
        if phase < 0.0 {
            phase = -phase;
            if phase > len {
                phase %= len;
            }
            phase = len - phase;
            if phase == len {
                phase = 0.0;
            }
        } else if phase >= len {
            phase %= len;
        }
        let mut ph = phase;
        let mut found = (intervals[0], 0usize);
        for (i, &gap) in intervals.iter().enumerate() {
            if ph > gap || (ph == gap && gap != 0.0) {
                ph -= gap;
            } else {
                found = (gap - ph, i);
                break;
            }
        }
        (found.0, found.1, phase)
    };
    let mut culled: Option<SkPath> = None;
    if let Some(cull) = cull {
        let mut radius = rec.width * 0.5;
        if radius == 0.0 {
            radius = 1.0;
        }
        if let Some(m) = rec.miter {
            radius *= m;
        }
        let bounds = Rect {
            left: cull.left - radius,
            top: cull.top - radius,
            right: cull.right + radius,
            bottom: cull.bottom + radius,
        };
        if let Some(mut pts) = as_line(src) {
            if clip_line(&mut pts, &bounds, len, 0.0) {
                let mut b = SkPath::new();
                b.move_to(pts[0]);
                b.line_to(pts[1]);
                culled = Some(b);
            }
        } else if src.is_rect().is_some() {
            let mut b = SkPath::new();
            let mut accum = 0.0f64;
            let mut last = None;
            let mut first = None;
            let mut lines = Vec::new();
            let mut pi = 0;
            for &v in &src.verbs {
                match v {
                    Verb::Move => {
                        if first.is_some() {
                            break;
                        }
                        first = Some(src.pts[pi]);
                        last = Some(src.pts[pi]);
                        pi += 1;
                    }
                    Verb::Line => {
                        let e = src.pts[pi];
                        lines.push([last.unwrap_or(e), e]);
                        last = Some(e);
                        pi += 1;
                    }
                    Verb::Close => {
                        if let (Some(l), Some(f)) = (last, first)
                            && l != f
                        {
                            lines.push([l, f]);
                        }
                        break;
                    }
                    _ => break,
                }
            }
            for line in lines {
                let v = (line[1].x - line[0].x, line[1].y - line[0].y);
                let mut pts = line;
                if clip_line(&mut pts, &bounds, len, (accum % f64::from(len)) as f32) {
                    if b.last_pt() != Some(pts[0]) {
                        b.move_to(pts[0]);
                    }
                    b.line_to(pts[1]);
                }
                accum += f64::from((v.0 + v.1).abs());
            }
            if !b.is_empty() {
                let closed = src.verbs.last() == Some(&Verb::Close);
                if closed && initial_index % 2 == 0 {
                    let perimeter: f32 = contours(src, rec.res_scale)
                        .first()
                        .map_or(0.0, |c| c.length);
                    let mut end_phase = (perimeter + phase) % len;
                    let mut index = 0;
                    while end_phase > intervals[index] {
                        end_phase -= intervals[index];
                        index += 1;
                        if index == count {
                            end_phase = 0.0;
                            break;
                        }
                    }
                    if (index % 2 == 0) == (end_phase > 0.0) {
                        let pts = &src.pts;
                        let mid = pts[0];
                        let mut last_i = pts.len() - 1;
                        while mid == pts[last_i] {
                            last_i -= 1;
                        }
                        let mut next = 1;
                        while mid == pts[next] {
                            next += 1;
                        }
                        let tiny = 1.0 / 4096.0;
                        let v = p(
                            (mid.x - pts[last_i].x) * tiny,
                            (mid.y - pts[last_i].y) * tiny,
                        );
                        b.move_to(p(mid.x - v.x, mid.y - v.y));
                        b.line_to(mid);
                        let v = p((mid.x - pts[next].x) * tiny, (mid.y - pts[next].y) * tiny);
                        b.line_to(p(mid.x - v.x, mid.y - v.y));
                    }
                }
                culled = Some(b);
            }
        }
    } else if let Some(mut pts) = as_line(src)
        && pts[0] == pts[1]
    {
        adjust_zero_length_line(&mut pts);
        let mut b = SkPath::new();
        b.move_to(pts[0]);
        b.line_to(pts[1]);
        culled = Some(b);
    }
    let src = culled.as_ref().unwrap_or(src);
    let mut dst = SkPath::new();
    let special = (!rec.hairline && rec.cap == LineCap::Butt)
        .then(|| as_line(src))
        .flatten()
        .and_then(|pts| {
            let path_length = length(pts[0], pts[1]);
            let t = p(pts[1].x - pts[0].x, pts[1].y - pts[0].y);
            if t.x == 0.0 && t.y == 0.0 {
                return None;
            }
            let inv = 1.0 / path_length;
            let tangent = p(t.x * inv, t.y * inv);
            if !(tangent.x.is_finite() && tangent.y.is_finite()) {
                return None;
            }
            let half = rec.width * 0.5;
            let normal = p(tangent.y * half, -tangent.x * half);
            let pt_count = (path_length * (count >> 1) as f32 / len).min(MAX_DASH_COUNT);
            if pt_count.is_nan() {
                return None;
            }
            Some((pts, tangent, normal, path_length))
        });
    let mut dash_count = 0.0f32;
    for c in contours(src, rec.res_scale) {
        let mut skip_first = c.closed;
        let mut added = false;
        let length = c.length;
        let mut index = initial_index;
        dash_count += length * (count >> 1) as f32 / len;
        if dash_count > MAX_DASH_COUNT {
            return None;
        }
        let mut dist = 0.0f64;
        let mut dlen = f64::from(initial_len);
        while dist < f64::from(length) {
            added = false;
            if index % 2 == 0 && !skip_first {
                added = true;
                let (d0, d1) = (dist as f32, (dist + dlen) as f32);
                match special {
                    Some((pts, tangent, normal, path_length)) => {
                        let d1 = d1.min(path_length);
                        let x0 = pts[0].x + tangent.x * d0;
                        let x1 = pts[0].x + tangent.x * d1;
                        let y0 = pts[0].y + tangent.y * d0;
                        let y1 = pts[0].y + tangent.y * d1;
                        dst.move_to(p(x0 + normal.x, y0 + normal.y));
                        dst.line_to(p(x1 + normal.x, y1 + normal.y));
                        dst.line_to(p(x1 - normal.x, y1 - normal.y));
                        dst.line_to(p(x0 - normal.x, y0 - normal.y));
                    }
                    None => c.segment(d0, d1, &mut dst, true),
                }
            }
            dist += dlen;
            skip_first = false;
            index += 1;
            if index == count {
                index = 0;
            }
            dlen = f64::from(intervals[index]);
        }
        if c.closed && initial_index % 2 == 0 && initial_len >= 0.0 {
            c.segment(0.0, initial_len, &mut dst, !added);
        }
    }
    Some(Dashed {
        path: dst,
        filled: special.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashes_a_line_into_its_on_intervals() {
        let mut src = SkPath::new();
        src.move_to(p(10.0, 33.0));
        src.line_to(p(290.0, 33.5));
        let dash = Dash {
            intervals: vec![10.0, 3.0, 2.0, 3.0],
            phase: 0.0,
        };
        let rec = StrokeRec {
            width: 2.0,
            hairline: false,
            cap: LineCap::Round,
            miter: None,
            res_scale: 1.0,
        };
        let out = dash_path(&src, &dash, &rec, None).unwrap();
        assert!(!out.filled);
        let moves = out.path.verbs.iter().filter(|v| **v == Verb::Move).count();
        assert_eq!(moves, 31);
    }
}
