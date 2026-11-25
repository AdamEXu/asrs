// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011 Google Inc.). See THIRD_PARTY_NOTICES.md.

use tiny_skia::{IntSize, Pixmap};

use crate::display::ImageData;

pub fn to_unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}

/// Skia's raster-pipeline `unpremul` + `store_8888` for one premultiplied pixel.
pub fn unpremultiply(p: [u8; 4]) -> [u8; 4] {
    let a = p[3];
    if a == 0 {
        return [0; 4];
    }
    let scale = 1.0 / (f32::from(a) * (1.0 / 255.0));
    let c = |v: u8| to_unorm(f32::from(v) * (1.0 / 255.0) * scale);
    [c(p[0]), c(p[1]), c(p[2]), a]
}

/// Skia's raster-pipeline `premul` + `store_8888` for one straight-alpha pixel.
pub fn premultiply(p: [u8; 4]) -> [u8; 4] {
    let a = f32::from(p[3]) * (1.0 / 255.0);
    let c = |v: u8| to_unorm(f32::from(v) * (1.0 / 255.0) * a);
    [c(p[0]), c(p[1]), c(p[2]), p[3]]
}

/// Straight-alpha RGBA of a premultiplied pixmap, as `getImageData` reads a canvas.
pub fn to_straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&p| unpremultiply(p))
        .collect()
}

/// Straight-alpha BGRA of a premultiplied pixmap: the layout GPUI's `RenderImage` takes.
pub fn to_straight_bgra(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for &p in pixmap.data().as_chunks::<4>().0 {
        let [r, g, b, a] = unpremultiply(p);
        out.extend_from_slice(&[b, g, r, a]);
    }
    out
}

/// A premultiplied pixmap holding straight-alpha RGBA, as `putImageData` writes it.
pub fn pixmap_from_straight_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Pixmap> {
    if rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&p| premultiply(p))
        .collect();
    Pixmap::from_vec(data, IntSize::from_wh(width, height)?)
}

/// The straight-alpha pixels of `pixmap` inside the device rect, transparent outside it.
/// The pixels of `[x, x + width) x [y, y + height)` as `getImageData` returns them, or `None`
/// when the buffer cannot be allocated.
pub fn read_rect(pixmap: &Pixmap, x: i64, y: i64, width: u32, height: u32) -> Option<ImageData> {
    let len = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    let mut data = Vec::new();
    data.try_reserve_exact(len).ok()?;
    data.resize(len, 0);
    let (pw, ph) = (i64::from(pixmap.width()), i64::from(pixmap.height()));
    let src = pixmap.data();
    for row in 0..i64::from(height) {
        let sy = y + row;
        if !(0..ph).contains(&sy) {
            continue;
        }
        for col in 0..i64::from(width) {
            let sx = x + col;
            if !(0..pw).contains(&sx) {
                continue;
            }
            let i = ((sy * pw + sx) * 4) as usize;
            let o = ((row * i64::from(width) + col) * 4) as usize;
            let p = [src[i], src[i + 1], src[i + 2], src[i + 3]];
            data[o..o + 4].copy_from_slice(&unpremultiply(p));
        }
    }
    Some(ImageData {
        width,
        height,
        data,
    })
}

/// Writes straight-alpha pixels into `pixmap` at device offset `(x, y)`, replacing what was
/// there, as `putImageData` does.
pub fn write_rect(pixmap: &mut Pixmap, image: &ImageData, x: i64, y: i64) {
    let (pw, ph) = (i64::from(pixmap.width()), i64::from(pixmap.height()));
    let dst = pixmap.data_mut();
    for row in 0..i64::from(image.height) {
        let dy = y + row;
        if !(0..ph).contains(&dy) {
            continue;
        }
        for col in 0..i64::from(image.width) {
            let dx = x + col;
            if !(0..pw).contains(&dx) {
                continue;
            }
            let s = ((row * i64::from(image.width) + col) * 4) as usize;
            let d = ((dy * pw + dx) * 4) as usize;
            let p = [
                image.data[s],
                image.data[s + 1],
                image.data[s + 2],
                image.data[s + 3],
            ];
            dst[d..d + 4].copy_from_slice(&premultiply(p));
        }
    }
}

/// The output a thread is worth starting for.
const BAND_BYTES: usize = 1 << 18;

/// `src` as a compositor draws a transformed layer into a `width` x `height` device-pixel
/// target: each target pixel centre is mapped back through `inverse` (`[a, b, c, d, e, f]`,
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`) and `src` is sampled bilinearly there, transparent
/// outside it. A pixel centre that lands on a source pixel centre copies that pixel exactly.
pub fn transform_bilinear(
    src: &Pixmap,
    width: u32,
    height: u32,
    inverse: [f64; 6],
) -> Option<Pixmap> {
    let mut out = Pixmap::new(width, height)?;
    let [a, b, c, d, e, f] = inverse;
    let (sw, sh) = (i64::from(src.width()), i64::from(src.height()));
    let data = src.data();
    let texel = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= sw || y >= sh {
            return [0.0; 4];
        }
        let i = ((y * sw + x) * 4) as usize;
        [0, 1, 2, 3].map(|k| f32::from(data[i + k]))
    };
    let split = |v: f64| {
        let i = v.floor();
        let t = v - i;
        if t < 1e-6 {
            (i as i64, 0.0)
        } else if t > 1.0 - 1e-6 {
            (i as i64 + 1, 0.0)
        } else {
            (i as i64, t as f32)
        }
    };
    let row = width as usize * 4;
    let draw_row = |y: usize, pixels: &mut [u8]| {
        let py = y as f64 + 0.5;
        for x in 0..width {
            let px = f64::from(x) + 0.5;
            let (x0, tx) = split(a * px + c * py + e - 0.5);
            let (y0, ty) = split(b * px + d * py + f - 0.5);
            let p = if tx == 0.0 && ty == 0.0 {
                texel(x0, y0)
            } else {
                let (p00, p10) = (texel(x0, y0), texel(x0 + 1, y0));
                let (p01, p11) = (texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
                std::array::from_fn(|k| {
                    let top = p00[k] + (p10[k] - p00[k]) * tx;
                    let bottom = p01[k] + (p11[k] - p01[k]) * tx;
                    top + (bottom - top) * ty
                })
            };
            let i = x as usize * 4;
            for k in 0..4 {
                pixels[i + k] = p[k].round().clamp(0.0, 255.0) as u8;
            }
        }
    };
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let threads = (out.data().len() / BAND_BYTES + 1).min(cores);
    let band = (height as usize).div_ceil(threads);
    std::thread::scope(|scope| {
        let draw_row = &draw_row;
        for (i, rows) in out.data_mut().chunks_mut(band * row).enumerate() {
            let mut draw = move || {
                for (j, pixels) in rows.chunks_mut(row).enumerate() {
                    draw_row(i * band + j, pixels);
                }
            };
            if threads == 1 {
                draw();
            } else {
                scope.spawn(draw);
            }
        }
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_and_clear_pixels_round_trip() {
        for p in [[0, 0, 0, 255], [12, 200, 255, 255], [0, 0, 0, 0]] {
            assert_eq!(unpremultiply(premultiply(p)), p);
        }
        assert_eq!(unpremultiply([10, 20, 30, 0]), [0, 0, 0, 0]);
    }

    #[test]
    fn half_alpha_rounds_like_skia() {
        assert_eq!(premultiply([255, 128, 1, 128]), [128, 64, 1, 128]);
        assert_eq!(unpremultiply([64, 32, 1, 128]), [128, 64, 2, 128]);
    }

    #[test]
    fn bgra_swaps_red_and_blue() {
        let pixmap = pixmap_from_straight_rgba(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]).unwrap();
        assert_eq!(to_straight_bgra(&pixmap), [0, 0, 255, 255, 255, 0, 0, 128]);
        assert_eq!(to_straight_rgba(&pixmap), [255, 0, 0, 255, 0, 0, 255, 128]);
    }

    #[test]
    fn reads_outside_the_canvas_as_transparent() {
        let pixmap = pixmap_from_straight_rgba(1, 1, &[9, 9, 9, 255]).unwrap();
        let data = read_rect(&pixmap, -1, 0, 2, 1).unwrap();
        assert_eq!(data.data, [0, 0, 0, 0, 9, 9, 9, 255]);
    }

    #[test]
    fn quarter_turn_copies_pixels_exactly() {
        let mut src = Pixmap::new(3, 2).expect("pixmap");
        for (i, p) in src.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *p = [i as u8 * 10, 0, 0, 255];
        }
        let out = transform_bilinear(&src, 2, 3, [0.0, 1.0, -1.0, 0.0, 3.0, 0.0]).expect("out");
        let reds: Vec<u8> = out.data().as_chunks::<4>().0.iter().map(|p| p[0]).collect();
        assert_eq!(reds, [20, 50, 10, 40, 0, 30]);
    }
}
