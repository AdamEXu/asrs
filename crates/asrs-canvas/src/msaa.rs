//! Graphite's tessellated path fills under 4x MSAA, the way Chromium rasterizes DOM paths on
//! macOS: every curve becomes a cubic or conic patch linearized at `2^resolveLevel` uniform
//! parametric steps (Wang's formula at precision 4, at most 32 segments), vertices snap to the
//! rasterizer's 1/256 px grid, and a pixel's coverage is how many of Metal's four standard sample
//! positions the winding rule puts inside, with ties on an edge broken by the top-left rule.

use crate::blit::Spans;
use crate::hairline::P;
use crate::skpath::{SkPath, Verb};

const GRID: f32 = 256.0;
const PRECISION: f32 = 4.0;
const MAX_RESOLVE_LEVEL: f32 = 5.0;

/// Metal's standard 4x sample positions in 1/256 px, listed by row.
const SAMPLES: [(i64, i64); 4] = [(96, 32), (224, 96), (32, 160), (160, 224)];

fn mix(a: P, b: P, t: f32) -> P {
    P {
        x: (b.x - a.x).mul_add(t, a.x),
        y: (b.y - a.y).mul_add(t, a.y),
    }
}

fn cubic_resolve_level([p0, p1, p2, p3]: [P; 4]) -> f32 {
    let d0 = P {
        x: (-2.0f32).mul_add(p1.x, p2.x) + p0.x,
        y: (-2.0f32).mul_add(p1.y, p2.y) + p0.y,
    };
    let d1 = P {
        x: (-2.0f32).mul_add(p2.x, p3.x) + p1.x,
        y: (-2.0f32).mul_add(p2.y, p3.y) + p1.y,
    };
    let m = (d0.x * d0.x + d0.y * d0.y).max(d1.x * d1.x + d1.y * d1.y);
    let length_term_pow2 = (9.0 * 4.0 / 64.0) * PRECISION * PRECISION;
    ((length_term_pow2 * m).max(1.0).log2() * 0.25).ceil()
}

fn conic_resolve_level([p0, p1, p2]: [P; 3], w: f32) -> f32 {
    let c = P {
        x: (p0.x.min(p1.x).min(p2.x) + p0.x.max(p1.x).max(p2.x)) * 0.5,
        y: (p0.y.min(p1.y).min(p2.y) + p0.y.max(p1.y).max(p2.y)) * 0.5,
    };
    let [q0, q1, q2] = [p0, p1, p2].map(|p| P {
        x: p.x - c.x,
        y: p.y - c.y,
    });
    let len2 = |p: P| p.x * p.x + p.y * p.y;
    let m = len2(q0).max(len2(q1)).max(len2(q2)).sqrt();
    let dp = P {
        x: (-2.0 * w).mul_add(q1.x, q0.x) + q2.x,
        y: (-2.0 * w).mul_add(q1.y, q0.y) + q2.y,
    };
    let dw = (-2.0f32).mul_add(w, 2.0).abs();
    let rp_minus_1 = m.mul_add(PRECISION, -1.0).max(0.0);
    let numer = len2(dp).sqrt() * PRECISION + rp_minus_1 * dw;
    let denom = 4.0 * w.min(1.0);
    ((numer / denom).max(1.0).log2() * 0.5).ceil()
}

/// `tessellate_filled_curve`: the points after `p0` at `T = i / 2^level`.
fn linearize(p: [P; 4], w: Option<f32>, out: &mut Vec<P>) {
    let level = match w {
        Some(w) => conic_resolve_level([p[0], p[1], p[2]], w),
        None => cubic_resolve_level(p),
    }
    .clamp(0.0, MAX_RESOLVE_LEVEL);
    let n = 1u32 << level as u32;
    let (p0, mut p1, p2, p3) = (p[0], p[1], p[2], if w.is_some() { p[2] } else { p[3] });
    if let Some(w) = w {
        p1 = P {
            x: p1.x * w,
            y: p1.y * w,
        };
    }
    for i in 1..n {
        let t = (i << (5 - level as u32)) as f32 * (1.0 / 32.0);
        let ab = mix(p0, p1, t);
        let bc = mix(p1, p2, t);
        let cd = mix(p2, p3, t);
        let abc = mix(ab, bc, t);
        out.push(match w {
            None => mix(abc, mix(bc, cd, t), t),
            Some(w) => {
                let u = (w - 1.0).mul_add(t, 1.0);
                let v = w + 1.0 - u;
                let uv = (v - u).mul_add(t, u);
                P {
                    x: abc.x / uv,
                    y: abc.y / uv,
                }
            }
        });
    }
    out.push(if w.is_some() { p[2] } else { p[3] });
}

fn contours(path: &SkPath) -> Vec<Vec<P>> {
    let mut out: Vec<Vec<P>> = Vec::new();
    let mut pi = 0;
    let mut wi = 0;
    for &verb in &path.verbs {
        let last = out.last().and_then(|c| c.last().copied());
        match verb {
            Verb::Move => {
                out.push(vec![path.pts[pi]]);
            }
            Verb::Line => {
                if let Some(c) = out.last_mut() {
                    c.push(path.pts[pi]);
                }
            }
            Verb::Quad => {
                if let (Some(p0), Some(c)) = (last, out.last_mut()) {
                    let (p1, p2) = (path.pts[pi], path.pts[pi + 1]);
                    let c1 = mix(p0, p1, 2.0 / 3.0);
                    let c2 = mix(p2, p1, 2.0 / 3.0);
                    linearize([p0, c1, c2, p2], None, c);
                }
            }
            Verb::Conic => {
                if let (Some(p0), Some(c)) = (last, out.last_mut()) {
                    let (p1, p2) = (path.pts[pi], path.pts[pi + 1]);
                    linearize([p0, p1, p2, p2], Some(path.weights[wi]), c);
                }
                wi += 1;
            }
            Verb::Cubic => {
                if let (Some(p0), Some(c)) = (last, out.last_mut()) {
                    let pts = [p0, path.pts[pi], path.pts[pi + 1], path.pts[pi + 2]];
                    linearize(pts, None, c);
                }
            }
            Verb::Close => {}
        }
        pi += verb.points();
    }
    out
}

fn snap(v: f32) -> i64 {
    (v * GRID).round() as i64
}

/// The coverage (0 to 4 samples) of every pixel of a `width` x `height` target.
pub fn sample_counts(path: &SkPath, width: u32, height: u32, even_odd: bool) -> Vec<u8> {
    let mut counts = vec![0u8; (width * height) as usize];
    let edges: Vec<[i64; 4]> = contours(path)
        .iter()
        .filter(|c| c.len() > 1)
        .flat_map(|c| {
            let pts: Vec<(i64, i64)> = c.iter().map(|p| (snap(p.x), snap(p.y))).collect();
            (0..pts.len()).map(move |i| {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                [a.0, a.1, b.0, b.1]
            })
        })
        .filter(|e| e[1] != e[3])
        .collect();
    if edges.is_empty() {
        return counts;
    }
    let min_y = edges.iter().map(|e| e[1].min(e[3])).min().unwrap_or(0);
    let max_y = edges.iter().map(|e| e[1].max(e[3])).max().unwrap_or(0);
    let row0 = (min_y.div_euclid(256)).max(0);
    let row1 = (max_y.div_euclid(256) + 1).min(i64::from(height));
    let mut active = Vec::new();
    for row in row0..row1 {
        for &(sx, sy) in &SAMPLES {
            let y = row * 256 + sy;
            active.clear();
            active.extend(
                edges
                    .iter()
                    .filter(|e| (e[1] <= y) != (e[3] <= y))
                    .copied(),
            );
            if active.is_empty() {
                continue;
            }
            for col in 0..i64::from(width) {
                let x = col * 256 + sx;
                let mut winding = 0i32;
                for &[xa, ya, xb, yb] in &active {
                    let cross = (xb - xa) * (y - ya) - (yb - ya) * (x - xa);
                    if yb > ya {
                        if cross > 0 {
                            winding += 1;
                        }
                    } else if cross < 0 {
                        winding -= 1;
                    }
                }
                let inside = if even_odd {
                    winding & 1 != 0
                } else {
                    winding != 0
                };
                if inside {
                    counts[(row * i64::from(width) + col) as usize] += 1;
                }
            }
        }
    }
    counts
}

/// [`sample_counts`] as blits, each pixel's coverage the fraction of its samples inside.
pub fn fill_path(path: &SkPath, width: u32, height: u32, even_odd: bool, out: &mut Spans) {
    let counts = sample_counts(path, width, height, even_odd);
    let alpha = |n: u8| ((u32::from(n) * 255 + 2) / 4) as u8;
    for (y, row) in counts.chunks(width as usize).enumerate() {
        let alphas: Vec<u8> = row.iter().map(|&n| alpha(n)).collect();
        out.anti_h(0, y as i32, &alphas);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> P {
        P { x, y }
    }

    #[test]
    fn a_sample_on_a_right_edge_is_outside_and_on_a_left_edge_inside() {
        let mut tri = SkPath::new();
        tri.move_to(pt(8.0, 5.0));
        tri.line_to(pt(19.0, 12.0));
        tri.line_to(pt(8.0, 19.0));
        tri.close();
        let counts = sample_counts(&tri, 24, 24, false);
        assert_eq!(counts[7 * 24 + 12], 0);
        assert_eq!(counts[10 * 24 + 8], 4);
    }

    #[test]
    fn quarter_pixel_edges_cover_whole_sample_columns() {
        let mut rect = SkPath::new();
        rect.move_to(pt(1.25, 0.0));
        rect.line_to(pt(3.5, 0.0));
        rect.line_to(pt(3.5, 2.0));
        rect.line_to(pt(1.25, 2.0));
        rect.close();
        let counts = sample_counts(&rect, 4, 2, false);
        assert_eq!(&counts[..4], &[0, 3, 4, 2]);
    }
}
