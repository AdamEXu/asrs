use std::io::Cursor;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<[u8; 3]>,
}

pub fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| format!("png: {e}"))?;
    let size = reader.output_buffer_size().ok_or("png: image too large")?;
    let mut buf = vec![0; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png: {e}"))?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("png: unsupported bit depth {:?}", info.bit_depth));
    }
    buf.truncate(info.buffer_size());
    let data = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => return Err(format!("png: unsupported colour type {other:?}")),
    };
    Ok(Rgba {
        width: info.width,
        height: info.height,
        data,
    })
}

pub fn encode_png(image: &Rgba) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header into a Vec");
    writer
        .write_image_data(&image.data)
        .expect("png data into a Vec");
    writer.finish().expect("png end into a Vec");
    out
}

/// The profile of a PNG's `iCCP` chunk, if it has one.
pub fn png_icc_profile(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(|e| format!("png: {e}"))?;
    Ok(reader.info().icc_profile.as_ref().map(|p| p.to_vec()))
}

pub fn encode_png_with_icc(image: &Rgba, icc: &[u8]) -> Vec<u8> {
    let mut info = png::Info::with_size(image.width, image.height);
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(icc.to_vec().into());
    let mut out = Vec::new();
    let encoder = png::Encoder::with_info(&mut out, info).expect("png info");
    let mut writer = encoder.write_header().expect("png header into a Vec");
    writer
        .write_image_data(&image.data)
        .expect("png data into a Vec");
    writer.finish().expect("png end into a Vec");
    out
}

pub fn read_png(path: &Path) -> Result<Rgba, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_png(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write_png(path: &Path, image: &Rgba) -> Result<(), String> {
    std::fs::write(path, encode_png(image)).map_err(|e| format!("{}: {e}", path.display()))
}

fn blend(value: u32, alpha: u32, background: u32) -> u8 {
    let premultiplied = (value * alpha + 127) / 255;
    (premultiplied + (background * (255 - alpha) + 127) / 255).min(255) as u8
}

/// Composites straight-alpha pixels (as `getImageData` returns them) over an opaque background.
pub fn composite(image: &Rgba, background: [u8; 3]) -> Rgb {
    let data = image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let a = u32::from(p[3]);
            [0, 1, 2].map(|c| blend(u32::from(p[c]), a, u32::from(background[c])))
        })
        .collect();
    Rgb {
        width: image.width,
        height: image.height,
        data,
    }
}

fn yiq_delta(a: [u8; 3], b: [u8; 3]) -> f64 {
    let yiq = |p: [u8; 3]| {
        let (r, g, b) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        (
            r * 0.298_895_31 + g * 0.586_622_47 + b * 0.114_482_23,
            r * 0.595_977_99 - g * 0.274_176_10 - b * 0.321_801_89,
            r * 0.211_470_17 - g * 0.522_617_11 + b * 0.311_146_94,
        )
    };
    let (y1, i1, q1) = yiq(a);
    let (y2, i2, q2) = yiq(b);
    0.5053 * (y1 - y2).powi(2) + 0.299 * (i1 - i2).powi(2) + 0.1957 * (q1 - q2).powi(2)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub pixels: u64,
    pub inked: u64,
    pub exact_mismatch: u64,
    pub mismatch: u64,
    pub mismatch_pct: f64,
    pub max_blob: u64,
    pub max_channel: u8,
    pub mean_abs: f64,
}

pub struct Comparison {
    pub stats: Stats,
    pub mask: Vec<bool>,
}

/// pixelmatch with YIQ threshold 0.1 and no anti-aliasing exclusion, plus the largest
/// 8-connected blob of differing pixels.
pub fn compare(a: &Rgb, b: &Rgb, background: [u8; 3]) -> Result<Comparison, String> {
    if (a.width, a.height) != (b.width, b.height) {
        return Err(format!(
            "size {}x{} differs from {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    let max_delta = 35215.0 * 0.1 * 0.1;
    let n = a.data.len();
    let mut mask = vec![false; n];
    let mut stats = Stats {
        pixels: n as u64,
        ..Stats::default()
    };
    let mut sum = 0u64;
    for (i, (&pa, &pb)) in a.data.iter().zip(&b.data).enumerate() {
        if pa != background || pb != background {
            stats.inked += 1;
        }
        if pa != pb {
            stats.exact_mismatch += 1;
        }
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            stats.max_channel = stats.max_channel.max(d);
            sum += u64::from(d);
        }
        if yiq_delta(pa, pb) > max_delta {
            mask[i] = true;
            stats.mismatch += 1;
        }
    }
    stats.mean_abs = sum as f64 / (n.max(1) as f64 * 3.0);
    stats.mismatch_pct = 100.0 * stats.mismatch as f64 / n.max(1) as f64;
    stats.max_blob = max_blob(&mask, a.width as usize, a.height as usize);
    Ok(Comparison { stats, mask })
}

pub fn max_blob(mask: &[bool], width: usize, height: usize) -> u64 {
    let mut seen = vec![false; mask.len()];
    let mut best = 0;
    let mut stack = Vec::new();
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
                    let j = ny * width + nx;
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

/// The reference dimmed to grey with differing pixels in red.
pub fn diff_image(reference: &Rgb, mask: &[bool]) -> Rgba {
    let data = reference
        .data
        .iter()
        .zip(mask)
        .flat_map(|(p, &bad)| {
            if bad {
                [255, 0, 0, 255]
            } else {
                let g = ((u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 9 + 170) as u8;
                [g, g, g, 255]
            }
        })
        .collect();
    Rgba {
        width: reference.width,
        height: reference.height,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, p: [u8; 4]) -> Rgba {
        Rgba {
            width: w,
            height: h,
            data: p.repeat((w * h) as usize),
        }
    }

    #[test]
    fn png_round_trip() {
        let mut image = solid(5, 3, [1, 2, 3, 4]);
        image.data[7] = 200;
        assert_eq!(decode_png(&encode_png(&image)).unwrap(), image);
    }

    #[test]
    fn icc_profile_round_trip() {
        let image = solid(2, 2, [9, 8, 7, 255]);
        let tagged = encode_png_with_icc(&image, b"not really a profile");
        assert_eq!(decode_png(&tagged).unwrap(), image);
        assert_eq!(
            png_icc_profile(&tagged).unwrap().as_deref(),
            Some(&b"not really a profile"[..])
        );
        assert_eq!(png_icc_profile(&encode_png(&image)).unwrap(), None);
    }

    #[test]
    fn identical_images_match() {
        let a = composite(&solid(4, 4, [10, 20, 30, 128]), [255, 255, 255]);
        let c = compare(&a, &a.clone(), [255, 255, 255]).unwrap();
        assert_eq!(c.stats.mismatch, 0);
        assert_eq!(c.stats.max_blob, 0);
        assert_eq!(c.stats.inked, 16);
    }

    #[test]
    fn blob_is_eight_connected() {
        let bg = [255, 255, 255];
        let a = composite(&solid(6, 6, [0, 0, 0, 0]), bg);
        let mut b = a.clone();
        for (x, y) in [(0, 0), (1, 1), (2, 2), (5, 0)] {
            b.data[y * 6 + x] = [0, 0, 0];
        }
        let c = compare(&a, &b, bg).unwrap();
        assert_eq!(c.stats.mismatch, 4);
        assert_eq!(c.stats.max_blob, 3);
        assert_eq!(c.stats.max_channel, 255);
    }

    #[test]
    fn small_differences_pass_the_yiq_threshold() {
        let bg = [255, 255, 255];
        let a = composite(&solid(2, 1, [100, 100, 100, 255]), bg);
        let mut b = a.clone();
        b.data[0] = [104, 100, 100];
        let c = compare(&a, &b, bg).unwrap();
        assert_eq!((c.stats.exact_mismatch, c.stats.mismatch), (1, 0));
    }
}
