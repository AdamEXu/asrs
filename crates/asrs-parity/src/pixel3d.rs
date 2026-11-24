//! Gate 4 of PLAN section 2.5: the 3D viewport, WebGL drawing buffer against the port's
//! readback, edge aware.

use crate::pixel::Rgba;

pub const SSIM_MIN: f64 = 0.995;
pub const INTERIOR_MAX_DELTA: u8 = 2;
pub const INTERIOR_DIFFERING_MAX_PCT: f64 = 0.05;
pub const DIFFERING_MAX_PCT: f64 = 1.0;
pub const EDGE_RADIUS: usize = 2;
/// Every mode, edge zone included: no 8-connected blob of more than `OUTLIER_BLOB_MAX` pixels
/// may differ by more than `OUTLIER_DELTA` levels. The edge mask comes from the port's own
/// buffers, so without this a port bug that adds or moves thin geometry (cap shape, line
/// topology) hides in its own edge zone.
pub const OUTLIER_DELTA: u8 = 32;
pub const OUTLIER_BLOB_MAX: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Gate4 {
    pub ssim: f64,
    pub differing_pct: f64,
    pub interior_max_delta: u8,
    pub interior_differing_pct: f64,
    pub interior_pixels: usize,
    pub max_delta: u8,
    pub mean_delta_e: f64,
    /// Largest 8-connected blob of pixels differing by more than [`OUTLIER_DELTA`].
    pub outlier_blob: usize,
    pub pass: bool,
}

fn luma(image: &Rgba) -> Vec<f64> {
    image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]))
        .collect()
}

fn blur(src: &[f64], w: usize, h: usize, kernel: &[f64]) -> Vec<f64> {
    let r = kernel.len() / 2;
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, k) in kernel.iter().enumerate() {
                s += k * src[y * w + (x + i).saturating_sub(r).min(w - 1)];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, k) in kernel.iter().enumerate() {
                s += k * tmp[(y + i).saturating_sub(r).min(h - 1) * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// Mean SSIM over luma with an 11-tap Gaussian window (sigma 1.5), the measure S3 used.
pub fn ssim(a: &Rgba, b: &Rgba) -> f64 {
    let (w, h) = (a.width as usize, a.height as usize);
    let sigma: f64 = 1.5;
    let kernel: Vec<f64> = (-5..=5)
        .map(|i: i32| (-f64::from(i * i) / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f64 = kernel.iter().sum();
    let kernel: Vec<f64> = kernel.iter().map(|v| v / sum).collect();
    let x = luma(a);
    let y = luma(b);
    let product =
        |p: &[f64], q: &[f64]| -> Vec<f64> { p.iter().zip(q).map(|(a, b)| a * b).collect() };
    let mx = blur(&x, w, h, &kernel);
    let my = blur(&y, w, h, &kernel);
    let sxx = blur(&product(&x, &x), w, h, &kernel);
    let syy = blur(&product(&y, &y), w, h, &kernel);
    let sxy = blur(&product(&x, &y), w, h, &kernel);
    let c1 = (0.01f64 * 255.0).powi(2);
    let c2 = (0.03f64 * 255.0).powi(2);
    let mut total = 0.0;
    for i in 0..w * h {
        let (mx, my) = (mx[i], my[i]);
        let vx = sxx[i] - mx * mx;
        let vy = syy[i] - my * my;
        let cxy = sxy[i] - mx * my;
        total +=
            ((2.0 * mx * my + c1) * (2.0 * cxy + c2)) / ((mx * mx + my * my + c1) * (vx + vy + c2));
    }
    total / (w * h) as f64
}

fn srgb_to_lab(p: &[u8; 4]) -> [f64; 3] {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(p[0]), lin(p[1]), lin(p[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference (Sharma, Wu and Dalal 2005), kL = kC = kH = 1.
pub fn ciede2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let c7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c7 / (c7 + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let d_hp = 2.0 * (c1p * c2p).sqrt() * (dhp / 2.0).to_radians().sin();
    let lp_bar = (l1 + l2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;
    let hp_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let cp7 = cp_bar.powi(7);
    let rc = 2.0 * (cp7 / (cp7 + 25f64.powi(7))).sqrt();
    let l50 = (lp_bar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    ((dlp / sl).powi(2) + (dcp / sc).powi(2) + (d_hp / sh).powi(2) + rt * (dcp / sc) * (d_hp / sh))
        .sqrt()
}

/// Mean CIEDE2000 over all pixels of two sRGB images.
pub fn mean_delta_e(a: &Rgba, b: &Rgba) -> f64 {
    let pa = a.data.as_chunks::<4>().0;
    let pb = b.data.as_chunks::<4>().0;
    let total: f64 = pa
        .iter()
        .zip(pb)
        .map(|(x, y)| {
            if x[..3] == y[..3] {
                0.0
            } else {
                ciede2000(srgb_to_lab(x), srgb_to_lab(y))
            }
        })
        .sum();
    total / pa.len().max(1) as f64
}

/// Pixels within `radius` (Chebyshev distance) of an edge pixel.
pub fn dilate(mask: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut rows = vec![false; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(width - 1);
            rows[y * width + x] = (lo..=hi).any(|xx| mask[y * width + xx]);
        }
    }
    let mut out = vec![false; mask.len()];
    for y in 0..height {
        let lo = y.saturating_sub(radius);
        let hi = (y + radius).min(height - 1);
        for x in 0..width {
            out[y * width + x] = (lo..=hi).any(|yy| rows[yy * width + x]);
        }
    }
    out
}

/// Size of the largest 8-connected component of `mask`.
pub fn largest_blob(mask: &[bool], width: usize, height: usize) -> usize {
    let mut seen = vec![false; mask.len()];
    let mut stack = Vec::new();
    let mut largest = 0;
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut size = 0;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % width, i / width);
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let n = ny * width + nx;
                    if mask[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        largest = largest.max(size);
    }
    largest
}

/// Compares RGB only: both sides render an opaque background, so alpha is 255 throughout.
pub fn gate4(reference: &Rgba, port: &Rgba, edges: &[bool]) -> Result<Gate4, String> {
    if (reference.width, reference.height) != (port.width, port.height) {
        return Err(format!(
            "size differs: reference {}x{}, port {}x{}",
            reference.width, reference.height, port.width, port.height
        ));
    }
    let (w, h) = (port.width as usize, port.height as usize);
    if edges.len() != w * h {
        return Err("edge mask size differs from the image".into());
    }
    let edge_zone = dilate(edges, w, h, EDGE_RADIUS);
    let mut differing = 0usize;
    let mut interior = 0usize;
    let mut interior_differing = 0usize;
    let mut interior_max = 0u8;
    let mut max_delta = 0u8;
    let mut outliers = vec![false; w * h];
    for i in 0..w * h {
        let delta = (0..3)
            .map(|c| reference.data[i * 4 + c].abs_diff(port.data[i * 4 + c]))
            .max()
            .unwrap_or(0);
        max_delta = max_delta.max(delta);
        outliers[i] = delta > OUTLIER_DELTA;
        if delta > 0 {
            differing += 1;
        }
        if !edge_zone[i] {
            interior += 1;
            interior_max = interior_max.max(delta);
            if delta > 0 {
                interior_differing += 1;
            }
        }
    }
    let outlier_blob = largest_blob(&outliers, w, h);
    let pct = |n: usize, of: usize| {
        if of == 0 {
            0.0
        } else {
            n as f64 * 100.0 / of as f64
        }
    };
    let ssim = ssim(reference, port);
    let mean_delta_e = mean_delta_e(reference, port);
    let differing_pct = pct(differing, w * h);
    let interior_differing_pct = pct(interior_differing, interior);
    let pass = ssim >= SSIM_MIN
        && differing_pct <= DIFFERING_MAX_PCT
        && outlier_blob <= OUTLIER_BLOB_MAX
        && interior_max <= INTERIOR_MAX_DELTA
        && interior_differing_pct <= INTERIOR_DIFFERING_MAX_PCT;
    Ok(Gate4 {
        ssim,
        differing_pct,
        interior_max_delta: interior_max,
        interior_differing_pct,
        interior_pixels: interior,
        max_delta,
        mean_delta_e,
        outlier_blob,
        pass,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Rgba {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y));
            }
        }
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn identical_images_pass() {
        let a = image(40, 30, |x, y| [x as u8 * 5, y as u8 * 7, 90, 255]);
        let g = gate4(&a, &a, &vec![false; 1200]).unwrap();
        assert!(g.pass);
        assert!((g.ssim - 1.0).abs() < 1e-12);
        assert_eq!(g.differing_pct, 0.0);
    }

    #[test]
    fn interior_difference_fails_and_edge_difference_does_not() {
        let a = image(40, 30, |_, _| [100, 100, 100, 255]);
        let b = image(40, 30, |x, y| {
            if (x, y) == (20, 15) {
                [110, 100, 100, 255]
            } else {
                [100, 100, 100, 255]
            }
        });
        let mut edges = vec![false; 1200];
        assert!(!gate4(&a, &b, &edges).unwrap().pass);
        edges[15 * 40 + 22] = true;
        let g = gate4(&a, &b, &edges).unwrap();
        assert_eq!(g.interior_max_delta, 0);
        assert!(g.pass);
    }

    #[test]
    fn ciede2000_matches_sharma_reference_pairs() {
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, -1.0, 2.0], [50.0, 0.0, 0.0], 2.3669),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            (
                [60.2574, -34.0099, 36.2677],
                [60.4626, -34.1751, 39.4387],
                1.2644,
            ),
            ([2.0776, 0.0795, -1.135], [0.9033, -0.0636, -0.5514], 0.9082),
        ];
        for (a, b, expected) in cases {
            assert!((ciede2000(a, b) - expected).abs() < 1e-4, "{a:?} {b:?}");
        }
    }

    #[test]
    fn edge_zone_outlier_blobs_fail() {
        let a = image(100, 100, |_, _| [100, 100, 100, 255]);
        let blob = |n: u32| {
            image(100, 100, move |x, y| {
                if y == 50 && (10..10 + n).contains(&x) {
                    [133, 100, 100, 255]
                } else {
                    [100, 100, 100, 255]
                }
            })
        };
        let edges = vec![true; 10_000];
        let g = gate4(&a, &blob(4), &edges).unwrap();
        assert_eq!(g.outlier_blob, 4);
        assert!(g.pass);
        let g = gate4(&a, &blob(5), &edges).unwrap();
        assert_eq!(g.outlier_blob, 5);
        assert!(!g.pass);
    }

    #[test]
    fn blobs_are_8_connected() {
        let mut mask = vec![false; 25];
        for i in [0, 6, 12, 4, 9] {
            mask[i] = true;
        }
        assert_eq!(largest_blob(&mask, 5, 5), 3);
        assert_eq!(largest_blob(&[false; 4], 2, 2), 0);
    }

    #[test]
    fn dilation_is_chebyshev() {
        let mut mask = vec![false; 49];
        mask[3 * 7 + 3] = true;
        let d = dilate(&mask, 7, 7, 2);
        assert_eq!(d.iter().filter(|v| **v).count(), 25);
        assert!(d[8] && d[40] && !d[0]);
    }
}
