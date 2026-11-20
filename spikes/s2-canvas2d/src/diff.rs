use serde::Serialize;

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<[u8; 3]>,
}

pub fn from_unpremul(width: u32, height: u32, rgba: &[u8], bg: [u8; 3]) -> Image {
    let rgb = rgba
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as u32;
            let mut o = [0u8; 3];
            for c in 0..3 {
                let pre = (p[c] as u32 * a + 127) / 255;
                o[c] = (pre + (bg[c] as u32 * (255 - a) + 127) / 255).min(255) as u8;
            }
            o
        })
        .collect();
    Image { width, height, rgb }
}

pub fn from_premul(width: u32, height: u32, rgba: &[u8], bg: [u8; 3]) -> Image {
    let rgb = rgba
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as u32;
            let mut o = [0u8; 3];
            for c in 0..3 {
                o[c] = (p[c] as u32 + (bg[c] as u32 * (255 - a) + 127) / 255).min(255) as u8;
            }
            o
        })
        .collect();
    Image { width, height, rgb }
}

fn yiq_delta(a: [u8; 3], b: [u8; 3]) -> f64 {
    let f = |p: [u8; 3]| {
        let (r, g, b) = (p[0] as f64, p[1] as f64, p[2] as f64);
        (
            r * 0.29889531 + g * 0.58662247 + b * 0.11448223,
            r * 0.59597799 - g * 0.27417610 - b * 0.32180189,
            r * 0.21147017 - g * 0.52261711 + b * 0.31114694,
        )
    };
    let (y1, i1, q1) = f(a);
    let (y2, i2, q2) = f(b);
    0.5053 * (y1 - y2).powi(2) + 0.299 * (i1 - i2).powi(2) + 0.1957 * (q1 - q2).powi(2)
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Stats {
    pub pixels: u64,
    pub inked: u64,
    pub exact_mismatch: u64,
    pub pm_mismatch: u64,
    pub pm_pct: f64,
    pub pm_pct_of_inked: f64,
    pub max_blob: u64,
    pub max_channel: u8,
    pub mean_abs: f64,
}

pub fn compare(a: &Image, b: &Image, bg: [u8; 3], diff_png: Option<&std::path::Path>) -> Stats {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let max_delta = 35215.0 * 0.1 * 0.1;
    let n = a.rgb.len();
    let mut mask = vec![false; n];
    let mut s = Stats { pixels: n as u64, ..Default::default() };
    let mut sum = 0u64;
    for i in 0..n {
        let (pa, pb) = (a.rgb[i], b.rgb[i]);
        if pa != bg || pb != bg {
            s.inked += 1;
        }
        if pa != pb {
            s.exact_mismatch += 1;
        }
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            s.max_channel = s.max_channel.max(d);
            sum += d as u64;
        }
        if yiq_delta(pa, pb) > max_delta {
            mask[i] = true;
            s.pm_mismatch += 1;
        }
    }
    s.mean_abs = sum as f64 / (n as f64 * 3.0);
    s.pm_pct = 100.0 * s.pm_mismatch as f64 / n as f64;
    s.pm_pct_of_inked = 100.0 * s.pm_mismatch as f64 / s.inked.max(1) as f64;
    s.max_blob = max_blob(&mask, a.width as usize, a.height as usize);
    if let Some(path) = diff_png {
        let mut img = image::RgbImage::new(a.width, a.height);
        for (i, p) in img.pixels_mut().enumerate() {
            let g = (a.rgb[i][0] as u32 + a.rgb[i][1] as u32 + a.rgb[i][2] as u32) / 3;
            let g = (g / 3 + 170) as u8;
            *p = if mask[i] { image::Rgb([255, 0, 0]) } else { image::Rgb([g, g, g]) };
        }
        img.save(path).unwrap();
    }
    s
}

fn max_blob(mask: &[bool], w: usize, h: usize) -> u64 {
    let mut seen = vec![false; mask.len()];
    let mut best = 0u64;
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut size = 0u64;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if mask[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        best = best.max(size);
    }
    best
}

pub fn save_png(img: &Image, path: &std::path::Path) {
    let mut out = image::RgbImage::new(img.width, img.height);
    for (i, p) in out.pixels_mut().enumerate() {
        *p = image::Rgb(img.rgb[i]);
    }
    out.save(path).unwrap();
}
