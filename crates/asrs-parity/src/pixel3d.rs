//! Gate 4 of PLAN section 2.5: the 3D viewport, WebGL drawing buffer against the port's
//! readback, edge aware.

use crate::pixel::Rgba;

pub const SSIM_MIN: f64 = 0.995;
pub const INTERIOR_MAX_DELTA: u8 = 2;
pub const INTERIOR_DIFFERING_MAX_PCT: f64 = 0.05;
pub const DIFFERING_MAX_PCT: f64 = 1.0;
pub const EDGE_RADIUS: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Gate4 {
    pub ssim: f64,
    pub differing_pct: f64,
    pub interior_max_delta: u8,
    pub interior_differing_pct: f64,
    pub interior_pixels: usize,
    pub max_delta: u8,
    pub pass: bool,
}

fn luma(image: &Rgba) -> Vec<f64> {
    image
        .data
        .chunks_exact(4)
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
    let product = |p: &[f64], q: &[f64]| -> Vec<f64> { p.iter().zip(q).map(|(a, b)| a * b).collect() };
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
        total += ((2.0 * mx * my + c1) * (2.0 * cxy + c2))
            / ((mx * mx + my * my + c1) * (vx + vy + c2));
    }
    total / (w * h) as f64
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
    for i in 0..w * h {
        let delta = (0..3)
            .map(|c| reference.data[i * 4 + c].abs_diff(port.data[i * 4 + c]))
            .max()
            .unwrap_or(0);
        max_delta = max_delta.max(delta);
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
    let pct = |n: usize, of: usize| if of == 0 { 0.0 } else { n as f64 * 100.0 / of as f64 };
    let ssim = ssim(reference, port);
    let differing_pct = pct(differing, w * h);
    let interior_differing_pct = pct(interior_differing, interior);
    let pass = ssim >= SSIM_MIN
        && interior_max <= INTERIOR_MAX_DELTA
        && interior_differing_pct <= INTERIOR_DIFFERING_MAX_PCT
        && differing_pct <= DIFFERING_MAX_PCT;
    Ok(Gate4 {
        ssim,
        differing_pct,
        interior_max_delta: interior_max,
        interior_differing_pct,
        interior_pixels: interior,
        max_delta,
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
        Rgba { width: w, height: h, data }
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
        let b = image(40, 30, |x, y| if (x, y) == (20, 15) { [110, 100, 100, 255] } else { [100, 100, 100, 255] });
        let mut edges = vec![false; 1200];
        assert!(!gate4(&a, &b, &edges).unwrap().pass);
        edges[15 * 40 + 22] = true;
        let g = gate4(&a, &b, &edges).unwrap();
        assert_eq!(g.interior_max_delta, 0);
        assert!(g.pass);
    }

    #[test]
    fn dilation_is_chebyshev() {
        let mut mask = vec![false; 49];
        mask[3 * 7 + 3] = true;
        let d = dilate(&mask, 7, 7, 2);
        assert_eq!(d.iter().filter(|v| **v).count(), 25);
        assert!(d[1 * 7 + 1] && d[5 * 7 + 5] && !d[0]);
    }
}
