// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2006-2023 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! The gradients of COLR version 1 glyphs, parametrized as `SkScalerContext_DW` hands them to
//! Skia's shaders.

use crate::color::Rgba;
use crate::gradient::Colors;
use crate::path::Affine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Clamp,
    Repeat,
    Mirror,
}

pub type Stop = (f32, Rgba);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Linear {
        p0: (f32, f32),
        p1: (f32, f32),
    },
    Radial {
        c0: (f32, f32),
        r0: f32,
        c1: (f32, f32),
        r1: f32,
    },
    Sweep {
        center: (f32, f32),
        /// Turns clockwise of the positive x axis.
        start: f32,
        end: f32,
    },
}

/// What a COLR fill paints.
pub enum Fill {
    Nothing,
    Solid(Rgba),
    Gradient(Gradient),
}

pub struct Gradient {
    kind: Kind,
    tile: Tile,
    colors: Colors,
}

/// Stop positions moved to `[0, 1]`, with where 0 and 1 were; `None` where nothing is drawn.
fn normalize(stops: &mut Vec<Stop>, tile: Tile) -> Option<(f32, f32)> {
    let (first, mut last) = (stops[0].0, stops[stops.len() - 1].0);
    if last - first == 0.0 {
        if tile != Tile::Clamp {
            return None;
        }
        last += 1.0;
        stops.push((last, stops[stops.len() - 1].1));
    }
    let range = last - first;
    if range != 1.0 || first != 0.0 {
        let scale = 1.0 / range;
        for stop in stops {
            stop.0 = (stop.0 - first) * scale;
        }
    }
    Some((first, last))
}

fn lerp(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// `truncateToStopInterpolating`.
fn truncate(stops: &mut Vec<Stop>, at: f32, start: bool) {
    if stops.len() <= 1 || at < stops[0].0 || stops[stops.len() - 1].0 < at {
        return;
    }
    let after = stops
        .iter()
        .position(|s| if start { s.0 >= at } else { s.0 > at })
        .unwrap_or(stops.len() - 1)
        .max(1);
    let (a, b) = (stops[after - 1], stops[after]);
    let t = ((at - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    let mix = |a: f32, b: f32| a + (b - a) * t;
    let color = Rgba {
        r: mix(a.1.r, b.1.r),
        g: mix(a.1.g, b.1.g),
        b: mix(a.1.b, b.1.b),
        a: mix(a.1.a, b.1.a),
    };
    if start {
        stops.splice(..after, [(0.0, color)]);
    } else {
        stops.splice(after.., [(1.0, color)]);
    }
}

fn gradient(kind: Kind, tile: Tile, stops: &[Stop]) -> Fill {
    Fill::Gradient(Gradient {
        kind,
        tile,
        colors: Colors::new(stops),
    })
}

pub fn linear(points: [(f32, f32); 3], mut stops: Vec<Stop>, tile: Tile) -> Fill {
    let Some(&(_, first)) = stops.first() else {
        return Fill::Nothing;
    };
    let [p0, p1, p2] = points;
    let (to1, to2) = ((p1.0 - p0.0, p1.1 - p0.1), (p2.0 - p0.0, p2.1 - p0.1));
    let cross = to1.0 * to2.1 - to1.1 * to2.0;
    if stops.len() == 1 || p1 == p0 || p2 == p0 || cross == 0.0 {
        return Fill::Solid(first);
    }
    let normal = (to2.1, -to2.0);
    let along = (to1.0 * normal.0 + to1.1 * normal.1) / (normal.0 * normal.0 + normal.1 * normal.1);
    let p3 = (p0.0 + normal.0 * along, p0.1 + normal.1 * along);
    let Some((from, to)) = normalize(&mut stops, tile) else {
        return Fill::Nothing;
    };
    let kind = Kind::Linear {
        p0: lerp(p0, p3, from),
        p1: lerp(p0, p3, to),
    };
    gradient(kind, tile, &stops)
}

pub fn radial(
    (c0, r0): ((f32, f32), f32),
    (c1, r1): ((f32, f32), f32),
    mut stops: Vec<Stop>,
    tile: Tile,
) -> Fill {
    match stops[..] {
        [] => return Fill::Nothing,
        [(_, color)] => return Fill::Solid(color),
        _ => {}
    }
    let Some((from, to)) = normalize(&mut stops, tile) else {
        return Fill::Nothing;
    };
    let (mut start, mut end) = (lerp(c0, c1, from), lerp(c0, c1, to));
    let (mut start_radius, mut end_radius) = (r0 + (r1 - r0) * from, r0 + (r1 - r0) * to);
    if start_radius < 0.0 || end_radius < 0.0 {
        if start_radius == end_radius {
            return Fill::Nothing;
        }
        if tile == Tile::Clamp {
            let (full_start, full_end) = (start, end);
            let difference = end_radius - start_radius;
            let zero = -start_radius / difference;
            let truncate_start = start_radius < 0.0;
            if start_radius < 0.0 {
                start_radius = 0.0;
                start = lerp(full_start, full_end, zero);
            }
            if end_radius < 0.0 {
                end_radius = 0.0;
                end = lerp(full_start, full_end, zero);
            }
            if start_radius != 0.0 || end_radius != 0.0 {
                truncate(&mut stops, zero, truncate_start);
            } else if difference > 0.0 {
                let shift = (full_end.0 - full_start.0, full_end.1 - full_start.1);
                end = (start.0 + shift.0, start.1 + shift.1);
                end_radius = difference;
                stops.drain(..stops.len() - 1);
            } else {
                let shift = (full_end.0 - full_start.0, full_end.1 - full_start.1);
                start = (start.0 - shift.0, start.1 - shift.1);
                start_radius = -difference;
                stops.truncate(1);
            }
            if let [(_, color)] = stops[..] {
                return Fill::Solid(color);
            }
        } else {
            let difference = end_radius - start_radius;
            let crossing = start_radius / (start_radius - end_radius);
            let in_range = (0.0..=1.0).contains(&crossing);
            let factor = crossing
                * if in_range && difference < 0.0 {
                    -1.0
                } else {
                    1.0
                };
            let mut multiple = if factor > 0.0 {
                factor.ceil()
            } else {
                factor.floor() - 1.0
            };
            if tile == Tile::Mirror && multiple % 2.0 != 0.0 {
                multiple += if multiple < 0.0 { -1.0 } else { 1.0 };
            }
            let shift = ((end.0 - start.0) * multiple, (end.1 - start.1) * multiple);
            start = (start.0 + shift.0, start.1 + shift.1);
            end = (end.0 + shift.0, end.1 + shift.1);
            start_radius += multiple * difference;
            end_radius += multiple * difference;
        }
    }
    let kind = Kind::Radial {
        c0: start,
        r0: start_radius,
        c1: end,
        r1: end_radius,
    };
    gradient(kind, tile, &stops)
}

/// A sweep from `start` to `end` degrees counter-clockwise.
pub fn sweep(center: (f32, f32), (start, end): (f32, f32), stops: Vec<Stop>, tile: Tile) -> Fill {
    match stops[..] {
        [] => return Fill::Nothing,
        [(_, color)] => return Fill::Solid(color),
        _ => {}
    }
    let mut stops = stops;
    let sector = end - start;
    if sector == 0.0 && tile != Tile::Clamp {
        return Fill::Nothing;
    }
    let Some((from, to)) = normalize(&mut stops, tile) else {
        return Fill::Nothing;
    };
    let mut clockwise = [start + sector * from, start + sector * to].map(|angle| 360.0 - angle);
    if clockwise[0] >= clockwise[1] {
        clockwise.swap(0, 1);
        stops.reverse();
        for stop in &mut stops {
            stop.0 = 1.0 - stop.0;
        }
    }
    let kind = Kind::Sweep {
        center,
        start: clockwise[0] / 360.0,
        end: clockwise[1] / 360.0,
    };
    gradient(kind, tile, &stops)
}

impl Kind {
    fn t(&self, x: f64, y: f64) -> Option<f64> {
        let f = f64::from;
        match *self {
            Kind::Linear { p0, p1 } => {
                let (vx, vy) = (f(p1.0) - f(p0.0), f(p1.1) - f(p0.1));
                let length = vx * vx + vy * vy;
                (length > 0.0).then(|| ((x - f(p0.0)) * vx + (y - f(p0.1)) * vy) / length)
            }
            Kind::Radial { c0, r0, c1, r1 } => {
                let (cx, cy) = (f(c1.0) - f(c0.0), f(c1.1) - f(c0.1));
                let (px, py) = (x - f(c0.0), y - f(c0.1));
                let dr = f(r1) - f(r0);
                let a = cx * cx + cy * cy - dr * dr;
                let b = px * cx + py * cy + f(r0) * dr;
                let c = px * px + py * py - f(r0) * f(r0);
                let valid = |t: &f64| t.is_finite() && f(r0) + t * dr >= 0.0;
                if a.abs() < 1e-12 {
                    return Some(c / (2.0 * b)).filter(valid);
                }
                let discriminant = b * b - a * c;
                if discriminant < 0.0 {
                    return None;
                }
                let root = discriminant.sqrt();
                let (t0, t1) = ((b + root) / a, (b - root) / a);
                [t0.max(t1), t0.min(t1)].into_iter().find(valid)
            }
            Kind::Sweep { center, start, end } => {
                let turns = (y - f(center.1)).atan2(x - f(center.0)) / std::f64::consts::TAU;
                let turns = if turns < 0.0 { turns + 1.0 } else { turns };
                let sector = f(end) - f(start);
                if sector <= 0.0 {
                    return Some(if turns < f(start) { 0.0 } else { 1.0 });
                }
                Some((turns - f(start)) / sector)
            }
        }
    }
}

impl Gradient {
    /// The premultiplied colour of a device pixel under the inverse of the matrix the fill is
    /// drawn with.
    pub fn at(&self, inverse: &Affine, x: usize, y: usize) -> [u8; 4] {
        let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
        let local = (
            inverse.a * x + inverse.c * y + inverse.e,
            inverse.b * x + inverse.d * y + inverse.f,
        );
        let Some(t) = self.kind.t(local.0, local.1) else {
            return [0; 4];
        };
        let t = match self.tile {
            Tile::Clamp => t.clamp(0.0, 1.0),
            Tile::Repeat => t - t.floor(),
            Tile::Mirror => {
                let t = (t - 1.0) * 0.5;
                ((t - t.floor()) * 2.0 - 1.0).abs()
            }
        };
        let store = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        self.colors.at(t as f32).map(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba = Rgba::from_u8(255, 0, 0, 1.0);
    const BLUE: Rgba = Rgba::from_u8(0, 0, 255, 1.0);

    fn gradient_of(fill: Fill) -> Gradient {
        match fill {
            Fill::Gradient(g) => g,
            _ => panic!("not a gradient"),
        }
    }

    #[test]
    fn linear_gradients_project_onto_the_normal() {
        let stops = vec![(0.0, RED), (1.0, BLUE)];
        let points = [(0.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let g = gradient_of(linear(points, stops.clone(), Tile::Clamp));
        assert_eq!(g.kind.t(5.0, 0.0), Some(0.5));
        assert_eq!(g.kind.t(5.0, 7.0), Some(0.5));
        let flat = [(0.0, 0.0), (10.0, 0.0), (5.0, 0.0)];
        assert!(matches!(linear(flat, stops, Tile::Clamp), Fill::Solid(c) if c == RED));
    }

    #[test]
    fn stops_inside_the_line_move_its_ends() {
        let stops = vec![(0.25, RED), (0.75, BLUE)];
        let points = [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)];
        let g = gradient_of(linear(points, stops, Tile::Clamp));
        assert_eq!(g.kind.t(2.0, 0.0), Some(0.0));
        assert_eq!(g.kind.t(6.0, 0.0), Some(1.0));
    }

    #[test]
    fn conical_gradients_take_the_larger_root() {
        let stops = vec![(0.0, RED), (1.0, BLUE)];
        let g = gradient_of(radial(
            ((0.0, 0.0), 0.0),
            ((0.0, 0.0), 4.0),
            stops.clone(),
            Tile::Clamp,
        ));
        assert_eq!(g.kind.t(0.0, 2.0), Some(0.5));
        let focal = gradient_of(radial(
            ((0.0, 0.0), 0.0),
            ((4.0, 0.0), 4.0),
            stops,
            Tile::Clamp,
        ));
        assert_eq!(focal.kind.t(8.0, 0.0), Some(1.0));
        assert_eq!(focal.kind.t(-1.0, 0.0), None);
    }

    #[test]
    fn sweeps_run_counter_clockwise() {
        let stops = vec![(0.0, RED), (1.0, BLUE)];
        let g = gradient_of(sweep((0.0, 0.0), (0.0, 360.0), stops, Tile::Clamp));
        assert_eq!(g.kind.t(0.0, 1.0), Some(0.25));
        assert_eq!(g.at(&Affine::scaling(1.0, 1.0), 4, 4)[0], 32);
    }
}
