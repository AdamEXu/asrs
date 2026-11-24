// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2006-2008 The Android Open Source Project). See THIRD_PARTY_NOTICES.md.

//! Blink's synthetic bold: `SkScalerContextRec::useStrokeForFakeBold` and the stroke-and-fill
//! `SkScalerContext::internalGetPath` turns it into.

use tiny_skia::{LineCap, LineJoin};

use crate::hairline::P;
use crate::skpath::{SkPath, Verb};
use crate::stroke::{self, Style};

/// The width text of `size` px is stroked with: `kStdFakeBoldInterpKeys` and `Values`.
pub fn extra_width(size: f32) -> f32 {
    let (small, large) = ((9.0, 1.0 / 24.0), (36.0, 1.0 / 32.0));
    let t = ((size - small.0) / (large.0 - small.0)).clamp(0.0, 1.0);
    size * (small.1 + t * (large.1 - small.1))
}

struct Contour {
    verbs: Vec<Verb>,
    points: Vec<P>,
}

fn contours(path: &SkPath) -> Vec<Contour> {
    let mut out: Vec<Contour> = Vec::new();
    let mut points = path.pts.iter().copied();
    for &verb in &path.verbs {
        if verb == Verb::Move {
            out.push(Contour {
                verbs: Vec::new(),
                points: points.by_ref().take(1).collect(),
            });
        } else if let Some(contour) = out.last_mut() {
            contour.verbs.push(verb);
            contour.points.extend(points.by_ref().take(verb.points()));
        }
    }
    out
}

/// `SkPathPriv::ComputeFirstDirection` by the area of the lowest contour.
fn is_counter_clockwise(contours: &[Contour]) -> bool {
    let bottom = |c: &&Contour| c.points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let Some(lowest) = contours
        .iter()
        .max_by(|a, b| bottom(a).total_cmp(&bottom(b)))
    else {
        return false;
    };
    let next = lowest.points.iter().cycle().skip(1);
    let area: f64 = lowest
        .points
        .iter()
        .zip(next)
        .map(|(a, b)| f64::from(a.x) * f64::from(b.y) - f64::from(a.y) * f64::from(b.x))
        .sum();
    area < 0.0
}

fn add(dst: &mut SkPath, contour: &Contour, reverse: bool) {
    let mut points = contour.points.clone();
    let mut verbs = contour.verbs.clone();
    let closed = verbs.last() == Some(&Verb::Close);
    if closed {
        verbs.pop();
    }
    if reverse {
        points.reverse();
        verbs.reverse();
    }
    let mut rest = &points[..];
    let mut next = || {
        let (&p, tail) = rest.split_first().expect("a point per verb");
        rest = tail;
        p
    };
    dst.move_to(next());
    for verb in verbs {
        match verb {
            Verb::Line => dst.line_to(next()),
            Verb::Quad => dst.quad_to(next(), next()),
            Verb::Cubic => dst.cubic_to(next(), next(), next()),
            Verb::Move | Verb::Conic | Verb::Close => {}
        }
    }
    if closed {
        dst.close();
    }
}

/// `path` filled and stroked `width` wide.
pub fn embolden(path: &SkPath, width: f32) -> SkPath {
    let style = Style {
        width,
        miter_limit: 4.0,
        cap: LineCap::Butt,
        join: LineJoin::Miter,
        res_scale: 1.0,
    };
    let mut out = stroke::stroke_path(path, &style);
    let contours = contours(path);
    let reverse = is_counter_clockwise(&contours);
    for contour in &contours {
        add(&mut out, contour, reverse);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::path_mask;

    fn square(clockwise: bool) -> SkPath {
        let mut corners = [(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0)];
        if !clockwise {
            corners.reverse();
        }
        let mut path = SkPath::new();
        path.move_to(P {
            x: corners[0].0,
            y: corners[0].1,
        });
        path.quad_to(
            P {
                x: 5.0,
                y: corners[1].1,
            },
            P {
                x: corners[1].0,
                y: corners[1].1,
            },
        );
        for &(x, y) in &corners[2..] {
            path.line_to(P { x, y });
        }
        path.close();
        path
    }

    #[test]
    fn fake_bold_follows_skias_table() {
        assert_eq!(extra_width(6.0), 0.25);
        assert_eq!(extra_width(9.0), 0.375);
        assert_eq!(extra_width(48.0), 1.5);
    }

    #[test]
    fn both_directions_grow_and_stay_filled() {
        for clockwise in [true, false] {
            let mask = path_mask::gray(&embolden(&square(clockwise), 2.0)).expect("ink");
            assert_eq!((mask.left, mask.top, mask.width, mask.height), (1, 1, 8, 8));
            let at = |x: usize, y: usize| mask.data[y * 8 + x];
            assert_eq!(at(4, 4), 255);
            assert_eq!(at(0, 4), 255);
        }
    }
}
