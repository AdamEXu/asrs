fn luma(rgba: &[u8], w: usize, h: usize) -> Vec<f64> {
    (0..w * h)
        .map(|i| {
            let p = &rgba[i * 4..i * 4 + 4];
            0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
        })
        .collect()
}

fn blur(src: &[f64], w: usize, h: usize, k: &[f64]) -> Vec<f64> {
    let r = k.len() / 2;
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x + i).saturating_sub(r).min(w - 1);
                s += kv * src[y * w + xx];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let yy = (y + i).saturating_sub(r).min(h - 1);
                s += kv * tmp[yy * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

pub fn ssim(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    let sigma: f64 = 1.5;
    let k: Vec<f64> = (-5..=5).map(|i: i32| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f64 = k.iter().sum();
    let k: Vec<f64> = k.iter().map(|v| v / sum).collect();
    let x = luma(a, w, h);
    let y = luma(b, w, h);
    let xx: Vec<f64> = x.iter().map(|v| v * v).collect();
    let yy: Vec<f64> = y.iter().map(|v| v * v).collect();
    let xy: Vec<f64> = x.iter().zip(&y).map(|(a, b)| a * b).collect();
    let mx = blur(&x, w, h, &k);
    let my = blur(&y, w, h, &k);
    let sxx = blur(&xx, w, h, &k);
    let syy = blur(&yy, w, h, &k);
    let sxy = blur(&xy, w, h, &k);
    let c1 = (0.01f64 * 255.0).powi(2);
    let c2 = (0.03f64 * 255.0).powi(2);
    let mut total = 0.0;
    for i in 0..w * h {
        let (mx, my) = (mx[i], my[i]);
        let vx = sxx[i] - mx * mx;
        let vy = syy[i] - my * my;
        let cxy = sxy[i] - mx * my;
        total += ((2.0 * mx * my + c1) * (2.0 * cxy + c2)) / ((mx * mx + my * my + c1) * (vx + vy + c2));
    }
    total / (w * h) as f64
}

pub struct DiffStats {
    pub mean_abs: f64,
    pub max_abs: u8,
    pub exact_frac: f64,
    pub within2_frac: f64,
}

pub fn diff(a: &[u8], b: &[u8]) -> DiffStats {
    let mut sum = 0u64;
    let mut max = 0u8;
    let mut exact = 0usize;
    let mut within2 = 0usize;
    let n = a.len() / 4;
    for i in 0..n {
        let mut pmax = 0u8;
        for c in 0..3 {
            let d = a[i * 4 + c].abs_diff(b[i * 4 + c]);
            sum += d as u64;
            pmax = pmax.max(d);
        }
        max = max.max(pmax);
        if pmax == 0 {
            exact += 1;
        }
        if pmax <= 2 {
            within2 += 1;
        }
    }
    DiffStats {
        mean_abs: sum as f64 / (n * 3) as f64,
        max_abs: max,
        exact_frac: exact as f64 / n as f64,
        within2_frac: within2 as f64 / n as f64,
    }
}
