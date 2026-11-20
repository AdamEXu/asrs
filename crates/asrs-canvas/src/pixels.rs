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
    let len = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
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
}
